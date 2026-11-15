// server/audiolib-server/tests/security.rs
//! Security admission regressions; all state is confined to an owned temporary directory.
mod support;

use audiolib_server::{
    BearerToken, ConnectionInfo, Listener, Security, ServerConfig, Service, router,
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "security-fixture-0123456789abcdef0123456789abcdef";

#[test]
fn generated_tokens_initialize_a_missing_store_before_the_first_create() {
    let directory = tempfile::tempdir().unwrap();
    let token_file = directory.path().join("tokens.json");
    let config = ServerConfig::from_toml_str("library_db='unused.db'\n", directory.path()).unwrap();
    let security = Security::from_config(&config, None);
    assert!(!token_file.exists());
    assert!(security.current_tokens().unwrap().is_empty());
    assert!(
        !token_file.exists(),
        "reading an empty store must not create token data"
    );
    let (token, secret) = security.create_token("first generated token").unwrap();
    assert!(security.verify(&secret));
    let persisted = std::fs::read_to_string(&token_file).unwrap();
    assert!(
        !persisted.contains(&secret),
        "plaintext token must not be persisted"
    );
    let persisted: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(persisted["version"], 1);
    assert_eq!(persisted["tokens"][0]["id"], token.id);
    let fresh = Security::from_config(&config, None);
    assert!(fresh.verify(&secret));
    assert_eq!(fresh.current_tokens().unwrap().len(), 1);
    assert!(!directory.path().join("unused.db").exists());
}

#[test]
fn generated_token_revocation_is_current_across_instances_and_never_resurrected() {
    let directory = tempfile::tempdir().unwrap();
    let config = ServerConfig::from_toml_str("library_db='unused.db'\n", directory.path()).unwrap();
    let first = Security::from_config(&config, None);
    let (old, secret) = first.create_token("revoke across instances").unwrap();
    let second = Security::from_config(&config, None);
    assert!(first.revoke_token(&old.id).unwrap());
    assert!(!first.verify(&secret));
    assert!(
        !second.verify(&secret),
        "a stale instance must observe revocation before touch"
    );
    let (_, other) = second.create_token("preserve this newer token").unwrap();
    assert!(
        first.verify(&other),
        "an already running instance must load a newly created token"
    );
    assert!(!first.verify(&secret));
    let fresh = Security::from_config(&config, None);
    assert!(
        !fresh.verify(&secret),
        "bookkeeping must not resurrect a revoked token on disk"
    );
    assert!(fresh.verify(&other));
    assert_eq!(fresh.current_tokens().unwrap().len(), 1);
    assert!(!directory.path().join("unused.db").exists());
}

#[test]
#[ignore = "owned subprocess entry for concurrent_token_creators_keep_both_tokens"]
fn token_creation_child() {
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };
    let Some(directory) = std::env::var_os("AUDIOLIB_TOKEN_TEST_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let label = std::env::var("AUDIOLIB_TOKEN_TEST_LABEL").unwrap();
    assert!(["first", "second"].contains(&label.as_str()));
    assert_eq!(
        fs::read(directory.join("owned-fixture")).unwrap(),
        b"audiolib-token-test-v1"
    );
    let config = ServerConfig::from_toml_str("library_db='unused.db'\n", &directory).unwrap();
    let security = Security::from_config(&config, None);
    fs::write(directory.join(format!("ready-{label}")), b"ready").unwrap();
    let start = Instant::now();
    while !directory.join("start").exists() {
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(10));
    }
    let (_, secret) = security.create_token(&label).unwrap();
    assert!(security.verify(&secret));
}

