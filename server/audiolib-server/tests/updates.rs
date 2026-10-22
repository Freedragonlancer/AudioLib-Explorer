// server/audiolib-server/tests/updates.rs
//! Update administration stays behind the existing LAN, origin and marker guard.
use audiolib_server::{ConnectionInfo, Listener, Service, router};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn request(method: Method, path: &str, peer: &str, marked: bool) -> axum::response::Response {
    request_with_origin(method, path, peer, marked, None).await
}

async fn request_with_origin(
    method: Method,
    path: &str,
    peer: &str,
    marked: bool,
    origin: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "192.168.1.5:8790");
    if marked {
        request = request.header("X-AudioLib-Admin", "1");
    }
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectionInfo {
        peer: peer.parse().unwrap(),
        listener: Listener::Http,
        local: Some("192.168.1.5:8790".parse().unwrap()),
    });
    router(Service::starting(None))
        .oneshot(request)
        .await
        .unwrap()
}

#[tokio::test]
async fn public_peers_cannot_read_or_trigger_server_updates() {
    for (method, path) in [
        (Method::GET, "/admin/api/update"),
        (Method::POST, "/admin/api/update/check"),
        (Method::POST, "/admin/api/update/install"),
    ] {
        assert_eq!(
            request(method, path, "203.0.113.8:4000", true)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn update_changes_require_admin_marker_and_disabled_process_does_not_check_network() {
    for path in ["/admin/api/update/check", "/admin/api/update/install"] {
        assert_eq!(
            request(Method::POST, path, "192.168.1.2:4000", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request_with_origin(
                Method::POST,
                path,
                "192.168.1.2:4000",
                true,
                Some("http://evil.example")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = request(Method::GET, "/admin/api/update", "192.168.1.2:4000", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status["state"], "disabled");
    assert_eq!(
        request(
            Method::POST,
            "/admin/api/update/check",
            "192.168.1.2:4000",
            true
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
