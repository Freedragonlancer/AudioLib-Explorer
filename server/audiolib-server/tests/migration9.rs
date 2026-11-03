// server/audiolib-server/tests/migration9.rs
//! Migration wire contract and authorization on isolated libraries, without a listener.
use audiolib_db::{
    SqliteProvider,
    migration9::{Report, State},
};
use audiolib_server::{AppState, Security, ServerConfig, router};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use std::{fs, path::PathBuf};
use tower::ServiceExt;

fn fixture(manage: &str) -> (tempfile::TempDir, PathBuf, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.db");
    drop(SqliteProvider::open(&db).unwrap());
    let config=ServerConfig::from_toml_str(&format!("bind='127.0.0.1:0'\nlibrary_db='library.db'\nauth_mode='off'\nmanage_mode='{manage}'\nlist_waveform_workers=1\n"),dir.path()).unwrap();
    let security = Security::from_config(&config, None);
    let state = AppState::open_with_security(&config, security).unwrap();
    (dir, db, state)
}
async fn request(state: &AppState, method: Method, path: &str, marked: bool) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost");
    if marked {
        request = request.header("x-audiolib-admin", "1");
    }
    // Router-level LAN fixture; native loopback/proxy classification has separate
    // security.rs coverage. No network connection is made by this helper.
    let request = request.body(Body::empty()).unwrap();
    router(state.clone()).oneshot(request).await.unwrap()
}
async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn record(db: &std::path::Path, state: &AppState, report: Report) {
    let mut name = db.as_os_str().to_os_string();
    name.push(".migration9.json");
    fs::write(std::path::Path::new(&name),serde_json::to_vec(&serde_json::json!({"protocol":1,"library_id":state.provider().album_namespace(),"backup":null,"report":report})).unwrap()).unwrap();
}

#[tokio::test]
async fn admin_requires_manage_and_csrf_guard() {
    let (_dir, _db, state) = fixture("off");
    assert_eq!(
        request(&state, Method::GET, "/admin/api/migration/status", true)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&state, Method::POST, "/admin/api/migration/start", true)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    state.shutdown(std::time::Duration::from_secs(5)).unwrap();
    let (_dir, _db, state) = fixture("all");
    assert_eq!(
        request(&state, Method::POST, "/admin/api/migration/start", false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let status = request(&state, Method::GET, "/admin/api/migration/status", true).await;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(json(status).await["state"], "idle");
    state.shutdown(std::time::Duration::from_secs(5)).unwrap();
}

#[tokio::test]
async fn auth_off_does_not_expose_migration_admin_on_native_loopback() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (_dir, _db, state) = fixture("all");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(audiolib_server::serve(
        listener,
        router(state.clone()),
        async {
            let _ = stopped.await;
        },
        audiolib_server::ServeOptions::default(),
    ));
    let mut replies = Vec::new();
    for (method, path) in [
        ("GET", "/api/v1/health"),
        ("GET", "/admin/api/migration/status"),
        ("POST", "/admin/api/migration/start"),
    ] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        // Even a correctly marked native request must not bypass the LAN gate:
        // a local reverse proxy can produce exactly the same socket peer.
        let request = format!(
            "{method} {path} HTTP/1.0\r\nHost: {address}\r\nX-AudioLib-Admin: 1\r\nContent-Length: 0\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stream.read_to_end(&mut reply),
        )
        .await
        .unwrap()
        .unwrap();
        replies.push(String::from_utf8(reply).unwrap());
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    state.shutdown(std::time::Duration::from_secs(5)).unwrap();
    assert!(replies[0].starts_with("HTTP/1.0 200"), "{}", replies[0]);
    for reply in &replies[1..] {
        assert!(reply.starts_with("HTTP/1.0 403"), "{reply}");
        let (_, body) = reply.split_once("\r\n\r\n").unwrap();
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["error_code"], "ADMIN_LAN_ONLY");
    }
}

#[tokio::test]
async fn paused_reads_scopes_and_switching_writes_follow_http_schema() {
    let (_dir, db, state) = fixture("all");
    record(
        &db,
        &state,
        Report {
            state: State::Paused,
            ..Default::default()
        },
    );
    let main = request(&state, Method::GET, "/api/v1/search?q=wind", false).await;
    assert_eq!(main.status(), StatusCode::OK);
    assert_eq!(main.headers()["x-audiolib-migration"], "paused");
    assert!(!main.headers().contains_key("x-audiolib-schema-migration"));
    let all = request(
        &state,
        Method::GET,
        "/api/v1/search?q=wind&scope=all",
        false,
    )
    .await;
    assert_eq!(all.status(), StatusCode::OK);
    assert_eq!(all.headers()["x-audiolib-path-search"], "unavailable");
    assert_eq!(json(main).await["results"], json(all).await["results"]);
    let path = request(
        &state,
        Method::GET,
        "/api/v1/search?q=wind&scope=path",
        false,
    )
    .await;
    assert_eq!(path.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(path.headers()["retry-after"], "30");
    assert_eq!(json(path).await["error_code"], "migration_in_progress");
    let mut lock = db.as_os_str().to_os_string();
    lock.push(".migration9.lock");
    let owner = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(std::path::Path::new(&lock))
        .unwrap();
    owner.try_lock().unwrap();
    record(
        &db,
        &state,
        Report {
            state: State::Switching,
            ..Default::default()
        },
    );
    let write = request(&state, Method::PATCH, "/api/v1/audio/1/metadata", false).await;
    assert_eq!(write.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(write.headers()["retry-after"], "5");
    assert_eq!(write.headers()["x-audiolib-migration"], "switching");
    assert_eq!(json(write).await["error_code"], "migration_switching");
    assert_eq!(
        request(&state, Method::GET, "/api/v1/search?q=wind", false)
            .await
            .status(),
        StatusCode::OK
    );
    drop(owner);
    let report = audiolib_db::migration9::execute(
        &db,
        audiolib_db::migration9::Action::Start,
        &Default::default(),
        &std::sync::atomic::AtomicBool::new(false),
    );
    assert_eq!(report.state, State::Done, "{report:?}");
    let after = request(
        &state,
        Method::GET,
        "/api/v1/search?q=wind&scope=path",
        false,
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    assert!(!after.headers().contains_key("x-audiolib-migration"));
    let rollback = request(&state, Method::POST, "/admin/api/migration/rollback", true).await;
    assert_eq!(rollback.status(), StatusCode::OK);
    assert_eq!(json(rollback).await["state"], "rolled_back");
    state.shutdown(std::time::Duration::from_secs(5)).unwrap();
}
