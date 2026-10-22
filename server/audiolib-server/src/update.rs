// server/audiolib-server/src/update.rs
//! Process-local release checks and confirmed server updates. Downloads never hold a
//! library lock; deployment is handed to main only after normal shutdown has drained.
use crate::ApiError;
use audiolib_update::{Channel, PreparedUpdate, Product, ReleaseUpdate, UpdateClient};
use axum::http::StatusCode;
use serde::Serialize;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use tokio::sync::watch;

/// Admin-facing update status. No local paths or downloaded release text are exposed.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    /// `disabled`, `checking`, `current`, `available`, `downloading`, `ready` or `error`.
    pub state: String,
    /// The currently running program version.
    pub current_version: String,
    /// Installed release channel, absent while updates are disabled.
    pub channel: Option<&'static str>,
    /// Release tag that must be confirmed by the install request.
    pub tag: Option<String>,
    /// Fixed-repository GitHub Release page for the available update.
    pub release_url: Option<String>,
    /// Human-readable status suitable for the LAN admin page.
    pub message: String,
}

struct Context {
    install_dir: PathBuf,
    restart_args: Vec<OsString>,
    client: UpdateClient,
}

struct State {
    status: UpdateStatus,
    context: Option<Arc<Context>>,
    release: Option<ReleaseUpdate>,
    prepared: Option<PreparedUpdate>,
    stopping: bool,
}

struct Inner {
    state: Mutex<State>,
    shutdown: watch::Sender<bool>,
}

/// One serialized release operation per server process; cloneable router state.
#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<Inner>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    status: UpdateStatus {
                        state: "disabled".into(),
                        current_version: crate::VERSION.into(),
                        channel: None,
                        tag: None,
                        release_url: None,
                        message: "此进程未启用自动更新".into(),
                    },
                    context: None,
                    release: None,
                    prepared: None,
                    stopping: false,
                }),
                shutdown,
            }),
        }
    }
}

impl UpdateManager {
    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Configure updates beside the running server and check once without delaying bind.
    pub fn start(&self, restart_args: Vec<OsString>) -> anyhow::Result<()> {
        let result = self.configure(restart_args);
        if let Err(error) = &result {
            self.failed("更新检查无法初始化，服务继续运行", error);
        }
        result
    }

    fn configure(&self, restart_args: Vec<OsString>) -> anyhow::Result<()> {
        if std::env::var_os("AUDIOLIB_UPDATE_DISABLE").as_deref() == Some(std::ffi::OsStr::new("1"))
        {
            return Ok(());
        }
        let executable = std::env::current_exe()?.canonicalize()?;
        let install_dir = executable
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Server executable has no parent directory"))?
            .to_path_buf();
        let client = UpdateClient::for_install(Product::Server, crate::VERSION, &install_dir)?;
        {
            let mut state = self.state();
            state.status.current_version = client.current_version();
            state.status.channel = Some(match client.channel() {
                Channel::Stable => "stable",
                Channel::Beta => "beta",
            });
            state.context = Some(Arc::new(Context {
                install_dir,
                restart_args,
                client,
            }));
        }
        self.check()
            .map_err(|error| anyhow::anyhow!("Cannot start update check: {}", error.code()))?;
        Ok(())
    }

    /// Cheap owned status for the LAN-only admin endpoint.
    pub fn status(&self) -> UpdateStatus {
        self.state().status.clone()
    }

    /// Start a bounded network check on a dedicated thread. Concurrent operations fail.
    pub fn check(&self) -> Result<UpdateStatus, ApiError> {
        let context = {
            let mut state = self.state();
            if state.stopping {
                return Err(ApiError::unavailable("The server is shutting down"));
            }
            if matches!(
                state.status.state.as_str(),
                "checking" | "downloading" | "ready"
            ) {
                return Err(busy("An update operation is already in progress"));
            }
            let context = state
                .context
                .clone()
                .ok_or_else(|| ApiError::unavailable("Automatic updates are disabled"))?;
            state.release = None;
            state.status.state = "checking".into();
            state.status.tag = None;
            state.status.release_url = None;
            state.status.message = "正在检查 GitHub Release…".into();
            context
        };
        let manager = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("server-update-check".into())
            .spawn(move || match context.client.check() {
                Ok(release) => {
                    let mut state = manager.state();
                    if state.stopping {
                        return;
                    }
                    match &release {
                        Some(release) => {
                            tracing::info!(tag = %release.tag, "Server update available; confirm on /admin");
                            state.status.state = "available".into();
                            state.status.tag = Some(release.tag.clone());
                            state.status.release_url = Some(release.release_url.clone());
                            state.status.message = format!("发现新版本 {}，确认后下载并自动部署", release.tag);
                        }
                        None => {
                            state.status.state = "current".into();
                            state.status.message = "当前已是此更新通道的最新版本".into();
                        }
                    }
                    state.release = release;
                }
                Err(error) => manager.failed("检查更新失败，请稍后重试", &error),
            })
        {
            self.failed("无法启动更新检查", &error.into());
            return Err(ApiError::internal("Cannot start the update check"));
        }
        Ok(self.status())
    }

