// app/audiolib-desktop/src/request_recovery_tests.rs
//! Selection/page ordering, failed-close recovery and authoritative server history.
use super::*;
use std::cell::Cell;
use std::rc::Rc;

const ROOT_ENV: &str = "AUDIOLIB_REQUEST_RECOVERY_TEST_ROOT";

fn isolated(child: &str) {
    let root = std::env::temp_dir().join(format!(
        "audiolib-request-recovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("controller::request_recovery_tests::{child}"),
            "--ignored",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, &root)
        .output()
        .unwrap();
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{child}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (
    Arc<SqliteProvider>,
    audiolib_audio::AudioEngine,
    AppWindow,
    Arc<Controller>,
) {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    let sounds = root.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend(996u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(48_000u32.to_le_bytes());
    wav.extend(96_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(960u32.to_le_bytes());
    wav.resize(1004, 0);
    for name in ["A.wav", "B.wav"] {
        std::fs::write(sounds.join(name), &wav).unwrap();
    }
    let provider = Arc::new(SqliteProvider::open(&root.join("library.db")).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &sounds,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let playback = PlaybackConfig {
        volume: 0.0,
        ..Default::default()
    };
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let mut config = AppConfig::load_in(root.join("preferences")).unwrap();
    config.paths.database = provider.database_path().into();
    config.language = "en".into();
    config.audition_play_on_single_click = false;
    let controller = Controller::new(&ui, provider.clone(), engine.handle(), config, playback);
    (provider, engine, ui, controller)
}

fn run_until(ui: &AppWindow, ready: impl Fn() -> bool + 'static) {
    let started = Instant::now();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(5),
        move || {
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "Request recovery timed out"
            );
            if ready() {
                slint::quit_event_loop().unwrap();
            }
        },
    );
    ui.show().unwrap();
    slint::run_event_loop().unwrap();
    timer.stop();
}

fn page_published(controller: &Controller) -> bool {
    controller
        .search_publication
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|publication| publication.model_published)
}

fn hold_metadata_barrier(controller: &Controller) {
    // Deterministically hold an admitted FIFO write while the real selection
    // worker and independent page worker race; no source/database write is needed.
    let mut state = controller.metadata.lock().unwrap();
    assert!(state.jobs.is_empty() && state.in_flight.is_none());
    state.sequence += 1;
}

fn release_metadata_barrier(controller: &Controller) {
    let mut state = controller.metadata.lock().unwrap();
    state.completed = state.sequence;
    drop(state);
    controller.metadata_wake.notify_all();
}

#[test]
fn a_lone_letter_cancels_searches_and_rejects_late_publications() {
    isolated("child_a_lone_letter_cancels_searches");
}

#[test]
#[ignore = "isolated Slint short-query cancellation and publication helper"]
fn child_a_lone_letter_cancels_searches() {
    let (_provider, _engine, ui, controller) = fixture();
    let retained_folders = Arc::new(vec![Folder {
        id: 999,
        name: "Retained navigation".into(),
        file_count: 1,
        ..Default::default()
    }]);
    controller.tree.lock().unwrap().matches = retained_folders.clone();
    publish_audio_rows(
        &ui,
        vec![audio_row(&AudioFile {
            id: 999,
            filename: "Retained.wav".into(),
            ..Default::default()
        })],
    );
    for (query, chinese, immediate) in [("e", true, false), (" (e\u{301}) ", false, true)] {
        ui.set_query("A.wav".into());
        controller.request_search(&ui, false, true);
        let previous_cancel = controller.search_cancel.lock().unwrap().clone();
        let previous_generation = controller.search_generation.load(Ordering::Acquire);
        let previous_tree = controller.tree_generation.load(Ordering::Acquire);
        controller.chinese.store(chinese, Ordering::Release);
        ui.set_chinese(chinese);
        ui.set_query(query.into());
        controller.request_search(&ui, immediate, true);
        assert!(previous_cancel.load(Ordering::Acquire));
        assert!(controller.search_generation.load(Ordering::Acquire) > previous_generation);
        assert!(controller.tree_generation.load(Ordering::Acquire) > previous_tree);
        assert!(controller.search_job.lock().unwrap().is_none());
        assert!(controller.facet_job.lock().unwrap().is_none());
        assert!(controller.search_publication.lock().unwrap().is_none());
        assert!(!ui.get_searching());
        assert_eq!(
            ui.get_status(),
            if chinese {
                "字母搜索请至少输入 2 个字符"
            } else {
                "Enter at least 2 characters for a letter search"
            }
        );
        // Completions from an already-running page/facet query cannot restore its
        // publication state or replace the navigation while the input is waiting.
        controller.search_page_published(&ui, previous_generation, true);
        controller.search_folders_computed(&ui, previous_generation, Ok(Arc::new(Vec::new())));
        assert!(controller.search_publication.lock().unwrap().is_none());
        assert!(Arc::ptr_eq(
            &controller.tree.lock().unwrap().matches,
            &retained_folders
        ));
    }
    let c = controller.clone();
    let window = ui.as_weak();
    let started = Instant::now();
    let stage = Rc::new(Cell::new(0));
    let observed = stage.clone();
    run_until(&ui, move || {
        let ui = window.upgrade().unwrap();
        match observed.get() {
            0 if started.elapsed() >= Duration::from_millis(220) => {
                assert!(!ui.get_searching());
                assert_eq!(ui.get_rows().row_count(), 1);
                assert_eq!(ui.get_rows().row_data(0).unwrap().name, "Retained.wav");
                assert_eq!(
                    ui.get_status(),
                    "Enter at least 2 characters for a letter search"
                );
                assert!(c.search_job.lock().unwrap().is_none());
                assert!(c.facet_job.lock().unwrap().is_none());
                assert!(c.search_publication.lock().unwrap().is_none());
                assert!(Arc::ptr_eq(
                    &c.tree.lock().unwrap().matches,
                    &retained_folders
                ));
                ui.set_query("".into());
                c.request_search(&ui, true, true);
                observed.set(1);
            }
            1 if page_published(&c) && ui.get_rows().row_count() == 2 => return true,
            _ => {}
        }
        false
    });
    controller.shutdown();
    ui.hide().unwrap();
    assert_eq!(stage.get(), 1);
}

#[test]
fn selection_completion_tracks_the_current_page_identity() {
    isolated("child_selection_completion_tracks_page");
}

#[test]
#[ignore = "isolated Slint delayed selection/page ordering helper"]
fn child_selection_completion_tracks_page() {
    let (_provider, _engine, ui, controller) = fixture();
    let stage = Rc::new(Cell::new(0));
    let observed = stage.clone();
    let c = controller.clone();
    let window = ui.as_weak();
    run_until(&ui, move || {
        let ui = window.upgrade().unwrap();
        match observed.get() {
            0 if page_published(&c) && ui.get_rows().row_count() == 2 => {
                c.select_row(0, false, false, false);
                observed.set(1);
            }
            1 if ui.get_selected_name() == "A.wav" && ui.get_audio_ready() => {
                hold_metadata_barrier(&c);
                c.select_row(1, false, false, false);
                assert_eq!(ui.get_selected_row(), 1);
                c.request_search(&ui, true, false);
                observed.set(2);
            }
            2 if page_published(&c) && ui.get_selected_row() == 0 => {
                assert_eq!(ui.get_selected_name(), "A.wav");
                release_metadata_barrier(&c);
                observed.set(3);
            }
            3 if ui.get_selected_name() == "B.wav" && ui.get_audio_ready() => {
                let row = ui
                    .get_rows()
                    .row_data(ui.get_selected_row() as usize)
                    .unwrap();
                assert_eq!(row.id, ui.get_selected_id());
                assert_eq!(row.name, "B.wav");
                c.select_row(0, false, false, false);
                observed.set(4);
            }
            4 if ui.get_selected_name() == "A.wav" && ui.get_audio_ready() => {
                hold_metadata_barrier(&c);
                c.select_row(1, false, false, false);
                ui.set_query("A.wav".into());
                c.request_search(&ui, true, true);
                observed.set(5);
            }
            5 if page_published(&c) && ui.get_rows().row_count() == 1 => {
                assert_eq!(ui.get_rows().row_data(0).unwrap().name, "A.wav");
                release_metadata_barrier(&c);
                observed.set(6);
            }
            6 if ui.get_selected_name() == "B.wav" => {
                assert_eq!(ui.get_selected_row(), -1);
                observed.set(7);
                return true;
            }
            _ => {}
        }
        false
    });
    controller.shutdown();
    ui.hide().unwrap();
    assert_eq!(stage.get(), 7);
}

#[test]
fn failed_close_resumes_reads_and_preserves_failed_drafts_without_replaying() {
    isolated("child_failed_close_resumes_reads");
}

#[test]
#[ignore = "isolated Slint failed Metadata close/read recovery helper"]
fn child_failed_close_resumes_reads() {
    let (provider, _engine, ui, controller) = fixture();
    let stage = Rc::new(Cell::new(0));
    let observed = stage.clone();
    let loaded_generation = Rc::new(Cell::new(0));
    let initial_audio = loaded_generation.clone();
    let requested_sequence = Rc::new(Cell::new(0));
    let close_sequence = requested_sequence.clone();
    let selected_id = Rc::new(Cell::new(0));
    let removed_id = selected_id.clone();
    let c = controller.clone();
    let window = ui.as_weak();
    run_until(&ui, move || {
        let ui = window.upgrade().unwrap();
        match observed.get() {
            0 if page_published(&c) && ui.get_rows().row_count() == 2 => {
                c.select_row(0, false, false, false);
                observed.set(1);
            }
            1 if ui.get_selected_name() == "A.wav" && ui.get_audio_ready() => {
                // A second instance removes the displayed record before this
                // window saves it. The source WAV remains and the real write fails.
                let id = ui.get_selected_id().parse::<i64>().unwrap();
                removed_id.set(id);
                initial_audio.set(c.audio.status().generation);
                provider.delete_audio(id).unwrap();
                ui.set_edit_notes("must survive failed close".into());
                ui.set_metadata_dirty(true);
                c.select_row(1, false, false, true);
                ui.set_query("B.wav".into());
                // Recovery must also retain the save error when a shrinking
                // result set forces the page worker to correct the old offset.
                c.request.lock().unwrap().offset = PAGE_SIZE;
                c.request_search(&ui, false, false);
                ui.set_waveform_ready(false);
                c.request_close(&ui);
                close_sequence.set(c.metadata.lock().unwrap().sequence);
                observed.set(2);
            }
            2 if !c.closing.load(Ordering::Acquire)
                && page_published(&c)
                && ui.get_waveform_ready() =>
            {
                assert_eq!(ui.get_query(), "B.wav");
                assert_eq!(ui.get_rows().row_count(), 1);
                assert_eq!(ui.get_rows().row_data(0).unwrap().name, "B.wav");
                assert_eq!(c.tree.lock().unwrap().matches[0].file_count, 1);
                assert_eq!(ui.get_selected_name(), "A.wav");
                assert_eq!(ui.get_selected_row(), -1);
                assert!(ui.get_status().starts_with("Not found:"));
                assert!(!ui.get_searching() && !ui.get_closing());
                let state = c.metadata.lock().unwrap();
                assert_eq!(state.sequence, close_sequence.get());
                assert_eq!(state.completed, close_sequence.get());
                assert!(state.jobs.is_empty() && state.in_flight.is_none());
                assert_eq!(
                    state.drafts[&removed_id.get()].patch.notes.as_deref(),
                    Some("must survive failed close")
                );
                assert!(c.latest_selection.lock().unwrap().is_none());
                assert_eq!(c.audio.status().generation, initial_audio.get());
                assert_ne!(c.audio.status().state, PlaybackState::Playing);
                observed.set(3);
                return true;
            }
            _ => {}
        }
        false
    });
    controller.shutdown();
    ui.hide().unwrap();
    assert_eq!(stage.get(), 3);
}

#[test]
fn server_play_history_is_empty_offline_while_scan_history_uses_the_snapshot() {
    isolated("child_server_history_routes_provider");
}

#[test]
#[ignore = "isolated Slint local mock server history provider helper"]
fn child_server_history_routes_provider() {
    let (provider, _engine, ui, controller) = fixture();
    let files = provider.search(&SearchRequest::default()).unwrap().results;
    provider.record_play(files[0].id).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    // A stale UI list must be cleared by the completed empty-history response.
    controller.plays.lock().unwrap().push(PlayRecord {
        id: 999,
        ..Default::default()
    });
    controller.config.lock().unwrap().data_source.server_url = format!("http://{address}");
    controller.remote.lock().unwrap().state = Some(audiolib_remote::CacheState {
        library_id: "history-test-library".into(),
        database_file: "snapshot.db".into(),
        server_url: format!("http://{address}"),
        ..Default::default()
    });
    controller.remote_mode.store(true, Ordering::Release);
    ui.set_history_kind(1);
    controller.load_history(0);
    let c = controller.clone();
    let window = ui.as_weak();
    let stage = Rc::new(Cell::new(0));
    let observed = stage.clone();
    run_until(&ui, move || {
        let ui = window.upgrade().unwrap();
        match observed.get() {
            0 if c.plays.lock().unwrap().is_empty() => {
                assert_eq!(ui.get_history_rows().row_count(), 0);
                ui.set_history_kind(0);
                c.load_history(0);
                observed.set(1);
            }
            1 if ui.get_history_rows().row_count() == 1 => {
                assert!(
                    ui.get_history_rows()
                        .row_data(0)
                        .unwrap()
                        .contains("sounds")
                );
                assert_eq!(c.scans.lock().unwrap().len(), 1);
                observed.set(2);
                return true;
            }
            _ => {}
        }
        false
    });
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert!(provider.play_history(50, 0).unwrap().is_empty());
    controller.shutdown();
    ui.hide().unwrap();
    assert_eq!(stage.get(), 2);
}
