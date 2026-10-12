// app/audiolib-desktop/src/close_tests.rs
//! Real Windows close callbacks, isolated from the testing backend and user libraries.

#[cfg(windows)]
#[test]
#[ignore = "requires an interactive Windows desktop and the real Slint backend"]
fn real_windows_close_hides_drained_window_and_exits() {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    let root = std::env::temp_dir().join(format!(
        "audiolib-native-close-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("sounds")).unwrap();
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend(38u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(48000u32.to_le_bytes());
    wav.extend(96000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(2u32.to_le_bytes());
    wav.extend(0i16.to_le_bytes());
    let source = root.join("sounds/close.wav");
    std::fs::write(&source, &wav).unwrap();
    let provider = Arc::new(SqliteProvider::open(&root.join("library.db")).unwrap());
    audiolib_db::ScanService::new(provider.clone())
        .scan(
            &root.join("sounds"),
            &audiolib_db::ScanOptions::default(),
            &audiolib_db::ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let file_id = provider
        .search(&audiolib_core::SearchRequest::default())
        .unwrap()
        .results[0]
        .id;
    let engine = AudioEngine::new(PlaybackConfig::default()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = controller::Controller::new(
        &ui,
        provider.clone(),
        engine.handle(),
        AppConfig::load_in(root.join("preferences")).unwrap(),
        PlaybackConfig::default(),
    );
    let close_control = controller.clone();
    let weak = ui.as_weak();
    let close_count = Rc::new(Cell::new(0));
    let observed = close_count.clone();
    ui.window().on_close_requested(move || {
        observed.set(observed.get() + 1);
        close_control.request_close(&weak.upgrade().unwrap());
        slint::CloseRequestResponse::KeepWindowShown
    });
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(150),
        move || {
            let ui = weak.upgrade().unwrap();
            ui.set_selected_id(file_id.to_string().into());
            ui.set_edit_notes("unsaved native-close draft".into());
            ui.set_metadata_dirty(true);
            // Same platform event used by the title-bar close button. A second
            // request while draining must neither abandon writes nor keep it open.
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
        },
    );
    let timed_out = Rc::new(Cell::new(false));
    let failure = timed_out.clone();
    let watchdog = slint::Timer::default();
    watchdog.start(
        slint::TimerMode::SingleShot,
        Duration::from_secs(5),
        move || {
            failure.set(true);
            slint::quit_event_loop().unwrap();
        },
    );
    ui.show().unwrap();
    slint::run_event_loop().unwrap();
    let hidden = !ui.window().is_visible();
    controller.shutdown();
    ui.hide().unwrap();
    drop(engine);
    assert_eq!(close_count.get(), 2);
    assert!(
        !timed_out.get(),
        "Native close did not finish within five seconds"
    );
    assert!(
        hidden,
        "Drained close left the Saving changes window visible"
    );
    assert_eq!(
        provider.get_audio(file_id).unwrap().notes,
        "unsaved native-close draft"
    );
    assert_eq!(std::fs::read(source).unwrap(), wav);
    drop(ui);
    drop(controller);
    drop(provider);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    // Detached workers observe the shutdown request before dropping their providers.
    std::thread::sleep(Duration::from_millis(100));
    std::fs::remove_dir_all(root).ok();
}
