// app/audiolib-desktop/src/client_review_tests.rs
//! UI controller regressions use owned empty libraries and no playback device.
use super::*;

fn fixture() -> (
    tempfile::TempDir,
    AppWindow,
    audiolib_audio::AudioEngine,
    Arc<Controller>,
) {
    i_slint_backend_testing::init_no_event_loop();
    make_fixture()
}

fn make_fixture() -> (
    tempfile::TempDir,
    AppWindow,
    audiolib_audio::AudioEngine,
    Arc<Controller>,
) {
    let root = tempfile::tempdir().unwrap();
    let config = AppConfig::load_in(root.path().join("preferences")).unwrap();
    let provider = Arc::new(SqliteProvider::open(&config.paths.database).unwrap());
    let playback = PlaybackConfig::default();
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    (root, ui, engine, controller)
}

#[test]
fn clearing_filters_leaves_missing_view_and_restores_present_results() {
    let (_root, ui, _engine, controller) = fixture();
    ui.set_visibility_index(1);
    ui.set_favorites_only(true);
    ui.set_duration_min("3".into());
    ui.invoke_reset_filters();
    assert_eq!(ui.get_visibility_index(), 0);
    assert!(!ui.get_favorites_only());
    assert!(ui.get_duration_min().is_empty());
    assert_eq!(
        controller.request.lock().unwrap().filters.visibility,
        FileVisibility::Present
    );
    controller.shutdown();
}

#[test]
fn persistent_playback_error_does_not_replace_later_feedback() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::client_review_tests::child_playback_feedback",
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
#[ignore = "status worker feedback uses an isolated event loop"]
fn child_playback_feedback() {
    use std::{cell::Cell, rc::Rc};
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (_root, ui, _engine, controller) = make_fixture();
    let stage = Rc::new(Cell::new(0));
    let observed = stage.clone();
    let feedback_time = Rc::new(Cell::new(Instant::now()));
    let since = feedback_time.clone();
    let started = Instant::now();
    let c = controller.clone();
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(10),
        move || {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "playback feedback timed out at stage {}",
                observed.get()
            );
            let ui = weak.upgrade().unwrap();
            match observed.get() {
                0 if !ui.get_searching() => {
                    c.command(AudioCommand::Seek(f64::NAN));
                    observed.set(1);
                }
                1 if c
                    .audio
                    .status()
                    .error
                    .as_ref()
                    .is_some_and(|error| ui.get_status().as_str() == error) =>
                {
                    ui.set_status("Later Metadata feedback".into());
                    since.set(Instant::now());
                    observed.set(2);
                }
                2 if since.get().elapsed() > Duration::from_millis(300) => {
                    assert_eq!(ui.get_status(), "Later Metadata feedback");
                    assert!(
                        c.audio.status().error.is_some(),
                        "the original engine error remains active"
                    );
                    observed.set(3);
                    c.shutdown();
                    ui.hide().unwrap();
                    slint::quit_event_loop().unwrap();
                }
                _ => {}
            }
        },
    );
    ui.run().unwrap();
    timer.stop();
    assert_eq!(stage.get(), 3);
}

#[test]
fn reopening_output_form_restores_acknowledged_configuration() {
    let (_root, ui, _engine, controller) = fixture();
    let active = PlaybackConfig {
        backend: AudioBackend::Asio,
        rate_policy: RatePolicy::Native,
        buffer_frames: 256,
        output_pair: 3,
        source_pair: 2,
        ..Default::default()
    };
    *controller.playback.lock().unwrap() = active;
    ui.set_backend_index(0);
    ui.set_rate_index(0);
    ui.set_buffer_frames("bad draft".into());
    ui.set_pair_index("9".into());
    ui.set_source_pair("8".into());
    controller.restore_output_form(&ui);
    assert_eq!(ui.get_backend_index(), 1);
    assert_eq!(ui.get_rate_index(), 1);
    assert_eq!(ui.get_buffer_frames(), "256");
    assert_eq!(ui.get_pair_index(), "3");
    assert_eq!(ui.get_source_pair(), "2");
    controller.shutdown();
}

#[test]
fn completed_scan_and_output_validation_follow_runtime_language() {
    let (_root, ui, _engine, controller) = fixture();
    let progress = audiolib_core::ScanProgress {
        processed: 12,
        added: 3,
        ..Default::default()
    };
    controller.publish_scan_completion(&ui, 0, 0, false, false, Ok(progress));
    assert!(ui.get_scan_summary().contains("3 added"));
    controller.chinese.store(true, Ordering::Release);
    ui.set_chinese(true);
    controller.refresh_generated_text(&ui);
    assert!(ui.get_scan_summary().contains("新增 3"));
    assert!(!ui.get_scan_summary().contains("added"));
    ui.set_buffer_frames("invalid".into());
    controller.apply_output(&ui, false);
    assert!(ui.get_device_info().contains("缓冲"));
    controller.shutdown();
}
