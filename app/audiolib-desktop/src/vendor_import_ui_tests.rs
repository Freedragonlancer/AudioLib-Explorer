// app/audiolib-desktop/src/vendor_import_ui_tests.rs
//! Vendor editor state and controlled long-write lifecycle regressions.
use super::*;
use audiolib_core::{self as core, vendor_import::*};
use slint::Model;
use std::sync::{Condvar, Mutex, mpsc};

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
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
        }
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
        Ok(VendorImportResult {
            import_id: id.into(),
            state: Some("paused".into()),
            written: 1,
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
    c.vendor_write(true);
    seen.recv_timeout(Duration::from_secs(5))
        .expect("commit starts");
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

fn wait_vendor_job(c: &Controller) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.search_v2_busy.load(Ordering::Acquire) && Instant::now() < deadline {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !c.search_v2_busy.load(Ordering::Acquire),
        "vendor page request completes"
    );
}

#[test]
fn candidates_beyond_inline_cap_can_be_paged_inspected_and_explicitly_confirmed() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
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
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
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
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let (ui, c, _engine) = controller(root.path());
    let (provider, _) = controlled(&c, &ui);
    c.request_close(&ui);
    provider.wait_cancel();
    assert!(c.close_after_operation.load(Ordering::Acquire));
    assert!(c.search_v2_busy.load(Ordering::Acquire));
    assert!(!c.closing.load(Ordering::Acquire));
    provider.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.search_v2_busy.load(Ordering::Acquire) && Instant::now() < deadline {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!c.search_v2_busy.load(Ordering::Acquire));
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
