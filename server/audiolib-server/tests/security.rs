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
    let mut request =
        support::modern(Request::get("/api/v1/capabilities")).header(header::HOST, host);
    if token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    let mut request = request.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: peer.parse().unwrap(),
        listener: Listener::Http,
    });
    router(service.clone()).oneshot(request).await.unwrap()
}

#[tokio::test]
async fn token_free_api_rejects_rebinding_hosts_and_accepts_explicit_server_hosts() {
    for mode in ["external", "off"] {
        let (_directory, security, service) = fixture(&format!(
            "auth_mode = '{mode}'\npublic_url = 'https://audio.example.com:8443'\n"
        ));
        security
            .update_settings(|settings| {
                settings.lan_url = Some("http://private.example.com:8790".into())
            })
            .unwrap();
        for host in ["evil.example.com:8790", "localhost:bad", "[::1]garbage"] {
            let response = request(&service, host, false, "192.168.1.2:4000").await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{mode} {host}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error_code"], "HOST_FORBIDDEN");
        }
        for host in [
            "192.168.1.10:8790",
            "[::1]:8790",
            "nas.lan:8790",
            "audio.example.com:8443",
            "private.example.com:8790",
        ] {
            assert_eq!(
                request(&service, host, false, "192.168.1.2:4000")
                    .await
                    .status(),
                StatusCode::OK,
                "{mode} {host}"
            );
        }
        assert_eq!(
            request(&service, "evil.example.com:8790", true, "192.168.1.2:4000")
                .await
                .status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn duplicate_and_missing_hosts_cannot_bypass_token_free_admission() {
    let (_directory, _security, service) = fixture("");
    for hosts in [vec![], vec!["localhost", "evil.example.com"]] {
        let mut request = support::modern(Request::get("/api/v1/capabilities"));
        for host in hosts {
            request = request.header(header::HOST, host);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectionInfo {
            peer: "192.168.1.2:4000".parse().unwrap(),
            listener: Listener::Http,
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
                })
            )
            .unwrap_err()
            .code(),
        "MANAGE_FORBIDDEN"
    );
}

#[tokio::test]
async fn failed_authentication_is_limited_by_socket_ip_not_forwarding_headers() {
    for (mode, expected) in [
        ("all", StatusCode::UNAUTHORIZED),
        ("external", StatusCode::FORBIDDEN),
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
                peer: "192.168.1.2:4000".parse().unwrap(),
                listener: Listener::Http,
            });
            assert_eq!(
                router(service.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
        let limited = request(&service, host, true, "192.168.1.2:5000").await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()[header::RETRY_AFTER], "60");
        assert_eq!(
            request(&service, "localhost", true, "192.168.1.3:5000")
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
