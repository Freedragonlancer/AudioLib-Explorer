// server/audiolib-server/tests/ai_relay.rs
//! AI policy and transport tests use only owned temporary state and mock loopback upstreams.
use audiolib_ai::{AiError, relay::RelayResponse};
use audiolib_server::{
    BearerToken, ConnectionInfo, Listener, Security, ServerConfig, Service, router,
    router_with_request_timeout,
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
};
use tower::ServiceExt;

const TOKEN: &str = "ai-relay-server-fixture-0123456789abcdef0123456789abcdef";
const PROVIDER_KEY: &str = "owned-provider-key";

fn fixture(base: Option<&str>, extra: &str) -> (tempfile::TempDir, Service) {
    let directory = tempfile::tempdir().unwrap();
    let upstreams = base
        .map(|base| format!("ai_upstreams = [{{ protocol = 'magpie', base_url = '{base}' }}]\n"))
        .unwrap_or_default();
    let config = ServerConfig::from_toml_str(
        &format!("library_db = 'never-opened.db'\n{upstreams}{extra}"),
        directory.path(),
    )
    .unwrap();
    let security = Security::from_config(&config, Some(BearerToken::new(TOKEN).unwrap()));
    let service = Service::starting_with_config(security, &config).unwrap();
    (directory, service)
}

fn models_body(base: &str) -> Value {
    json!({
        "profile": {"protocol":"magpie", "base_url":base, "model":"group/opus", "timeout_seconds":2},
        "operation": {"type":"models"},
        "api_key": PROVIDER_KEY
    })
}

fn request(path: &str, body: &Value, token: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut request = Request::post(path)
        .header(header::HOST, "nas.lan:8790")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    let mut request = request.body(Body::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: "192.168.1.2:42000".parse().unwrap(),
        listener: Listener::Http,
        local: Some("192.168.1.3:8790".parse().unwrap()),
    });
    request
}

