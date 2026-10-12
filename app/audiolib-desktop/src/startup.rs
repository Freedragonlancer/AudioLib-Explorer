// app/audiolib-desktop/src/startup.rs
//! Show a responsive window while owned library initialization runs off the UI thread.
use crate::{AppWindow, controller};
use anyhow::{Context, Result};
use audiolib_core::config::{AppConfig, DataSourceMode};
use audiolib_db::{SqliteProvider, import_legacy};
use audiolib_remote::CachePreparation;
use slint::ComponentHandle;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type OpenedLibrary = (Arc<SqliteProvider>, Option<controller::RemoteStartup>);

/// Owned startup status, coalesced before publishing on the Slint event loop.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum StartupProgress {
    #[default]
    Opening,
    Importing,
    Search,
    Preferences,
    Placeholder,
    Remote(CachePreparation),
}

impl StartupProgress {
    pub(crate) fn label(self, chinese: bool) -> &'static str {
        let (cn, en) = match self {
            Self::Opening => ("正在打开音效库", "Opening the library"),
            Self::Importing => ("正在导入原有音效库", "Importing the existing library"),
            Self::Search => ("正在准备搜索索引", "Preparing the search index"),
            Self::Preferences => ("正在保存初始设置", "Saving initial preferences"),
            Self::Placeholder => ("正在准备服务器视图", "Preparing the server view"),
            Self::Remote(CachePreparation::Checking) => {
                ("正在检查服务器缓存", "Checking the server cache")
            }
            Self::Remote(CachePreparation::Copying { .. }) => ("正在复制缓存", "Copying the cache"),
            Self::Remote(CachePreparation::Validating) => {
                ("正在校验缓存完整性", "Validating cache integrity")
            }
            Self::Remote(CachePreparation::Upgrading) => {
                ("正在升级搜索索引", "Upgrading the search index")
            }
            Self::Remote(CachePreparation::Saving) => ("正在保存缓存", "Saving the cache"),
        };
        if chinese { cn } else { en }
    }

    /// Only the copy stage has a measurable total; all other stages are indeterminate.
    pub(crate) fn fraction(self) -> f32 {
        match self {
            Self::Remote(CachePreparation::Copying { completed, total }) if total > 0 => {
                (completed.min(total) as f64 / total as f64) as f32
            }
            _ => -1.0,
        }
    }
}

/// Open the selected source without touching the local working library in server mode.
pub(super) fn open_library(config: &AppConfig) -> Result<OpenedLibrary> {
    open_library_with_progress(config, &mut |_| {})
}

fn open_library_with_progress(
    config: &AppConfig,
    progress: &mut dyn FnMut(StartupProgress),
) -> Result<OpenedLibrary> {
    let started = Instant::now();
    let opened = if config.data_source.mode == DataSourceMode::Server {
        let startup = controller::open_remote_startup_with_progress(config, progress)
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
            progress(StartupProgress::Importing);
            let report =
                import_legacy(source, &config.paths.database).context("Import legacy library")?;
            tracing::info!(?report, "Legacy library imported; source preserved");
        }
        progress(StartupProgress::Opening);
        let sqlite = if config.paths.database.exists() {
            SqliteProvider::open_existing(&config.paths.database)
        } else {
            SqliteProvider::open(&config.paths.database)
        }
        .context("Open library database")?;
        // Keep the feature branch's marker-guarded source-tag FTS upgrade inside
        // this worker, so the visible startup window remains responsive and a
        // requested close drains initialization before releasing the instance lease.
        progress(StartupProgress::Search);
        prepare_local_search(&sqlite);
        (Arc::new(sqlite), None)
    };
    if !config.paths.settings.exists() {
        progress(StartupProgress::Preferences);
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
    if server {
        // Server cache work belongs to the manager, not a second startup pipeline.
        // This small placeholder never touches the configured local working library.
        let remote = controller::open_remote_placeholder(&config)?;
        if !config.paths.settings.exists() {
            config.save()?;
        }
        return Ok(Some((remote.provider.clone(), Some(remote))));
    }
    show_while_working(ui, server, move |progress| {
        open_library_with_progress(&config, progress)
    })
}

