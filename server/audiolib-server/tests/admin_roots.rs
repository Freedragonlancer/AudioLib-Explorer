// server/audiolib-server/tests/admin_roots.rs
//! Admin web API for library roots (doc/api/README.md §2.3.1): server disk browser
//! `GET /admin/api/fs`, `POST /admin/api/roots`, `DELETE /admin/api/roots/{name}` and the
//! admin scan wrappers, against temporary libraries and roots of tiny generated WAVs.
//! Nothing here touches anything outside the temporary directories; every WAV must still
//! be on disk after a root was removed.
mod support;

use audiolib_core::{DataProvider, MetadataPatch};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use audiolib_server::{
    AppState, BearerToken, ConnectionInfo, ErrorBody, LibraryResponse, Listener, RootResponse,
    ScanResponse, Security, ServerConfig, Service, router,
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, SemaphorePermit};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef0123456789abcdef";
// The production filesystem budget is process-wide, including across AppStates.
// Give these independent endpoint fixtures exclusive access so parallel test runners
// do not turn ordinary local reads into unrelated BUSY responses. The separate
// admin unit test exercises the actual four-slot saturation policy.
static ROOT_ENDPOINT_FIXTURES: Semaphore = Semaphore::const_new(1);
/// A tiny silent 16-bit mono WAV whose length depends on `samples`.
fn wav(path: &Path, samples: usize) {
    let data = vec![0_u8; samples * 2];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn real_path(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap();
    let text = canonical.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => rest.into(),
        _ => canonical,
    }
}

