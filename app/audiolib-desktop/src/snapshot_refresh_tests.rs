// app/audiolib-desktop/src/snapshot_refresh_tests.rs
//! Isolated regressions for automatic server refresh during audition and search retention.
use super::super::*;
use super::source_transition_tests::{
    configuration, isolated_module, request, respond, root, run_until,
};
use super::*;
use std::io::Write;
use std::sync::atomic::AtomicUsize;

fn fixture(
    count: usize,
) -> (
    Arc<Controller>,
    AppWindow,
    audiolib_audio::AudioEngine,
    DataSourceSwitch,
    PathBuf,
) {
    let root = root();
    let sounds = root.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(996u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(48_000u32.to_le_bytes());
    bytes.extend(96_000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(960u32.to_le_bytes());
    bytes.resize(1004, 0);
    for index in 0..count {
        std::fs::write(sounds.join(format!("metal-{index:04}.wav")), &bytes).unwrap();
    }
    std::fs::write(sounds.join("wind.wav"), &bytes).unwrap();
    let source = Arc::new(SqliteProvider::open(&root.join("source.db")).unwrap());
    ScanService::new(source.clone())
        .scan(
            &sounds,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let ids = source
        .search(&SearchRequest {
            limit: 500,
            ..Default::default()
        })
        .unwrap()
        .results
        .into_iter()
        .map(|file| file.id)
        .collect();
    source
        .update_metadata_batch(&BatchMetadataRequest {
            ids,
            patch: MetadataPatch {
                tags: Some("reviewed".into()),
                favorite: Some(true),
                ..Default::default()
            },
            tag_mode: TagEditMode::Replace,
        })
        .unwrap();
    let mut config = configuration(&root.join("preferences"));
    config.data_source.mode = audiolib_core::config::DataSourceMode::Server;
    config.data_source.server_url = "http://127.0.0.1:9".into();
    config.data_source.poll_interval_secs = 0;
    let cache = RemoteCache::new(&config.data_root());
    let old = CacheState {
        library_id: "audition-library".into(),
        server_url: config.data_source.server_url.clone(),
        revision: 1,
        database_file: "snapshot-r1.db".into(),
        ..Default::default()
    };
    let next = CacheState {
        revision: 2,
        database_file: "snapshot-r2.db".into(),
        ..old.clone()
    };
    let database = cache.database_path(&old);
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    source.backup_library(&database).unwrap();
    source.backup_library(&cache.database_path(&next)).unwrap();
    cache.save_state(&old).unwrap();
    let provider = Arc::new(SqliteProvider::open_read_only(&database).unwrap());
    let playback = PlaybackConfig {
        volume: 0.0,
        ..Default::default()
    };
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback);
    controller.remote_mode.store(true, Ordering::Release);
    controller.remote.lock().unwrap().state = Some(old);
    (
        controller,
        ui,
        engine,
        DataSourceSwitch::Server {
            database: cache.database_path(&next),
            state: Some(next),
        },
        sounds,
    )
}

fn await_results(controller: &Arc<Controller>, ui: &AppWindow, rows: usize) {
    let c = controller.clone();
    let window = ui.as_weak();
    run_until(ui, move || {
        window.upgrade().is_some_and(|ui| {
            !c.library_busy.load(Ordering::Acquire)
                && !ui.get_searching()
                && ui.get_rows().row_count() == rows
                && c.search_publication
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|publication| {
                        publication.generation == c.search_generation.load(Ordering::Acquire)
                            && publication.model_published
                    })
        })
    });
}

#[test]
fn a_same_library_refresh_retains_the_filtered_search_folder_and_page() {
    isolated_module("snapshot_refresh_tests", "child_search_retention");
}

#[test]
#[ignore = "isolated Slint snapshot search retention regression"]
fn child_search_retention() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (controller, ui, _engine, target, _) = fixture(405);
    let folder = controller.provider().folders(None).unwrap().remove(0);
    ui.set_query("metal".into());
    ui.set_sample_filter(2);
    ui.set_bit_filter(1);
    ui.set_channel_filter(1);
    ui.set_favorites_only(true);
    ui.set_tagged_only(true);
    ui.set_tag_filter("reviewed".into());
    ui.set_duration_min("0".into());
    ui.set_duration_max("1".into());
    ui.set_sort_index(1);
    ui.set_sort_descending(true);
    {
        let mut tree = controller.tree.lock().unwrap();
        tree.selected = Some(folder.id);
        tree.expanded.insert(folder.id);
        tree.known = Arc::new(HashMap::from([(folder.id, folder.clone())]));
    }
    {
        let mut request = controller.request.lock().unwrap();
        request.folder_id = Some(folder.id);
        request.offset = PAGE_SIZE;
    }
    controller.request_search(&ui, true, false);
    await_results(&controller, &ui, PAGE_SIZE);
    let before = controller.request.lock().unwrap().clone();
    let old_provider = controller.provider();
    // An already queued search cannot republish old snapshot rows after the swap.
    controller.request_search(&ui, false, false);
    let old_cancel = controller.search_cancel.lock().unwrap().clone();
    controller.run_library_operation(LibraryOperation::DataSource(target.clone()));
    await_results(&controller, &ui, PAGE_SIZE);
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&old_provider, &controller.provider()));
    assert_eq!(ui.get_query(), "metal");
    assert_eq!(ui.get_sort_index(), 1);
    assert!(ui.get_sort_descending());
    assert_eq!(ui.get_sample_filter(), 2);
    assert!(ui.get_favorites_only() && ui.get_tagged_only());
    assert_eq!(ui.get_tag_filter(), "reviewed");
    assert_eq!(ui.get_page_label(), "2/3");
    let after = controller.request.lock().unwrap().clone();
    assert_eq!(after.query, before.query);
    assert_eq!(after.filters, before.filters);
    assert_eq!(after.folder_id, before.folder_id);
    assert_eq!(after.offset, before.offset);
    assert_eq!(after.order_by, before.order_by);
    assert_eq!(after.descending, before.descending);
    assert_eq!(controller.tree.lock().unwrap().selected, Some(folder.id));
    assert!(
        controller
            .tree
            .lock()
            .unwrap()
            .expanded
            .contains(&folder.id)
    );
    assert_eq!(ui.get_selected_folder_path(), folder.path);
    assert!(
        (0..ui.get_rows().row_count())
            .all(|row| { ui.get_rows().row_data(row).unwrap().name.contains("metal") })
    );
    let DataSourceSwitch::Server { database, state } = target else {
        unreachable!()
    };
    let mut other_endpoint = state.clone().unwrap();
    other_endpoint.server_url = "http://127.0.0.1:10".into();
    assert!(
        !controller.is_same_library_snapshot_refresh(&DataSourceSwitch::Server {
            database: database.clone(),
            state: Some(other_endpoint),
        })
    );
    // A genuine library replacement still invalidates the old folder IDs and query.
    let mut other_library = state.unwrap();
    other_library.library_id = "replacement-library".into();
    let cache = RemoteCache::new(&controller.data_root());
    let replacement_database = cache.database_path(&other_library);
    std::fs::create_dir_all(replacement_database.parent().unwrap()).unwrap();
    controller
        .provider()
        .backup_library(&replacement_database)
        .unwrap();
    controller.run_library_operation(LibraryOperation::DataSource(DataSourceSwitch::Server {
        database: replacement_database,
        state: Some(other_library),
    }));
    await_results(&controller, &ui, PAGE_SIZE);
    assert_eq!(ui.get_query(), "");
    assert_eq!(controller.request.lock().unwrap().folder_id, None);
    assert_eq!(controller.request.lock().unwrap().offset, 0);
    // Sorting is now a global table preference, independent of library query and IDs.
    assert_eq!(ui.get_sort_index(), 1);
    assert!(ui.get_sort_descending());
    assert_eq!(
        controller.request.lock().unwrap().order_by,
        SortOrder::Filename
    );
    assert!(controller.request.lock().unwrap().descending);
    assert!(!ui.get_favorites_only());
    controller.shutdown();
    ui.hide().unwrap();
}