    /// Download only the checked release whose exact tag the administrator confirmed.
    pub fn install(&self, tag: &str) -> Result<UpdateStatus, ApiError> {
        let (context, release) = {
            let mut state = self.state();
            if state.stopping {
                return Err(ApiError::unavailable("The server is shutting down"));
            }
            if state.status.state != "available" {
                return Err(busy("Check for an available update before installing"));
            }
            let release = state
                .release
                .clone()
                .filter(|release| release.tag == tag)
                .ok_or_else(|| ApiError::validation("Confirm the exact available release tag"))?;
            let context = state
                .context
                .clone()
                .ok_or_else(|| ApiError::unavailable("Automatic updates are disabled"))?;
            state.status.state = "downloading".into();
            state.status.message = "正在下载并校验；完成后服务端会排空请求、部署并重启".into();
            (context, release)
        };
        let manager = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("server-update-download".into())
            .spawn(
                move || match context.client.prepare(&release, &context.install_dir) {
                    Ok(prepared) => {
                        {
                            let mut state = manager.state();
                            if state.stopping {
                                // An external stop won the race with the download. Drop the
                                // staging directory and let that stop finish normally.
                                return;
                            }
                            state.prepared = Some(prepared);
                            state.status.state = "ready".into();
                            state.status.message = "更新校验完成，正在安全关闭并重启服务端…".into();
                            manager.inner.shutdown.send_replace(true);
                        }
                    }
                    Err(error) => {
                        manager.failed("下载或校验失败，原程序保持运行，请重试检查更新", &error)
                    }
                },
            )
        {
            self.failed("无法启动更新下载", &error.into());
            return Err(ApiError::internal("Cannot start the update download"));
        }
        Ok(self.status())
    }

    fn failed(&self, message: &str, error: &anyhow::Error) {
        tracing::warn!(%error, "Server update failed");
        let mut state = self.state();
        state.status.state = "error".into();
        state.status.message = message.into();
    }

    /// Persistent notification; completion cannot be missed before main starts waiting.
    pub fn shutdown_watch(&self) -> watch::Receiver<bool> {
        self.inner.shutdown.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn signal_shutdown_for_test(&self) {
        self.inner.shutdown.send_replace(true);
    }

    /// Refuse new checks/downloads once normal graceful shutdown begins.
    pub fn stop_admission(&self) {
        self.state().stopping = true;
    }

    /// Take the verified update only after library shutdown succeeded. An unclean close
    /// drops staging and never replaces the installed server.
    pub fn take_prepared(&self) -> Option<(PreparedUpdate, Vec<OsString>)> {
        let mut state = self.state();
        let args = state.context.as_ref()?.restart_args.clone();
        state.prepared.take().map(|prepared| (prepared, args))
    }
}

fn busy(message: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "UPDATE_BUSY", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_router_state_never_contacts_network_or_requests_shutdown() {
        let manager = UpdateManager::default();
        assert_eq!(manager.status().state, "disabled");
        assert!(manager.check().is_err());
        assert!(manager.install("v99.0.0").is_err());
        assert!(!*manager.shutdown_watch().borrow());
        assert!(manager.take_prepared().is_none());
    }

    #[test]
    fn checking_and_downloading_cannot_be_reentered() {
        let manager = UpdateManager::default();
        for state in ["checking", "downloading", "ready"] {
            manager.state().status.state = state.into();
            let error = manager.check().unwrap_err();
            assert_eq!(error.code(), "UPDATE_BUSY");
            assert!(manager.install("v99.0.0").is_err());
        }
    }

    #[test]
    fn unchecked_or_stale_confirmation_never_enters_downloading() {
        let manager = UpdateManager::default();
        manager.state().status.state = "available".into();
        assert_eq!(manager.install("v99.0.0").unwrap_err().code(), "VALIDATION");
        assert_eq!(manager.status().state, "available");
        assert!(!*manager.shutdown_watch().borrow());
    }

    #[test]
    fn failed_download_keeps_process_running_and_shutdown_refuses_new_work() {
        let manager = UpdateManager::default();
        manager.state().status.state = "downloading".into();
        manager.failed("下载失败", &anyhow::anyhow!("checksum mismatch"));
        assert_eq!(manager.status().state, "error");
        assert!(!*manager.shutdown_watch().borrow());
        assert!(manager.take_prepared().is_none());
        manager.stop_admission();
        assert_eq!(manager.check().unwrap_err().code(), "UNAVAILABLE");
        assert_eq!(
            manager.install("v99.0.0").unwrap_err().code(),
            "UNAVAILABLE"
        );
    }
}
