// app/audiolib-desktop/src/vendor_import_ui_tests.rs
//! Vendor editor state and controlled long-write lifecycle regressions.
use super::*;
use audiolib_core::{self as core, vendor_import::*};
use slint::Model;
use std::sync::{Condvar, Mutex, mpsc};

const ASYNC_ROOT_ENV: &str = "AUDIOLIB_VENDOR_UI_ASYNC_ROOT";

fn isolated_async(child: &str) {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("controller::search_v2_ux::vendor_ui::tests::{child}"),
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(ASYNC_ROOT_ENV, root.path())
        .env("AUDIOLIB_DATA_DIR", root.path().join("preferences"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{child}: {}\n{stdout}\n{stderr}",
        output.status
    );
}

fn pump_until(ready: impl Fn() -> bool + 'static) {
    let ready = std::rc::Rc::new(ready);
    if ready() {
        return;
    }
    let started = Instant::now();
    let completed = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    let poll = ready.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(10),
        move || {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "vendor UI event-loop transition completes"
            );
            if poll() && !observed.replace(true) {
                slint::quit_event_loop().unwrap();
            }
        },
    );
    slint::run_event_loop().unwrap();
    timer.stop();
    assert!(
        completed.get() || ready(),
        "event loop exited before the expected vendor publication"
    );
}

fn async_controller() -> (AppWindow, Arc<Controller>, audiolib_audio::AudioEngine) {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let root = std::path::PathBuf::from(
        std::env::var_os(ASYNC_ROOT_ENV).expect("isolated parent supplies an owned root"),
    );
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    let preferences = root.join("preferences");
    std::fs::create_dir_all(&preferences).unwrap();
    // Explicit fixture settings prevent first-run discovery of any working library.
    std::fs::write(
        preferences.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "active_database": root.join("library.db"),
            "language": "en",
            "audition_play_on_single_click": false,
        }))
        .unwrap(),
    )
    .unwrap();
    let (ui, c, engine) = controller(&root);
    let weak = ui.as_weak();
    let pending = c.clone();
    pump_until(move || {
        weak.upgrade().is_some_and(|ui| {
            !ui.get_search_workspace_loading()
                && !ui.get_task_restoring()
                && !ui.get_searching()
                && !pending.task_workspace.lock().unwrap().loading
        })
    });
    (ui, c, engine)
}

