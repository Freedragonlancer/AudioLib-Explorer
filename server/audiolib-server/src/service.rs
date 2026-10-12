// server/audiolib-server/src/service.rs
//! Listen-first readiness: the HTTP listener is served immediately while the library opens
//! in the background, so a slow or blocked open (for example a macOS removable-volume
//! permission prompt) never leaves the port dead.
//!
//! Until the library is ready, `GET /health` reports `starting` and authenticated endpoints
//! answer 503 `UNAVAILABLE`. A failed open is logged, reported as `error`, and retried with
//! exponential backoff instead of exiting, so launchd does not crash-loop the process.
use crate::{
    ApiError, AppState, ServerConfig, auth::Security, config::BearerToken,
    state::spawn_revision_poller,
};
use std::{
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

/// Fixed client-facing text for a failed library open (`/health` and 503 replies).
pub const FAILED_MESSAGE: &str = "The library could not be opened; the server keeps retrying";

/// Library lifecycle as seen by request handlers.
#[derive(Clone)]
pub enum Readiness {
    /// The library is still being opened.
    Starting,
    /// The library is open and endpoints are served.
    Ready(AppState),
    /// The last open attempt failed with this error (logged, never sent to clients);
    /// a retry is pending.
    Failed(String),
}

struct ServiceInner {
    security: Arc<Security>,
    readiness: RwLock<Readiness>,
}

/// Router state: authentication and admin settings ([`Security`], known before the
/// library opens) plus [`Readiness`].
#[derive(Clone)]
pub struct Service {
    inner: Arc<ServiceInner>,
}

impl Service {
    /// A service whose library is not open yet with fixed in-memory security: the token
    /// is required everywhere, `None` disables authentication ([`Security::fixed`]).
    pub fn starting(token: impl Into<Option<BearerToken>>) -> Self {
        Self::with(Security::fixed(token.into()), Readiness::Starting)
    }

    /// A service whose library is not open yet, using `security` (built from the
    /// configuration by [`Security::from_config`]).
    pub fn starting_with(security: Arc<Security>) -> Self {
        Self::with(security, Readiness::Starting)
    }

    fn with(security: Arc<Security>, readiness: Readiness) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                security,
                readiness: RwLock::new(readiness),
            }),
        }
    }

    /// Current readiness (a cheap clone).
    pub fn readiness(&self) -> Readiness {
        self.inner
            .readiness
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Mark the library as open; later [`Service::set_failed`] calls are ignored.
    pub fn set_ready(&self, state: AppState) {
        *self
            .inner
            .readiness
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Readiness::Ready(state);
    }

    /// Record a failed open attempt unless the library is already open.
    pub fn set_failed(&self, message: impl Into<String>) {
        let mut readiness = self
            .inner
            .readiness
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if !matches!(*readiness, Readiness::Ready(_)) {
            *readiness = Readiness::Failed(message.into());
        }
    }

    /// The open library, or 503 `UNAVAILABLE` while starting or after a failed open.
    pub fn app_state(&self) -> Result<AppState, ApiError> {
        match self.readiness() {
            Readiness::Ready(state) => Ok(state),
            Readiness::Starting => Err(ApiError::unavailable(
                "The library is still opening; retry shortly",
            )),
            // The raw open error (paths, OS errors) stays in the server log only.
            Readiness::Failed(_) => Err(ApiError::unavailable(FAILED_MESSAGE)),
        }
    }

    /// Authentication and admin settings.
    pub fn security(&self) -> &Arc<Security> {
        &self.inner.security
    }
}

impl From<AppState> for Service {
    fn from(state: AppState) -> Self {
        Self::with(state.security().clone(), Readiness::Ready(state))
    }
}

/// Backoff between failed library open attempts.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Delay after the first failure; doubled after each further failure.
    pub initial: Duration,
    /// Upper bound for the delay.
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(5),
            max: Duration::from_secs(300),
        }
    }
}

/// Open the library on a dedicated OS thread (not the Tokio blocking pool, so a stuck open
/// never delays runtime shutdown), retrying failures per `retry`. On success the service
/// becomes ready (sharing the service's [`Security`]) and the revision poller starts on
/// the runtime behind `runtime`.
pub fn open_in_background(
    service: Service,
    config: ServerConfig,
    retry: RetryPolicy,
    runtime: tokio::runtime::Handle,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let security = service.security().clone();
    std::thread::Builder::new()
        .name("audiolib-library-open".into())
        .spawn(move || {
            let mut delay = retry.initial;
            let mut attempt: u32 = 1;
            loop {
                tracing::info!(library = %config.library_db.display(), attempt, "Opening library");
                // A panic inside the open must not leave the service `starting` forever.
                let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    AppState::open_with_security(&config, security.clone())
                }))
                .unwrap_or_else(|_| {
                    Err(audiolib_core::Error::Database(
                        "Library open panicked".into(),
                    ))
                });
                match opened {
                    Ok(state) => {
                        match state.revision() {
                            Ok(revision) => tracing::info!(
                                library = %config.library_db.display(),
                                library_id = %revision.library_id,
                                revision = revision.revision,
                                roots = config.roots.len(),
                                "Library opened"
                            ),
                            Err(error) => tracing::warn!(%error, "Library opened without revision"),
                        }
                        let _runtime = runtime.enter();
                        spawn_revision_poller(
                            state.clone(),
                            Duration::from_secs(config.revision_poll_secs),
                        );
                        service.set_ready(state);
                        return;
                    }
                    Err(error) => {
                        tracing::error!(
                            library = %config.library_db.display(),
                            %error,
                            retry_secs = delay.as_secs_f64(),
                            "Cannot open library; serving /health with status error and retrying"
                        );
                        service.set_failed(error.to_string());
                    }
                }
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2).min(retry.max);
                attempt = attempt.saturating_add(1);
            }
        })
}