fn show_while_working<T: Send + 'static>(
    ui: &AppWindow,
    server: bool,
    open: impl FnOnce(&mut dyn FnMut(StartupProgress)) -> Result<T> + Send + 'static,
) -> Result<Option<T>> {
    ui.set_startup_busy(true);
    let closing = Rc::new(Cell::new(false));
    let close_state = closing.clone();
    ui.window().on_close_requested(move || {
        close_state.set(true);
        slint::CloseRequestResponse::KeepWindowShown
    });
    let initial = if server {
        StartupProgress::Remote(CachePreparation::Checking)
    } else {
        StartupProgress::Opening
    };
    update_progress(ui, initial, server, false, Duration::ZERO);
    ui.show().context("Show startup window")?;
    // One latest status bounds memory even when a large backup reports many steps.
    // The worker never calls Slint or waits for the event loop to consume a message.
    let progress = Arc::new(Mutex::new(initial));
    let worker_progress = progress.clone();
    let (complete, completion) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("library-startup".into())
        .spawn(move || {
            let result = open(&mut |next| *worker_progress.lock().unwrap() = next);
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
                let latest = *progress.lock().unwrap();
                update_progress(&ui, latest, server, close_state.get(), started.elapsed());
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

fn update_progress(
    ui: &AppWindow,
    progress: StartupProgress,
    server: bool,
    closing: bool,
    elapsed: Duration,
) {
    let chinese = ui.get_chinese();
    ui.set_startup_closing(closing);
    ui.set_startup_stage(progress.label(chinese).into());
    ui.set_startup_progress(progress.fraction());
    // A moving segment expresses activity without inventing an overall percentage.
    ui.set_startup_motion((1.0 - (elapsed.as_secs_f32() * std::f32::consts::PI).cos()) / 2.0);
    ui.set_startup_message(wait_message(chinese, server, closing).into());
    let seconds = elapsed.as_secs();
    ui.set_startup_elapsed(
        if chinese {
            format!("已用时 {seconds} 秒")
        } else {
            format!("Elapsed: {seconds} s")
        }
        .into(),
    );
}

fn wait_message(chinese: bool, server: bool, closing: bool) -> &'static str {
    match (chinese, server, closing) {
        (true, _, true) => "正在安全完成音效库准备，完成后退出。",
        (false, _, true) => "Finishing library preparation safely before closing.",
        (true, true, false) => "首次升级大型音效库可能需要几分钟，请稍候。",
        (false, true, false) => "The first upgrade of a large library may take a few minutes.",
        (true, false, false) => "首次导入或升级大型音效库可能需要几分钟，请稍候。",
        (false, false, false) => {
            "The first import or upgrade of a large library may take a few minutes."
        }
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
        for (name, chinese, stage) in [
            (
                "copy-cn",
                true,
                CachePreparation::Copying {
                    completed: 3,
                    total: 8,
                },
            ),
            ("index-cn", true, CachePreparation::Upgrading),
            (
                "copy-en",
                false,
                CachePreparation::Copying {
                    completed: 3,
                    total: 8,
                },
            ),
            ("index-en", false, CachePreparation::Upgrading),
        ] {
            ui.set_chinese(chinese);
            let weak = ui.as_weak();
            let rendered = Rc::new(Cell::new(false));
            let observed = rendered.clone();
            let (snapshot_ready, snapshot_taken) = std::sync::mpsc::channel();
            let started = Instant::now();
            let timer = slint::Timer::default();
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(50),
                move || {
                    if observed.get() {
                        return;
                    }
                    assert!(started.elapsed() < Duration::from_secs(10));
                    let ui = weak.upgrade().unwrap();
                    // Native window creation can consume the first timer deadline.
                    // Wait for the worker's stage to reach the UI before capturing.
                    if ui.get_startup_stage() != StartupProgress::Remote(stage).label(chinese) {
                        return;
                    }
                    assert!(ui.get_startup_busy() && ui.window().is_visible());
                    let pixels = ui.window().take_snapshot().unwrap();
                    assert!(pixels.width() >= 1080 && pixels.height() >= 700);
                    assert_eq!(
                        ui.get_startup_stage(),
                        StartupProgress::Remote(stage).label(chinese)
                    );
                    assert_eq!(
                        ui.get_startup_progress(),
                        StartupProgress::Remote(stage).fraction()
                    );
                    if let Some(directory) = std::env::var_os("AUDIOLIB_STARTUP_SCREENSHOT_DIR") {
                        let path = std::path::Path::new(&directory).join(format!("{name}.png"));
                        image::save_buffer(
                            path,
                            pixels.as_bytes(),
                            pixels.width(),
                            pixels.height(),
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                    observed.set(true);
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                    snapshot_ready.send(()).unwrap();
                },
            );
            let result = show_while_working(&ui, true, move |progress| {
                progress(StartupProgress::Remote(stage));
                snapshot_taken.recv_timeout(Duration::from_secs(10))?;
                std::thread::sleep(Duration::from_millis(400));
                Ok(())
            })
            .unwrap();
            timer.stop();
            assert!(rendered.get() && result.is_none());
            assert!(started.elapsed() >= Duration::from_millis(400));
            assert!(!ui.window().is_visible());
            assert!(ui.get_startup_closing());
        }
        // The same native event loop can subsequently open the normal window.
        assert_eq!(show_while_working(&ui, false, |_| Ok(9)).unwrap(), Some(9));
        ui.hide().unwrap();
        // The manager must keep preparation progress visible through the source
        // write gate, in both languages and at the native window's scale.
        for chinese in [true, false] {
            ui.set_chinese(chinese);
            ui.set_library_visible(true);
            ui.set_sm_sources(slint::ModelRc::new(slint::VecModel::from(vec![
                crate::SourceRow {
                    name: "Local library".into(),
                    ..Default::default()
                },
                crate::SourceRow {
                    name: "AudioLib Server".into(),
                    kind: 1,
                    active: true,
                    ..Default::default()
                },
            ])));
            ui.set_sm_selected(1);
            ui.set_sm_page(0);
            ui.set_ds_applying(true);
            ui.set_library_busy(true);
            ui.set_ds_syncing(true);
            ui.set_ds_progress(0.375);
            ui.set_ds_sync_info(
                if chinese {
                    "正在复制缓存 · 38%"
                } else {
                    "Copying the cache · 38%"
                }
                .into(),
            );
            let weak = ui.as_weak();
            let timer = slint::Timer::default();
            timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(100),
                move || {
                    let ui = weak.upgrade().unwrap();
                    let pixels = ui.window().take_snapshot().unwrap();
                    if let Some(directory) = std::env::var_os("AUDIOLIB_STARTUP_SCREENSHOT_DIR") {
                        let name = if chinese {
                            "manager-cn.png"
                        } else {
                            "manager-en.png"
                        };
                        image::save_buffer(
                            std::path::Path::new(&directory).join(name),
                            pixels.as_bytes(),
                            pixels.width(),
                            pixels.height(),
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                    slint::quit_event_loop().unwrap();
                },
            );
            ui.show().unwrap();
            slint::run_event_loop().unwrap();
            ui.hide().unwrap();
            ui.set_ds_syncing(false);
        }
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
        let saw_copy = Rc::new(Cell::new(false));
        let saw_index = Rc::new(Cell::new(false));
        let saw_motion = Rc::new(Cell::new(false));
        let copy = saw_copy.clone();
        let index = saw_index.clone();
        let motion = saw_motion.clone();
        let observed = ticks.clone();
        let weak = ui.as_weak();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(10),
            move || {
                let ui = weak.upgrade().unwrap();
                assert!(ui.get_startup_busy());
                if ui.get_startup_stage() == "Copying the cache" {
                    assert_eq!(ui.get_startup_progress(), 0.375);
                    copy.set(true);
                }
                if ui.get_startup_stage() == "Upgrading the search index" {
                    assert!(ui.get_startup_progress() < 0.0);
                    index.set(true);
                }
                if ui.get_startup_motion() > 0.0 {
                    motion.set(true);
                }
                observed.set(observed.get() + 1);
            },
        );
        ui.set_chinese(false);
        let value = show_while_working(&ui, true, |progress| {
            progress(StartupProgress::Remote(CachePreparation::Copying {
                completed: 3,
                total: 8,
            }));
            std::thread::sleep(Duration::from_millis(200));
            progress(StartupProgress::Remote(CachePreparation::Upgrading));
            std::thread::sleep(Duration::from_millis(200));
            Ok(42)
        })
        .unwrap();
        timer.stop();
        assert_eq!(value, Some(42));
        assert!(ticks.get() > 3);
        assert!(saw_copy.get() && saw_index.get() && saw_motion.get());
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
        let closed = show_while_working(&ui, true, move |_| {
            std::thread::sleep(Duration::from_millis(100));
            finished.store(true, std::sync::atomic::Ordering::Release);
            Ok(7)
        })
        .unwrap();
        assert!(closed.is_none());
        assert!(complete.load(std::sync::atomic::Ordering::Acquire));
        assert!(ui.get_startup_closing());
        assert!(ui.get_startup_message().contains("safely before closing"));
        assert!(!ui.window().is_visible());
        timer.stop();
        let error = show_while_working::<()>(&ui, false, |_| anyhow::bail!("fixture open failure"))
            .unwrap_err();
        assert!(error.to_string().contains("fixture open failure"));
    }
}
