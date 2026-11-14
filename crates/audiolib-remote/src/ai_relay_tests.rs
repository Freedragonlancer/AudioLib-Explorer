// crates/audiolib-remote/src/ai_relay_tests.rs
//! Owned-loopback regression coverage for AI forwarding; no real settings or keys.
use super::*;
use crate::test_server::{TestResponse, TestServer};
use audiolib_ai::{config::DEFAULT_PROMPT, relay::RelayOperation};
use audiolib_core::ai_search::{AiQuery, AiQueryKind, AiQueryPlan};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    time::Instant,
};

fn profile() -> Profile {
    Profile {
        model: "owned-model".into(),
        timeout_seconds: 180,
        ..Profile::default()
    }
}

fn key() -> ApiKey {
    ApiKey::new("owned-vendor-secret".into()).unwrap()
}

fn generated_reply() -> RelayResponse {
    RelayResponse::Generated {
        plan: AiQueryPlan {
            schema_version: 1,
            intent: "wind".into(),
            queries: vec![AiQuery {
                text: "wind".into(),
                reason: "Core sound".into(),
                kind: AiQueryKind::Main,
                layer: String::new(),
                source_ids: Vec::new(),
                cat_ids: Vec::new(),
            }],
            cautions: Vec::new(),
        },
        input_tokens: 13,
        output_tokens: 7,
        decision: None,
    }
}

fn json_reply(reply: &RelayResponse) -> TestResponse {
    TestResponse::json(200, &serde_json::to_string(reply).unwrap())
}

#[test]
fn model_probe_and_generation_forward_only_through_the_server() {
    let direct = Arc::new(AtomicUsize::new(0));
    let direct_seen = direct.clone();
    let vendor = TestServer::start(move |_| {
        direct_seen.fetch_add(1, Ordering::AcqRel);
        TestResponse::json(500, "{}")
    });
    let mut profile = profile();
    profile.base_url = format!("{}/v1", vendor.url);
    let local_id = profile.id.clone();
    let local_key_ref = profile.credential_ref.clone();
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    let server = TestServer::start(move |request| {
        observed.fetch_add(1, Ordering::AcqRel);
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.header("authorization"),
            Some("Bearer owned-audiolib-token")
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert!(request.header(crate::LIBRARY_ID_HEADER).is_none());
        let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(value["api_key"], "owned-vendor-secret");
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert_eq!(value["profile"].as_object().unwrap().len(), 4);
        let text = String::from_utf8(request.body.clone()).unwrap();
        assert!(!text.contains(&local_id));
        assert!(!text.contains(&local_key_ref));
        let request_body: RelayRequest = serde_json::from_value(value).unwrap();
        assert!(!format!("{request_body:?}").contains("owned-vendor-secret"));
        match request_body.operation {
            RelayOperation::Models => {
                assert_eq!(request.path, "/prefix/api/v1/ai/models");
                json_reply(&RelayResponse::Models {
                    models: vec!["group/owned-model".into()],
                })
            }
            RelayOperation::Generate {
                intent, session, ..
            } => {
                assert_eq!(request.path, "/prefix/api/v1/ai/generate");
                assert_eq!(intent, "wind");
                assert!(matches!(session.as_str(), "owned-probe" | "owned-generate"));
                json_reply(&generated_reply())
            }
        }
    });
    let client = RemoteClient::new(
        &format!("{}/prefix/api/v1", server.url),
        "owned-audiolib-token",
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        client.ai_models(&profile, &key(), &cancel).unwrap(),
        ["group/owned-model"]
    );
    for session in ["owned-probe", "owned-generate"] {
        let output = client
            .ai_generate(
                &profile,
                &key(),
                "wind",
                false,
                DEFAULT_PROMPT,
                session,
                &cancel,
            )
            .unwrap();
        assert_eq!(output.plan.intent, "wind");
        assert_eq!(output.plan.queries[0].text, "wind");
        assert_eq!((output.input_tokens, output.output_tokens), (13, 7));
    }
    assert_eq!(seen.load(Ordering::Acquire), 3);
    assert_eq!(direct.load(Ordering::Acquire), 0);
}

#[test]
fn provider_failures_keep_their_type_through_a_successful_relay_response() {
    for provider_status in [401, 403, 429, 503] {
        let server = TestServer::start(move |_| {
            json_reply(&RelayResponse::from_models(Err(AiError::Http(
                provider_status,
            ))))
        });
        let client = RemoteClient::new(&server.url, "").unwrap();
        assert!(matches!(
            client.ai_models(&profile(), &key(), &AtomicBool::new(false)),
            Err(AiError::Http(code)) if code == provider_status
        ));
    }
    let server =
        TestServer::start(|_| json_reply(&RelayResponse::from_models(Err(AiError::Timeout(180)))));
    let client = RemoteClient::new(&server.url, "").unwrap();
    assert!(matches!(
        client.ai_models(&profile(), &key(), &AtomicBool::new(false)),
        Err(AiError::Timeout(180))
    ));
}