#[test]
fn automatic_resync_waits_for_loading_audio_and_coalesces_duplicate_requests() {
    isolated_module("snapshot_refresh_tests", "child_loading_refresh");
}

#[test]
#[ignore = "isolated Slint in-flight audition refresh regression"]
fn child_loading_refresh() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (controller, ui, _engine, _, sounds) = fixture(1);
    ui.set_query("metal".into());
    controller.request_search(&ui, true, true);
    await_results(&controller, &ui, 1);
    let old_provider = controller.provider();
    let bytes = Arc::new(std::fs::read(sounds.join("metal-0000.wav")).unwrap());
    let virtual_path = sounds.join("loading.wav");
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let first = AtomicBool::new(true);
    let opener: audiolib_audio::SourceOpener = Arc::new(move || {
        if first.swap(false, Ordering::AcqRel) {
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(4))
                .unwrap();
        }
        let source: Box<dyn audiolib_audio::ByteSource> =
            Box::new(std::io::Cursor::new(bytes.as_ref().clone()));
        Ok((source, bytes.len() as u64))
    });
    audiolib_audio::register_stream_source(&virtual_path, opener);
    controller
        .audio
        .send(AudioCommand::Load(virtual_path.clone()))
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(4)).unwrap();
    assert_eq!(controller.audio.status().state, PlaybackState::Loading);
    controller.config.lock().unwrap().data_source.auto_sync = true;
    controller.apply_revision_check(
        &ui,
        Some(CachedIdentity {
            library_id: "audition-library".into(),
            revision: 1,
        }),
        RevisionCheck::NewRevision {
            cached: 1,
            server: 2,
        },
        None,
        true,
    );
    assert!(controller.remote.lock().unwrap().auto_deferred);
    assert!(!controller.remote_busy.load(Ordering::Acquire));
    controller.post_remote_resync("owned test revision mismatch");
    controller.post_remote_resync("owned duplicate revision mismatch");
    assert_eq!(controller.remote.lock().unwrap().resync_pending, Some(0));
    let c = controller.clone();
    let started = Instant::now();
    let release = Arc::new(AtomicBool::new(false));
    let released = release.clone();
    let window = ui.as_weak();
    run_until(&ui, move || {
        if started.elapsed() < Duration::from_millis(250) {
            assert!(Arc::ptr_eq(&old_provider, &c.provider()));
            assert!(!c.library_busy.load(Ordering::Acquire));
            assert_eq!(window.upgrade().unwrap().get_query(), "metal");
            assert_eq!(window.upgrade().unwrap().get_rows().row_count(), 1);
            return false;
        }
        if !released.swap(true, Ordering::AcqRel) {
            release_tx.send(()).unwrap();
        }
        c.remote_generation.load(Ordering::Acquire) == 1 && !c.remote_busy.load(Ordering::Acquire)
    });
    assert!(release.load(Ordering::Acquire));
    assert_eq!(ui.get_query(), "metal");
    assert_eq!(ui.get_rows().row_count(), 1);
    assert_eq!(controller.audio.status().state, PlaybackState::Ready);
    assert_eq!(controller.remote_generation.load(Ordering::Acquire), 1);
    assert!(controller.remote.lock().unwrap().resync_pending.is_none());
    controller.shutdown();
    audiolib_audio::unregister_stream_source(&virtual_path);
    ui.hide().unwrap();
}

