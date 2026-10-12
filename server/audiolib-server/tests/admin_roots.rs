// server/audiolib-server/tests/admin_roots.rs
//! Admin web API for library roots (doc/api/README.md §2.3.1): server disk browser
//! `GET /admin/api/fs`, `POST /admin/api/roots`, `DELETE /admin/api/roots/{name}` and the
//! admin scan wrappers, against temporary libraries and roots of tiny generated WAVs.
//! Nothing here touches anything outside the temporary directories; every WAV must still
//! be on disk after a root was removed.
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
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";
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
fn fixture() -> Fixture {
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
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    if let Some(connection) = connection {
        request.extensions_mut().insert(connection);
    }
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
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let scan: ScanResponse = ok(admin(
            state,
            Method::GET,
            &format!("/admin/api/scans/{}", started.progress.session_id),
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
    let f = fixture();
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

#[tokio::test]
async fn adding_a_root_validates_applies_at_once_and_survives_a_restart() {
    let f = fixture();
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
    let f = fixture();
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
    let f = fixture();
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
    let f = fixture();
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
