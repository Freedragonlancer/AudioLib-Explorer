// server/audiolib-server/src/main.rs
//! `audiolib-server` entry point: load configuration, bind and serve `/api/v1` immediately,
//! and open the library in the background (see `audiolib_server::service`).
use anyhow::{Context, Result, bail};
use audiolib_server::{
    API_BASE, AuthMode, BearerToken, Readiness, RetryPolicy, Security, ServeOptions, ServerConfig,
    Service, config::CONFIG_ENV, open_in_background, router, serve, tls,
};
use std::{ffi::OsString, path::PathBuf, time::Duration};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "Usage: audiolib-server [--config <path>]

Options:
  --config <path>  Server TOML configuration (default: $AUDIOLIB_SERVER_CONFIG)
  -h, --help       Print this help
  -V, --version    Print the version

Authentication mode (`auth_mode`, admin page): `external` by default - LAN-direct
requests need no token, proxied or public ones need `Authorization: Bearer`.
`auth = true` means `all` and requires the token in the environment variable named
by `token_env` (default AUDIOLIB_SERVER_TOKEN); otherwise that token is optional
and accepted next to tokens generated on the LAN-only admin page
(http://<host>:<port>/admin). Logging honours RUST_LOG.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve(Option<PathBuf>),
    Help,
    Version,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command> {
    let mut args = args.into_iter();
    let mut config = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--config" => {
                let value = args.next().context("--config requires a path")?;
                config = Some(PathBuf::from(value));
            }
            other => match other.strip_prefix("--config=") {
                Some(value) if !value.is_empty() => config = Some(PathBuf::from(value)),
                _ => bail!("Unexpected argument {other:?}\n\n{USAGE}"),
            },
        }
    }
    Ok(Command::Serve(config))
}

/// Upper bound for runtime teardown after the server stopped: blocking library tasks
/// (e.g. a snapshot backup) are abandoned instead of delaying process exit.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);
/// How long shutdown waits for a cancelled scan to record its session as cancelled.
const SCAN_SHUTDOWN: Duration = Duration::from_secs(10);