#[test]
fn reordered_write_callbacks_keep_the_current_snapshot_revision() {
    isolated_module("snapshot_refresh_tests", "child_write_callback_order");
}

#[test]
#[ignore = "isolated Slint write callback ordering regression"]
fn child_write_callback_order() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (controller, ui, _engine, _, _) = fixture(1);
    let mut state = controller.remote.lock().unwrap().state.clone().unwrap();
    // Both writes have already published their cache updates under WRITE_LOCK.
    // Their provider listeners run after that lock is released and can reorder.
    state.revision = 45;
    let cache = RemoteCache::new(&controller.data_root());
    cache.save_state(&state).unwrap();
    controller.remote.lock().unwrap().check = Some(RevisionCheck::NewRevision {
        cached: 1,
        server: 45,
    });
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let delayed = controller.clone();
    let library_id = state.library_id.clone();
    let database_file = state.database_file.clone();
    let first = std::thread::spawn(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(4)).unwrap();
        delayed.remote_write_event(
            &library_id,
            &database_file,
            WriteEvent::Applied { revision: 44 },
        );
    });
    entered_rx.recv_timeout(Duration::from_secs(4)).unwrap();
    controller.remote_write_event(
        &state.library_id,
        &state.database_file,
        WriteEvent::Applied { revision: 45 },
    );
    release_tx.send(()).unwrap();
    first.join().unwrap();
    {
        let remote = controller.remote.lock().unwrap();
        assert_eq!(remote.state.as_ref().unwrap().revision, 45);
        assert!(matches!(
            remote.check,
            Some(RevisionCheck::UpToDate { revision: 45 })
        ));
    }
    assert_eq!(cache.state(&state.library_id).unwrap().revision, 45);
    // A revision check made against the latest published cache remains current,
    // rather than being discarded because a delayed callback regressed runtime.
    controller.apply_revision_check(
        &ui,
        Some(CachedIdentity {
            library_id: state.library_id,
            revision: 45,
        }),
        RevisionCheck::UpToDate { revision: 45 },
        None,
        false,
    );
    assert_eq!(ui.get_remote_notice_kind(), NOTICE_NONE);
    assert_eq!(controller.remote_generation.load(Ordering::Acquire), 0);
    assert!(!controller.remote_busy.load(Ordering::Acquire));
    controller.shutdown();
    ui.hide().unwrap();
}

