// app/audiolib-desktop/src/ipc_load_tests.rs
//! Explicit IPC paths supersede stale inspector reloads and delayed GUI preparations.
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
    // Play reaches the real engine but cannot open a hardware output.
    let playback = PlaybackConfig {
        device_id: Some("audiolib-test-no-such-output-device".into()),
        volume: 0.0,
        ..Default::default()
    };
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    (root, ui, engine, controller)
}

fn local_file(path: &Path, id: i64) -> AudioFile {
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(38_u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(96_000_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2_u32.to_le_bytes());
    bytes.extend(1000_i16.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
    AudioFile::from(audiolib_core::AudioFileData {
        id,
        filename: path.file_name().unwrap().to_string_lossy().into_owned(),
        file_path: path.to_string_lossy().into_owned(),
        channels: 1,
        sample_rate: 48_000,
        duration: 1.0 / 48_000.0,
        ..Default::default()
    })
}

#[test]
fn ipc_load_then_play_keeps_the_explicit_path_after_an_inspector_load_failed() {
    let (root, ui, _engine, controller) = fixture();
    let a = local_file(&root.path().join("a.wav"), 1);
    let b = local_file(&root.path().join("b.wav"), 2);
    std::fs::remove_file(&a.file_path).unwrap();
    *controller.selected.lock().unwrap() = Some(a.clone());
    present_file(&ui, &a);
    controller.mark_audio_reload(a.id);
    assert!(
        controller
            .audio
            .send_and_wait(
                AudioCommand::Load(PathBuf::from(&a.file_path)),
                Duration::from_secs(2),
            )
            .is_err()
    );
    controller.metadata.lock().unwrap().edit(
        a.id,
        MetadataPatch {
            notes: Some("unsaved A notes".into()),
            ..Default::default()
        },
    );
    let metadata_sequence = controller.metadata.lock().unwrap().sequence;
    let loaded = controller.ipc(serde_json::json!({"command":"load", "path":b.file_path}));
    assert_eq!(loaded["success"], true, "{loaded}");
    assert_eq!(controller.audio_reload_target(), None);
    let generation = controller.selection_generation.load(Ordering::Acquire);
    let played = controller.ipc(serde_json::json!({"command":"play"}));
    // A deliberately nonexistent output reports a synchronous engine failure.
    // A stale inspector reload would instead return success and defer A to Slint.
    assert_eq!(played["success"], false, "{played}");
    assert_eq!(
        controller.selection_generation.load(Ordering::Acquire),
        generation
    );
    assert_eq!(
        controller.audio.status().file_path,
        Some(PathBuf::from(&b.file_path))
    );
    assert!(controller.latest_selection.lock().unwrap().is_none());
    let metadata = controller.metadata.lock().unwrap();
    assert_eq!(metadata.sequence, metadata_sequence);
    assert_eq!(
        metadata.drafts[&a.id].patch.notes.as_deref(),
        Some("unsaved A notes")
    );
    drop(metadata);
    controller.shutdown();
}

#[test]
fn ipc_load_cancels_a_waiting_old_gui_load_and_stop_keeps_the_new_source() {
    let (root, ui, _engine, controller) = fixture();
    let a = local_file(&root.path().join("a.wav"), 1);
    let b = local_file(&root.path().join("b.wav"), 2);
    *controller.selected.lock().unwrap() = Some(a.clone());
    present_file(&ui, &a);
    controller.mark_audio_reload(a.id);
    let generation = controller.selection_generation.load(Ordering::Acquire);
    let cancel = Arc::new(AtomicBool::new(false));
    *controller.remote_audio_cancel.lock().unwrap() = cancel.clone();
    *controller.latest_selection.lock().unwrap() = Some(SelectionJob {
        id: a.id,
        play: true,
        generation,
        barrier: 0,
        provider: controller.provider(),
    });
    let preparation = controller.remote_audio_prepare.lock().unwrap();
    let old = controller.clone();
    let (started, waiting) = std::sync::mpsc::channel();
    let stale = std::thread::spawn(move || {
        started.send(()).unwrap();
        old.load_selection_audio(SelectionAudioJob {
            file: a,
            play: true,
            remote: false,
            generation,
            cancel,
        });
    });
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    let loaded = controller.ipc(serde_json::json!({"command":"load", "path":b.file_path}));
    assert_eq!(loaded["success"], true, "{loaded}");
    drop(preparation);
    stale.join().unwrap();
    let status = controller
        .audio
        .send_and_wait(AudioCommand::Synchronize, Duration::from_secs(2))
        .unwrap();
    assert_eq!(status.file_path, Some(PathBuf::from(&b.file_path)));
    assert_eq!(status.state, PlaybackState::Ready);
    let stopped = controller.ipc(serde_json::json!({"command":"stop"}));
    assert_eq!(stopped["success"], true, "{stopped}");
    let status = controller.audio.status();
    assert_eq!(status.file_path, Some(PathBuf::from(&b.file_path)));
    assert_eq!(status.state, PlaybackState::Stopped);
    assert_eq!(status.play_session_id, 0);
    assert!(controller.latest_selection.lock().unwrap().is_none());
    assert_eq!(controller.audio_reload_target(), None);
    controller.shutdown();
}

#[test]
fn ipc_load_cancels_an_entered_ui_publication_waiting_on_metadata_capture() {
    let isolation = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::ipc_load_tests::child_ipc_load_during_metadata_capture",
            "--ignored",
            "--test-threads=1",
        ])
        .env("AUDIOLIB_DATA_DIR", isolation.path().join("preferences"))
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn run_until(ui: &AppWindow, ready: impl Fn() -> bool + 'static) {
    let started = Instant::now();
    let finished = std::cell::Cell::new(false);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(10),
        move || {
            if finished.get() {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "UI publication timed out"
            );
            if ready() {
                finished.set(true);
                slint::quit_event_loop().unwrap();
            }
        },
    );
    ui.show().unwrap();
    slint::run_event_loop().unwrap();
    timer.stop();
}

