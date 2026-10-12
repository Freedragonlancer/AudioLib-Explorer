// server/audiolib-server/tests/scans.rs
//! Remote scans (`POST /scans` and friends), history lists and the `/roots` scan fields,
//! against temporary libraries and roots of tiny generated WAVs. Nothing real is touched.
use audiolib_core::DataProvider;
use audiolib_db::SqliteProvider;
use audiolib_server::{
    AppState, BearerToken, ConnectionInfo, CurrentScanResponse, ErrorBody, LibraryResponse,
    Listener, RootMonitor, RootPolicy, RootProbe, RootResponse, ScanResponse, ServerConfig,
    Service, router,
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

struct Fixture {
    _dir: tempfile::TempDir,
    sounds: PathBuf,
    config: ServerConfig,
    state: AppState,
}

/// An empty library and a root `SFX` with `Doors/door_1.wav`, `Doors/door_2.wav` and
/// `wind.wav`; a second root `Offline` that does not exist. `extra` adds TOML lines.
fn fixture(extra: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = real_path(dir.path());
    let sounds = base.join("sounds");
    wav(&sounds.join("Doors").join("door_1.wav"), 4);
    wav(&sounds.join("Doors").join("door_2.wav"), 4);
    wav(&sounds.join("wind.wav"), 4);
    fs::create_dir_all(base.join("outside")).unwrap();
    let db = dir.path().join("library").join("library.db");
    drop(SqliteProvider::open(&db).unwrap());
    let config = ServerConfig::from_toml_str(
        &format!(
            "bind = \"127.0.0.1:0\"\nlibrary_db = 'library/library.db'\nsnapshot_dir = 'snapshots'\n{extra}\
             [[roots]]\nname = \"SFX\"\nserver_path = '{}'\n\
             [[roots]]\nname = \"Offline\"\nserver_path = '{}'\n",
            sounds.display(),
            base.join("not-mounted").display(),
        ),
        dir.path(),
    )
    .unwrap();
    let state = AppState::open(&config, BearerToken::new(TOKEN).unwrap()).unwrap();
    Fixture {
        _dir: dir,
        sounds,
        config,
        state,
    }
}

async fn call(
    state: &AppState,
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
    router(Service::from(state.clone()))
        .oneshot(request)
        .await
        .unwrap()
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

async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let body = bytes(response).await;
    serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&body)))
}