#[test]
fn concurrent_token_creators_keep_both_tokens() {
    use std::{
        fs,
        process::Command,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("owned-fixture"),
        b"audiolib-token-test-v1",
    )
    .unwrap();
    let mut children = ["first", "second"].map(|label| {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "token_creation_child",
                "--ignored",
                "--nocapture",
            ])
            .env("AUDIOLIB_TOKEN_TEST_DIRECTORY", directory.path())
            .env("AUDIOLIB_TOKEN_TEST_LABEL", label)
            .spawn()
            .unwrap()
    });
    let started = Instant::now();
    while !["first", "second"]
        .iter()
        .all(|label| directory.path().join(format!("ready-{label}")).exists())
    {
        if started.elapsed() > Duration::from_secs(20) {
            // The children have their own bounded wait and are reaped before the
            // owned fixture disappears, including this failure path.
            for child in &mut children {
                let _ = child.wait();
            }
            panic!("token creator subprocesses did not reach the ready barrier");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(directory.path().join("start"), b"start").unwrap();
    let results: Vec<_> = children
        .iter_mut()
        .map(|child| child.wait().unwrap())
        .collect();
    assert!(
        results.iter().all(std::process::ExitStatus::success),
        "{results:?}"
    );
    let config = ServerConfig::from_toml_str("library_db='unused.db'\n", directory.path()).unwrap();
    let security = Security::from_config(&config, None);
    let mut names: Vec<_> = security
        .current_tokens()
        .unwrap()
        .into_iter()
        .map(|token| token.name)
        .collect();
    names.sort();
    assert_eq!(names, ["first", "second"]);
}

fn fixture(extra: &str) -> (tempfile::TempDir, Arc<Security>, Service) {
    let directory = tempfile::tempdir().unwrap();
    let config = ServerConfig::from_toml_str(
        &format!("library_db = 'library.db'\n{extra}"),
        directory.path(),
    )
    .unwrap();
    let security = Security::from_config(&config, Some(BearerToken::new(TOKEN).unwrap()));
    let service = Service::starting_with(security.clone());
    (directory, security, service)
}

async fn request(service: &Service, host: &str, token: bool, peer: &str) -> Response {
    request_on(service, host, token, peer, None).await
}

async fn request_on(
    service: &Service,
    host: &str,
    token: bool,
    peer: &str,
    local: Option<&str>,
) -> Response {
    let mut request =
        support::modern(Request::get("/api/v1/capabilities")).header(header::HOST, host);
    if token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    let mut request = request.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: peer.parse().unwrap(),
        listener: Listener::Http,
        local: local.map(|local| local.parse().unwrap()),
    });
    router(service.clone()).oneshot(request).await.unwrap()
}