#[test]
#[ignore = "isolated real Slint publication and Metadata capture admission regression"]
fn child_ipc_load_during_metadata_capture() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (root, ui, _engine, controller) = make_fixture();
    let startup = ui.as_weak();
    run_until(&ui, move || {
        startup.upgrade().is_some_and(|ui| {
            !ui.get_search_workspace_loading() && !ui.get_task_restoring() && !ui.get_searching()
        })
    });
    let sounds = root.path().join("sounds");
    std::fs::create_dir(&sounds).unwrap();
    let a_path = sounds.join("a.wav");
    let b_path = sounds.join("b.wav");
    let current_path = sounds.join("current.wav");
    local_file(&a_path, 0);
    local_file(&b_path, 0);
    local_file(&current_path, 0);
    let provider = controller.provider();
    ScanService::new(provider.clone())
        .scan(
            &sounds,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let a = provider.find_audio_by_path(&a_path).unwrap().unwrap();
    let current = provider.find_audio_by_path(&current_path).unwrap().unwrap();
    *controller.selected.lock().unwrap() = Some(current.clone());
    present_file(&ui, &current);
    ui.set_edit_notes("current draft survives the cancelled selection".into());
    ui.set_metadata_dirty(true);
    controller.capture_metadata(&ui);
    let generation = controller.selection_generation.load(Ordering::Acquire);
    let stale = SelectionJob {
        id: a.id,
        play: true,
        generation,
        barrier: 0,
        provider: provider.clone(),
    };
    *controller.latest_selection.lock().unwrap() = Some(stale.clone());
    let (entered, capture_entered) = std::sync::mpsc::sync_channel(1);
    *controller.selection_metadata_capture_probe.lock().unwrap() = Some(entered);
    // Queue the real callback before taking Metadata's mutex: its earlier read
    // barrier must finish, but the Slint callback has not run yet.
    controller.load_selection(stale);
    let (held, metadata_held) = std::sync::mpsc::channel();
    let published = Arc::new(AtomicBool::new(false));
    let completed = published.clone();
    let worker = controller.clone();
    let loaded_path = b_path.clone();
    let ipc = std::thread::spawn(move || {
        let metadata = worker.metadata.lock().unwrap();
        held.send(()).unwrap();
        capture_entered
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let reply = worker.ipc(serde_json::json!({"command":"load", "path":loaded_path}));
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(worker.audio.status().file_path, Some(loaded_path));
        drop(metadata);
        // This marker cannot run until the entered old callback has returned.
        post(worker.ui.clone(), move |_| {
            completed.store(true, Ordering::Release)
        });
    });
    metadata_held.recv_timeout(Duration::from_secs(2)).unwrap();
    run_until(&ui, move || published.load(Ordering::Acquire));
    ipc.join().unwrap();
    assert_eq!(
        controller
            .selected
            .lock()
            .unwrap()
            .as_ref()
            .map(|file| file.id),
        Some(current.id)
    );
    assert_eq!(ui.get_selected_id().as_str(), current.id.to_string());
    assert!(controller.latest_selection.lock().unwrap().is_none());
    assert!(controller.selection_audio_job.lock().unwrap().is_none());
    assert_eq!(controller.audio_reload_target(), None);
    assert_eq!(controller.audio.status().file_path, Some(b_path.clone()));
    assert_eq!(controller.audio.status().state, PlaybackState::Ready);
    let sequence = controller.metadata.lock().unwrap().sequence;
    controller.wait_metadata(sequence, true).unwrap();
    assert_eq!(
        provider.get_audio(current.id).unwrap().notes,
        "current draft survives the cancelled selection"
    );
    let accepted_generation = controller.selection_generation.load(Ordering::Acquire);
    assert!(accepted_generation > generation);
    let played = controller.ipc(serde_json::json!({"command":"play"}));
    assert_eq!(played["success"], false, "{played}");
    assert_eq!(controller.audio.status().file_path, Some(b_path));
    assert_eq!(
        controller.selection_generation.load(Ordering::Acquire),
        accepted_generation
    );
    controller.shutdown();
    ui.hide().unwrap();
}

