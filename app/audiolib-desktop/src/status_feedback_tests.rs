// app/audiolib-desktop/src/status_feedback_tests.rs
//! Isolated Slint feedback regressions with owned libraries and gated decoder opens.
use super::*;
use std::io::Cursor;

const ROOT_ENV: &str = "AUDIOLIB_STATUS_FEEDBACK_TEST_ROOT";

fn isolated(child: &str) {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("controller::status_feedback::tests::{child}"),
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(ROOT_ENV, root.path())
        .env("AUDIOLIB_DATA_DIR", root.path().join("preferences"))
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{child}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_until(ui: &AppWindow, ready: impl Fn() -> bool + 'static) {
    let timer = slint::Timer::default();
    let started = Instant::now();
    let finished = std::cell::Cell::new(false);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(10),
        move || {
            if finished.get() {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "feedback timed out"
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

struct Fixture {
    root: PathBuf,
    ui: AppWindow,
    _engine: audiolib_audio::AudioEngine,
    controller: Arc<Controller>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.controller.shutdown();
        self.ui.hide().unwrap();
    }
}

fn fixture() -> Fixture {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("isolated owned root"));
    let preferences = root.join("preferences");
    std::fs::create_dir_all(&preferences).unwrap();
    let playback = PlaybackConfig {
        volume: 0.0,
        ..Default::default()
    };
    // Existing explicit preferences prevent first-run legacy-library discovery.
    std::fs::write(
        preferences.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "active_database": root.join("library.db"),
            "language": "en",
            "audition_play_on_single_click": false,
            "playback": playback,
        }))
        .unwrap(),
    )
    .unwrap();
    let config = AppConfig::load_in(preferences).unwrap();
    let provider = Arc::new(SqliteProvider::open(&config.paths.database).unwrap());
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    let c = controller.clone();
    run_until(&ui, move || {
        c.ui.upgrade().is_some_and(|ui| {
            !ui.get_searching() && !ui.get_search_workspace_loading() && !ui.get_task_restoring()
        }) && c
            .search_publication
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|state| state.page == SearchPagePhase::Published && state.model_published)
    });
    Fixture {
        root,
        ui,
        _engine: engine,
        controller,
    }
}

#[derive(Default)]
struct Gate {
    entered: bool,
    released: bool,
}