#[allow(dead_code)]
struct SnapshotServer {
    url: String,
    requests: Arc<AtomicUsize>,
    snapshots: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<()>,
}

#[allow(dead_code)]
impl SnapshotServer {
    fn start(library_id: &str, revision: u64, bytes: Vec<u8>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let snapshots = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = requests.clone();
        let downloaded = snapshots.clone();
        let stopping = stop.clone();
        let library_id = library_id.to_owned();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let mut socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept snapshot mock: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                let request = request(&mut socket);
                seen.fetch_add(1, Ordering::AcqRel);
                let path = request.split_whitespace().nth(1).unwrap();
                match path {
                    "/api/v1/health" => {
                        respond(&mut socket, "200 OK", r#"{"status":"ok","version":"test"}"#)
                    }
                    "/api/v1/library" => respond(
                        &mut socket,
                        "200 OK",
                        &serde_json::json!({
                            "library_id": library_id,
                            "identity_bound_writes": true,
                            "revision": revision,
                            "schema_version": 5,
                        })
                        .to_string(),
                    ),
                    "/api/v1/roots" => respond(&mut socket, "200 OK", "[]"),
                    "/api/v1/library/snapshot" => {
                        downloaded.fetch_add(1, Ordering::AcqRel);
                        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nX-Library-Id: {library_id}\r\nX-Library-Revision: {revision}\r\nX-Schema-Version: 5\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
                        socket.write_all(&bytes).unwrap();
                    }
                    _ => respond(
                        &mut socket,
                        "404 Not Found",
                        r#"{"error_code":"NOT_FOUND","message":"unsupported test endpoint"}"#,
                    ),
                }
            }
        });
        Self {
            url,
            requests,
            snapshots,
            stop,
            worker,
        }
    }

    fn finish(self) {
        self.stop.store(true, Ordering::Release);
        self.worker.join().unwrap();
    }
}
