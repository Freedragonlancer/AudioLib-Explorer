// server/audiolib-server/src/tls.rs
//! Optional native TLS listener: certificate loading (also used to validate admin-page
//! input before it is saved) and a manager task that (re)binds the listener whenever the
//! effective TLS settings change, so no restart is needed. The plain HTTP listener is
//! never affected.
use crate::{
    auth::{Security, TlsSettings, TlsStatus},
    server::{ServeOptions, serve_tls},
};
use axum::Router;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::{
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use tokio_rustls::TlsAcceptor;

/// How long a rebind of the same port retries while the previous listener closes.
const REBIND_WAIT: Duration = Duration::from_secs(3);

/// Load a PEM certificate chain and private key into a rustls server configuration
/// (HTTP/1.1 via ALPN). Errors are short, user-facing texts.
pub fn load_server_config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    if !cert.is_absolute() || !key.is_absolute() {
        return Err("Certificate and key paths must be absolute".into());
    }
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|error| format!("Cannot read certificate {}: {error}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("Invalid certificate PEM {}: {error}", cert.display()))?;
    if certs.is_empty() {
        return Err(format!("No certificate found in {}", cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|error| format!("Cannot read private key {}: {error}", key.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("TLS setup failed: {error}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| format!("Certificate and key do not form a usable pair: {error}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Validate admin-page TLS input: absolute paths, loadable PEM files, a port other than
/// the HTTP port.
pub fn validate(settings: &TlsSettings, http_port: u16) -> Result<(), String> {
    if settings.port == 0 || settings.port == http_port {
        return Err(format!(
            "The HTTPS port must be 1..=65535 and differ from the HTTP port {http_port}"
        ));
    }
    load_server_config(&settings.cert_path, &settings.key_path).map(|_| ())
}

/// Run the TLS listener manager until `stop` turns `true`: serve `app` over TLS on
/// `bind_ip:<port>` whenever TLS is configured, rebinding on every settings change, and
/// report the state through [`Security::set_tls_status`].
pub fn spawn_manager(
    security: Arc<Security>,
    app: Router,
    bind_ip: IpAddr,
    options: ServeOptions,
    mut stop: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut settings = security.tls_watch();
        let mut current: Option<(watch::Sender<bool>, JoinHandle<()>)> = None;
        loop {
            let (generation, wanted) = settings.borrow_and_update().clone();
            if let Some((stop_listener, task)) = current.take() {
                let _ = stop_listener.send(true);
                // The listener socket closes right away; in-flight requests finish within
                // the grace period in the background.
                tokio::spawn(task);
            }
            match wanted {
                None => security.set_tls_status(TlsStatus {
                    state: "off".into(),
                    generation,
                    ..TlsStatus::default()
                }),
                Some(tls) => {
                    security.set_tls_status(TlsStatus {
                        state: "starting".into(),
                        port: Some(tls.port),
                        generation,
                        message: None,
                    });
                    match start(&tls, bind_ip, app.clone(), options).await {
                        Ok(started) => {
                            tracing::info!(port = tls.port, cert = %tls.cert_path.display(), "Native TLS listener started");
                            security.set_tls_status(TlsStatus {
                                state: "listening".into(),
                                port: Some(tls.port),
                                generation,
                                message: None,
                            });
                            current = Some(started);
                        }
                        Err(message) => {
                            tracing::error!(port = tls.port, %message, "Native TLS listener not started");
                            security.set_tls_status(TlsStatus {
                                state: "error".into(),
                                port: Some(tls.port),
                                generation,
                                message: Some(message),
                            });
                        }
                    }
                }
            }
            tokio::select! {
                changed = settings.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                _ = stop.wait_for(|stopped| *stopped) => break,
            }
        }
        if let Some((stop_listener, task)) = current.take() {
            let _ = stop_listener.send(true);
            let _ = task.await;
        }
    })
}

async fn start(
    tls: &TlsSettings,
    bind_ip: IpAddr,
    app: Router,
    options: ServeOptions,
) -> Result<(watch::Sender<bool>, JoinHandle<()>), String> {
    let config = load_server_config(&tls.cert_path, &tls.key_path)?;
    let address = SocketAddr::new(bind_ip, tls.port);
    let deadline = tokio::time::Instant::now() + REBIND_WAIT;
    let listener = loop {
        match TcpListener::bind(address).await {
            Ok(listener) => break listener,
            Err(error)
                if error.kind() == std::io::ErrorKind::AddrInUse
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(format!("Cannot listen on {address}: {error}")),
        }
    };
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let task = tokio::spawn(serve_tls(
        listener,
        app,
        TlsAcceptor::from(config),
        async move {
            let _ = stop_rx.wait_for(|stopped| *stopped).await;
        },
        options,
    ));
    Ok((stop_tx, task))
}