struct GatedSource {
    path: PathBuf,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

impl GatedSource {
    fn new(fixture: &Fixture) -> Self {
        let path = fixture.root.join("gated.wav");
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend(38u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(48_000u32.to_le_bytes());
        bytes.extend(96_000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(0i16.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let gate = Arc::new((Mutex::new(Gate::default()), Condvar::new()));
        let opened = gate.clone();
        let opener: audiolib_audio::SourceOpener = Arc::new(move || {
            let (state, wake) = &*opened;
            let mut state = state.lock().unwrap();
            state.entered = true;
            wake.notify_all();
            while !state.released {
                state = wake.wait(state).unwrap();
            }
            let source: Box<dyn audiolib_audio::ByteSource> = Box::new(Cursor::new(bytes.clone()));
            Ok((source, bytes.len() as u64))
        });
        audiolib_audio::register_stream_source(&path, opener);
        // This is the displayed source, so the status worker does not queue an
        // unrelated external-source Waveform while this test owns the Load.
        *fixture.controller.selected.lock().unwrap() =
            Some(AudioFile::from(audiolib_core::AudioFileData {
                id: 1,
                filename: "gated.wav".into(),
                file_path: path.to_string_lossy().into_owned(),
                ..Default::default()
            }));
        Self { path, gate }
    }

    fn wait_entered(&self) {
        let (state, wake) = &*self.gate;
        let mut state = state.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !state.entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "decoder never opened the gated source"
            );
            state = wake.wait_timeout(state, remaining).unwrap().0;
        }
    }

    fn release(&self) {
        self.gate.0.lock().unwrap().released = true;
        self.gate.1.notify_all();
    }
}

impl Drop for GatedSource {
    fn drop(&mut self) {
        self.release();
        audiolib_audio::unregister_stream_source(&self.path);
    }
}

#[test]
fn pending_audio_confirmation_shows_progress_until_the_real_load_completes() {
    isolated("child_delayed_audio_confirmation");
}

#[test]
#[ignore = "owned decoder gate and Slint event loop run in a child"]
fn child_delayed_audio_confirmation() {
    let fixture = fixture();
    let source = GatedSource::new(&fixture);
    let completion = fixture
        .controller
        .audio
        .enqueue_with_ack(AudioCommand::Load(source.path.clone()))
        .unwrap();
    source.wait_entered();
    fixture.ui.set_status("Existing Metadata feedback".into());
    let cancel = Arc::new(AtomicBool::new(false));
    let generation = fixture
        .controller
        .selection_generation
        .load(Ordering::Acquire);
    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let (c, cancel, done) = (fixture.controller.clone(), cancel.clone(), done.clone());
        std::thread::spawn(move || {
            let result = c.wait_audio_confirmation(
                &completion,
                &cancel,
                generation,
                AudioActivity::Loading,
                |_| true,
            );
            done.store(true, Ordering::Release);
            result
        })
    };
    let weak = fixture.ui.as_weak();
    run_until(&fixture.ui, move || {
        weak.upgrade().is_some_and(|ui| {
            ui.get_remote_audition_status() == AudioActivity::Loading.pending(false)
        })
    });
    assert!(!done.load(Ordering::Acquire));
    assert_eq!(fixture.ui.get_status(), "Existing Metadata feedback");
    assert!(fixture.controller.audio.status().error.is_none());
    source.release();
    let weak = fixture.ui.as_weak();
    run_until(&fixture.ui, move || {
        done.load(Ordering::Acquire)
            && weak
                .upgrade()
                .is_some_and(|ui| ui.get_remote_audition_status().is_empty())
    });
    let confirmed = worker.join().unwrap().unwrap();
    assert_eq!(confirmed.state, PlaybackState::Ready);
    assert!(confirmed.error.is_none());
    assert_eq!(fixture.ui.get_status(), "Existing Metadata feedback");
}

#[test]
fn stop_cancels_a_pending_audio_confirmation_without_publishing_an_error() {
    isolated("child_cancel_audio_confirmation");
}

#[test]
#[ignore = "owned decoder gate and Slint event loop run in a child"]
fn child_cancel_audio_confirmation() {
    let fixture = fixture();
    let source = GatedSource::new(&fixture);
    let completion = fixture
        .controller
        .audio
        .enqueue_with_ack(AudioCommand::Load(source.path.clone()))
        .unwrap();
    source.wait_entered();
    fixture.ui.set_status("New selection feedback".into());
    let cancel = Arc::new(AtomicBool::new(false));
    *fixture.controller.remote_audio_cancel.lock().unwrap() = cancel.clone();
    let generation = fixture
        .controller
        .selection_generation
        .load(Ordering::Acquire);
    let entered = Arc::new(AtomicBool::new(false));
    let wait_epoch = fixture
        .controller
        .remote_audition_status_epoch
        .load(Ordering::Acquire);
    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let (c, cancel, entered, done) = (
            fixture.controller.clone(),
            cancel.clone(),
            entered.clone(),
            done.clone(),
        );
        std::thread::spawn(move || {
            entered.store(true, Ordering::Release);
            let result = c.wait_audio_confirmation(
                &completion,
                &cancel,
                generation,
                AudioActivity::Loading,
                |_| true,
            );
            done.store(true, Ordering::Release);
            result
        })
    };
    let waiting = fixture.controller.clone();
    run_until(&fixture.ui, move || {
        entered.load(Ordering::Acquire)
            && waiting.remote_audition_status_epoch.load(Ordering::Acquire) != wait_epoch
    });
    let started = Instant::now();
    let stop = fixture.controller.enqueue_stop().unwrap();
    let weak = fixture.ui.as_weak();
    run_until(&fixture.ui, move || {
        done.load(Ordering::Acquire)
            && weak
                .upgrade()
                .is_some_and(|ui| ui.get_remote_audition_status().is_empty())
    });
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        worker.join().unwrap(),
        Err(audiolib_core::Error::Cancelled)
    ));
    assert_eq!(fixture.ui.get_status(), "New selection feedback");
    source.release();
    let stopped = stop.wait(Duration::from_secs(2)).unwrap();
    assert!(stopped.error.is_none());
    assert_ne!(stopped.state, PlaybackState::Error);
    assert!(fixture.ui.get_remote_audition_status().is_empty());
}

fn feedback_stays(ui: &AppWindow, expected: &'static str) {
    let started = Instant::now();
    let weak = ui.as_weak();
    run_until(ui, move || {
        assert_eq!(weak.upgrade().unwrap().get_status(), expected);
        started.elapsed() >= Duration::from_millis(150)
    });
}

#[test]
fn queued_job_feedback_rechecks_selection_waveform_source_and_close_ownership() {
    isolated("child_status_delivery_guards");
}

#[test]
#[ignore = "queued feedback delivery uses an isolated event loop"]
fn child_status_delivery_guards() {
    let fixture = fixture();
    let c = &fixture.controller;
    fixture.ui.set_status("Newer operation feedback".into());
    let selection = c.selection_generation.load(Ordering::Acquire);
    c.status_when("Old selection failure".into(), move |c| {
        c.selection_generation.load(Ordering::Acquire) == selection
    });
    c.selection_generation.fetch_add(1, Ordering::AcqRel);
    feedback_stays(&fixture.ui, "Newer operation feedback");
    let waveform = c.waveform_generation.load(Ordering::Acquire);
    c.status_when("Old Waveform failure".into(), move |c| {
        c.waveform_generation.load(Ordering::Acquire) == waveform
    });
    c.waveform_generation.fetch_add(1, Ordering::AcqRel);
    feedback_stays(&fixture.ui, "Newer operation feedback");
    c.status_when("Old source failure".into(), |_| true);
    c.source_generation.fetch_add(1, Ordering::AcqRel);
    feedback_stays(&fixture.ui, "Newer operation feedback");
    c.status_when("Old library failure".into(), |_| true);
    c.library_generation.fetch_add(1, Ordering::AcqRel);
    feedback_stays(&fixture.ui, "Newer operation feedback");
    c.status_when("Queued close failure".into(), |_| true);
    c.close_retains_status.store(true, Ordering::Release);
    feedback_stays(&fixture.ui, "Newer operation feedback");
    c.close_retains_status.store(false, Ordering::Release);
    c.status_when("Current audio execution failed".into(), |_| true);
    let weak = fixture.ui.as_weak();
    run_until(&fixture.ui, move || {
        weak.upgrade()
            .is_some_and(|ui| ui.get_status() == "Current audio execution failed")
    });
}
