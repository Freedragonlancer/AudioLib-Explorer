// app/audiolib-desktop/src/single_click_play_tests.rs
//! Exercise preference callbacks against a contended configuration writer and shutdown,
//! and keyboard audition in single-click playback mode.
use super::*;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;

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

const KEYBOARD_ROOT_ENV: &str = "AUDIOLIB_KEYBOARD_AUDITION_TEST_ROOT";
const FIRST_SOUND: &str = "A first.wav";
const SECOND_SOUND: &str = "B second.wav";

/// Single-click playback must also cover the arrow keys, which otherwise only move the
/// highlight: the row they select is auditioned exactly like a clicked row. A step that
/// cannot move (already at the first/last row) must leave the current sound playing
/// instead of reloading it, and an unchecked checkbox must keep keyboard steps silent.
#[test]
#[ignore = "requires muted Windows output"]
fn keyboard_selection_auditions_in_single_click_mode() {
    let root = std::env::temp_dir().join(format!(
        "audiolib-keyboard-audition-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::single_click_play_tests::child_keyboard_selection_audition",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(KEYBOARD_ROOT_ENV, &root)
        .env("AUDIOLIB_DATA_DIR", root.join("preferences"))
        .output()
        .unwrap();
    assert!(
        owned_keyboard_root(&root),
        "Refusing cleanup outside the owned test root"
    );
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "Keyboard audition regression failed: {}\n{stdout}\n{stderr}",
        output.status
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum KeyboardPhase {
    ClickFirst,
    AwaitFirstPlayback,
    AwaitSecondPlayback,
    AwaitRetainedPlayback,
    AwaitFirstAfterUp,
    AwaitStopped,
    AwaitSilentStep,
    Complete,
}

struct KeyboardProgress {
    phase: KeyboardPhase,
    started: Instant,
    stage_started: Instant,
    /// (load generation, play session) of the second record's confirmed playback.
    retained: (u64, u64),
    failure: Option<String>,
}

#[test]
#[ignore = "requires muted Windows output; invoked by the isolated parent regression"]
fn child_keyboard_selection_audition() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let root = PathBuf::from(
        std::env::var_os(KEYBOARD_ROOT_ENV).expect("Parent test must supply its root"),
    );
    assert!(owned_keyboard_root(&root), "Test root must be owned");
    let sounds = root.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    // Long enough that no phase can reach the source's end mid-assertion.
    write_silent_wav(&sounds.join(FIRST_SOUND));
    write_silent_wav(&sounds.join(SECOND_SOUND));
    let playback = PlaybackConfig {
        volume: 0.0,
        ..Default::default()
    };
    let preferences = root.join("preferences");
    std::fs::create_dir_all(&preferences).unwrap();
    // An existing settings file keeps legacy YAML discovery out of the test; the owned
    // fields below then describe this fixture library.
    std::fs::write(preferences.join("settings.json"), b"{}").unwrap();
    let mut config = AppConfig::load_in(preferences.clone()).unwrap();
    config.language = "en".into();
    config.playback = serde_json::to_value(&playback).unwrap();
    config.audition_play_on_single_click = true;
    config.audio_table = AudioTablePreferences {
        order_by: SortOrder::Filename,
        ..Default::default()
    };
    config.allowed_scan_roots = vec![sounds.canonicalize().unwrap()];
    config.paths.legacy_database = None;
    config.save().unwrap();
    let provider = Arc::new(SqliteProvider::open(&config.paths.database).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &sounds,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let records = provider.search(&SearchRequest::default()).unwrap().results;
    assert_eq!(records.len(), 2);
    let path_of = |name: &str| {
        PathBuf::from(
            &records
                .iter()
                .find(|record| record.filename == name)
                .expect("Scanned fixture record")
                .file_path,
        )
    };
    let (first, second) = (path_of(FIRST_SOUND), path_of(SECOND_SOUND));
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    assert!(ui.get_audition_play_on_single_click());

    let started = Instant::now();
    let progress = Rc::new(RefCell::new(KeyboardProgress {
        phase: KeyboardPhase::ClickFirst,
        started,
        stage_started: started,
        retained: (0, 0),
        failure: None,
    }));
    let checking = progress.clone();
    let window = ui.as_weak();
    let timed = controller.clone();
    let (first_path, second_path) = (first.clone(), second.clone());
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(5),
        move || {
            let Some(ui) = window.upgrade() else { return };
            let mut state = checking.borrow_mut();
            let status = timed.audio.status();
            let playing = |path: &PathBuf| {
                status.state == PlaybackState::Playing && status.file_path.as_ref() == Some(path)
            };
            if state.started.elapsed() > Duration::from_secs(40)
                || state.stage_started.elapsed() > Duration::from_secs(10)
            {
                state.failure = Some(format!(
                    "Keyboard audition stalled in {:?}: state={:?} path={:?} row={}",
                    state.phase,
                    status.state,
                    status.file_path,
                    ui.get_selected_row()
                ));
                slint::quit_event_loop().ok();
                return;
            }
            let outcome = (|| -> std::result::Result<KeyboardPhase, String> {
                match state.phase {
                    KeyboardPhase::ClickFirst => {
                        if ui.get_searching() || ui.get_rows().row_count() != 2 {
                            return Ok(KeyboardPhase::ClickFirst);
                        }
                        if result_names(&ui) != [FIRST_SOUND, SECOND_SOUND] {
                            return Err(format!(
                                "Unexpected result order: {:?}",
                                result_names(&ui)
                            ));
                        }
                        // Clicking focuses the result list and auditions the first row.
                        click_at(&ui, result_row_position(&ui, FIRST_SOUND)?);
                        Ok(KeyboardPhase::AwaitFirstPlayback)
                    }
                    KeyboardPhase::AwaitFirstPlayback if playing(&first_path) => {
                        crate::ui_tests::send_keys(&ui, &[slint::platform::Key::DownArrow.into()]);
                        Ok(KeyboardPhase::AwaitSecondPlayback)
                    }
                    KeyboardPhase::AwaitSecondPlayback if playing(&second_path) => {
                        state.retained = (status.generation, status.play_session_id);
                        // Already at the last row: this step cannot move.
                        crate::ui_tests::send_keys(&ui, &[slint::platform::Key::DownArrow.into()]);
                        Ok(KeyboardPhase::AwaitRetainedPlayback)
                    }
                    KeyboardPhase::AwaitRetainedPlayback
                        if state.stage_started.elapsed() >= Duration::from_millis(400) =>
                    {
                        if !playing(&second_path) {
                            return Err(format!(
                                "A step at the last row stopped playback: {:?}",
                                status.state
                            ));
                        }
                        let retained = (status.generation, status.play_session_id);
                        if retained != state.retained {
                            return Err(format!(
                                "A step at the last row reloaded the sound: {:?} -> {retained:?}",
                                state.retained
                            ));
                        }
                        crate::ui_tests::send_keys(&ui, &[slint::platform::Key::UpArrow.into()]);
                        Ok(KeyboardPhase::AwaitFirstAfterUp)
                    }
                    KeyboardPhase::AwaitFirstAfterUp if playing(&first_path) => {
                        ui.invoke_stop_requested();
                        Ok(KeyboardPhase::AwaitStopped)
                    }
                    KeyboardPhase::AwaitStopped
                        if !matches!(
                            status.state,
                            PlaybackState::Playing | PlaybackState::Loading
                        ) =>
                    {
                        // The unchecked checkbox must restore silent keyboard selection.
                        ui.set_audition_play_on_single_click(false);
                        crate::ui_tests::send_keys(&ui, &[slint::platform::Key::DownArrow.into()]);
                        Ok(KeyboardPhase::AwaitSilentStep)
                    }
                    KeyboardPhase::AwaitSilentStep
                        if state.stage_started.elapsed() >= Duration::from_millis(500) =>
                    {
                        if status.state == PlaybackState::Playing {
                            return Err(
                                "Keyboard steps must stay silent without single-click play".into(),
                            );
                        }
                        if ui.get_selected_row() != 1 {
                            return Err(format!(
                                "Keyboard selection did not move: row={}",
                                ui.get_selected_row()
                            ));
                        }
                        slint::quit_event_loop().ok();
                        Ok(KeyboardPhase::Complete)
                    }
                    phase => Ok(phase),
                }
            })();
            match outcome {
                Ok(phase) if phase != state.phase => {
                    state.phase = phase;
                    state.stage_started = Instant::now();
                }
                Ok(_) => {}
                Err(error) => {
                    state.failure = Some(error);
                    slint::quit_event_loop().ok();
                }
            }
        },
    );
    ui.show().unwrap();
    slint::run_event_loop().unwrap();
    timer.stop();
    controller.shutdown();
    ui.hide().unwrap();
    let state = progress.borrow();
    assert!(state.failure.is_none(), "{:?}", state.failure);
    assert_eq!(
        state.phase,
        KeyboardPhase::Complete,
        "Incomplete keyboard audition phases"
    );
}

fn owned_keyboard_root(root: &Path) -> bool {
    root.is_absolute()
        && root.starts_with(std::env::temp_dir())
        && root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("audiolib-keyboard-audition-")
        })
}

