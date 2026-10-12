// app/audiolib-desktop/src/startup.rs
//! Show a responsive window while owned library initialization runs off the UI thread.
use crate::{AppWindow, controller};
use anyhow::{Context, Result};
use audiolib_core::config::{AppConfig, DataSourceMode};
use audiolib_db::{SqliteProvider, import_legacy};
use slint::ComponentHandle;
use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

type OpenedLibrary = (Arc<SqliteProvider>, Option<controller::RemoteStartup>);

/// Open the selected source without touching the local working library in server mode.
pub(super) fn open_library(config: &AppConfig) -> Result<OpenedLibrary> {
    let started = Instant::now();
    let opened = if config.data_source.mode == DataSourceMode::Server {
        let startup = controller::open_remote_startup(config)
            .context("Open the server-mode library cache")?;
        tracing::info!(
            synced = startup.state.is_some(),
            "Starting with the server data source"
        );
        (startup.provider.clone(), Some(startup))
    } else {
        if !config.paths.database.exists()
            && let Some(source) = &config.paths.legacy_database
        {
            let report =
                import_legacy(source, &config.paths.database).context("Import legacy library")?;
            tracing::info!(?report, "Legacy library imported; source preserved");
        }
        let sqlite = if config.paths.database.exists() {
            SqliteProvider::open_existing(&config.paths.database)
        } else {
            SqliteProvider::open(&config.paths.database)
        }
        .context("Open library database")?;
        // Keep the feature branch's marker-guarded source-tag FTS upgrade inside
        // this worker, so the visible startup window remains responsive and a
        // requested close drains initialization before releasing the instance lease.
        prepare_local_search(&sqlite);
        (Arc::new(sqlite), None)
    };
    if !config.paths.settings.exists() {
        config
            .save()
            .context("Persist initial library preferences")?;
    }
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        "Library startup complete"
    );
    Ok(opened)
}

/// Repair marker-guarded source-tag FTS state in the caller's existing library worker.
/// A competing scan/maintenance lease defers this derived upgrade until the next open.
pub(super) fn prepare_local_search(provider: &SqliteProvider) {
    match provider.ensure_source_search_current() {
        Ok(true) => tracing::info!("Source search text rebuilt for extracted tags"),
        Ok(false) => {}
        Err(error) => tracing::warn!(%error, "Source search text migration deferred"),
    }
}

/// Display preparation status before a potentially large snapshot copy/index upgrade.
/// Closing waits for the worker to finish safely while the instance lease stays held.
pub(super) fn show_while_opening(
    ui: &AppWindow,
    config: AppConfig,
) -> Result<Option<OpenedLibrary>> {
    let server = config.data_source.mode == DataSourceMode::Server;
    show_while_working(ui, server, move || open_library(&config))
}

fn show_while_working<T: Send + 'static>(
    ui: &AppWindow,
    server: bool,
    open: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<Option<T>> {
    ui.set_startup_busy(true);
    let closing = Rc::new(Cell::new(false));
    let close_state = closing.clone();
    ui.window().on_close_requested(move || {
        close_state.set(true);
        slint::CloseRequestResponse::KeepWindowShown
    });
    ui.set_startup_message(wait_message(ui.get_chinese(), server, false, 0).into());
    ui.show().context("Show startup window")?;
    let (complete, completion) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("library-startup".into())
        .spawn(move || {
            let result = open();
            let _ = complete.send(());
            result
        })
        .context("Start library initialization worker")?;
    let weak = ui.as_weak();
    let started = Instant::now();
    let timer = slint::Timer::default();
    let close_state = closing.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_startup_message(
                    wait_message(
                        ui.get_chinese(),
                        server,
                        close_state.get(),
                        started.elapsed().as_secs(),
                    )
                    .into(),
                );
            }
            // Disconnection also wakes the UI if initialization panics; join reports it.
            if !matches!(
                completion.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                let _ = slint::quit_event_loop();
            }
        },
    );
    let event_loop = slint::run_event_loop().context("Run startup event loop");
    timer.stop();
    let opened = worker
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Library initialization worker panicked")));
    ui.set_startup_busy(false);
    if event_loop.is_err() || opened.is_err() || closing.get() {
        let _ = ui.hide();
    }
    event_loop?;
    let opened = opened?;
    Ok((!closing.get()).then_some(opened))
}