fn preview() -> VendorImportPreview {
    VendorImportPreview {
        import_id: "current-library-batch".into(),
        state: Some("preview".into()),
        rows: 1,
        matched: 1,
        can_commit: true,
        sheets: vec![VendorSheetPreview {
            source_file: "catalog.csv".into(),
            sheet: "CSV".into(),
            header_row: 1,
            headers: vec!["Sound".into(), "Text".into()],
            mapping: VendorColumnMapping {
                filename: Some("Sound".into()),
                ..Default::default()
            },
            rows: 1,
        }],
        samples: vec![VendorRowPreview {
            row_id: "0:CSV:2".into(),
            source_file: "catalog.csv".into(),
            sheet: "CSV".into(),
            row: 2,
            suggested: true,
            candidates: vec![
                VendorMatchCandidate {
                    id: 11,
                    path: "/A/door.wav".into(),
                },
                VendorMatchCandidate {
                    id: 22,
                    path: "/B/door.wav".into(),
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn controller(root: &std::path::Path) -> (AppWindow, Arc<Controller>, audiolib_audio::AudioEngine) {
    let config = core::config::AppConfig::load_in(root.join("preferences")).unwrap();
    let provider = Arc::new(audiolib_db::SqliteProvider::open(&config.paths.database).unwrap());
    let engine = audiolib_audio::AudioEngine::new(Default::default()).unwrap();
    let ui = AppWindow::new().unwrap();
    let c = Controller::new(&ui, provider, engine.handle(), config, Default::default());
    (ui, c, engine)
}

#[test]
fn local_import_and_explicit_mapping_confirmation_keep_library_admission() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
    ui.invoke_vendor_import_requested();
    assert_eq!(ui.get_vi_state(), VI_CHOOSE);
    c.publish_vendor_preview(&ui, preview(), None);
    assert_eq!(ui.get_vi_rows().row_data(0).unwrap().selected, 0);
    ui.invoke_vi_confirm_row(0, 2);
    assert!(!ui.get_vi_can_commit());
    assert!(ui.get_vi_mapping_dirty());
    {
        let state = c.search_v2_ux.lock().unwrap();
        assert_eq!(
            state
                .vendor
                .pending
                .as_ref()
                .unwrap()
                .preview
                .options
                .confirmations
                .get("0:CSV:2"),
            Some(&22)
        );
    }
    ui.invoke_vi_mapping_changed(2, 2);
    ui.set_vi_custom_field("licence".into());
    ui.set_vi_custom_header(2);
    ui.invoke_vi_add_mapping();
    {
        let state = c.search_v2_ux.lock().unwrap();
        let mapping = &state.vendor.pending.as_ref().unwrap().preview.sheets[0].mapping;
        assert_eq!(
            mapping.fields.get("description").map(String::as_str),
            Some("Text")
        );
        assert_eq!(
            mapping.fields.get("licence").map(String::as_str),
            Some("Text")
        );
    }
    let generation = c.library_generation.load(Ordering::Acquire);
    let request = c.search_v2_ux.lock().unwrap().vendor.request;
    c.search_v2_ux.lock().unwrap().reset();
    assert!(!c.vendor_context(generation, request));
    c.shutdown();
}

#[test]
fn paused_history_retains_original_commit_key_and_disables_mapping_changes() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
    let mut preview = preview();
    preview.state = Some("paused".into());
    let status = VendorImportStatus {
        preview: preview.clone(),
        idempotency_key: Some("durable-original-key".into()),
        result: Some(VendorImportResult {
            state: Some("paused".into()),
            written: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    c.publish_vendor_preview(&ui, preview, Some(&status));
    assert!(ui.get_vi_resumable());
    assert!(!ui.get_vi_editable());
    assert!(ui.get_vi_can_rollback());
    ui.invoke_vi_mapping_changed(0, 0);
    let state = c.search_v2_ux.lock().unwrap();
    let pending = state.vendor.pending.as_ref().unwrap();
    assert_eq!(pending.key, "durable-original-key");
    assert_eq!(
        pending.preview.sheets[0].mapping.filename.as_deref(),
        Some("Sound")
    );
    drop(state);
    c.shutdown();
}

#[test]
fn failed_history_keeps_persisted_error_visible_without_offering_a_resume() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
    let mut preview = preview();
    preview.state = Some("failed".into());
    let status = VendorImportStatus {
        preview: preview.clone(),
        result: Some(VendorImportResult {
            state: Some("failed".into()),
            written: 1,
            ..Default::default()
        }),
        idempotency_key: Some("failed-original-key".into()),
        error: Some("owned preview is incomplete".into()),
        ..Default::default()
    };
    c.publish_vendor_preview(&ui, preview, Some(&status));
    assert!(ui.get_vi_text().contains("owned preview is incomplete"));
    assert!(!ui.get_vi_resumable());
    assert!(!ui.get_vi_can_commit());
    assert!(!ui.get_vi_editable());
    assert!(ui.get_vi_can_rollback());
    ui.invoke_vi_commit();
    assert!(!c.search_v2_busy.load(Ordering::Acquire));
    c.shutdown();
}

#[test]
fn parser_options_round_trip_and_csv_export_neutralizes_formulas() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let options = VendorImportOptions {
        vendor_root: Some("/素材/厂商".into()),
        encoding: Some("GB18030".into()),
        delimiter: Some("\t".into()),
        fuzzy: true,
        overwrite: true,
        preferred_languages: vec!["zh".into(), "en".into()],
        ..Default::default()
    };
    set_options(&ui, &options);
    assert_eq!(current_options(&ui, None), options);
    assert_eq!(csv_cell("=SUM(A1)"), "\"'=SUM(A1)\"");
    assert_eq!(csv_cell("safe\"text"), "\"safe\"\"text\"");
}

struct ControlledProvider {
    inner: Arc<audiolib_db::SqliteProvider>,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    released: Mutex<bool>,
    cancelled: Mutex<Vec<String>>,
    wake: Condvar,
    cancel_wake: Condvar,
    row_calls: Mutex<Vec<u64>>,
    candidate_calls: Mutex<Vec<u64>>,
}

impl ControlledProvider {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn wait_cancel(&self) {
        let cancelled = self.cancelled.lock().unwrap();
        let (cancelled, timeout) = self
            .cancel_wake
            .wait_timeout_while(cancelled, Duration::from_secs(5), |calls| calls.is_empty())
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "close/source transition must cancel the original batch"
        );
        assert_eq!(&*cancelled, &["current-library-batch"]);
    }
    fn wait_write_release(&self) {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
        }
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {$(
        fn $name(&self,$($arg:$ty),*) -> core::Result<$ret> { self.inner.$name($($arg),*) }
    )*};
}

impl DataProvider for ControlledProvider {
    delegate! {
        search(request:&core::SearchRequest)->core::SearchPage;
        search_folders(request:&core::SearchRequest)->Vec<core::Folder>;
        get_audio(id:i64)->core::AudioFile;
        folders(parent_id:Option<i64>)->Vec<core::Folder>;
        update_metadata(id:i64,patch:&core::MetadataPatch)->core::AudioFile;
        stats()->core::LibraryStats;
        library_info()->core::LibraryInfo;
        check_library()->core::IntegrityReport;
        backup_library(destination:&std::path::Path)->core::BackupReport;
        restore_library(source:&std::path::Path)->core::RestoreReport;
        maintain_library(action:core::MaintenanceAction)->core::MaintenanceReport;
        update_metadata_batch(request:&core::BatchMetadataRequest)->core::BatchMetadataReport;
        find_audio_by_path(path:&std::path::Path)->Option<core::AudioFile>;
        relink_audio(id:i64,path:&std::path::Path)->core::AudioFile;
        remove_audio(ids:&[i64])->u64;
        remove_folder(id:i64)->u64;
        scan_history(limit:usize,offset:usize)->Vec<core::ScanSession>;
        export_library(destination:&std::path::Path,format:core::LibraryExportFormat)->core::TransferReport;
        import_annotations(source:&std::path::Path)->core::TransferReport;
    }
    fn vendor_import_status(&self, _: &str) -> core::Result<VendorImportStatus> {
        let mut preview = preview();
        preview.state = Some("running".into());
        Ok(VendorImportStatus {
            preview,
            total: 1,
            cancellable: true,
            ..Default::default()
        })
    }
    fn vendor_import_commit(&self, id: &str, _: &str) -> core::Result<VendorImportResult> {
        self.wait_write_release();
        Ok(VendorImportResult {
            import_id: id.into(),
            state: Some("paused".into()),
            written: 1,
            ..Default::default()
        })
    }
    fn vendor_import_rollback(&self, id: &str) -> core::Result<VendorImportResult> {
        self.wait_write_release();
        Ok(VendorImportResult {
            import_id: id.into(),
            state: Some("rolled_back".into()),
            restored: 1,
            ..Default::default()
        })
    }
    fn vendor_import_cancel(&self, id: &str) -> core::Result<VendorImportStatus> {
        self.cancelled.lock().unwrap().push(id.into());
        self.cancel_wake.notify_all();
        self.vendor_import_status(id)
    }
    fn vendor_import_rows(
        &self,
        _: &str,
        offset: u64,
        limit: u64,
    ) -> core::Result<Vec<VendorRowPreview>> {
        self.row_calls.lock().unwrap().push(offset);
        Ok((offset..9)
            .take(limit.min(3) as usize)
            .map(|index| VendorRowPreview {
                row_id: index.to_string(),
                row: index + 1,
                ..Default::default()
            })
            .collect())
    }
    fn vendor_import_candidates(
        &self,
        _: &str,
        _: &str,
        offset: u64,
        limit: u64,
    ) -> core::Result<VendorCandidatePage> {
        self.candidate_calls.lock().unwrap().push(offset);
        Ok(VendorCandidatePage {
            offset,
            total: 75,
            candidates: (offset..75)
                .take(limit as usize)
                .map(|index| VendorMatchCandidate {
                    id: index as i64 + 1,
                    path: format!("/candidate-{}/door.wav", index + 1),
                })
                .collect(),
        })
    }
}

fn controlled(
    c: &Arc<Controller>,
    ui: &AppWindow,
) -> (Arc<ControlledProvider>, mpsc::Receiver<()>) {
    controlled_operation(c, ui, true)
}

fn controlled_operation(
    c: &Arc<Controller>,
    ui: &AppWindow,
    commit: bool,
) -> (Arc<ControlledProvider>, mpsc::Receiver<()>) {
    let (entered, seen) = mpsc::channel();
    let provider = Arc::new(ControlledProvider {
        inner: c.provider(),
        entered: Mutex::new(Some(entered)),
        released: Mutex::new(false),
        cancelled: Mutex::new(Vec::new()),
        wake: Condvar::new(),
        cancel_wake: Condvar::new(),
        row_calls: Mutex::new(Vec::new()),
        candidate_calls: Mutex::new(Vec::new()),
    });
    c.search_v2_ux.lock().unwrap().vendor.provider_override = Some(provider.clone());
    c.publish_vendor_preview(ui, preview(), None);
    if !commit {
        ui.set_vi_can_rollback(true);
        ui.set_vi_rollback_confirm(true);
    }
    c.vendor_write(commit);
    seen.recv_timeout(Duration::from_secs(5))
        .expect("controlled vendor write starts");
    (provider, seen)
}

fn paged_provider(c: &Arc<Controller>) -> Arc<ControlledProvider> {
    let provider = Arc::new(ControlledProvider {
        inner: c.provider(),
        entered: Mutex::new(None),
        released: Mutex::new(true),
        cancelled: Mutex::new(Vec::new()),
        wake: Condvar::new(),
        cancel_wake: Condvar::new(),
        row_calls: Mutex::new(Vec::new()),
        candidate_calls: Mutex::new(Vec::new()),
    });
    c.search_v2_ux.lock().unwrap().vendor.provider_override = Some(provider.clone());
    provider
}

fn wait_vendor_job(c: &Arc<Controller>) {
    let pending = c.clone();
    pump_until(move || !pending.search_v2_busy.load(Ordering::Acquire));
    assert!(
        !c.search_v2_busy.load(Ordering::Acquire),
        "vendor page request completes"
    );
}

#[test]
fn candidates_beyond_inline_cap_can_be_paged_inspected_and_explicitly_confirmed() {
    isolated_async("child_candidates_beyond_inline_cap");
}

#[test]
#[ignore = "isolated real event-loop candidate paging regression"]
fn child_candidates_beyond_inline_cap() {
    let (ui, c, _engine) = async_controller();
    let provider = paged_provider(&c);
    let mut preview = preview();
    preview.samples[0].candidates = (1..=32)
        .map(|id| VendorMatchCandidate {
            id,
            path: format!("/candidate-{id}/door.wav"),
        })
        .collect();
    preview.samples[0].candidate_count = 75;
    preview.samples[0].candidates_truncated = true;
    c.publish_vendor_preview(&ui, preview, None);
    assert!(ui.get_vi_rows().row_data(0).unwrap().more_candidates);
    ui.invoke_vi_candidates_requested(0);
    wait_vendor_job(&c);
    assert_eq!(ui.get_vi_candidates_range(), "1–32 / 75");
    assert_eq!(ui.get_vi_candidate_index(), 0);
    ui.invoke_vi_candidates_page(1);
    wait_vendor_job(&c);
    assert_eq!(ui.get_vi_candidates_range(), "33–64 / 75");
    ui.invoke_vi_candidates_page(-1);
    wait_vendor_job(&c);
    assert_eq!(ui.get_vi_candidates_range(), "1–32 / 75");
    ui.invoke_vi_candidates_page(1);
    wait_vendor_job(&c);
    ui.set_vi_candidate_index(9);
    ui.invoke_vi_candidate_selected();
    assert_eq!(ui.get_vi_candidate_path(), "/candidate-41/door.wav");
    assert!(
        c.search_v2_ux
            .lock()
            .unwrap()
            .vendor
            .pending
            .as_ref()
            .unwrap()
            .preview
            .options
            .confirmations
            .is_empty()
    );
    ui.invoke_vi_candidate_confirmed();
    assert_eq!(
        c.search_v2_ux
            .lock()
            .unwrap()
            .vendor
            .pending
            .as_ref()
            .unwrap()
            .preview
            .options
            .confirmations
            .get("0:CSV:2"),
        Some(&41)
    );
    assert!(!ui.get_vi_candidates_visible());
    assert!(ui.get_vi_mapping_dirty());
    assert!(!ui.get_vi_can_commit());
    assert!(ui.get_vi_rows().row_data(0).unwrap().paged_choice);
    assert_eq!(&*provider.candidate_calls.lock().unwrap(), &[0, 32, 0, 32]);
    c.shutdown();
}

#[test]
fn byte_bounded_row_pages_advance_by_actual_length_and_previous_offsets() {
    isolated_async("child_byte_bounded_row_pages");
}

#[test]
#[ignore = "isolated real event-loop row paging regression"]
fn child_byte_bounded_row_pages() {
    let (ui, c, _engine) = async_controller();
    let provider = paged_provider(&c);
    let mut preview = preview();
    preview.rows = 9;
    preview.samples = (0..2)
        .map(|index| VendorRowPreview {
            row_id: index.to_string(),
            row: index + 1,
            ..Default::default()
        })
        .collect();
    c.publish_vendor_preview(&ui, preview, None);
    let mut seen = vec!["0".to_string(), "1".into()];
    for _ in 0..3 {
        ui.invoke_vi_page_rows(1);
        wait_vendor_job(&c);
        seen.extend(
            c.search_v2_ux
                .lock()
                .unwrap()
                .vendor
                .pending
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .map(|row| row.row_id.clone()),
        );
    }
    assert_eq!(
        seen,
        (0..9).map(|index| index.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(ui.get_vi_rows_label(), "9–9 / 9");
    assert!(!ui.get_vi_rows_next());
    for _ in 0..3 {
        ui.invoke_vi_page_rows(-1);
        wait_vendor_job(&c);
    }
    assert_eq!(&*provider.row_calls.lock().unwrap(), &[2, 5, 8, 5, 2, 0]);
    assert!(!ui.get_vi_rows_previous());
    c.shutdown();
}

#[test]
fn close_cancels_original_import_and_waits_for_safe_worker_completion() {
    isolated_async("child_close_cancels_original_import");
}

#[test]
#[ignore = "isolated real event-loop cancelled import close regression"]
fn child_close_cancels_original_import() {
    let (ui, c, _engine) = async_controller();
    let (provider, _) = controlled(&c, &ui);
    assert!(ui.get_vi_cancellable());
    c.request_close(&ui);
    provider.wait_cancel();
    assert!(c.close_after_operation.load(Ordering::Acquire));
    assert!(c.search_v2_busy.load(Ordering::Acquire));
    assert!(!c.closing.load(Ordering::Acquire));
    provider.release();
    wait_vendor_job(&c);
    assert!(!c.search_v2_busy.load(Ordering::Acquire));
    assert!(c.closing.load(Ordering::Acquire));
    c.shutdown();
}

#[test]
fn rollback_cannot_be_cancelled_by_poll_or_close_and_drains_before_exit() {
    isolated_async("child_rollback_drains_before_exit");
}

#[test]
#[ignore = "isolated real event-loop rollback progress and close regression"]
fn child_rollback_drains_before_exit() {
    let (ui, c, _engine) = async_controller();
    let (provider, _) = controlled_operation(&c, &ui, false);
    assert!(!ui.get_vi_cancellable());
    // The controlled provider deliberately returns cancellable=true, as an old
    // import status could before the new rollback phase becomes visible.
    let weak = ui.as_weak();
    pump_until(move || {
        weak.upgrade()
            .is_some_and(|ui| ui.get_vi_progress_text() == "0 / 1")
    });
    assert_eq!(
        ui.get_vi_progress_text(),
        "0 / 1",
        "status polling actually reaches the UI"
    );
    assert!(!ui.get_vi_cancellable());
    ui.invoke_vi_cancel();
    c.request_close(&ui);
    assert!(provider.cancelled.lock().unwrap().is_empty());
    assert!(c.search_v2_busy.load(Ordering::Acquire));
    assert!(!c.closing.load(Ordering::Acquire));
    assert!(c.close_after_operation.load(Ordering::Acquire));
    provider.release();
    wait_vendor_job(&c);
    assert!(provider.cancelled.lock().unwrap().is_empty());
    assert!(c.closing.load(Ordering::Acquire));
    c.shutdown();
}

#[test]
fn forced_library_swap_cancels_old_provider_and_discards_its_late_result() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
    let (provider, _) = controlled(&c, &ui);
    c.library_generation.fetch_add(1, Ordering::AcqRel);
    *c.sqlite.write().unwrap() =
        Arc::new(audiolib_db::SqliteProvider::open(&root.path().join("new-library.db")).unwrap());
    c.reset_library_view(&ui, true, false);
    provider.wait_cancel();
    ui.set_vi_state(VI_CHOOSE);
    ui.set_vi_text("new-library-form".into());
    provider.release();
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(ui.get_vi_text(), "new-library-form");
    assert_eq!(ui.get_vi_state(), VI_CHOOSE);
    assert!(!c.search_v2_busy.load(Ordering::Acquire));
    assert!(c.search_v2_ux.lock().unwrap().vendor.pending.is_none());
    c.shutdown();
}
