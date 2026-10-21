// app/audiolib-desktop/src/source_roots_tests.rs
//! Remote root GUI regressions use owned snapshots and a loopback HTTP fixture.
use super::super::{PAGE_CONNECTION, Selection};
use super::{Controller, publish_directory};
use crate::AppWindow;
use audiolib_audio::{AudioEngine, PlaybackConfig};
use audiolib_core::config::{AppConfig, DataSourceMode};
use audiolib_core::profiles::ServerProfile;
use audiolib_core::{AudioFile, DataProvider, SearchRequest};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use audiolib_remote::{CacheState, RemoteCache, RemoteDirectory, RemoteRoot};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model, ModelRc, VecModel, platform::WindowEvent};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const LIBRARY_ID: &str = "root-controller-test";
const PROFILE_ID: &str = "root-test-profile";
const CHILD_PREFIX: &str =
    "controller::remote_mode::source_manager::source_roots::controller_tests::";

#[test]
fn remote_root_forms_and_browser_ignore_obsolete_results() {
    isolated_child("child_remote_root_forms_and_browse");
}

#[test]
fn root_changes_wait_for_metadata_before_update_and_delete_requests() {
    isolated_child("child_root_changes_drain_metadata");
}

fn isolated_child(name: &str) {
    let temporary = std::env::temp_dir();
    let root = temporary.join(format!(
        "audiolib-root-controller-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{CHILD_PREFIX}{name}"),
            "--ignored",
            "--test-threads=1",
        ])
        .env("AUDIOLIB_ROOT_CONTROLLER_TEST_ROOT", &root)
        .output()
        .unwrap();
    // Resolve the owned child directory before recursive cleanup on Windows.
    let resolved = root.canonicalize().unwrap();
    assert!(resolved.starts_with(temporary.canonicalize().unwrap()));
    assert!(
        resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("audiolib-root-controller-")
    );
    let cleanup = (|| {
        let started = Instant::now();
        loop {
            match std::fs::remove_dir_all(&resolved) {
                Ok(()) => return Ok(()),
                Err(error)
                    if matches!(error.raw_os_error(), Some(5 | 32))
                        && started.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        }
    })();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "Remote root child failed: {}\n{stdout}\n{stderr}",
        output.status
    );
    cleanup.unwrap_or_else(|error| {
        panic!(
            "Could not clean owned fixture {}: {error}",
            resolved.display()
        )
    });
}

struct Fixture {
    ui: AppWindow,
    controller: Arc<Controller>,
    provider: Arc<SqliteProvider>,
    file: AudioFile,
    source_wav: PathBuf,
    local_database: PathBuf,
    _engine: AudioEngine,
}

impl Fixture {
    fn new(server_url: &str) -> Self {
        let root = PathBuf::from(std::env::var_os("AUDIOLIB_ROOT_CONTROLLER_TEST_ROOT").unwrap());
        let sounds = root.join("sounds");
        std::fs::create_dir_all(&sounds).unwrap();
        let source_wav = sounds.join("test.wav");
        write_wav(&source_wav);
        let preferences = root.join("preferences");
        let snapshot = preferences
            .join("remote")
            .join(LIBRARY_ID)
            .join("library-r1-1.db");
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        {
            let server = Arc::new(SqliteProvider::open(&root.join("server.db")).unwrap());
            ScanService::new(server.clone())
                .scan(
                    &sounds,
                    &ScanOptions::default(),
                    &ScanCancellation::default(),
                    |_| {},
                )
                .unwrap();
            server.backup_library(&snapshot).unwrap();
        }
        let provider = Arc::new(SqliteProvider::open_read_only(&snapshot).unwrap());
        let file = provider.search(&SearchRequest::default()).unwrap().results[0].clone();
        let mut config = AppConfig::load_in(preferences.clone()).unwrap();
        config.data_source.mode = DataSourceMode::Server;
        config.data_source.server_url = server_url.to_owned();
        config.data_source.active = Some(PROFILE_ID.into());
        config.data_source.profiles = vec![ServerProfile {
            id: PROFILE_ID.into(),
            name: "Fixture server".into(),
            server_url: server_url.to_owned(),
            library_id: Some(LIBRARY_ID.into()),
            ..Default::default()
        }];
        config.data_source.auto_sync = false;
        config.data_source.poll_interval_secs = 300;
        let local_database = config.paths.database.clone();
        let state = CacheState {
            library_id: LIBRARY_ID.into(),
            revision: 1,
            database_file: "library-r1-1.db".into(),
            server_url: server_url.into(),
            audio_files: 1,
            roots: vec![remote_root("SFX", "/Volumes/SFX")],
            ..Default::default()
        };
        RemoteCache::new(&preferences).save_state(&state).unwrap();
        let playback = PlaybackConfig::default();
        let engine = AudioEngine::new(playback.clone()).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        // Install the owned read-only snapshot without initiating a startup sync.
        let controller = Controller::new(&ui, provider.clone(), engine.handle(), config, playback);
        controller.remote_mode.store(true, Ordering::Release);
        controller.chinese.store(false, Ordering::Release);
        {
            let mut remote = controller.remote.lock().unwrap();
            remote.state = Some(state);
            remote.secrets_loaded = true;
            remote.manager.selected = Selection::Profile(PROFILE_ID.into());
        }
        ui.set_chinese(false);
        controller.publish_data_source(&ui);
        let fixture = Self {
            ui,
            controller,
            provider,
            file,
            source_wav,
            local_database,
            _engine: engine,
        };
        fixture.install_library_page(vec![remote_root("SFX", "/Volumes/SFX")]);
        fixture.ui.show().unwrap();
        fixture
    }

