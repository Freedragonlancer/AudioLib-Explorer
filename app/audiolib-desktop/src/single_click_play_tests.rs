// app/audiolib-desktop/src/single_click_play_tests.rs
//! Exercise preference callbacks against a contended configuration writer and shutdown.
use super::*;

#[test]
fn single_click_preference_keeps_gui_responsive_and_survives_immediate_shutdown() {
    let root = std::env::temp_dir().join(format!(
        "audiolib-click-preference-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::single_click_play_tests::child_contended_preference_shutdown",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("AUDIOLIB_CLICK_PREF_TEST_ROOT", &root)
        .output()
        .unwrap();
    // The isolated child has exited, so no worker or SQLite handle retains this root.
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "Preference regression failed: {}\n{stdout}\n{stderr}",
        output.status
    );
    println!("{stdout}");
}

#[test]
#[ignore = "Slint/controller regression invoked by its isolated parent test"]
fn child_contended_preference_shutdown() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let root = PathBuf::from(std::env::var_os("AUDIOLIB_CLICK_PREF_TEST_ROOT").unwrap());
    let mut config = AppConfig::load_in(root.clone()).unwrap();
    config.language = "cn".into();
    let playback = PlaybackConfig {
        volume: 0.25,
        ..Default::default()
    };
    config.playback = serde_json::to_value(&playback).unwrap();
    config.save().unwrap();
    let provider = Arc::new(SqliteProvider::open(&config.paths.database).unwrap());
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    // Both directions must reach disk without closing the window.
    for enabled in [true, false] {
        ui.set_audition_play_on_single_click(enabled);
        ui.invoke_single_click_play_requested(enabled);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap())
                    .unwrap();
            if saved["audition_play_on_single_click"] == enabled {
                break;
            }
            assert!(Instant::now() < deadline, "Preference worker did not save");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let held = Arc::new(AtomicBool::new(false));
    let writer_held = held.clone();
    let writer = controller.clone();
    let slow_writer = std::thread::spawn(move || {
        let _config = writer.config.lock().unwrap();
        writer_held.store(true, Ordering::Release);
        ready_tx.send(()).unwrap();
        // A blocked disk write holds this same lock. Keep it until shutdown starts,
        // with a fallback deadline so the old synchronous callback can fail safely.
        let deadline = Instant::now() + Duration::from_millis(500);
        while !writer.shutdown.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        writer_held.store(false, Ordering::Release);
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let started = Instant::now();
    for enabled in [true, false, true, false, true] {
        ui.set_audition_play_on_single_click(enabled);
        ui.invoke_single_click_play_requested(enabled);
    }
    let elapsed = started.elapsed();
    let returned_while_locked = held.load(Ordering::Acquire);
    // Closing immediately must persist the last request even if the save worker
    // has not yet acquired the configuration lock.
    controller.shutdown();
    slow_writer.join().unwrap();
    let saved = AppConfig::load_in(root).unwrap();
    assert!(saved.audition_play_on_single_click);
    assert_eq!(saved.language, "cn");
    assert_eq!(saved.playback["volume"], serde_json::json!(0.25));
    println!("Preference callbacks returned in {elapsed:?}");
    assert!(
        returned_while_locked,
        "GUI callback waited for the configuration writer ({elapsed:?})"
    );
}