fn wait_message(chinese: bool, server: bool, closing: bool, seconds: u64) -> String {
    let message = match (chinese, server, closing) {
        (true, _, true) => "正在安全完成音效库准备，完成后退出。",
        (false, _, true) => "Finishing library preparation safely before closing.",
        (true, true, false) => "正在准备服务器缓存。首次升级大型音效库可能需要几分钟。",
        (false, true, false) => {
            "Preparing the server cache. The first upgrade of a large library may take a few minutes."
        }
        (true, false, false) => {
            "正在准备音效库与搜索索引。首次导入、升级或重建大型音效库可能需要几分钟。"
        }
        (false, false, false) => {
            "Preparing your library and search index. The first import, upgrade or rebuild of a large library may take a few minutes."
        }
    };
    if chinese {
        format!("{message}\n已等待 {seconds} 秒")
    } else {
        format!("{message}\nElapsed: {seconds} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an interactive Windows desktop and real Slint backend"]
    fn real_windows_startup_renders_and_drains_before_closing() {
        let ui = AppWindow::new().unwrap();
        ui.set_chinese(true);
        let weak = ui.as_weak();
        let rendered = Rc::new(Cell::new(false));
        let observed = rendered.clone();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(100),
            move || {
                let ui = weak.upgrade().unwrap();
                assert!(ui.get_startup_busy() && ui.window().is_visible());
                let pixels = ui.window().take_snapshot().unwrap();
                assert!(pixels.width() >= 1080 && pixels.height() >= 700);
                observed.set(true);
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            },
        );
        let started = Instant::now();
        let result = show_while_working(&ui, true, || {
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        })
        .unwrap();
        assert!(rendered.get() && result.is_none());
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert!(!ui.window().is_visible());
        // The same native event loop can subsequently open the normal window.
        assert_eq!(show_while_working(&ui, false, || Ok(9)).unwrap(), Some(9));
        ui.hide().unwrap();
    }

    #[test]
    fn startup_keeps_event_loop_responsive_and_returns_worker_errors() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::tests::child_startup",
                "--ignored",
                "--test-threads=1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "isolated Slint startup helper invoked by parent test"]
    fn child_startup() {
        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        let ticks = Rc::new(Cell::new(0));
        let observed = ticks.clone();
        let weak = ui.as_weak();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(10),
            move || {
                assert!(weak.upgrade().unwrap().get_startup_busy());
                observed.set(observed.get() + 1);
            },
        );
        let value = show_while_working(&ui, true, || {
            std::thread::sleep(Duration::from_millis(120));
            Ok(42)
        })
        .unwrap();
        timer.stop();
        assert_eq!(value, Some(42));
        assert!(ticks.get() > 3);
        assert!(!ui.get_startup_busy());
        let weak = ui.as_weak();
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(20),
            move || {
                weak.upgrade()
                    .unwrap()
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            },
        );
        let complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = complete.clone();
        let closed = show_while_working(&ui, true, move || {
            std::thread::sleep(Duration::from_millis(100));
            finished.store(true, std::sync::atomic::Ordering::Release);
            Ok(7)
        })
        .unwrap();
        assert!(closed.is_none());
        assert!(complete.load(std::sync::atomic::Ordering::Acquire));
        assert!(!ui.window().is_visible());
        timer.stop();
        let error = show_while_working::<()>(&ui, false, || anyhow::bail!("fixture open failure"))
            .unwrap_err();
        assert!(error.to_string().contains("fixture open failure"));
    }
}