    fn install_library_page(&self, roots: Vec<RemoteRoot>) {
        self.controller.remote.lock().unwrap().manager.roots = roots.clone();
        self.ui.set_sm_roots(ModelRc::new(VecModel::from(
            roots
                .into_iter()
                .map(|root| crate::ServerRootRow {
                    name: root.name.into(),
                    path: root.server_path.into(),
                    status: "online".into(),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        self.ui.set_library_visible(true);
        self.controller.publish_sources(&self.ui);
        assert_eq!(self.ui.get_sm_selected(), 1);
        assert!(self.ui.get_sm_sources().row_data(1).unwrap().active);
        self.ui.set_sm_page(1);
        self.ui.set_sm_root_selected(0);
        self.ui.set_sm_root_controls(true);
        self.ui.set_sm_can_manage(true);
        self.ui.set_ds_syncing(false);
    }
}

fn remote_root(name: &str, path: &str) -> RemoteRoot {
    RemoteRoot {
        name: name.into(),
        server_path: path.into(),
        online: Some(true),
        ..Default::default()
    }
}

fn write_wav(path: &Path) {
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(38u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(48000u32.to_le_bytes());
    bytes.extend(96000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2u32.to_le_bytes());
    bytes.extend(0i16.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

/// Keep request replies under test control to reorder independent directory responses.
struct MockServer {
    address: String,
    requests: mpsc::Receiver<MockRequest>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct MockRequest {
    raw: String,
    stream: TcpStream,
}

impl MockServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (sender, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let mut required = None;
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0, "HTTP request ended before its body");
                    bytes.extend_from_slice(&buffer[..count]);
                    if required.is_none()
                        && let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        required = Some(end + 4 + length);
                    }
                    if required.is_some_and(|required| bytes.len() >= required) {
                        break;
                    }
                }
                if sender
                    .send(MockRequest {
                        raw: String::from_utf8(bytes).unwrap(),
                        stream,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn next(&self) -> MockRequest {
        self.requests.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn next_write(&self, revision: usize) -> MockRequest {
        let preflight = self.next();
        assert!(
            preflight.raw.starts_with("GET /api/v1/library "),
            "Every mutation must verify server identity before writing: {}",
            preflight.raw
        );
        preflight.reply(
            "200 OK",
            serde_json::json!({
                "library_id": LIBRARY_ID,
                "revision": revision,
                "schema_version": 5,
                "identity_bound_writes": true
            }),
        );
        let health = self.next();
        assert!(health.raw.starts_with("GET /api/v1/health "));
        health.reply(
            "200 OK",
            serde_json::json!({"status":"ok", "version":"0.4.11"}),
        );
        let capabilities = self.next();
        assert!(capabilities.raw.starts_with("GET /api/v1/capabilities "));
        capabilities.reply("404 Not Found", serde_json::json!({}));
        self.next()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        self.worker.take().unwrap().join().unwrap();
    }
}

impl MockRequest {
    fn reply(mut self, status: &str, value: serde_json::Value) {
        let body = value.to_string();
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        self.stream.write_all(header.as_bytes()).unwrap();
        self.stream.write_all(body.as_bytes()).unwrap();
    }

    fn directory(self, path: &str) {
        self.reply(
            "200 OK",
            serde_json::json!({
                "path":path, "parent":"/Volumes", "entries":[
                    {"name":"Foley", "path":format!("{path}/Foley")}
                ]
            }),
        );
    }
}

fn pump_until(predicate: impl Fn() -> bool + 'static) {
    let satisfied = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = satisfied.clone();
    let started = Instant::now();
    let timer = std::rc::Rc::new(slint::Timer::default());
    let callback_timer = std::rc::Rc::downgrade(&timer);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(5),
        move || {
            let done = predicate();
            if done || started.elapsed() > Duration::from_secs(5) {
                // The test backend updates timers before consuming Quit. Stop now
                // so it cannot enqueue another Quit for the next event-loop pump.
                callback_timer.upgrade().unwrap().stop();
                observed.set(done);
                slint::quit_event_loop().unwrap();
            }
        },
    );
    slint::run_event_loop().unwrap();
    timer.stop();
    assert!(satisfied.get(), "Timed out waiting for a GUI publication");
}

fn pump_for(duration: Duration) {
    let started = Instant::now();
    pump_until(move || started.elapsed() >= duration);
}

fn directory_finished(ui: &AppWindow) {
    let weak = ui.as_weak();
    pump_until(move || !weak.upgrade().unwrap().get_sm_fs_busy());
}

fn click_label(ui: &AppWindow, label: &str) {
    let element = ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("Missing GUI action: {label}"));
    click_element(ui, &element);
}

fn click_field(ui: &AppWindow, label: &str) {
    let element = ElementHandle::find_by_element_type_name(ui, "ThemedLineEdit")
        .find(|element| {
            let label = label.to_owned();
            element
                .query_descendants()
                .match_predicate(move |child| {
                    child.accessible_label().is_some_and(|value| value == label)
                })
                .find_first()
                .is_some()
        })
        .unwrap_or_else(|| panic!("Missing GUI field: {label}"));
    click_element(ui, &element);
}

fn click_element(ui: &AppWindow, element: &ElementHandle) {
    let origin = element.absolute_position();
    let size = element.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
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

fn escape(ui: &AppWindow) {
    // Modal callbacks schedule focus for the next event-loop turn after replacing
    // their controls; deliver the key only after that normal focus publication.
    pump_for(Duration::from_millis(10));
    crate::ui_tests::send_keys(ui, &[slint::platform::Key::Escape.into()]);
}

#[test]
#[ignore = "isolated GUI helper invoked by remote_root_forms_and_browser_ignore_obsolete_results"]
fn child_remote_root_forms_and_browse() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let server = MockServer::new();
    let fixture = Fixture::new(&server.url());
    let ui = &fixture.ui;
    fixture.install_library_page(vec![
        remote_root("SFX", "/Volumes/SFX"),
        remote_root("Second", "/Volumes/Second"),
    ]);
    ui.set_ds_syncing(true);
    click_label(ui, "+ Add sound root");
    assert!(
        !ui.get_sm_root_form(),
        "Snapshot reconciliation freezes root changes"
    );
    ui.set_ds_syncing(false);
    ui.set_sm_scan_active(true);
    ui.invoke_sm_root_add_requested();
    assert!(!ui.get_sm_root_form(), "Running scans freeze root changes");
    ui.set_sm_scan_active(false);
    click_label(ui, "+ Add sound root");
    assert!(ui.get_sm_root_form() && !ui.get_sm_root_editing());
    assert!(ui.get_sm_root_name().is_empty() && ui.get_sm_root_path().is_empty());
    assert!(
        fixture
            .controller
            .remote
            .lock()
            .unwrap()
            .manager
            .root_edit_target
            .is_none()
    );
    ui.set_sm_root_name("Draft root".into());
    escape(ui);
    assert!(!ui.get_sm_root_form() && ui.get_library_visible());
    ui.set_sm_root_selected(1);
    ui.invoke_sm_root_edit_requested();
    assert!(ui.get_sm_root_form() && ui.get_sm_root_editing());
    assert_eq!(ui.get_sm_root_name(), "Second");
    assert_eq!(ui.get_sm_root_path(), "/Volumes/Second");
    assert_eq!(
        fixture
            .controller
            .remote
            .lock()
            .unwrap()
            .manager
            .root_edit_target
            .as_deref(),
        Some("Second")
    );
    pump_for(Duration::from_millis(10));
    click_field(ui, "Root name");
    crate::ui_tests::send_keys(ui, &["renamed by accident".into()]);
    assert_eq!(
        ui.get_sm_root_name(),
        "Second",
        "Path edits keep the root name read-only"
    );

    let special = "/Volumes/音效 库?# ";
    ui.set_sm_root_path(special.into());
    ui.invoke_sm_fs_open_requested();
    let old_navigation = server.next();
    assert!(
        old_navigation.raw.starts_with(
            "GET /api/v1/fs?path=%2FVolumes%2F%E9%9F%B3%E6%95%88%20%E5%BA%93%3F%23%20&hidden=0 "
        ),
        "{}",
        old_navigation.raw
    );
    ui.invoke_sm_fs_navigate("/Volumes/Second".into());
    server.next().directory("/Volumes/Second");
    directory_finished(ui);
    assert_eq!(ui.get_sm_fs_path(), "/Volumes/Second");
    assert!(ui.get_sm_fs_can_select());
    assert_eq!(ui.get_sm_fs_rows().row_data(0).unwrap().name, "Foley");
    old_navigation.directory("/Volumes/Stale");
    pump_for(Duration::from_millis(80));
    assert_eq!(ui.get_sm_fs_path(), "/Volumes/Second");
    click_label(ui, "Select folder");
    assert!(!ui.get_sm_fs_visible() && ui.get_sm_root_form());
    assert_eq!(ui.get_sm_root_path(), "/Volumes/Second");

    ui.invoke_sm_fs_open_requested();
    let cancelled = server.next();
    let cancel = ElementHandle::find_by_element_id(ui, "AppWindow::remote-folder-cancel-action")
        .next()
        .unwrap();
    click_element(ui, &cancel);
    assert!(!ui.get_sm_fs_visible() && ui.get_sm_root_form());
    cancelled.directory("/Volumes/Cancelled");
    pump_for(Duration::from_millis(80));
    assert_eq!(ui.get_sm_root_path(), "/Volumes/Second");
    assert_eq!(ui.get_sm_fs_path(), "/Volumes/Second");

    ui.invoke_sm_fs_open_requested();
    let old_page = server.next();
    ui.set_sm_page(PAGE_CONNECTION);
    fixture.controller.show_page(PAGE_CONNECTION);
    assert!(!ui.get_sm_fs_visible() && !ui.get_sm_root_form());
    fixture.install_library_page(vec![remote_root("Second", "/Volumes/Second")]);
    ui.invoke_sm_root_edit_requested();
    ui.invoke_sm_fs_open_requested();
    let current_page = server.next();
    old_page.directory("/Volumes/ObsoletePage");
    pump_for(Duration::from_millis(80));
    assert!(ui.get_sm_fs_busy() && !ui.get_sm_fs_can_select());
    assert_eq!(ui.get_sm_fs_path(), "/Volumes/Second");
    current_page.directory(special);
    directory_finished(ui);
    click_label(ui, "Select folder");
    assert_eq!(
        ui.get_sm_root_path(),
        special,
        "Selected server paths retain trailing spaces"
    );

    ui.invoke_sm_fs_open_requested();
    server.next().directory(special);
    directory_finished(ui);
    escape(ui);
    assert!(!ui.get_sm_fs_visible() && ui.get_sm_root_form() && ui.get_library_visible());
    escape(ui);
    assert!(!ui.get_sm_root_form() && ui.get_library_visible());

    // A structural root refresh must also reject a real directory response that
    // arrives after the old editor was discarded. Retain an owned management
    // request so the refresh cannot replace its generation before completion.
    ui.invoke_sm_root_edit_requested();
    ui.invoke_sm_fs_open_requested();
    let replaced_root = server.next();
    let (_, pending) = fixture.controller.begin_manage_request(ui);
    fixture.controller.refresh_source_manager_roots(ui);
    assert!(!ui.get_sm_root_form() && !ui.get_sm_fs_visible());
    assert!(ui.get_sm_root_path().is_empty() && ui.get_sm_fs_path().is_empty());
    assert_eq!(
        fixture
            .controller
            .remote
            .lock()
            .unwrap()
            .manager
            .manage_pending,
        Some(pending),
        "Root refresh retains the in-flight request owner"
    );
    replaced_root.directory("/Volumes/OldRoot/Foley");
    pump_for(Duration::from_millis(80));
    assert!(ui.get_sm_root_path().is_empty() && ui.get_sm_fs_path().is_empty());
    assert_eq!(ui.get_sm_fs_rows().row_count(), 0);
    fixture
        .controller
        .remote
        .lock()
        .unwrap()
        .manager
        .invalidate_library();
    fixture.controller.publish_manage_busy(ui);
    escape(ui);
    assert!(!ui.get_library_visible());
    assert!(fixture.source_wav.exists() && !fixture.local_database.exists());
    ui.hide().unwrap();
}

#[test]
#[ignore = "isolated GUI helper invoked by root_changes_wait_for_metadata_before_update_and_delete_requests"]
fn child_root_changes_drain_metadata() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let server = MockServer::new();
    let fixture = Fixture::new(&server.url());
    let ui = &fixture.ui;
    let original_wav = std::fs::read(&fixture.source_wav).unwrap();
    *fixture.controller.selected.lock().unwrap() = Some(fixture.file.clone());
    crate::controller::present_file(ui, &fixture.file);
    for (iteration, notes) in [
        "draft before resource path update",
        "draft before root deletion",
    ]
    .into_iter()
    .enumerate()
    {
        ui.set_edit_notes(notes.into());
        ui.set_metadata_dirty(true);
        if iteration == 0 {
            ui.invoke_sm_root_edit_requested();
            ui.set_sm_root_name("tampered display name".into());
            ui.set_sm_root_path("/Volumes/Replacement".into());
            ui.invoke_sm_root_save_requested();
        } else {
            ui.set_sm_root_form(false);
            ui.invoke_sm_root_delete_requested();
        }
        assert!(ui.get_library_busy() && ui.get_sm_manage_busy());
        let metadata = server.next_write(1 + iteration);
        assert!(
            metadata.raw.starts_with(&format!(
                "PATCH /api/v1/audio/{}/metadata ",
                fixture.file.id
            )),
            "Root mutation overtook Metadata: {}",
            metadata.raw
        );
        let body: serde_json::Value =
            serde_json::from_str(metadata.raw.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["notes"], notes);
        assert!(
            server
                .requests
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "Root mutation was sent before the Metadata acknowledgement"
        );
        let mut saved = fixture.file.clone();
        saved.notes = notes.into();
        metadata.reply(
            "200 OK",
            serde_json::json!({
                "file": saved,
                "revision_before": 1 + iteration,
                "revision": 2 + iteration
            }),
        );
        let mutation = server.next_write(2 + iteration);
        let expected = if iteration == 0 { "PATCH" } else { "DELETE" };
        assert!(
            mutation
                .raw
                .starts_with(&format!("{expected} /api/v1/roots/SFX ")),
            "Update/delete must use captured root identity: {}",
            mutation.raw
        );
        assert!(
            mutation
                .raw
                .to_ascii_lowercase()
                .contains(&format!("x-audiolib-library-id: {LIBRARY_ID}"))
        );
        if iteration == 0 {
            let body: serde_json::Value =
                serde_json::from_str(mutation.raw.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["path"], "/Volumes/Replacement");
        }
        // A rejected change avoids a follow-up sync and exercises draft retention.
        mutation.reply(
            "422 Unprocessable Entity",
            serde_json::json!({
                "error":"VALIDATION_ERROR", "message":"fixture target rejected"
            }),
        );
        let weak = ui.as_weak();
        pump_until(move || !weak.upgrade().unwrap().get_library_busy());
        assert!(!ui.get_sm_manage_busy());
        assert!(ui.get_sm_manage_error());
        assert_eq!(
            fixture.provider.get_audio(fixture.file.id).unwrap().notes,
            notes
        );
        assert!(
            fixture
                .controller
                .metadata
                .lock()
                .unwrap()
                .failures
                .is_empty()
        );
    }
    assert_eq!(std::fs::read(&fixture.source_wav).unwrap(), original_wav);
    assert!(!fixture.local_database.exists());
    ui.hide().unwrap();
}

#[test]
fn published_remote_places_cannot_be_selected_as_a_folder() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    publish_directory(&ui, RemoteDirectory::default(), false);
    assert!(!ui.get_sm_fs_can_select());
    assert!(ui.get_sm_fs_path().is_empty());
    publish_directory(
        &ui,
        RemoteDirectory {
            path: Some("/Volumes/音效 库 ".into()),
            parent: Some("/Volumes".into()),
            truncated: true,
            ..Default::default()
        },
        false,
    );
    assert!(ui.get_sm_fs_can_select());
    assert_eq!(ui.get_sm_fs_path(), "/Volumes/音效 库 ");
    assert_eq!(ui.get_sm_fs_parent(), "/Volumes");
    assert!(ui.get_sm_fs_error().contains("truncated"));
    assert_eq!(
        ui.get_sm_fs_breadcrumbs().row_data(2).unwrap().path,
        "/Volumes/音效 库 "
    );
}