#[test]
fn ipc_load_replaces_cancelled_waveforms_for_selected_and_different_sources() {
    let isolation = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::ipc_load_tests::child_ipc_waveform_publication",
            "--ignored",
            "--test-threads=1",
        ])
        .env("AUDIOLIB_DATA_DIR", isolation.path().join("preferences"))
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "isolated IPC Load and queued Waveform publication regression"]
fn child_ipc_waveform_publication() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (root, ui, _engine, controller) = make_fixture();
    let startup = ui.as_weak();
    run_until(&ui, move || {
        startup.upgrade().is_some_and(|ui| {
            !ui.get_search_workspace_loading() && !ui.get_task_restoring() && !ui.get_searching()
        })
    });
    let a = local_file(&root.path().join("a.wav"), 1);
    let b = local_file(&root.path().join("b.wav"), 2);
    *controller.selected.lock().unwrap() = Some(a.clone());
    present_file(&ui, &a);
    controller.wave_view.lock().unwrap().0 = a.file_path.clone();
    *controller.waveform_size.lock().unwrap() = (64, 32);
    let data =
        audiolib_audio::waveform(Path::new(&a.file_path), WaveformRequest::default()).unwrap();
    let job = WaveJob {
        source: waveform_selection::WaveSource::capture(&controller, &a),
        path: PathBuf::from(&a.file_path),
        remote_id: None,
        sample_rate: a.sample_rate,
        frames: data.frames,
        start_frame: 0,
        end_frame: data.frames,
        duration: a.duration,
        view: WaveView::default(),
        generation: controller.waveform_generation.load(Ordering::Acquire),
        cancel: controller.waveform_cancel.lock().unwrap().clone(),
        width: 32,
        height: 16,
    };
    // Leave a real render publication queued in Slint. IPC invalidates it, then
    // external discovery must replace it even though A is already selected.
    controller.publish_waveform(&job, &data, 1.0);
    let loaded = controller.ipc(serde_json::json!({"command":"load", "path":a.file_path}));
    assert_eq!(loaded["success"], true, "{loaded}");
    let window = ui.as_weak();
    run_until(&ui, move || {
        window
            .upgrade()
            .is_some_and(|ui| ui.get_waveform_ready() && ui.get_waveform_image().size().width == 64)
    });
    assert!(ui.get_waveform_ready());
    assert_eq!(ui.get_waveform_image().size().width, 64);
    let a_identity = ui.get_wave_selection_identity();
    assert!(!a_identity.is_empty());

    // A queued render from A must still be rejected when the explicit path is B.
    // B is selected too, so its replacement also needs cancelled-work discovery.
    ui.set_waveform_ready(false);
    ui.set_waveform_image(Image::default());
    *controller.selected.lock().unwrap() = Some(b.clone());
    present_file(&ui, &b);
    let old_job = WaveJob {
        generation: controller.waveform_generation.load(Ordering::Acquire),
        cancel: controller.waveform_cancel.lock().unwrap().clone(),
        ..job
    };
    controller.publish_waveform(&old_job, &data, 1.0);
    let loaded = controller.ipc(serde_json::json!({"command":"load", "path":b.file_path}));
    assert_eq!(loaded["success"], true, "{loaded}");
    let window = ui.as_weak();
    run_until(&ui, move || {
        window.upgrade().is_some_and(|ui| {
            ui.get_waveform_ready()
                && ui.get_waveform_image().size().width == 64
                && ui.get_wave_selection_identity() != a_identity
        })
    });
    assert!(old_job.cancel.load(Ordering::Acquire));
    assert_eq!(ui.get_waveform_image().size().width, 64);
    assert_eq!(
        controller.audio.status().file_path,
        Some(PathBuf::from(&b.file_path))
    );
    controller.shutdown();
    ui.hide().unwrap();
}