fn result_names(ui: &AppWindow) -> [String; 2] {
    let names = ui
        .get_rows()
        .iter()
        .map(|row| row.name.to_string())
        .collect::<Vec<_>>();
    [
        names.first().cloned().unwrap_or_default(),
        names.get(1).cloned().unwrap_or_default(),
    ]
}

fn result_row_position(
    ui: &AppWindow,
    label: &str,
) -> std::result::Result<slint::LogicalPosition, String> {
    let row = ElementHandle::find_by_accessible_label(ui, label)
        .find(|element| element.accessible_role() == Some(AccessibleRole::ListItem))
        .ok_or_else(|| format!("Missing result row: {label}"))?;
    let origin = row.absolute_position();
    Ok(slint::LogicalPosition::new(
        origin.x + 8.0,
        origin.y + row.size().height / 2.0,
    ))
}

fn click_at(ui: &AppWindow, position: slint::LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
}

fn write_silent_wav(path: &Path) {
    let data_bytes = 5 * 48_000 * 2u32;
    let mut wav = Vec::with_capacity(data_bytes as usize + 44);
    wav.extend(b"RIFF");
    wav.extend((data_bytes + 36).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(48_000u32.to_le_bytes());
    wav.extend(96_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data_bytes.to_le_bytes());
    wav.resize(data_bytes as usize + 44, 0);
    std::fs::write(path, wav).unwrap();
}