#[tokio::test]
async fn same_host_peers_are_external_even_without_forwarding_headers() {
    // A reverse proxy on this host (e.g. Lucky) connects from loopback or from the
    // host's own LAN IP, so those peers must present a token like public traffic.
    let (_directory, _security, service) = fixture("auth_mode = 'external'\n");
    let same_host = [
        ("127.0.0.1:4000", None),
        ("127.0.0.1:4000", Some("127.0.0.1:8790")),
        ("[::1]:4000", Some("[::1]:8790")),
        ("[::ffff:127.0.0.1]:4000", None),
        ("192.168.1.5:4000", Some("192.168.1.5:8790")),
        ("[::ffff:192.168.1.5]:4000", Some("192.168.1.5:8790")),
    ];
    for (peer, local) in same_host {
        for host in ["localhost:8790", "192.168.1.5:8790", "audio.example.com"] {
            assert_eq!(
                request_on(&service, host, false, peer, local)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "{peer} {local:?} {host}"
            );
            assert_eq!(
                request_on(&service, host, true, peer, local).await.status(),
                StatusCode::OK,
                "{peer} {local:?} {host}"
            );
        }
    }
    // Another LAN machine reaching the same local address needs no token.
    assert_eq!(
        request_on(
            &service,
            "audio.example.com",
            false,
            "192.168.1.2:4000",
            Some("192.168.1.5:8790"),
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn token_free_off_mode_rejects_rebinding_hosts_from_non_lan_peers() {
    let (_directory, security, service) =
        fixture("auth_mode = 'off'\npublic_url = 'https://audio.example.com:8443'\n");
    security
        .update_settings(|settings| {
            settings.lan_url = Some("http://private.example.com:8790".into())
        })
        .unwrap();
    for host in ["evil.example.com:8790", "localhost:bad", "[::1]garbage"] {
        let response = request(&service, host, false, "203.0.113.7:4000").await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error_code"], "HOST_FORBIDDEN");
    }
    for host in [
        "192.0.2.10:8790",
        "[::1]:8790",
        "nas.lan:8790",
        "audio.example.com:8443",
        "private.example.com:8790",
    ] {
        assert_eq!(
            request(&service, host, false, "203.0.113.7:4000")
                .await
                .status(),
            StatusCode::OK,
            "{host}"
        );
    }
    assert_eq!(
        request(&service, "evil.example.com:8790", true, "203.0.113.7:4000")
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn lan_peers_need_no_token_whatever_the_host_header() {
    for mode in ["external", "off"] {
        let (_directory, _security, service) = fixture(&format!("auth_mode = '{mode}'\n"));
        for peer in [
            "192.168.1.2:4000",
            "10.0.0.5:4000",
            "[fe80::1]:4000",
            "[fd00::2]:4000",
            "[::ffff:192.168.1.2]:4000",
        ] {
            for host in [
                "audio.example.com:8443",
                "evil.example.com",
                "192.0.2.10:8790",
                "macnas.local:8790",
                "localhost:bad",
            ] {
                assert_eq!(
                    request(&service, host, false, peer).await.status(),
                    StatusCode::OK,
                    "{mode} {peer} {host}"
                );
            }
        }
    }
}

#[tokio::test]
async fn non_lan_or_forwarded_peers_still_need_a_token_in_external_mode() {
    let (_directory, _security, service) =
        fixture("auth_mode = 'external'\npublic_url = 'https://audio.example.com:8443'\n");
    for host in [
        "audio.example.com:8443",
        "192.0.2.10:8790",
        "evil.example.com",
    ] {
        assert_eq!(
            request(&service, host, false, "203.0.113.7:4000")
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "{host}"
        );
        assert_eq!(
            request(&service, host, true, "203.0.113.7:4000")
                .await
                .status(),
            StatusCode::OK,
            "{host}"
        );
    }
    // A loopback reverse proxy that marks the request as forwarded is external.
    let mut request = support::modern(Request::get("/api/v1/capabilities"))
        .header(header::HOST, "audio.example.com:8443")
        .header("x-forwarded-for", "203.0.113.7")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: "127.0.0.1:4000".parse().unwrap(),
        listener: Listener::Http,
        local: None,
    });
    assert_eq!(
        router(service).oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn duplicate_and_missing_hosts_cannot_bypass_token_free_admission() {
    let (_directory, _security, service) = fixture("auth_mode = 'off'\n");
    for hosts in [vec![], vec!["localhost", "evil.example.com"]] {
        let mut request = support::modern(Request::get("/api/v1/capabilities"));
        for host in hosts {
            request = request.header(header::HOST, host);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectionInfo {
            peer: "203.0.113.7:4000".parse().unwrap(),
            listener: Listener::Http,
            local: None,
        });
        assert_eq!(
            router(service.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn disabling_lan_trust_requires_tokens_and_closes_the_password_free_admin() {
    let (_directory, security, service) = fixture("trust_lan_peers = false\n");
    assert_eq!(
        request(&service, "localhost", false, "172.18.0.1:4000")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&service, "localhost", true, "172.18.0.1:4000")
            .await
            .status(),
        StatusCode::OK
    );
    let mut request = Request::get("/admin/api/status")
        .header(header::HOST, "localhost")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: "172.18.0.1:4000".parse().unwrap(),
        listener: Listener::Http,
        local: None,
    });
    assert_eq!(
        router(service).oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::HOST, "localhost".parse().unwrap());
    assert_eq!(
        security
            .authorize_manage(
                &headers,
                Some(&ConnectionInfo {
                    peer: "172.18.0.1:4000".parse().unwrap(),
                    listener: Listener::Http,
                    local: None,
                })
            )
            .unwrap_err()
            .code(),
        "MANAGE_FORBIDDEN"
    );
}

#[tokio::test]
async fn failed_authentication_is_limited_by_socket_ip_not_forwarding_headers() {
    // `external` uses a non-LAN socket peer: direct LAN peers need no token there.
    for (mode, peer, other) in [
        ("all", "192.168.1.2", "192.168.1.3"),
        ("external", "198.51.100.2", "198.51.100.3"),
    ] {
        let host = "untrusted.example.com";
        let (_directory, _security, service) = fixture(&format!("auth_mode = '{mode}'\n"));
        for index in 0..60 {
            let mut request = support::modern(Request::get("/api/v1/capabilities"))
                .header(header::HOST, host)
                .header(
                    if mode == "all" {
                        "x-forwarded-for"
                    } else {
                        "x-request-source"
                    },
                    format!("203.0.113.{index}"),
                )
                .header(header::AUTHORIZATION, "Bearer wrong")
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(ConnectionInfo {
                peer: format!("{peer}:4000").parse().unwrap(),
                listener: Listener::Http,
                local: None,
            });
            assert_eq!(
                router(service.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let limited = request(&service, host, true, &format!("{peer}:5000")).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()[header::RETRY_AFTER], "60");
        assert_eq!(
            request(&service, "localhost", true, &format!("{other}:5000"))
                .await
                .status(),
            StatusCode::OK
        );
    }
}

#[test]
fn management_defaults_reject_external_tokens_but_explicit_all_is_preserved() {
    let (_directory, security, _service) = fixture("");
    let headers = axum::http::HeaderMap::new();
    let peer = ConnectionInfo {
        peer: "203.0.113.1:4000".parse().unwrap(),
        listener: Listener::Http,
        local: None,
    };
    assert_eq!(
        security
            .authorize_manage(&headers, Some(&peer))
            .unwrap_err()
            .code(),
        "MANAGE_FORBIDDEN"
    );
    security
        .update_settings(|settings| settings.manage_mode = Some(audiolib_server::ManageMode::All))
        .unwrap();
    security.authorize_manage(&headers, Some(&peer)).unwrap();
}

/// Raw HTTP/1.1 exchange against a real listener; returns the lowercase header block.
async fn raw_reply(address: std::net::SocketAddr, method: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{}: {}\r\nConnection: close\r\n\r\n",
        audiolib_core::compatibility::ACCEPT_FORMATS_HEADER,
        audiolib_core::compatibility::PLAN_B_FORMAT,
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.unwrap();
    String::from_utf8_lossy(&reply).into_owned()
}

fn content_length(reply: &str) -> Option<usize> {
    reply.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().unwrap())
    })
}

#[tokio::test]
async fn head_401_has_the_same_headers_as_get_over_a_real_listener() {
    for mode in ["all", "external"] {
        head_matches_get(mode).await;
    }
}

async fn head_matches_get(mode: &str) {
    let (_directory, _security, service) = fixture(&format!("auth_mode = '{mode}'\n"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(audiolib_server::serve(
        listener,
        router(service.clone()),
        async {
            let _ = stopped.await;
        },
        audiolib_server::ServeOptions::default(),
    ));
    for path in ["/api/v1/capabilities", "/api/v1/library/snapshot"] {
        let get = raw_reply(address, "GET", path).await;
        let head = raw_reply(address, "HEAD", path).await;
        assert!(get.starts_with("HTTP/1.1 401"), "{get}");
        assert!(head.starts_with("HTTP/1.1 401"), "{head}");
        let (get_head, get_body) = get.split_once("\r\n\r\n").unwrap();
        let (head_head, head_body) = head.split_once("\r\n\r\n").unwrap();
        assert!(head_body.is_empty(), "HEAD carries no body: {head_body:?}");
        let length = content_length(get_head).expect("GET 401 has Content-Length");
        assert_eq!(length, get_body.len());
        assert_eq!(content_length(head_head), Some(length), "{head_head}");
        let names = |block: &str| {
            let mut names: Vec<String> = block
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':').map(|(n, _)| n.to_ascii_lowercase()))
                .filter(|name| name != "date")
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(get_head), names(head_head));
        // Loopback is never a trusted LAN peer; the message must say so, not "outside the LAN".
        let expected = if mode == "external" {
            "A bearer token is required: this request is not from a trusted direct LAN peer"
        } else {
            "A valid bearer token is required"
        };
        let body: serde_json::Value = serde_json::from_str(get_body).unwrap();
        assert_eq!(body["message"], expected, "{get_body}");
    }
    let _ = stop.send(());
    task.await.unwrap();
}