async fn get<T: serde::de::DeserializeOwned>(state: &AppState, path: &str) -> T {
    let response = call(
        state,
        Method::GET,
        &format!("/api/v1{path}"),
        &[],
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    json(response).await
}

async fn post(state: &AppState, path: &str, body: Option<&str>) -> Response {
    call(
        state,
        Method::POST,
        &format!("/api/v1{path}"),
        &[],
        None,
        body,
    )
    .await
}

async fn error(response: Response, status: StatusCode, code: &str) -> ErrorBody {
    assert_eq!(response.status(), status);
    let body: ErrorBody = json(response).await;
    assert_eq!(body.error_code, code, "{}", body.message);
    body
}

async fn start(state: &AppState, body: &str) -> ScanResponse {
    let response = post(state, "/scans", Some(body)).await;
    assert_eq!(response.status(), StatusCode::OK, "POST /scans {body}");
    let scan: ScanResponse = json(response).await;
    assert!(scan.progress.session_id > 0);
    assert!(scan.active);
    scan
}

async fn wait_done(state: &AppState, id: i64) -> ScanResponse {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let scan: ScanResponse = get(state, &format!("/scans/{id}")).await;
        if !scan.active {
            return scan;
        }
        assert!(Instant::now() < deadline, "scan {id} did not finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn revision(state: &AppState) -> u64 {
    get::<LibraryResponse>(state, "/library").await.revision
}

async fn sfx_root(state: &AppState) -> RootResponse {
    get::<Vec<RootResponse>>(state, "/roots")
        .await
        .into_iter()
        .find(|root| root.name == "SFX")
        .unwrap()
}

#[tokio::test]
async fn scans_run_to_completion_bump_the_revision_only_for_changes_and_feed_the_snapshot() {
    let f = fixture("");
    let state = &f.state;
    let current: CurrentScanResponse = get(state, "/scans/current").await;
    assert!(current.scan.is_none());
    let root = sfx_root(state).await;
    assert_eq!((root.missing_count, root.scanning), (0, false));
    assert!(root.last_scan.is_none());
    let before = revision(state).await;

    // First (incremental) scan of the whole root adds everything.
    let scan = start(
        state,
        r#"{"root":"SFX","subpath":"","mode":"incremental","workers":null}"#,
    )
    .await;
    assert_eq!(scan.root.as_deref(), Some("SFX"));
    assert_eq!(scan.mode, "incremental");
    let done = wait_done(state, scan.progress.session_id).await;
    assert_eq!(done.progress.state, audiolib_core::ScanState::Completed);
    assert_eq!((done.progress.added, done.progress.errors), (3, 0));
    let first = revision(state).await;
    assert_eq!(first, before + 1, "one bump per scan, not per batch");
    assert_eq!(done.revision, Some(first));

    // The snapshot of the new revision contains the files.
    let response = call(
        state,
        Method::GET,
        "/api/v1/library/snapshot",
        &[],
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = f.config.library_db.with_file_name("check-snapshot.db");
    fs::write(&snapshot, bytes(response).await).unwrap();
    let count: i64 = rusqlite::Connection::open(&snapshot)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM audio_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 3);

    // A no-op incremental scan does not bump.
    let noop = wait_done(
        state,
        start(state, r#"{"root":"SFX"}"#).await.progress.session_id,
    )
    .await;
    assert_eq!(
        (
            noop.progress.added,
            noop.progress.updated,
            noop.progress.unchanged
        ),
        (0, 0, 3)
    );
    assert_eq!(
        revision(state).await,
        first,
        "no-op scan keeps the revision"
    );
    assert_eq!(noop.revision, Some(first));

    // A subdirectory scan finds a new file and keeps the subtree under the root.
    wav(&f.sounds.join("Doors").join("Metal").join("door_3.wav"), 4);
    let sub = start(state, r#"{"root":"SFX","subpath":"Doors/Metal/"}"#).await;
    assert_eq!(sub.subpath.as_deref(), Some("Doors/Metal"));
    let sub = wait_done(state, sub.progress.session_id).await;
    assert_eq!(sub.progress.added, 1);
    let second = revision(state).await;
    assert_eq!(second, first + 1);
    let top = f.state.provider().folders(None).unwrap();
    assert_eq!(top.len(), 1, "no extra top-level folder: {top:?}");
    assert_eq!(top[0].file_count, 4);

    // A full scan after a deletion marks the file missing (and rewrites records: bump).
    fs::remove_file(f.sounds.join("wind.wav")).unwrap();
    let full = wait_done(
        state,
        start(state, r#"{"root":"SFX","mode":"full","workers":2}"#)
            .await
            .progress
            .session_id,
    )
    .await;
    assert_eq!((full.progress.removed, full.progress.updated), (1, 3));
    assert_eq!(revision(state).await, second + 1);
    let root = sfx_root(state).await;
    assert_eq!(root.missing_count, 1);
    assert_eq!(root.file_count, Some(3));
    assert!(!root.scanning);
    let last = root.last_scan.unwrap();
    assert_eq!(last.id, full.progress.session_id);
    assert_eq!(
        (last.mode.as_str(), last.state.as_str()),
        ("full", "completed")
    );
    assert_eq!(
        (last.subpath.as_str(), last.removed, last.scanned),
        ("", 1, 3)
    );
    assert!(last.started_at.is_some() && last.finished_at >= last.started_at);

    // History: newest first, paginated; finished sessions stay queryable.
    let history: Vec<audiolib_core::ScanSession> = get(state, "/history/scans").await;
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].id, full.progress.session_id);
    let page: Vec<audiolib_core::ScanSession> = get(state, "/history/scans?limit=1&offset=1").await;
    assert_eq!(page[0].id, sub.progress.session_id);
    for bad in ["limit=0", "limit=10001", "offset=-1", "limit=x"] {
        let response = call(
            state,
            Method::GET,
            &format!("/api/v1/history/scans?{bad}"),
            &[],
            None,
            None,
        )
        .await;
        error(response, StatusCode::BAD_REQUEST, "VALIDATION").await;
    }
    let again: ScanResponse = get(state, &format!("/scans/{}", scan.progress.session_id)).await;
    assert_eq!(again.progress.added, 3);
    assert!(!again.active);
    error(
        call(state, Method::GET, "/api/v1/scans/999", &[], None, None).await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    )
    .await;
    error(
        call(state, Method::GET, "/api/v1/scans/abc", &[], None, None).await,
        StatusCode::BAD_REQUEST,
        "VALIDATION",
    )
    .await;
    for action in ["pause", "resume", "cancel"] {
        let path = format!("/scans/{}/{action}", full.progress.session_id);
        error(
            post(state, &path, None).await,
            StatusCode::CONFLICT,
            "SCAN_STATE",
        )
        .await;
        error(
            post(state, &format!("/scans/999/{action}"), None).await,
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        )
        .await;
    }

    // Play history.
    let id = f
        .state
        .provider()
        .find_audio_by_path(&f.sounds.join("Doors").join("door_1.wav"))
        .unwrap()
        .unwrap()
        .id;
    assert_eq!(
        post(state, &format!("/audio/{id}/plays"), None)
            .await
            .status(),
        StatusCode::OK
    );
    let plays: Vec<audiolib_core::PlayRecord> = get(state, "/history/plays?limit=10").await;
    assert_eq!(plays.len(), 1);
    assert_eq!(plays[0].audio_file_id, id);
}

#[tokio::test]
async fn scan_requests_are_validated_before_anything_runs() {
    let f = fixture("");
    let state = &f.state;
    fs::write(f.sounds.join("notes.txt"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        f.sounds.parent().unwrap().join("outside"),
        f.sounds.join("escape"),
    )
    .unwrap();
    let cases: &[(&str, StatusCode, &str)] = &[
        (r#"{"root":"Nope"}"#, StatusCode::NOT_FOUND, "NOT_FOUND"),
        (
            r#"{"root":"SFX","subpath":"../outside"}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (
            r#"{"root":"SFX","subpath":"/etc"}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (
            r#"{"root":"SFX","subpath":"Doors\\x"}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (
            r#"{"root":"SFX","subpath":"Absent"}"#,
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
        (
            r#"{"root":"SFX","subpath":"notes.txt"}"#,
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
        (
            r#"{"root":"SFX","workers":17}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (
            r#"{"root":"SFX","workers":-1}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (
            r#"{"root":"SFX","mode":"quick"}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
        (r#"{"subpath":""}"#, StatusCode::BAD_REQUEST, "VALIDATION"),
        ("not json", StatusCode::BAD_REQUEST, "VALIDATION"),
        (
            r#"{"root":"Offline"}"#,
            StatusCode::SERVICE_UNAVAILABLE,
            "ROOT_OFFLINE",
        ),
        #[cfg(unix)]
        (
            r#"{"root":"SFX","subpath":"escape"}"#,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        ),
    ];
    let before = revision(state).await;
    for (body, status, code) in cases {
        let response = post(state, "/scans", Some(body)).await;
        assert_eq!(response.status(), *status, "{body}");
        let body_text: ErrorBody = json(response).await;
        assert_eq!(body_text.error_code, *code, "{body}: {}", body_text.message);
    }
    let history: Vec<audiolib_core::ScanSession> = get(state, "/history/scans").await;
    assert!(history.is_empty(), "rejected requests create no session");
    let current: CurrentScanResponse = get(state, "/scans/current").await;
    assert!(current.scan.is_none(), "slot released after failures");
    assert_eq!(revision(state).await, before);
}

#[tokio::test]
async fn offline_root_uses_the_bounded_recheck_and_answers_root_offline() {
    let f = fixture("");
    let probe: RootProbe =
        Arc::new(|_: &Path| Err("Root directory not found (volume not mounted?)".to_string()));
    let monitor = RootMonitor::start_with_probe(&f.config.roots, RootPolicy::default(), probe);
    let state =
        AppState::open_with_monitor(&f.config, BearerToken::new(TOKEN).unwrap(), monitor).unwrap();
    let started = Instant::now();
    let body = error(
        post(&state, "/scans", Some(r#"{"root":"SFX"}"#)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "ROOT_OFFLINE",
    )
    .await;
    assert!(
        body.message
            .starts_with("Library root \"SFX\" is offline: "),
        "{}",
        body.message
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

async fn wait_paused(state: &AppState, id: i64) -> ScanResponse {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let scan: ScanResponse = get(state, &format!("/scans/{id}")).await;
        if scan.progress.paused {
            return scan;
        }
        assert!(scan.active, "scan finished before pausing: {scan:?}");
        assert!(Instant::now() < deadline, "scan {id} did not pause");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn pause_resume_cancel_and_one_scan_at_a_time() {
    let f = fixture("");
    let state = &f.state;
    for index in 0..5000 {
        wav(
            &f.sounds
                .join("Bulk")
                .join(format!("{:02}", index % 50))
                .join(format!("file_{index:04}.wav")),
            4 + index % 7,
        );
    }
    let before = revision(state).await;
    let scan = start(state, r#"{"root":"SFX","mode":"full","workers":1}"#).await;
    let id = scan.progress.session_id;
    let paused = post(state, &format!("/scans/{id}/pause"), None).await;
    assert_eq!(paused.status(), StatusCode::OK);
    let paused: ScanResponse = json(paused).await;
    assert!(paused.pause_requested && paused.active);
    let paused = wait_paused(state, id).await;
    // Idempotent repeat.
    assert_eq!(
        post(state, &format!("/scans/{id}/pause"), None)
            .await
            .status(),
        StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still: ScanResponse = get(state, &format!("/scans/{id}")).await;
    assert_eq!(
        still.progress.processed, paused.progress.processed,
        "paused work stops"
    );

    // While it runs (paused): one scan at a time, batch edits busy, revision held.
    let busy = post(state, "/scans", Some(r#"{"root":"SFX"}"#)).await;
    let busy = error(busy, StatusCode::CONFLICT, "BUSY").await;
    assert!(busy.message.contains(&id.to_string()), "{}", busy.message);
    let current: CurrentScanResponse = get(state, "/scans/current").await;
    assert_eq!(current.scan.unwrap().progress.session_id, id);
    assert!(sfx_root(state).await.scanning);
    let batch = post(
        state,
        "/audio/metadata/batch",
        Some(r#"{"ids":[1],"patch":{"favorite":true}}"#),
    )
    .await;
    error(batch, StatusCode::CONFLICT, "BUSY").await;
    assert_eq!(
        revision(state).await,
        before,
        "revision held while scanning (also after a rejected write)"
    );

    // Resume, let it advance, pause again and cancel while paused.
    let resumed: ScanResponse = json(post(state, &format!("/scans/{id}/resume"), None).await).await;
    assert!(!resumed.pause_requested);
    assert_eq!(
        post(state, &format!("/scans/{id}/resume"), None)
            .await
            .status(),
        StatusCode::OK
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let scan: ScanResponse = get(state, &format!("/scans/{id}")).await;
        if scan.progress.processed > paused.progress.processed {
            break;
        }
        assert!(
            scan.active && Instant::now() < deadline,
            "no progress after resume"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        post(state, &format!("/scans/{id}/pause"), None)
            .await
            .status(),
        StatusCode::OK
    );
    wait_paused(state, id).await;
    let cancelled: ScanResponse =
        json(post(state, &format!("/scans/{id}/cancel"), None).await).await;
    assert!(cancelled.cancel_requested);
    error(
        post(state, &format!("/scans/{id}/resume"), None).await,
        StatusCode::CONFLICT,
        "SCAN_STATE",
    )
    .await;
    let done = wait_done(state, id).await;
    assert_eq!(done.progress.state, audiolib_core::ScanState::Cancelled);
    assert_eq!(
        done.progress.removed, 0,
        "no missing reconciliation on cancel"
    );
    assert!(done.progress.processed < 5003);
    assert_eq!(
        revision(state).await,
        before + 1,
        "cancelled scan with commits bumps once"
    );
    let current: CurrentScanResponse = get(state, "/scans/current").await;
    assert!(current.scan.is_none());
    let last = sfx_root(state).await.last_scan.unwrap();
    assert_eq!((last.id, last.state.as_str()), (id, "cancelled"));
}

#[tokio::test]
async fn shutdown_cancels_the_running_scan_and_records_it() {
    let f = fixture("");
    let state = &f.state;
    for index in 0..5000 {
        wav(
            &f.sounds.join("Bulk").join(format!("file_{index:04}.wav")),
            4,
        );
    }
    let id = start(state, r#"{"root":"SFX","workers":1}"#)
        .await
        .progress
        .session_id;
    let manager_state = state.clone();
    let stopped = tokio::task::spawn_blocking(move || {
        manager_state.scans().shutdown(Duration::from_secs(20))
    })
    .await
    .unwrap();
    assert!(stopped);
    let done: ScanResponse = get(state, &format!("/scans/{id}")).await;
    assert!(!done.active);
    assert_eq!(done.progress.state, audiolib_core::ScanState::Cancelled);
    assert!(done.progress.message.contains("shutting down"));
    let history: Vec<audiolib_core::ScanSession> = get(state, "/history/scans").await;
    assert_eq!(history[0].state, audiolib_core::ScanState::Cancelled);
    assert!(history[0].end_time.is_some());
    let refused = error(
        post(state, "/scans", Some(r#"{"root":"SFX"}"#)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "UNAVAILABLE",
    )
    .await;
    assert!(refused.message.contains("shutting down"));
}

#[tokio::test]
async fn scan_and_history_endpoints_follow_the_auth_mode() {
    // `auth` absent: external mode; TOKEN is the environment token.
    let f = fixture("");
    let state = &f.state;
    let lan = ConnectionInfo {
        peer: "10.0.0.20:50000".parse().unwrap(),
        listener: Listener::Http,
    };
    let public = ConnectionInfo {
        peer: "203.0.113.9:50000".parse().unwrap(),
        listener: Listener::Http,
    };
    let bearer = format!("Bearer {TOKEN}");
    let paths: &[(Method, &str, Option<&str>)] = &[
        (Method::GET, "/api/v1/history/scans", None),
        (Method::GET, "/api/v1/history/plays", None),
        (Method::GET, "/api/v1/scans/current", None),
        (Method::GET, "/api/v1/scans/1", None),
        (Method::POST, "/api/v1/scans/1/pause", None),
        (Method::POST, "/api/v1/scans", Some(r#"{"root":"Nope"}"#)),
    ];
    for (method, path, body) in paths {
        let external: [&[(&str, &str)]; 2] = [&[("x-forwarded-for", "10.0.0.5")], &[]];
        for (headers, connection) in [(external[0], lan), (external[1], public)] {
            let response = call(
                state,
                method.clone(),
                path,
                headers,
                Some(connection),
                *body,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} {headers:?}"
            );
            let mut with_token = headers.to_vec();
            with_token.push(("authorization", bearer.as_str()));
            let response = call(
                state,
                method.clone(),
                path,
                &with_token,
                Some(connection),
                *body,
            )
            .await;
            assert_ne!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
        // LAN-direct: no token needed, a garbage one is ignored.
        for headers in [&[][..], &[("authorization", "Bearer garbage")][..]] {
            let response = call(state, method.clone(), path, headers, Some(lan), *body).await;
            assert_ne!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }
    // A real scan from outside with the token works end to end.
    let response = call(
        state,
        Method::POST,
        "/api/v1/scans",
        &[
            ("x-forwarded-for", "198.51.100.1"),
            ("authorization", bearer.as_str()),
        ],
        Some(lan),
        Some(r#"{"root":"SFX"}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let scan: ScanResponse = json(response).await;
    assert_eq!(
        wait_done(state, scan.progress.session_id)
            .await
            .progress
            .added,
        3
    );
}