/// `base` joined with the `/`-separated `relative` path component by component, so the
/// result uses the platform separator (on Windows `join("a/b")` would keep the `/`, and
/// such a path never equals the `\\`-separated paths the scanner stores in the library).
fn under(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

struct Fixture {
    _filesystem_budget: SemaphorePermit<'static>,
    _dir: tempfile::TempDir,
    base: PathBuf,
    sounds: PathBuf,
    db: PathBuf,
    config: ServerConfig,
    state: AppState,
}

const FILES: [&str; 4] = [
    "Doors/door_1.wav",
    "Doors/Inner/knock.wav",
    "wind.wav",
    "雨声.wav",
];

/// A library with the configured root `SFX` (`FILES`, scanned), auth mode external.
async fn fixture() -> Fixture {
    let filesystem_budget = ROOT_ENDPOINT_FIXTURES.acquire().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = real_path(dir.path());
    let sounds = base.join("sounds");
    for file in FILES {
        wav(&under(&sounds, file), 4);
    }
    let db = dir.path().join("library").join("library.db");
    let writer = Arc::new(SqliteProvider::open(&db).unwrap());
    ScanService::new(writer)
        .scan(
            &sounds,
            &ScanOptions {
                incremental: false,
                ..Default::default()
            },
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let config = ServerConfig::from_toml_str(
        &format!(
            "bind = \"127.0.0.1:0\"\nlibrary_db = 'library/library.db'\nsnapshot_dir = 'snapshots'\n\
             [[roots]]\nname = \"SFX\"\nserver_path = '{}'\n",
            sounds.display()
        ),
        dir.path(),
    )
    .unwrap();
    let state = AppState::open(&config, BearerToken::new(TOKEN).unwrap()).unwrap();
    Fixture {
        _filesystem_budget: filesystem_budget,
        _dir: dir,
        base,
        sounds,
        db,
        config,
        state,
    }
}

impl Fixture {
    /// A directory of tiny WAVs next to the configured root (outside it).
    fn extra_root(&self, name: &str, files: &[&str]) -> PathBuf {
        let root = self.base.join(name);
        for file in files {
            wav(&under(&root, file), 4);
        }
        root
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        audiolib_db::register_library_functions(&conn).unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }
}

const LAN: &str = "10.0.0.20:50000";

fn peer(address: &str) -> ConnectionInfo {
    ConnectionInfo {
        peer: address.parse().unwrap(),
        listener: Listener::Http,
    }
}

async fn request(
    service: Service,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    connection: Option<ConnectionInfo>,
    body: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if path.starts_with("/api/v1/") {
        request = support::modern(request);
    }
    if let Ok(state) = service.app_state() {
        request = request.header(
            "x-audiolib-library-id",
            state.revision().unwrap().library_id,
        );
    }
    if body.is_some()
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        request = request.header("host", "localhost");
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    let connection = connection.unwrap_or(ConnectionInfo {
        peer: "127.0.0.1:1".parse().unwrap(),
        listener: Listener::Http,
    });
    request.extensions_mut().insert(connection);
    router(service).oneshot(request).await.unwrap()
}

/// An admin request as the admin page sends it from the LAN.
async fn admin(state: &AppState, method: Method, path: &str, body: Option<&str>) -> Response {
    request(
        Service::from(state.clone()),
        method,
        path,
        &[
            ("x-audiolib-admin", "1"),
            ("host", "192.0.2.10:8790"),
            ("content-type", "application/json"),
        ],
        Some(peer(LAN)),
        body,
    )
    .await
}

async fn api(state: &AppState, path: &str) -> Response {
    request(
        Service::from(state.clone()),
        Method::GET,
        &format!("/api/v1{path}"),
        &[],
        Some(peer(LAN)),
        None,
    )
    .await
}

async fn bytes(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

async fn ok<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let status = response.status();
    let body = bytes(response).await;
    assert!(
        status.is_success(),
        "{status}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

async fn error(response: Response, status: StatusCode, code: &str) -> ErrorBody {
    let actual = response.status();
    let body = bytes(response).await;
    let body: ErrorBody = serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (actual, body.error_code.as_str()),
        (status, code),
        "{}",
        body.message
    );
    body
}

fn quoted(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).unwrap()
}

async fn add(state: &AppState, name: &str, path: &Path) -> Response {
    admin(
        state,
        Method::POST,
        "/admin/api/roots",
        Some(&format!(r#"{{"name":"{name}","path":{}}}"#, quoted(path))),
    )
    .await
}

async fn scan_root(state: &AppState, name: &str) -> ScanResponse {
    let started: ScanResponse = ok(admin(
        state,
        Method::POST,
        "/admin/api/scans",
        Some(&format!(r#"{{"root":"{name}","mode":"full"}}"#)),
    )
    .await)
    .await;
    finish_scan(state, started.progress.session_id).await
}

async fn finish_scan(state: &AppState, session_id: i64) -> ScanResponse {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let scan: ScanResponse = ok(admin(
            state,
            Method::GET,
            &format!("/admin/api/scans/{session_id}"),
            None,
        )
        .await)
        .await;
        if !scan.active {
            return scan;
        }
        assert!(Instant::now() < deadline, "scan did not finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn revision(state: &AppState) -> u64 {
    ok::<LibraryResponse>(api(state, "/library").await)
        .await
        .revision
}

#[tokio::test]
async fn browsing_lists_sorted_sub_directories_with_counts_and_clear_errors() {
    let f = fixture().await;
    let dir = f.base.join("browse");
    for name in ["beta", "Alpha", "gamma", ".hidden", "雨 rain"] {
        fs::create_dir_all(dir.join(name)).unwrap();
    }
    wav(&dir.join("a.wav"), 4);
    wav(&dir.join("B.WAV"), 4);
    fs::write(dir.join("notes.txt"), "x").unwrap();
    // A link to a directory is listed (flagged), a link to a file is not.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("beta"), dir.join("link-dir")).unwrap();
        std::os::unix::fs::symlink(dir.join("notes.txt"), dir.join("link-file")).unwrap();
    }
    let get = |path: String| {
        let state = f.state.clone();
        async move {
            admin(
                &state,
                Method::GET,
                &format!("/admin/api/fs?path={}", percent(&path)),
                None,
            )
            .await
        }
    };
    let listing: serde_json::Value = ok(get(dir.display().to_string()).await).await;
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    #[cfg(unix)]
    assert_eq!(names, ["Alpha", "beta", "gamma", "link-dir", "雨 rain"]);
    #[cfg(not(unix))]
    assert_eq!(names, ["Alpha", "beta", "gamma", "雨 rain"]);
    assert_eq!(listing["wav_files"], 2);
    assert_eq!(listing["truncated"], false);
    assert_eq!(listing["path"], dir.display().to_string());
    assert_eq!(
        listing["parent"],
        dir.parent().unwrap().display().to_string()
    );
    #[cfg(unix)]
    {
        let link = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "link-dir")
            .unwrap();
        assert_eq!(link["is_symlink"], true);
        assert_eq!(listing["other_files"], 1);
    }
    // Hidden directories only on request.
    let with_hidden: serde_json::Value = ok(admin(
        &f.state,
        Method::GET,
        &format!(
            "/admin/api/fs?hidden=1&path={}",
            percent(&dir.display().to_string())
        ),
        None,
    )
    .await)
    .await;
    assert!(
        with_hidden["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == ".hidden")
    );

    // `..` segments are resolved; a symlinked path shows the canonical directory.
    let dotted = dir.join("beta").join("..");
    let resolved: serde_json::Value = ok(get(dotted.display().to_string()).await).await;
    assert_eq!(resolved["path"], dir.display().to_string());

    // Errors: relative (400), missing (404), a file (400), no permission (403, unix).
    for bad in ["relative/dir", "", ".."] {
        error(get(bad.into()).await, StatusCode::BAD_REQUEST, "VALIDATION").await;
    }
    let missing = dir.join("does-not-exist");
    error(
        get(missing.display().to_string()).await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    )
    .await;
    let file = error(
        get(dir.join("notes.txt").display().to_string()).await,
        StatusCode::BAD_REQUEST,
        "VALIDATION",
    )
    .await;
    assert!(file.message.contains("不是目录"), "{}", file.message);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root (CI containers) can read everything: only assert when access is denied.
        if fs::read_dir(&locked).is_err() {
            let denied = error(
                get(locked.display().to_string()).await,
                StatusCode::FORBIDDEN,
                "FS_FORBIDDEN",
            )
            .await;
            assert!(denied.message.contains("没有权限"), "{}", denied.message);
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Starting points: the file system root, the home directory and the library roots.
    let places: serde_json::Value =
        ok(admin(&f.state, Method::GET, "/admin/api/fs", None).await).await;
    let places = places["places"].as_array().unwrap();
    assert!(places.iter().any(|place| place["kind"] == "root"));
    assert!(places.iter().any(|place| place["kind"] == "library_root"
        && place["name"] == "SFX"
        && place["path"] == f.sounds.display().to_string()));
}

/// Percent-encode a path for a query string.
fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn client_root_request(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<&str>,
) -> Response {
    request(
        Service::from(state.clone()),
        method,
        &format!("/api/v1{path}"),
        &[],
        Some(peer(LAN)),
        body,
    )
    .await
}

#[tokio::test]
async fn stable_client_browser_and_root_operations_are_manage_gated_and_keep_admin_policy() {
    let f = fixture().await;
    let capabilities: serde_json::Value = ok(api(&f.state, "/capabilities").await).await;
    assert_eq!(capabilities["root_management"], true);
    let places: serde_json::Value = ok(api(&f.state, "/fs").await).await;
    assert!(
        places["places"]
            .as_array()
            .unwrap()
            .iter()
            .any(|place| place["name"] == "SFX")
    );
    let listing: serde_json::Value = ok(api(
        &f.state,
        &format!("/fs?path={}", percent(&f.sounds.display().to_string())),
    )
    .await)
    .await;
    assert_eq!(listing["wav_files"], 2);
    assert!(
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "Doors")
    );
    error(
        request(
            Service::from(f.state.clone()),
            Method::GET,
            "/api/v1/fs",
            &[],
            Some(peer("203.0.113.1:50000")),
            None,
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
    )
    .await;
    f.state
        .security()
        .update_settings(|settings| settings.manage_mode = Some(audiolib_server::ManageMode::Off))
        .unwrap();
    let extra = f.extra_root("extra", &["a.wav"]);
    for (method, path, body) in [
        (Method::GET, "/fs", None),
        (
            Method::POST,
            "/roots",
            Some(format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra))),
        ),
        (
            Method::PATCH,
            "/roots/SFX",
            Some(format!(r#"{{"path":{}}}"#, quoted(&extra))),
        ),
        (Method::DELETE, "/roots/SFX", None),
    ] {
        error(
            client_root_request(&f.state, method, path, body.as_deref()).await,
            StatusCode::FORBIDDEN,
            "MANAGE_FORBIDDEN",
        )
        .await;
    }
    let root: RootResponse = ok(add(&f.state, "AdminOnly", &extra).await).await;
    assert_eq!(root.name, "AdminOnly");
    assert!(under(&extra, "a.wav").is_file());
}

#[tokio::test]
async fn client_root_mutations_require_the_opened_library_identity() {
    let f = fixture().await;
    let extra = f.extra_root("extra", &["a.wav"]);
    for (method, path, body) in [
        (
            Method::POST,
            "/roots",
            Some(format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra))),
        ),
        (
            Method::PATCH,
            "/roots/SFX",
            Some(format!(r#"{{"path":{}}}"#, quoted(&extra))),
        ),
        (Method::DELETE, "/roots/SFX", None),
    ] {
        for (identity, code) in [
            (None, "LIBRARY_ID_REQUIRED"),
            (Some("another-library"), "LIBRARY_CHANGED"),
        ] {
            let mut builder = Request::builder()
                .method(method.clone())
                .uri(format!("/api/v1{path}"))
                .header("host", "localhost")
                .header("content-type", "application/json");
            builder = support::modern(builder);
            if let Some(identity) = identity {
                builder = builder.header("x-audiolib-library-id", identity);
            }
            let mut request = builder
                .body(
                    body.as_ref()
                        .map_or_else(Body::empty, |value| Body::from(value.clone())),
                )
                .unwrap();
            request.extensions_mut().insert(peer(LAN));
            error(
                router(f.state.clone()).oneshot(request).await.unwrap(),
                StatusCode::CONFLICT,
                code,
            )
            .await;
        }
    }
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 4);
    assert!(!f.config.admin_dir().join("admin-settings.json").exists());
}

#[tokio::test]
async fn root_path_update_preserves_ids_annotations_extension_metadata_and_tree_after_move() {
    let f = fixture().await;
    let first = f
        .state
        .provider()
        .find_audio_by_path(&under(&f.sounds, "Doors/door_1.wav"))
        .unwrap()
        .unwrap();
    f.state
        .provider()
        .update_metadata(
            first.id,
            &MetadataPatch {
                tags: Some("custom tag".into()),
                notes: Some("Keep me".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("UPDATE audio_files SET metadata_json=json_set(metadata_json,'$.unknown_extension','keep') WHERE id=?1", [first.id]).unwrap();
    let folder_ids: Vec<i64> = conn
        .prepare("SELECT id FROM folders ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(conn);
    let before = revision(&f.state).await;
    let moved = f.base.join("moved sounds");
    fs::rename(&f.sounds, &moved).unwrap();
    let root: RootResponse = ok(client_root_request(
        &f.state,
        Method::PATCH,
        "/roots/SFX",
        Some(&format!(r#"{{"path":{}}}"#, quoted(&moved))),
    )
    .await)
    .await;
    assert_eq!(root.server_path, moved.display().to_string());
    assert_eq!(root.file_count, Some(4));
    let changed = f.state.provider().get_audio(first.id).unwrap();
    assert_eq!(
        changed.file_path,
        under(&moved, "Doors/door_1.wav").display().to_string()
    );
    assert_eq!(
        (
            changed.tags.as_str(),
            changed.notes.as_str(),
            changed.favorite
        ),
        ("custom tag", "Keep me", true)
    );
    assert_eq!(changed.attributes["unknown_extension"], "keep");
    assert_eq!(changed.folder_id, first.folder_id);
    assert_eq!(revision(&f.state).await, before + 1);
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let after_ids: Vec<i64> = conn
        .prepare("SELECT id FROM folders ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(folder_ids, after_ids);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM sqlite_schema WHERE name='play_history'"),
        0
    );
    assert!(
        fs::read_dir(f.db.parent().unwrap().join("backups"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("before-root-relink-"))
    );
    let restarted = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(
        restarted.roots_response().unwrap()[0].server_path,
        moved.display().to_string()
    );
    assert_eq!(scan_root(&restarted, "SFX").await.progress.added, 0);
    let rescanned = restarted.provider().get_audio(first.id).unwrap();
    assert_eq!(rescanned.tags, "custom tag");
    assert_eq!(rescanned.attributes["unknown_extension"], "keep");
}

#[tokio::test]
async fn root_path_update_rejects_incomplete_and_mismatched_targets_atomically() {
    let f = fixture().await;
    let target = f.extra_root("incomplete", &["Doors/door_1.wav", "wind.wav", "雨声.wav"]);
    let before = revision(&f.state).await;
    let request_body = format!(r#"{{"path":{}}}"#, quoted(&target));
    error(
        client_root_request(&f.state, Method::PATCH, "/roots/SFX", Some(&request_body)).await,
        StatusCode::BAD_REQUEST,
        "VALIDATION",
    )
    .await;
    wav(&under(&target, "Doors/Inner/knock.wav"), 8);
    error(
        client_root_request(&f.state, Method::PATCH, "/roots/SFX", Some(&request_body)).await,
        StatusCode::BAD_REQUEST,
        "VALIDATION",
    )
    .await;
    assert_eq!(
        f.state.roots_response().unwrap()[0].server_path,
        f.sounds.display().to_string()
    );
    assert_eq!(revision(&f.state).await, before);
    for file in FILES {
        assert!(
            f.state
                .provider()
                .find_audio_by_path(&under(&f.sounds, file))
                .unwrap()
                .is_some()
        );
    }
    assert!(f.state.security().roots_override().is_some());
}

#[tokio::test]
async fn case_only_root_rename_preserves_records_and_recovers_the_exact_committed_path() {
    use audiolib_server::manage::RootRelinkJournal;
    let f = fixture().await;
    let moved = f.base.join("Sounds");
    if moved.exists() {
        // Run the actual rename only on a case-sensitive filesystem.
        return;
    }
    let original_path = under(&f.sounds, "Doors/door_1.wav");
    let original_bytes = fs::read(&original_path).unwrap();
    let first = f
        .state
        .provider()
        .find_audio_by_path(&original_path)
        .unwrap()
        .unwrap();
    f.state
        .provider()
        .update_metadata(
            first.id,
            &MetadataPatch {
                tags: Some("case rename tag".into()),
                notes: Some("case rename note".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute(
        "UPDATE audio_files SET metadata_json=json_set(metadata_json,'$.unknown_extension','keep') WHERE id=?1",
        [first.id],
    ).unwrap();
    drop(conn);
    let id = f.state.provider().folders(None).unwrap()[0].id;
    fs::rename(&f.sounds, &moved).unwrap();
    assert!(!f.sounds.exists());
    let mut after = f.config.roots.clone();
    after[0].server_path = moved.clone();
    let journal = RootRelinkJournal {
        root_id: id,
        source: audiolib_db::folder_path_text(&f.sounds),
        target: audiolib_db::folder_path_text(&moved),
        before: f.config.roots.clone(),
        after,
    };
    f.state
        .security()
        .update_settings(|settings| settings.root_relink = Some(journal.clone()))
        .unwrap();
    let uncommitted = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(
        uncommitted.roots_response().unwrap()[0].server_path,
        f.sounds.display().to_string()
    );
    let before = revision(&f.state).await;
    let root: RootResponse = ok(client_root_request(
        &f.state,
        Method::PATCH,
        "/roots/SFX",
        Some(&format!(r#"{{"path":{}}}"#, quoted(&moved))),
    )
    .await)
    .await;
    assert_eq!(root.server_path, moved.display().to_string());
    assert_eq!(revision(&f.state).await, before + 1);
    let changed = f.state.provider().get_audio(first.id).unwrap();
    assert_eq!(
        changed.file_path,
        under(&moved, "Doors/door_1.wav").display().to_string()
    );
    assert_eq!(changed.folder_id, first.folder_id);
    assert_eq!(
        (
            changed.tags.as_str(),
            changed.notes.as_str(),
            changed.favorite
        ),
        ("case rename tag", "case rename note", true)
    );
    assert_eq!(changed.attributes["unknown_extension"], "keep");
    assert_eq!(
        fs::read(under(&moved, "Doors/door_1.wav")).unwrap(),
        original_bytes
    );
    f.state
        .security()
        .update_settings(|settings| settings.root_relink = Some(journal))
        .unwrap();
    let committed = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(
        committed.roots_response().unwrap()[0].server_path,
        moved.display().to_string()
    );
    assert_eq!(
        committed.provider().get_folder(id).unwrap().path,
        audiolib_db::folder_path_text(&moved)
    );
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(f.config.admin_dir().join("admin-settings.json")).unwrap(),
    )
    .unwrap();
    assert!(settings.get("root_relink").is_none());
    assert_eq!(scan_root(&committed, "SFX").await.progress.added, 0);
    assert_eq!(
        committed.provider().get_audio(first.id).unwrap().notes,
        "case rename note"
    );
    for file in FILES {
        assert!(under(&moved, file).is_file());
    }
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 4);
}

#[tokio::test]
async fn another_live_server_reloads_a_relocated_root_before_scan() {
    let f = fixture().await;
    let second = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let originals: Vec<_> = FILES
        .iter()
        .map(|file| {
            f.state
                .provider()
                .find_audio_by_path(&under(&f.sounds, file))
                .unwrap()
                .unwrap()
        })
        .collect();
    let first = &originals[0];
    f.state
        .provider()
        .update_metadata(
            first.id,
            &MetadataPatch {
                tags: Some("shared root tag".into()),
                notes: Some("shared root note".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute(
        "UPDATE audio_files SET metadata_json=json_set(metadata_json,'$.unknown_extension','keep') WHERE id=?1",
        [first.id],
    ).unwrap();
    drop(conn);
    let moved = f.base.join("relocated");
    for file in FILES {
        let target = under(&moved, file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(under(&f.sounds, file), target).unwrap();
    }
    let changed: RootResponse = ok(client_root_request(
        &f.state,
        Method::PATCH,
        "/roots/SFX",
        Some(&format!(r#"{{"path":{}}}"#, quoted(&moved))),
    )
    .await)
    .await;
    assert_eq!(changed.server_path, moved.display().to_string());
    // Both servers were already running. The old WAVs remain accessible, so scanning
    // second's original in-memory root would resurrect four duplicate records.
    assert_eq!(
        second.roots_response().unwrap()[0].server_path,
        f.sounds.display().to_string()
    );
    let completed = scan_root(&second, "SFX").await;
    assert_eq!(
        completed.progress.state,
        audiolib_core::ScanState::Completed
    );
    assert_eq!(completed.progress.added, 0);
    assert_eq!(completed.progress.errors, 0);
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 4);
    assert_eq!(
        second.roots_response().unwrap()[0].server_path,
        moved.display().to_string()
    );
    for (file, original) in FILES.iter().zip(&originals) {
        let target = under(&moved, file);
        let record = second
            .provider()
            .find_audio_by_path(&target)
            .unwrap()
            .unwrap();
        assert_eq!(record.id, original.id);
        assert_eq!(record.folder_id, original.folder_id);
        assert_eq!(record.file_path, target.display().to_string());
        let source = under(&f.sounds, file);
        assert!(
            second
                .provider()
                .find_audio_by_path(&source)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(source).unwrap(), fs::read(target).unwrap());
    }
    let retained = second.provider().get_audio(first.id).unwrap();
    assert_eq!(
        (
            retained.tags.as_str(),
            retained.notes.as_str(),
            retained.favorite
        ),
        ("shared root tag", "shared root note", true)
    );
    assert_eq!(retained.attributes["unknown_extension"], "keep");
}

#[tokio::test]
async fn shared_management_policy_denies_the_first_scan_and_root_mutations() {
    let f = fixture().await;
    let extra = f.extra_root("permission-extra", &["extra.wav"]);
    let replacement = f.extra_root("permission-relocated", &FILES);
    let requests = [
        (
            Method::POST,
            "/scans",
            r#"{"root":"SFX","mode":"full"}"#.into(),
        ),
        (
            Method::POST,
            "/roots",
            format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra)),
        ),
        (
            Method::PATCH,
            "/roots/SFX",
            format!(r#"{{"path":{}}}"#, quoted(&replacement)),
        ),
        (Method::DELETE, "/roots/SFX", String::new()),
    ];
    // Every request uses an independently running server whose first API admission
    // still has the old LAN policy. One denied request must not mask the others.
    let second_servers: Vec<_> = requests
        .iter()
        .map(|_| AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap())
        .collect();
    let sessions_before = f.count("SELECT COUNT(*) FROM scan_sessions");
    let revision_before = revision(&f.state).await;
    let _: serde_json::Value = ok(admin(
        &f.state,
        Method::PUT,
        "/admin/api/settings",
        Some(r#"{"manage_mode":"off"}"#),
    )
    .await)
    .await;
    for (second, (method, path, body)) in second_servers.iter().zip(requests) {
        assert_eq!(
            second.security().manage_mode(),
            audiolib_server::ManageMode::Lan
        );
        error(
            client_root_request(
                second,
                method,
                path,
                (!body.is_empty()).then_some(body.as_str()),
            )
            .await,
            StatusCode::FORBIDDEN,
            "MANAGE_FORBIDDEN",
        )
        .await;
        assert_eq!(
            second.security().manage_mode(),
            audiolib_server::ManageMode::Off
        );
        assert!(second.scans().current().is_none());
        let roots = second.roots_response().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].server_path, f.sounds.display().to_string());
    }
    assert_eq!(
        f.count("SELECT COUNT(*) FROM scan_sessions"),
        sessions_before
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM audio_files"),
        FILES.len() as i64
    );
    assert_eq!(revision(&f.state).await, revision_before);
    for file in FILES {
        assert!(under(&f.sounds, file).is_file());
    }
    // The LAN admin authority remains independent even after its own policy is Off.
    let root: RootResponse = ok(add(&f.state, "AdminAllowed", &extra).await).await;
    assert_eq!(root.name, "AdminAllowed");
}

#[tokio::test]
async fn shared_authentication_policy_checks_the_first_managed_request_before_identity() {
    let f = fixture().await;
    let second_scan = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let second_root = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let extra = f.extra_root("auth-extra", &["extra.wav"]);
    let body = format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra));
    let sessions_before = f.count("SELECT COUNT(*) FROM scan_sessions");
    let _: serde_json::Value = ok(admin(
        &f.state,
        Method::PUT,
        "/admin/api/settings",
        Some(r#"{"auth_mode":"all"}"#),
    )
    .await)
    .await;
    for (second, path, body) in [
        (&second_scan, "/scans", r#"{"root":"SFX","mode":"full"}"#),
        (&second_root, "/roots", body.as_str()),
    ] {
        assert_eq!(
            second.security().auth_mode(),
            audiolib_server::AuthMode::External
        );
        error(
            request(
                Service::from(second.clone()),
                Method::POST,
                &format!("/api/v1{path}"),
                // The extra identity header makes identity invalid as well. Fresh
                // bearer admission must still answer 401 before the identity's 409.
                &[("x-audiolib-library-id", "unrelated-library")],
                Some(peer(LAN)),
                Some(body),
            )
            .await,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        )
        .await;
        assert_eq!(
            second.security().auth_mode(),
            audiolib_server::AuthMode::All
        );
        assert!(second.scans().current().is_none());
        assert_eq!(second.roots_response().unwrap().len(), 1);
    }
    assert_eq!(
        f.count("SELECT COUNT(*) FROM scan_sessions"),
        sessions_before
    );
    let token = format!("Bearer {TOKEN}");
    let added: RootResponse = ok(request(
        Service::from(second_root.clone()),
        Method::POST,
        "/api/v1/roots",
        &[("authorization", &token)],
        Some(peer(LAN)),
        Some(&body),
    )
    .await)
    .await;
    assert_eq!(added.name, "Extra");
    let started: ScanResponse = ok(request(
        Service::from(second_scan.clone()),
        Method::POST,
        "/api/v1/scans",
        &[("authorization", &token)],
        Some(peer(LAN)),
        Some(r#"{"root":"SFX","mode":"full"}"#),
    )
    .await)
    .await;
    assert_eq!(
        finish_scan(&second_scan, started.progress.session_id)
            .await
            .progress
            .state,
        audiolib_core::ScanState::Completed
    );
    let _: serde_json::Value =
        ok(admin(&second_scan, Method::GET, "/admin/api/status", None).await).await;
}

#[tokio::test]
async fn shared_relaxed_policies_allow_the_first_request_despite_old_stricter_memory() {
    let f = fixture().await;
    let _: serde_json::Value = ok(admin(
        &f.state,
        Method::PUT,
        "/admin/api/settings",
        Some(r#"{"auth_mode":"all","manage_mode":"off"}"#),
    )
    .await)
    .await;
    let second_scan = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let second_root = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let _: serde_json::Value = ok(admin(
        &f.state,
        Method::PUT,
        "/admin/api/settings",
        Some(r#"{"auth_mode":"off","manage_mode":"all"}"#),
    )
    .await)
    .await;
    let extra = f.extra_root("relaxed-extra", &["extra.wav"]);
    assert_eq!(
        second_root.security().auth_mode(),
        audiolib_server::AuthMode::All
    );
    assert_eq!(
        second_root.security().manage_mode(),
        audiolib_server::ManageMode::Off
    );
    let added: RootResponse = ok(client_root_request(
        &second_root,
        Method::POST,
        "/roots",
        Some(&format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra))),
    )
    .await)
    .await;
    assert_eq!(added.name, "Extra");
    assert_eq!(
        second_scan.security().auth_mode(),
        audiolib_server::AuthMode::All
    );
    assert_eq!(
        second_scan.security().manage_mode(),
        audiolib_server::ManageMode::Off
    );
    let started: ScanResponse = ok(client_root_request(
        &second_scan,
        Method::POST,
        "/scans",
        Some(r#"{"root":"SFX","mode":"full"}"#),
    )
    .await)
    .await;
    let completed = finish_scan(&second_scan, started.progress.session_id).await;
    assert_eq!(
        completed.progress.state,
        audiolib_core::ScanState::Completed
    );
    assert_eq!(completed.progress.added, 0);
    assert_eq!(
        second_scan.security().auth_mode(),
        audiolib_server::AuthMode::Off
    );
    assert_eq!(
        second_scan.security().manage_mode(),
        audiolib_server::ManageMode::All
    );
}

#[tokio::test]
async fn ordinary_reads_and_streams_do_not_wait_for_a_root_settings_lease() {
    let f = fixture().await;
    let second = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let id = f
        .state
        .provider()
        .find_audio_by_path(&under(&f.sounds, FILES[0]))
        .unwrap()
        .unwrap()
        .id;
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.config.admin_dir().join(".admin-settings.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    for path in [
        "/library".into(),
        "/roots".into(),
        "/capabilities".into(),
        format!("/audio/{id}/stream"),
    ] {
        let response = api(&second, &path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(!bytes(response).await.is_empty(), "{path}");
    }
    error(
        client_root_request(&second, Method::POST, "/scans", Some(r#"{"root":"SFX"}"#)).await,
        StatusCode::CONFLICT,
        "BUSY",
    )
    .await;
    assert!(second.scans().current().is_none());
    drop(lease);
}

#[tokio::test]
async fn stable_client_add_update_remove_empty_roots_persists_and_never_deletes_audio() {
    let f = fixture().await;
    let extra = f.extra_root("extra", &["a.wav"]);
    let replacement = f.extra_root("replacement", &["b.wav"]);
    let created: RootResponse = ok(client_root_request(
        &f.state,
        Method::POST,
        "/roots",
        Some(&format!(r#"{{"name":"Extra","path":{}}}"#, quoted(&extra))),
    )
    .await)
    .await;
    assert_eq!(created.name, "Extra");
    let moved: RootResponse = ok(client_root_request(
        &f.state,
        Method::PATCH,
        "/roots/Extra",
        Some(&format!(r#"{{"path":{}}}"#, quoted(&replacement))),
    )
    .await)
    .await;
    assert_eq!(moved.server_path, replacement.display().to_string());
    let removed: serde_json::Value =
        ok(client_root_request(&f.state, Method::DELETE, "/roots/Extra", None).await).await;
    assert_eq!(removed["removed"], 0);
    assert!(extra.join("a.wav").is_file());
    assert!(replacement.join("b.wav").is_file());
    let restarted = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(restarted.roots_response().unwrap().len(), 1);
}

#[tokio::test]
async fn interrupted_root_relocation_recovers_before_and_after_the_sqlite_commit() {
    use audiolib_server::manage::RootRelinkJournal;
    let f = fixture().await;
    let id = f.state.provider().folders(None).unwrap()[0].id;
    let moved = f.base.join("relocated");
    fs::rename(&f.sounds, &moved).unwrap();
    let before = f.config.roots.clone();
    let mut after = before.clone();
    after[0].server_path = moved.clone();
    let journal = RootRelinkJournal {
        root_id: id,
        source: audiolib_db::folder_path_text(&f.sounds),
        target: audiolib_db::folder_path_text(&moved),
        before,
        after,
    };
    f.state
        .security()
        .update_settings(|settings| settings.root_relink = Some(journal.clone()))
        .unwrap();
    let uncommitted = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(
        uncommitted.roots_response().unwrap()[0].server_path,
        f.sounds.display().to_string()
    );
    assert_eq!(uncommitted.provider().stats().unwrap().audio_files, 4);
    uncommitted
        .security()
        .update_settings(|settings| settings.root_relink = Some(journal))
        .unwrap();
    assert_eq!(
        uncommitted
            .provider()
            .relink_folder(&f.sounds, &moved)
            .unwrap(),
        4
    );
    let committed = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(
        committed.roots_response().unwrap()[0].server_path,
        moved.display().to_string()
    );
    assert_eq!(
        committed.provider().get_folder(id).unwrap().path,
        audiolib_db::folder_path_text(&moved)
    );
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(f.config.admin_dir().join("admin-settings.json")).unwrap(),
    )
    .unwrap();
    assert!(settings.get("root_relink").is_none());
}

#[tokio::test]
async fn interrupted_root_removal_recovers_before_and_after_the_sqlite_commit() {
    use audiolib_server::manage::RootRemovalJournal;
    let f = fixture().await;
    let id = f.state.provider().folders(None).unwrap()[0].id;
    let journal = RootRemovalJournal {
        root_id: id,
        source: audiolib_db::folder_path_text(&f.sounds),
        before: f.config.roots.clone(),
        after: Vec::new(),
    };
    f.state
        .security()
        .update_settings(|settings| settings.root_removal = Some(journal.clone()))
        .unwrap();
    let uncommitted = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert_eq!(uncommitted.roots_response().unwrap().len(), 1);
    assert_eq!(uncommitted.provider().stats().unwrap().audio_files, 4);
    uncommitted
        .security()
        .update_settings(|settings| settings.root_removal = Some(journal))
        .unwrap();
    assert_eq!(uncommitted.provider().remove_folder(id).unwrap(), 4);
    let committed = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert!(committed.roots_response().unwrap().is_empty());
    assert_eq!(committed.provider().stats().unwrap().audio_files, 0);
    for file in FILES {
        assert!(under(&f.sounds, file).is_file());
    }
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(f.config.admin_dir().join("admin-settings.json")).unwrap(),
    )
    .unwrap();
    assert!(settings.get("root_removal").is_none());
}

#[tokio::test]
async fn adding_a_root_validates_applies_at_once_and_survives_a_restart() {
    let f = fixture().await;
    let state = &f.state;
    let new_root = f.extra_root("music", &["Loops/a.wav", "Loops/b.wav", "c.wav"]);
    let file = new_root.join("c.wav");
    let before = revision(state).await;

    // Validation: names, paths, overlaps.
    for (name, path, why) in [
        ("", new_root.clone(), "empty name"),
        ("a/b", new_root.clone(), "separator"),
        (" Music", new_root.clone(), "padding"),
        ("Music", PathBuf::from("relative/music"), "relative"),
        ("Music", f.base.join("missing"), "missing"),
        ("Music", file, "a file"),
        ("sfx", new_root.clone(), "duplicate name (case-insensitive)"),
        ("Same", f.sounds.clone(), "same directory as a root"),
        ("Inside", f.sounds.join("Doors"), "inside a root"),
        ("Above", f.base.clone(), "containing a root"),
    ] {
        let response = add(state, name, &path).await;
        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::NOT_FOUND,
            "{why}: {}",
            response.status()
        );
    }
    let overlap = add(state, "Inside", &f.sounds.join("Doors")).await;
    let overlap = error(overlap, StatusCode::BAD_REQUEST, "VALIDATION").await;
    assert!(overlap.message.contains("\"SFX\""), "{}", overlap.message);
    let names: Vec<RootResponse> = ok(api(state, "/roots").await).await;
    assert_eq!(names.len(), 1, "nothing was added");
    assert!(!f.config.admin_dir().join("admin-settings.json").exists());

    // Success: 201, visible in the client API at once, `webdav_url` null.
    let added: RootResponse = {
        let response = add(state, "Music", &new_root).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        ok(response).await
    };
    assert_eq!(added.name, "Music");
    assert_eq!(added.webdav_url, None);
    let roots: Vec<RootResponse> = ok(api(state, "/roots").await).await;
    assert_eq!(
        roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["SFX", "Music"]
    );
    assert_eq!(roots[1].file_count, None, "not scanned yet");
    assert_eq!(
        revision(state).await,
        before,
        "adding a root changes no records"
    );

    // A scan of the new root through the admin wrapper, then the client API sees it.
    let done = scan_root(state, "Music").await;
    assert_eq!(done.progress.added, 3);
    assert!(done.revision.is_some_and(|revision| revision > before));
    let roots: Vec<RootResponse> = ok(api(state, "/roots").await).await;
    assert_eq!(roots[1].file_count, Some(3));
    assert_eq!(roots[1].last_scan.as_ref().map(|scan| scan.added), Some(3));
    // Streams under the new root are served (root lookup knows it).
    let id = f.count("SELECT id FROM audio_files WHERE filename = 'c.wav'");
    let stream = api(state, &format!("/audio/{id}/stream")).await;
    assert_eq!(stream.status(), StatusCode::OK);

    // Persisted next to the library; a restart (fresh state from the same files) has it.
    let saved = fs::read_to_string(f.config.admin_dir().join("admin-settings.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let saved_names: Vec<&str> = saved["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| root["name"].as_str().unwrap())
        .collect();
    assert_eq!(saved_names, ["SFX", "Music"]);
    let reopened = AppState::open(&f.config, None).unwrap();
    let roots: Vec<RootResponse> = ok(api(&reopened, "/roots").await).await;
    assert_eq!(
        roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["SFX", "Music"]
    );
    assert_eq!(
        Security::from_config(&f.config, None)
            .roots_override()
            .map(|roots| roots.len()),
        Some(2)
    );
}

#[tokio::test]
async fn removing_a_root_forgets_its_records_but_never_its_files() {
    let f = fixture().await;
    let state = &f.state;
    let music = f.extra_root("music", &["Loops/a.wav", "Loops/b.wav", "c.wav"]);
    let empty = f.extra_root("empty", &["x.wav"]);
    assert_eq!(
        add(state, "Music", &music).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        add(state, "Fresh", &empty).await.status(),
        StatusCode::CREATED
    );
    scan_root(state, "Music").await;
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 7);
    let before = revision(state).await;

    // Unknown root: 404. A root that was never scanned: removed, nothing to forget, the
    // revision stays.
    error(
        admin(state, Method::DELETE, "/admin/api/roots/Nope", None).await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    )
    .await;
    let fresh: serde_json::Value =
        ok(admin(state, Method::DELETE, "/admin/api/roots/Fresh", None).await).await;
    assert_eq!(fresh["removed"], 0);
    assert_eq!(fresh["revision"], fresh["revision_before"]);
    assert_eq!(revision(state).await, before);

    let removed: serde_json::Value =
        ok(admin(state, Method::DELETE, "/admin/api/roots/Music", None).await).await;
    assert_eq!(removed["name"], "Music");
    assert_eq!(removed["removed"], 3);
    assert_eq!(removed["folders_removed"], 2);
    assert_eq!(removed["revision_before"], before);
    assert_eq!(removed["revision"], before + 1);
    assert_eq!(revision(state).await, before + 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 4);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM folders WHERE folder_path LIKE '%music%'"),
        0
    );
    for file in ["Loops/a.wav", "Loops/b.wav", "c.wav"] {
        assert_eq!(fs::read(under(&music, file)).unwrap().len(), 52, "{file}");
    }
    let roots: Vec<RootResponse> = ok(api(state, "/roots").await).await;
    assert_eq!(roots.len(), 1);
    // The removed root is gone from the saved list too.
    let reopened = AppState::open(&f.config, None).unwrap();
    let roots: Vec<RootResponse> = ok(api(&reopened, "/roots").await).await;
    assert_eq!(roots.len(), 1);
    // A root can be removed even when it came from config.toml; then none is left and a
    // restart does not bring `config.toml`'s list back.
    ok::<serde_json::Value>(admin(state, Method::DELETE, "/admin/api/roots/SFX", None).await).await;
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 0);
    let reopened = AppState::open(&f.config, None).unwrap();
    let roots: Vec<RootResponse> = ok(api(&reopened, "/roots").await).await;
    assert!(roots.is_empty());
    for file in FILES {
        assert!(under(&f.sounds, file).is_file(), "{file}");
    }
    // Client-facing delete of a (re-added) root's top-level folder says where to go.
    assert_eq!(
        add(state, "SFX", &f.sounds).await.status(),
        StatusCode::CREATED
    );
    scan_root(state, "SFX").await;
    let top = f.count(&format!(
        "SELECT id FROM folders WHERE parent_id IS NULL AND folder_path = '{}'",
        audiolib_db::folder_path_text(&f.sounds)
    ));
    let response = admin(
        state,
        Method::DELETE,
        &format!("/api/v1/folders/{top}"),
        None,
    )
    .await;
    let refused = error(response, StatusCode::BAD_REQUEST, "VALIDATION").await;
    assert!(refused.message.contains("/admin") && refused.message.contains("库根目录"));
}

#[tokio::test]
async fn removing_a_root_is_busy_while_a_scan_runs() {
    let f = fixture().await;
    let state = &f.state;
    let bulk = f.extra_root("bulk", &[]);
    for index in 0..3000 {
        wav(
            &bulk
                .join(format!("{:02}", index % 30))
                .join(format!("f_{index:04}.wav")),
            4,
        );
    }
    assert_eq!(
        add(state, "Bulk", &bulk).await.status(),
        StatusCode::CREATED
    );
    let scan: ScanResponse = ok(admin(
        state,
        Method::POST,
        "/admin/api/scans",
        Some(r#"{"root":"Bulk","mode":"full","workers":1}"#),
    )
    .await)
    .await;
    let id = scan.progress.session_id;
    ok::<ScanResponse>(
        admin(
            state,
            Method::POST,
            &format!("/admin/api/scans/{id}/pause"),
            None,
        )
        .await,
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let now: ScanResponse =
            ok(admin(state, Method::GET, &format!("/admin/api/scans/{id}"), None).await).await;
        if now.progress.paused {
            break;
        }
        assert!(
            now.active && Instant::now() < deadline,
            "scan did not pause"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for name in ["Bulk", "SFX"] {
        error(
            admin(
                state,
                Method::DELETE,
                &format!("/admin/api/roots/{name}"),
                None,
            )
            .await,
            StatusCode::CONFLICT,
            "BUSY",
        )
        .await;
    }
    let current: serde_json::Value =
        ok(admin(state, Method::GET, "/admin/api/scans/current", None).await).await;
    assert_eq!(current["scan"]["session_id"], id);
    ok::<ScanResponse>(
        admin(
            state,
            Method::POST,
            &format!("/admin/api/scans/{id}/cancel"),
            None,
        )
        .await,
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while ok::<ScanResponse>(
        admin(state, Method::GET, &format!("/admin/api/scans/{id}"), None).await,
    )
    .await
    .active
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    ok::<serde_json::Value>(admin(state, Method::DELETE, "/admin/api/roots/Bulk", None).await)
        .await;
}

#[tokio::test]
async fn the_root_endpoints_keep_the_admin_rules() {
    let f = fixture().await;
    let service = Service::from(f.state.clone());
    let new_root = f.extra_root("music", &["a.wav"]);
    let body = format!(r#"{{"name":"Music","path":{}}}"#, quoted(&new_root));
    let changing: [(Method, &str, Option<&str>); 6] = [
        (Method::POST, "/admin/api/roots", Some(&body)),
        (Method::DELETE, "/admin/api/roots/SFX", None),
        (Method::POST, "/admin/api/scans", Some(r#"{"root":"SFX"}"#)),
        (Method::POST, "/admin/api/scans/1/pause", None),
        (Method::POST, "/admin/api/scans/1/resume", None),
        (Method::POST, "/admin/api/scans/1/cancel", None),
    ];
    let reading = [
        "/admin/api/fs",
        "/admin/api/roots",
        "/admin/api/scans/current",
        "/admin/api/scans/1",
    ];
    let host = ("host", "192.0.2.10:8790");
    // Without the admin header: 403, nothing changed.
    for (method, path, body) in &changing {
        let response = request(
            service.clone(),
            method.clone(),
            path,
            &[host],
            Some(peer(LAN)),
            *body,
        )
        .await;
        error(response, StatusCode::FORBIDDEN, "ADMIN_HEADER_REQUIRED").await;
    }
    // A cross-origin page: 403 even with the header.
    let response = request(
        service.clone(),
        Method::DELETE,
        "/admin/api/roots/SFX",
        &[
            host,
            ("x-audiolib-admin", "1"),
            ("origin", "http://evil.example"),
        ],
        Some(peer(LAN)),
        None,
    )
    .await;
    error(response, StatusCode::FORBIDDEN, "ADMIN_HEADER_REQUIRED").await;
    // Proxied (forwarded) and public requests, and public host names: ADMIN_LAN_ONLY.
    let marked = [host, ("x-audiolib-admin", "1")];
    for path in reading.iter().copied().chain(["/admin/api/roots/SFX"]) {
        for (headers, connection) in [
            (
                vec![marked[0], marked[1], ("x-forwarded-for", "198.51.100.7")],
                peer(LAN),
            ),
            (marked.to_vec(), peer("203.0.113.9:50000")),
            (vec![("host", "audiolib.example.com"), marked[1]], peer(LAN)),
        ] {
            let method = if path.ends_with("/SFX") {
                Method::DELETE
            } else {
                Method::GET
            };
            let response = request(
                service.clone(),
                method,
                path,
                &headers,
                Some(connection),
                None,
            )
            .await;
            error(response, StatusCode::FORBIDDEN, "ADMIN_LAN_ONLY").await;
        }
    }
    assert_eq!(f.count("SELECT COUNT(*) FROM audio_files"), 4);
    assert!(!f.config.admin_dir().join("admin-settings.json").exists());
    // Plain reads work from the LAN without a header; and the page ships the section.
    for path in [
        "/admin/api/fs",
        "/admin/api/roots",
        "/admin/api/scans/current",
    ] {
        let response = request(
            service.clone(),
            Method::GET,
            path,
            &[host],
            Some(peer(LAN)),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    let page = bytes(
        request(
            service.clone(),
            Method::GET,
            "/admin",
            &[host],
            Some(peer(LAN)),
            None,
        )
        .await,
    )
    .await;
    assert!(String::from_utf8_lossy(&page).contains("库根目录"));
}