fn main() -> Result<()> {
    if audiolib_update::handle_update_helper()? {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Cannot start the async runtime")?;
    let result = runtime.block_on(run());
    if matches!(&result, Ok(Some(_))) {
        // Updating must wait for every admitted blocking task. Abandoning one before
        // Unix exec could interrupt its database/cache write in the middle.
        drop(runtime);
    } else {
        runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    }
    if let Some((prepared, args)) = result? {
        prepared.deploy_and_restart(&args)?;
    }
    Ok(())
}

type Deployment = (audiolib_update::PreparedUpdate, Vec<OsString>);

async fn run() -> Result<Option<Deployment>> {
    let config_arg = match parse_args(std::env::args().skip(1))? {
        Command::Help => {
            println!("{USAGE}");
            return Ok(None);
        }
        Command::Version => {
            println!("audiolib-server {}", audiolib_server::VERSION);
            return Ok(None);
        }
        Command::Serve(path) => path,
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    // Diagnostics only: an old linked SQLite is reported, never fatal.
    // TODO(#98): adapt to `Option<warning>` once the new core signature lands.
    let linked_sqlite = rusqlite::version();
    match audiolib_core::compatibility::check_linked_sqlite(linked_sqlite) {
        Ok(()) => tracing::info!(sqlite = linked_sqlite, "Linked SQLite version"),
        Err(warning) => tracing::warn!(sqlite = linked_sqlite, "{warning}"),
    }
    let config_path = match config_arg {
        Some(path) => path,
        None => std::env::var_os(CONFIG_ENV)
            .map(PathBuf::from)
            .with_context(|| format!("Pass --config <path> or set {CONFIG_ENV}"))?,
    };
    let config = ServerConfig::load(&config_path)?;
    tracing::info!(config = %config_path.display(), auth = config.auth, "Configuration loaded");
    let token_env = config.token_env_name().to_string();
    let token = if config.auth {
        Some(BearerToken::from_env(&config)?)
    } else if std::env::var_os(&token_env).is_some() {
        match BearerToken::from_env(&config) {
            Ok(token) => Some(token),
            Err(error) => {
                tracing::warn!("Ignoring the environment token: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let security = Security::from_config(&config, token);
    let (mode, source) = security.auth_mode_with_source();
    for warning in security.warnings() {
        tracing::warn!("{warning}");
    }
    tracing::info!(
        auth_mode = %mode,
        source = ?source,
        env_token = security.env_token().is_some(),
        generated_tokens = security.token_count(),
        public_url = security.public_url().as_deref().unwrap_or("-"),
        "Authentication"
    );
    match mode {
        AuthMode::Off => tracing::warn!(
            "Authentication is DISABLED (auth_mode = off): every endpoint accepts requests without a token"
        ),
        AuthMode::External => tracing::info!(
            "LAN-direct requests need no token; proxied or public requests need a bearer token"
        ),
        AuthMode::All => tracing::info!(
            token_env,
            "Bearer authentication required on every endpoint"
        ),
    }
    let (manage_mode, manage_source) = security.manage_mode_with_source();
    tracing::info!(
        manage_mode = %manage_mode,
        source = ?manage_source,
        "Library management (scans, record and folder removal)"
    );
    // Listen first: a slow or blocked library open (e.g. a macOS permission prompt for an
    // external volume) must not leave the port dead. Endpoints answer 503 until ready.
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("Cannot bind {}", config.bind))?;
    let service = Service::starting_with(security.clone());
    let updates = service.updates().clone();
    if let Err(error) = updates.start(std::env::args_os().skip(1).collect()) {
        tracing::warn!(%error, "Automatic update check could not start; server remains available");
    }
    let mut update_stop = updates.shutdown_watch();
    let address = listener.local_addr()?;
    tracing::info!(%address, base = API_BASE, admin = %format!("http://{address}/admin"), "Serving");
    let bind_ip = config.bind.ip();
    let opening = open_in_background(
        service.clone(),
        config,
        RetryPolicy::default(),
        tokio::runtime::Handle::current(),
    )
    .context("Cannot start the library open thread")?;
    let app = router(service.clone());
    let options = ServeOptions::default();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // The optional native TLS listener follows the admin settings without a restart.
    let tls_manager = tls::spawn_manager(security, app.clone(), bind_ip, options, stop_rx);
    let signal_service = service.clone();
    serve(
        listener,
        app,
        async move {
            tokio::select! {
                () = shutdown_signal() => {},
                _ = async {
                    if update_stop.wait_for(|ready| *ready).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                } => tracing::info!("Verified update requested graceful shutdown"),
            }
            signal_service.updates().stop_admission();
            signal_service.request_stop();
            let _ = stop_tx.send(true);
            // Cancel a running scan right away; it finalizes while connections drain.
            if let Readiness::Ready(state) = signal_service.readiness() {
                state.scans().shutdown(Duration::ZERO);
            }
        },
        options,
    )
    .await;
    let _ = tls_manager.await;
    let updating = *updates.shutdown_watch().borrow();
    let mut clean_shutdown = true;
    let waiting = std::time::Instant::now();
    while !opening.is_finished() && waiting.elapsed() < SCAN_SHUTDOWN {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if opening.is_finished() {
        match opening.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "Unpublished library shutdown did not complete");
                clean_shutdown = false;
            }
            Err(_) => {
                tracing::warn!("Library startup thread panicked during shutdown");
                clean_shutdown = false;
            }
        }
    } else {
        tracing::warn!("Library startup has not reached a cancellation boundary");
        clean_shutdown = false;
    }
    if let Readiness::Ready(state) = service.readiness() {
        match tokio::task::spawn_blocking(move || {
            state.shutdown(SCAN_SHUTDOWN)?;
            if updating {
                state.list_waveforms().drain_for_update(SCAN_SHUTDOWN)?;
            }
            Ok::<(), audiolib_core::Error>(())
        })
        .await
        {
            Ok(Ok(())) => tracing::info!("Library checkpoint and snapshot cleanup complete"),
            Ok(Err(error)) => {
                tracing::warn!(%error, "Library shutdown maintenance incomplete");
                clean_shutdown = false;
            }
            Err(error) => {
                tracing::warn!(%error, "Library shutdown task failed");
                clean_shutdown = false;
            }
        }
    }
    drop(service);
    tracing::info!("Server stopped");
    if updating && !clean_shutdown {
        bail!(
            "Update deployment cancelled because library shutdown did not complete; installed files were preserved"
        );
    }
    Ok(if updating {
        updates.take_prepared()
    } else {
        None
    })
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "Cannot listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "Cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("Shutdown requested");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command> {
        parse_args(args.iter().map(|arg| (*arg).to_string()))
    }

    #[test]
    fn arguments_select_config_help_and_version() {
        assert_eq!(parse(&[]).unwrap(), Command::Serve(None));
        assert_eq!(
            parse(&["--config", "a.toml"]).unwrap(),
            Command::Serve(Some("a.toml".into()))
        );
        assert_eq!(
            parse(&["--config=b.toml"]).unwrap(),
            Command::Serve(Some("b.toml".into()))
        );
        assert_eq!(parse(&["--help"]).unwrap(), Command::Help);
        assert_eq!(parse(&["-V"]).unwrap(), Command::Version);
        assert!(parse(&["--config"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
