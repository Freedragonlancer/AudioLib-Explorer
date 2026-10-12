// server/audiolib-server/src/main.rs
//! `audiolib-server` entry point: load configuration, bind and serve `/api/v1` immediately,
//! and open the library in the background (see `audiolib_server::service`).
use anyhow::{Context, Result, bail};
use audiolib_server::{
    API_BASE, AuthMode, BearerToken, Readiness, RetryPolicy, Security, ServeOptions, ServerConfig,
    Service, config::CONFIG_ENV, open_in_background, router, serve, tls,
};
use std::{path::PathBuf, time::Duration};
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Cannot start the async runtime")?;
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    result
}

async fn run() -> Result<()> {
    let config_arg = match parse_args(std::env::args().skip(1))? {
        Command::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Command::Version => {
            println!("audiolib-server {}", audiolib_server::VERSION);
            return Ok(());
        }
        Command::Serve(path) => path,
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
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
    let address = listener.local_addr()?;
    tracing::info!(%address, base = API_BASE, admin = %format!("http://{address}/admin"), "Serving");
    let bind_ip = config.bind.ip();
    open_in_background(
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
            shutdown_signal().await;
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
    if let Readiness::Ready(state) = service.readiness() {
        let stopped =
            tokio::task::spawn_blocking(move || state.scans().shutdown(SCAN_SHUTDOWN)).await;
        if !matches!(stopped, Ok(true)) {
            tracing::warn!("Exiting while a scan is still finishing");
        }
    }
    // Deliberately never close the library's SQLite connections: when the last connection
    // closes, SQLite checkpoints the WAL into the main file, whose changed size/mtime would
    // look like an external change and bump the revision on the next start. Leaving the
    // WAL for the next open is safe (SQLite recovers it), and the process exits right after.
    std::mem::forget(service);
    tracing::info!("Server stopped");
    Ok(())
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