async fn json_response(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

struct MockUpstream {
    base: String,
    seen: oneshot::Receiver<String>,
    release: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

async fn upstream(status: u16, body: Value) -> MockUpstream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/gateway/v1", listener.local_addr().unwrap());
    let (seen_tx, seen) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut received = Vec::new();
        loop {
            let mut buffer = [0_u8; 4096];
            let count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(count, 0);
            received.extend_from_slice(&buffer[..count]);
            assert!(received.len() < 64 * 1024);
            if let Some(header_end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..header_end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if received.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
        let _ = seen_tx.send(String::from_utf8(received).unwrap());
        let _ = tokio::time::timeout(Duration::from_secs(5), release_rx).await;
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    MockUpstream {
        base,
        seen,
        release,
        task,
    }
}

#[test]
fn config_is_closed_by_default_and_rejects_invalid_or_duplicate_policy() {
    let directory = tempfile::tempdir().unwrap();
    let config =
        ServerConfig::from_toml_str("library_db = 'fixture.db'", directory.path()).unwrap();
    assert!(config.ai_upstreams.is_empty());
    assert_eq!(config.max_ai_requests, 4);
    for extra in [
        "max_ai_requests = 0",
        "max_ai_requests = 65",
        "ai_upstreams = [{protocol='magpie',base_url='http://127.0.0.1:3425/a/../v1'}]",
        "ai_upstreams = [{protocol='magpie',base_url='http://127.0.0.1:3425/v1'}, {protocol='magpie',base_url='http://127.0.0.1:3425/v1/chat/completions'}]",
    ] {
        assert!(
            ServerConfig::from_toml_str(
                &format!("library_db = 'fixture.db'\n{extra}"),
                directory.path()
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn every_ai_request_requires_a_verified_token_even_for_off_mode_lan_peers() {
    let base = "http://127.0.0.1:3425/v1";
    for mode in ["external", "off"] {
        let (_directory, service) = fixture(None, &format!("auth_mode = '{mode}'\n"));
        for token in [None, Some("invalid-but-present")] {
            let response = router(service.clone())
                .oneshot(request(
                    "/api/v1/ai/models",
                    &models_body(base),
                    token,
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = router(service)
            .oneshot(request(
                "/api/v1/ai/models",
                &models_body(base),
                Some(TOKEN),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(json_response(response).await["error_code"], "AI_DISABLED");
    }
}

#[tokio::test]
async fn policy_origin_operation_and_body_are_admitted_before_upstream_io() {
    let base = "http://127.0.0.1:3425/gateway/v1";
    let (directory, service) = fixture(Some(base), "");
    let cases = [
        (
            format!("{base}/other"),
            None,
            "/api/v1/ai/models",
            StatusCode::FORBIDDEN,
        ),
        (
            base.into(),
            Some("http://evil.example"),
            "/api/v1/ai/models",
            StatusCode::FORBIDDEN,
        ),
        (
            base.into(),
            None,
            "/api/v1/ai/generate",
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (url, origin, path, status) in cases {
        let response = router(service.clone())
            .oneshot(request(path, &models_body(&url), Some(TOKEN), origin))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let mut invalid = models_body(base);
    invalid["operation"]["type"] = json!(PROVIDER_KEY);
    let response = router(service.clone())
        .oneshot(request("/api/v1/ai/models", &invalid, Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        !json_response(response)
            .await
            .to_string()
            .contains(PROVIDER_KEY)
    );
    invalid = models_body(base);
    invalid["api_key"] = json!("x".repeat(65 * 1024));
    let response = router(service)
        .oneshot(request("/api/v1/ai/models", &invalid, Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!directory.path().join("never-opened.db").exists());
    assert!(!directory.path().join("ai-profiles.json").exists());
}

#[tokio::test]
async fn models_forward_only_provider_key_and_keep_magpie_model_ids() {
    let mock = upstream(
        200,
        json!({"data":[{"id":"group/opus"},{"id":"group/sonnet"}]}),
    )
    .await;
    let (_directory, service) = fixture(Some(&mock.base), "");
    let _ = mock.release.send(());
    // The approved /v1 base and documented completion endpoint are the same identity.
    let response = router(service)
        .oneshot(request(
            "/api/v1/ai/models",
            &models_body(&format!("{}/chat/completions", mock.base)),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: RelayResponse = serde_json::from_value(json_response(response).await).unwrap();
    assert_eq!(
        response.into_models().unwrap(),
        ["group/opus", "group/sonnet"]
    );
    let forwarded = mock.seen.await.unwrap();
    assert!(forwarded.starts_with("GET /gateway/v1/models HTTP/1.1\r\n"));
    assert!(
        forwarded
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {PROVIDER_KEY}"))
    );
    assert!(!forwarded.contains(TOKEN));
    mock.task.await.unwrap();
}

#[tokio::test]
async fn generation_reuses_structured_validation_and_redacts_upstream_errors() {
    let plan = json!({"schema_version":1,"intent":"wind","queries":[{
        "text":"wind","reason":"Direct wind","kind":"main","layer":"direct","source_ids":[],"cat_ids":[]
    }],"cautions":[]});
    let reply = json!({"choices":[{"finish_reason":"stop","message":{"content":plan.to_string()}}],
        "usage":{"prompt_tokens":13,"completion_tokens":7}});
    let mock = upstream(200, reply).await;
    let (_directory, service) = fixture(Some(&mock.base), "");
    let mut body = models_body(&mock.base);
    body["operation"] = json!({"type":"generate","intent":"wind","layers":false,
        "guidance":audiolib_ai::config::DEFAULT_PROMPT,"session":"fixture-session"});
    let _ = mock.release.send(());
    let response = router(service)
        .oneshot(request("/api/v1/ai/generate", &body, Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_response(response).await;
    assert_eq!(response["type"], "generated");
    assert_eq!(response["plan"], plan);
    assert_eq!(response["input_tokens"], 13);
    let forwarded = mock.seen.await.unwrap();
    assert!(forwarded.starts_with("POST /gateway/v1/chat/completions HTTP/1.1\r\n"));
    assert!(forwarded.contains("group/opus"));
    mock.task.await.unwrap();

    let mock = upstream(401, json!({"error":PROVIDER_KEY})).await;
    let (_directory, service) = fixture(Some(&mock.base), "");
    let _ = mock.release.send(());
    let response = router(service)
        .oneshot(request(
            "/api/v1/ai/models",
            &models_body(&mock.base),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_response(response).await;
    assert!(!response.to_string().contains(PROVIDER_KEY));
    let response: RelayResponse = serde_json::from_value(response).unwrap();
    assert!(matches!(response.into_models(), Err(AiError::Http(401))));
    mock.task.await.unwrap();
}

#[tokio::test]
async fn ai_does_not_inherit_the_ordinary_route_deadline() {
    let mock = upstream(200, json!({"data":[{"id":"group/opus"}]})).await;
    let (_directory, service) = fixture(Some(&mock.base), "");
    let body = models_body(&mock.base);
    let pending = tokio::spawn(
        router_with_request_timeout(service, Duration::from_millis(5)).oneshot(request(
            "/api/v1/ai/models",
            &body,
            Some(TOKEN),
            None,
        )),
    );
    mock.seen.await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    let _ = mock.release.send(());
    assert_eq!(pending.await.unwrap().unwrap().status(), StatusCode::OK);
    mock.task.await.unwrap();
}

#[tokio::test]
async fn cancelled_http_future_keeps_its_slot_until_blocking_upstream_returns() {
    let mock = upstream(200, json!({"data":[]})).await;
    let (_directory, service) = fixture(Some(&mock.base), "max_ai_requests = 1\n");
    let body = models_body(&mock.base);
    let pending = tokio::spawn(router(service.clone()).oneshot(request(
        "/api/v1/ai/models",
        &body,
        Some(TOKEN),
        None,
    )));
    mock.seen.await.unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let response = router(service)
        .oneshot(request("/api/v1/ai/models", &body, Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_response(response).await["error_code"], "BUSY");
    let _ = mock.release.send(());
    mock.task.await.unwrap();
}

#[tokio::test]
async fn remote_client_interoperates_over_http_without_an_open_library() {
    let models_upstream = upstream(200, json!({"data":[{"id":"group/opus"}]})).await;
    let plan = json!({"schema_version":1,"intent":"wind","queries":[{
        "text":"wind","reason":"Direct wind","kind":"main","layer":"direct","source_ids":[],"cat_ids":[]
    }],"cautions":[]});
    let generation_upstream = upstream(
        200,
        json!({
            "choices":[{"finish_reason":"stop","message":{"content":plan.to_string()}}],
            "usage":{"prompt_tokens":13,"completion_tokens":7}
        }),
    )
    .await;
    let (directory, service) = fixture(
        None,
        &format!(
            "ai_upstreams = [{{protocol='magpie',base_url='{}'}}, {{protocol='magpie',base_url='{}'}}]\n",
            models_upstream.base, generation_upstream.base,
        ),
    );
    assert!(matches!(
        service.readiness(),
        audiolib_server::Readiness::Starting
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(audiolib_server::serve(
        listener,
        router(service.clone()),
        async move {
            let _ = stopped.await;
        },
        audiolib_server::ServeOptions {
            shutdown_grace: Duration::from_secs(1),
            ..Default::default()
        },
    ));
    let models_profile = audiolib_ai::config::Profile {
        protocol: audiolib_ai::config::Protocol::Magpie,
        base_url: models_upstream.base.clone(),
        model: "group/opus".into(),
        timeout_seconds: 2,
        ..Default::default()
    };
    let generation_profile = audiolib_ai::config::Profile {
        base_url: generation_upstream.base.clone(),
        ..models_profile.clone()
    };
    let _ = models_upstream.release.send(());
    let _ = generation_upstream.release.send(());
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let request_cancel = cancel.clone();
    let calls = tokio::task::spawn_blocking(move || {
        let client = audiolib_remote::RemoteClient::new(&server_url, TOKEN).unwrap();
        let key = audiolib_ai::config::ApiKey::new(PROVIDER_KEY.into()).unwrap();
        let models = client.ai_models(&models_profile, &key, &request_cancel)?;
        let output = client.ai_generate(
            &generation_profile,
            &key,
            "wind",
            false,
            audiolib_ai::config::DEFAULT_PROMPT,
            "http-fixture-session",
            &request_cancel,
        )?;
        Ok::<_, AiError>((models, output))
    });
    let result = tokio::time::timeout(Duration::from_secs(8), calls).await;
    if result.is_err() {
        cancel.store(true, std::sync::atomic::Ordering::Release);
    }
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(2), serving)
        .await
        .unwrap()
        .unwrap();
    let (models, output) = result.unwrap().unwrap().unwrap();
    assert_eq!(models, ["group/opus"]);
    assert_eq!(serde_json::to_value(output.plan).unwrap(), plan);
    assert_eq!((output.input_tokens, output.output_tokens), (13, 7));
    for (seen, expected_method) in [
        (
            models_upstream.seen.await.unwrap(),
            "GET /gateway/v1/models HTTP/1.1\r\n",
        ),
        (
            generation_upstream.seen.await.unwrap(),
            "POST /gateway/v1/chat/completions HTTP/1.1\r\n",
        ),
    ] {
        assert!(seen.starts_with(expected_method));
        assert!(
            seen.to_ascii_lowercase()
                .contains(&format!("authorization: bearer {PROVIDER_KEY}"))
        );
        assert!(!seen.contains(TOKEN));
    }
    models_upstream.task.await.unwrap();
    generation_upstream.task.await.unwrap();
    assert!(matches!(
        service.readiness(),
        audiolib_server::Readiness::Starting
    ));
    assert!(!directory.path().join("never-opened.db").exists());
}

#[tokio::test]
async fn ready_library_migration_io_cannot_delay_ai_authentication_or_queries() {
    let mock = upstream(200, json!({"data":[{"id":"group/opus"}]})).await;
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("library.db");
    drop(audiolib_db::SqliteProvider::open(&database).unwrap());
    let config = ServerConfig::from_toml_str(
        &format!(
            "library_db='library.db'\nai_upstreams=[{{protocol='magpie',base_url='{}'}}]",
            mock.base,
        ),
        directory.path(),
    )
    .unwrap();
    let security = Security::from_config(&config, Some(BearerToken::new(TOKEN).unwrap()));
    let service = Service::starting_with_config(security.clone(), &config).unwrap();
    let state = audiolib_server::AppState::open_with_security(&config, security).unwrap();
    service.set_ready(state.clone());
    let report = audiolib_db::migration9::Report {
        state: audiolib_db::migration9::State::Paused,
        ..Default::default()
    };
    let mut report_path = database.as_os_str().to_os_string();
    report_path.push(".migration9.json");
    std::fs::write(std::path::Path::new(&report_path), serde_json::to_vec(&json!({
        "protocol":1,"library_id":state.provider().album_namespace(),"backup":null,"report":report
    })).unwrap()).unwrap();
    let mut lock_path = database.as_os_str().to_os_string();
    lock_path.push(".migration9-report.lock");
    let report_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(std::path::Path::new(&lock_path))
        .unwrap();
    report_lock.lock().unwrap();

    // Ordinary routes still read the report. Holding its real publication lease
    // reproduces unavailable library I/O without touching permissions or real data.
    let mut health = tokio::spawn(
        router(service.clone())
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap()),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut health)
            .await
            .is_err()
    );
    let migration_header = audiolib_core::server_upgrade::MIGRATION_HEADER;
    let body = models_body(&mock.base);
    for path in ["/api/v1/ai/models", "/api/v1/ai/generate"] {
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            router(service.clone()).oneshot(request(path, &body, None, None)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(migration_header));
    }
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        router(service.clone()).oneshot(request("/api/v1/ai/generate", &body, Some(TOKEN), None)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let _ = mock.release.send(());
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        router(service).oneshot(request("/api/v1/ai/models", &body, Some(TOKEN), None)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(migration_header));
    let response: RelayResponse = serde_json::from_value(json_response(response).await).unwrap();
    assert_eq!(response.into_models().unwrap(), ["group/opus"]);

    report_lock.unlock().unwrap();
    drop(report_lock);
    let health = tokio::time::timeout(Duration::from_secs(2), health)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(health.headers().get(migration_header).unwrap(), "paused");
    mock.task.await.unwrap();
}
