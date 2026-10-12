// crates/audiolib-ipc/src/lib.rs
//! Bounded, local-only named-pipe playback control. Never carries realtime PCM.
use audiolib_core::{Error, Result};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
// Only the Windows named-pipe implementation uses these; gating them keeps
// `clippy -D warnings` clean on other targets.
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
const MAX_MESSAGE: usize = 65_536;
type Handler = Arc<dyn Fn(Value) -> Value + Send + Sync>;

/// A server lifetime: dropping it requests background listener shutdown.
pub struct ServerGuard {
    shutdown: Arc<AtomicBool>,
}
impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

/// The pipe has a per-user name and rejects remote clients.
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "local".into());
    let user: String = user
        .chars()
        .filter(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    let name = format!(r"\\.\pipe\AudioLibExplorer.v1.{user}");
    // Unit tests must neither collide with nor send commands to a user's desktop.
    #[cfg(test)]
    let name = format!("{name}.test.{}", std::process::id());
    name
}

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    async fn read_message<T: tokio::io::AsyncRead + Unpin>(stream: &mut T) -> Result<Value> {
        let length = stream.read_u32_le().await? as usize;
        if length == 0 || length > MAX_MESSAGE {
            return Err(Error::Validation("IPC message is too large".into()));
        }
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await?;
        serde_json::from_slice(&bytes)
            .map_err(|error| Error::Validation(format!("IPC JSON: {error}")))
    }
    async fn write_message<T: tokio::io::AsyncWrite + Unpin>(
        stream: &mut T,
        value: &Value,
    ) -> Result<()> {
        let bytes =
            serde_json::to_vec(value).map_err(|error| Error::Validation(error.to_string()))?;
        if bytes.len() > MAX_MESSAGE {
            return Err(Error::Validation("IPC response is too large".into()));
        }
        stream.write_u32_le(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
        Ok(())
    }

    pub fn start_server(handler: Handler) -> Result<ServerGuard> {
        let name = pipe_name();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let server = {
            let _context = runtime.enter();
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)?
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let signal = shutdown.clone();
        std::thread::Builder::new()
            .name("audiolib-local-control".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let mut listener = server;
                    while !signal.load(Ordering::Acquire) {
                        match tokio::time::timeout(Duration::from_millis(200), listener.connect())
                            .await
                        {
                            Err(_) => continue,
                            Ok(Err(error)) => {
                                tracing::warn!(%error, "Local control connection failed");
                                break;
                            }
                            Ok(Ok(())) => {}
                        }
                        let next = match ServerOptions::new()
                            .reject_remote_clients(true)
                            .create(&name)
                        {
                            Ok(next) => next,
                            Err(error) => {
                                tracing::warn!(%error, "Local control listener failed");
                                break;
                            }
                        };
                        let mut connection = std::mem::replace(&mut listener, next);
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            let transaction = async {
                                let request = read_message(&mut connection).await?;
                                let reply = handler(request);
                                write_message(&mut connection, &reply).await
                            };
                            let _ = tokio::time::timeout(Duration::from_secs(5), transaction).await;
                        });
                    }
                });
            })?;
        Ok(ServerGuard { shutdown })
    }

    pub fn request(value: Value) -> Result<Value> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let mut stream = ClientOptions::new().open(pipe_name()).map_err(|error| {
                Error::NotFound(format!(
                    "AudioLib Explorer is not running or local control is unavailable: {error}"
                ))
            })?;
            tokio::time::timeout(Duration::from_secs(5), async {
                write_message(&mut stream, &value).await?;
                read_message(&mut stream).await
            })
            .await
            .map_err(|_| Error::Validation("Local playback control timed out".into()))?
        })
    }
}
#[cfg(windows)]
pub use platform::{request, start_server};

#[cfg(not(windows))]
pub fn start_server(_: Handler) -> Result<ServerGuard> {
    Err(Error::Validation("Windows named pipes are required".into()))
}
#[cfg(not(windows))]
pub fn request(_: Value) -> Result<Value> {
    Err(Error::Validation("Windows named pipes are required".into()))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn bounded_local_control_roundtrip() {
        let guard = start_server(Arc::new(audiolib_core::success)).unwrap();
        let value = request(serde_json::json!({"command":"status"})).unwrap();
        assert_eq!(value["data"]["command"], "status");
        assert!(request(serde_json::json!({"large":"x".repeat(MAX_MESSAGE)})).is_err());
        drop(guard);
    }
}