#[test]
fn lost_generation_response_is_not_retried() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let server = TestServer::start(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        json_reply(&generated_reply()).cut_after(0)
    });
    let client = RemoteClient::new(&server.url, "").unwrap();
    assert!(matches!(
        client.ai_generate(
            &profile(),
            &key(),
            "wind",
            false,
            DEFAULT_PROMPT,
            "owned-attempt",
            &AtomicBool::new(false),
        ),
        Err(AiError::RelayTransport)
    ));
    assert_eq!(attempts.load(Ordering::Acquire), 1);
}

struct DeadlineOverride;

impl DeadlineOverride {
    fn new(duration: Duration) -> Self {
        assert!(
            TEST_DEADLINE
                .with(|value| value.replace(Some(duration)))
                .is_none()
        );
        Self
    }
}

impl Drop for DeadlineOverride {
    fn drop(&mut self) {
        TEST_DEADLINE.with(|value| value.set(None));
    }
}

#[test]
fn relay_status_errors_are_distinct_redacted_and_never_retried() {
    for status in [401, 403, 404, 429, 503, 307] {
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let server = TestServer::start(move |request| {
            assert_eq!(request.path, "/api/v1/ai/models");
            observed.fetch_add(1, Ordering::AcqRel);
            TestResponse::json(status, r#"{"message":"owned-vendor-secret"}"#)
                .with_header("Location", "http://127.0.0.1:1/should-not-follow")
        });
        let client = RemoteClient::new(&server.url, "owned-audiolib-token").unwrap();
        let error = client
            .ai_models(&profile(), &key(), &AtomicBool::new(false))
            .unwrap_err();
        assert!(matches!(error, AiError::RelayHttp(code) if code == status));
        assert!(!error.to_string().contains("owned-vendor-secret"));
        assert!(!format!("{error:?}").contains("owned-vendor-secret"));
        assert_eq!(count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn relay_response_size_and_json_are_bounded() {
    for body in [vec![b' '; MAX_RELAY_BYTES as usize + 1], b"{}".to_vec()] {
        let server = TestServer::start(move |_| TestResponse::bytes(200, body.clone()));
        let client = RemoteClient::new(&server.url, "").unwrap();
        assert!(matches!(
            client.ai_models(&profile(), &key(), &AtomicBool::new(false)),
            Err(AiError::Response)
        ));
    }
}

#[test]
fn already_cancelled_relay_does_not_start_a_network_request() {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let server = TestServer::start(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        TestResponse::json(200, "{}")
    });
    let client = RemoteClient::new(&server.url, "").unwrap();
    assert!(matches!(
        client.ai_models(&profile(), &key(), &AtomicBool::new(true)),
        Err(AiError::Cancelled)
    ));
    assert_eq!(count.load(Ordering::Acquire), 0);
}

/// Read the whole POST, then stall in either the response head or body. The server
/// reports the peer closing, proving cancellation ends the connection itself.
fn stalled_server(body: bool) -> (String, Receiver<()>, Receiver<Instant>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (started, received) = mpsc::channel();
    let (closed, observed) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        reader.read_exact(&mut vec![0u8; length]).unwrap();
        if body {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{")
                .unwrap();
            stream.flush().unwrap();
        }
        started.send(()).unwrap();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = closed.send(Instant::now());
    });
    (url, received, observed)
}

#[test]
fn cancelling_relay_closes_connections_waiting_for_headers_and_body() {
    for body in [false, true] {
        let (url, started, closed) = stalled_server(body);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let cancellation = std::thread::spawn(move || {
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
            Instant::now()
        });
        let client = RemoteClient::new(&url, "").unwrap();
        assert!(matches!(
            client.ai_models(&profile(), &key(), &cancel),
            Err(AiError::Cancelled)
        ));
        let cancelled_at = cancellation.join().unwrap();
        let closed_at = closed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(closed_at.duration_since(cancelled_at) < Duration::from_secs(1));
    }
}

#[test]
fn relay_deadline_covers_slow_headers_and_stalled_body() {
    let _override = DeadlineOverride::new(Duration::from_millis(250));
    for body in [false, true] {
        let (url, _started, _closed) = stalled_server(body);
        let client = RemoteClient::new(&url, "").unwrap();
        let started_at = Instant::now();
        assert!(matches!(
            client.ai_models(&profile(), &key(), &AtomicBool::new(false)),
            Err(AiError::Timeout(180))
        ));
        assert!(started_at.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn response_body_uses_the_remaining_global_deadline() {
    let _override = DeadlineOverride::new(Duration::from_millis(350));
    let server = TestServer::start(|_| {
        // Each individual phase is below 350ms, but together they exceed it.
        // A response-head timeout followed by a fresh body timeout would pass.
        json_reply(&RelayResponse::Models {
            models: vec!["owned-model".into()],
        })
        .delayed(Duration::from_millis(200))
        .slow(24, Duration::from_millis(220))
    });
    let client = RemoteClient::new(&server.url, "").unwrap();
    assert!(matches!(
        client.ai_models(&profile(), &key(), &AtomicBool::new(false)),
        Err(AiError::Timeout(180))
    ));
}
