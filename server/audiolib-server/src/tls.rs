// server/audiolib-server/src/tls.rs
//! Optional native TLS listener: certificate loading (also used to validate admin-page
//! input before it is saved) and a manager task that (re)binds the listener whenever the
//! effective TLS settings change. Renewed PEM files are loaded into the existing
//! certificate resolver without interrupting established connections.
use crate::{
    auth::{Security, TlsSettings, TlsStatus},
    server::{ServeOptions, serve_tls},
};
use axum::Router;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::{
    fs,
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use tokio_rustls::TlsAcceptor;

/// How long a rebind of the same port retries while the previous listener closes.
const REBIND_WAIT: Duration = Duration::from_secs(3);
/// Check renewed certificate/key contents once a minute, including symlink replacements.
const RELOAD_EVERY: Duration = Duration::from_secs(60);

type CredentialsFingerprint = [u8; 32];

struct Credentials {
    cert: Vec<u8>,
    key: Vec<u8>,
    fingerprint: CredentialsFingerprint,
}

impl Credentials {
    fn read(cert: &Path, key: &Path) -> Result<Self, String> {
        if !cert.is_absolute() || !key.is_absolute() {
            return Err("Certificate and key paths must be absolute".into());
        }
        let cert = fs::read(cert)
            .map_err(|error| format!("Cannot read certificate {}: {error}", cert.display()))?;
        let key = fs::read(key)
            .map_err(|error| format!("Cannot read private key {}: {error}", key.display()))?;
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(&(cert.len() as u64).to_le_bytes());
        digest.update(&cert);
        digest.update(&key);
        let mut fingerprint = [0; 32];
        fingerprint.copy_from_slice(digest.finish().as_ref());
        Ok(Self {
            cert,
            key,
            fingerprint,
        })
    }

    fn config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&self.cert)
            .collect::<Result<_, _>>()
            .map_err(|error| format!("Invalid certificate PEM: {error}"))?;
        if certs.is_empty() {
            return Err("No certificate found in certificate PEM".into());
        }
        let key = PrivateKeyDer::from_pem_slice(&self.key)
            .map_err(|error| format!("Invalid private key PEM: {error}"))?;
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
}

#[derive(Debug)]
struct ReloadableResolver(RwLock<Arc<dyn ResolvesServerCert>>);

impl ResolvesServerCert for ReloadableResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .resolve(hello)
    }
}

struct ActiveListener {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
    tls: TlsSettings,
    resolver: Arc<ReloadableResolver>,
    fingerprint: CredentialsFingerprint,
    reload_error: Option<String>,
}

impl ActiveListener {
    async fn reload(&mut self) -> Result<bool, String> {
        let settings = self.tls.clone();
        let fingerprint = self.fingerprint;
        let changed = tokio::task::spawn_blocking(move || {
            let credentials = Credentials::read(&settings.cert_path, &settings.key_path)?;
            if credentials.fingerprint == fingerprint {
                return Ok::<_, String>(None);
            }
            let config = credentials.config()?;
            Ok(Some((config, credentials.fingerprint)))
        })
        .await
        .map_err(|error| format!("Certificate reload task failed: {error}"))??;
        let Some((config, fingerprint)) = changed else {
            return Ok(false);
        };
        // This swaps only certificate selection for new handshakes. Established TLS
        // sessions retain their negotiated credentials and are never disconnected.
        *self
            .resolver
            .0
            .write()
            .unwrap_or_else(PoisonError::into_inner) = config.cert_resolver.clone();
        self.fingerprint = fingerprint;
        Ok(true)
    }
}

/// Load a PEM certificate chain and private key into a rustls server configuration
/// (HTTP/1.1 via ALPN). Errors are short, user-facing texts.
pub fn load_server_config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    Credentials::read(cert, key)?.config()
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
/// check PEM contents every minute for renewal without rebinding, and report the state
/// through [`Security::set_tls_status`]. An invalid renewal keeps the working certificate.
pub fn spawn_manager(
    security: Arc<Security>,
    app: Router,
    bind_ip: IpAddr,
    options: ServeOptions,
    stop: watch::Receiver<bool>,
) -> JoinHandle<()> {
    spawn_manager_with_interval(security, app, bind_ip, options, stop, RELOAD_EVERY)
}

fn spawn_manager_with_interval(
    security: Arc<Security>,
    app: Router,
    bind_ip: IpAddr,
    options: ServeOptions,
    mut stop: watch::Receiver<bool>,
    reload_every: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut settings = security.tls_watch();
        let mut current: Option<ActiveListener> = None;
        let mut renewals =
            tokio::time::interval_at(tokio::time::Instant::now() + reload_every, reload_every);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        'manager: loop {
            if *stop.borrow() {
                break;
            }
            let (generation, wanted) = settings.borrow_and_update().clone();
            if let Some(active) = current.take() {
                let _ = active.stop.send(true);
                // The listener socket closes right away; in-flight requests finish within
                // the grace period in the background.
                tokio::spawn(active.task);
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
            loop {
                tokio::select! {
                    changed = settings.changed() => {
                        if changed.is_err() {
                            break 'manager;
                        }
                        break;
                    }
                    _ = renewals.tick() => {
                        if let Some(active) = current.as_mut() {
                            match active.reload().await {
                                Ok(changed) => {
                                    if changed {
                                        tracing::info!(port = active.tls.port, "Native TLS certificate renewed");
                                    }
                                    if active.reload_error.take().is_some() || changed {
                                        security.set_tls_status(TlsStatus {
                                            state: "listening".into(),
                                            port: Some(active.tls.port),
                                            generation,
                                            message: None,
                                        });
                                    }
                                }
                                Err(message) => {
                                    if active.reload_error.as_ref() != Some(&message) {
                                        tracing::warn!(port = active.tls.port, %message, "TLS renewal failed; keeping the current certificate");
                                    }
                                    active.reload_error = Some(message.clone());
                                    security.set_tls_status(TlsStatus {
                                        state: "listening".into(),
                                        port: Some(active.tls.port),
                                        generation,
                                        message: Some(format!("Certificate renewal failed; current certificate retained: {message}")),
                                    });
                                }
                            }
                        }
                    }
                    _ = async {
                        let _ = stop.wait_for(|stopped| *stopped).await;
                    } => break 'manager,
                }
            }
        }
        if let Some(active) = current.take() {
            let _ = active.stop.send(true);
            let _ = active.task.await;
        }
    })
}

async fn start(
    tls: &TlsSettings,
    bind_ip: IpAddr,
    app: Router,
    options: ServeOptions,
) -> Result<ActiveListener, String> {
    let settings = tls.clone();
    let (config, fingerprint) = tokio::task::spawn_blocking(move || {
        let credentials = Credentials::read(&settings.cert_path, &settings.key_path)?;
        Ok::<_, String>((credentials.config()?, credentials.fingerprint))
    })
    .await
    .map_err(|error| format!("TLS configuration task failed: {error}"))??;
    let resolver = Arc::new(ReloadableResolver(RwLock::new(
        config.cert_resolver.clone(),
    )));
    let mut config = (*config).clone();
    config.cert_resolver = resolver.clone();
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
        TlsAcceptor::from(Arc::new(config)),
        async move {
            let _ = stop_rx.wait_for(|stopped| *stopped).await;
        },
        options,
    ));
    Ok(ActiveListener {
        stop: stop_tx,
        task,
        tls: tls.clone(),
        resolver,
        fingerprint,
        reload_error: None,
    })
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
