// server/audiolib-server/src/server_tests.rs
//! Connection admission limits include idle HTTP sockets and pending TLS handshakes.
use super::*;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn connection_limit(tls: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let options = ServeOptions {
        max_connections: 1,
        header_read_timeout: Duration::from_secs(5),
        shutdown_grace: Duration::from_millis(100),
    };
    let directory = tempfile::tempdir().unwrap();
    let acceptor = tls.then(|| {
        let certificate =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = directory.path().join("cert.pem");
        let key = directory.path().join("key.pem");
        std::fs::write(&cert, certificate.cert.pem()).unwrap();
        std::fs::write(&key, certificate.signing_key.serialize_pem()).unwrap();
        TlsAcceptor::from(crate::tls::load_server_config(&cert, &key).unwrap())
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let app = Router::new().route("/", get(|| async { "ok" }));
    let task = tokio::spawn(serve_inner(
        listener,
        app,
        acceptor,
        async {
            let _ = stopped.await;
        },
        options,
    ));
    let mut admitted = tokio::net::TcpStream::connect(address).await.unwrap();
    if !tls {
        admitted.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    }
    // Allow the first socket to enter HTTP header parsing or TLS handshake handling.
    tokio::time::sleep(Duration::from_millis(40)).await;
    let mut excess = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut byte = [0; 1];
    let read = tokio::time::timeout(Duration::from_secs(2), excess.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "Excess connection was kept open: {read:?}"
    );
    drop(excess);
    drop(admitted);

    // Releasing a connection returns its slot, so subsequent clients are not locked out.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut replacement = tokio::net::TcpStream::connect(address).await.unwrap();
            if !tls {
                let _ = replacement
                    .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .await;
                let mut response = Vec::new();
                let _ = replacement.read_to_end(&mut response).await;
                if response.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            } else {
                // An admitted, incomplete TLS handshake stays open instead of closing.
                if tokio::time::timeout(Duration::from_millis(40), replacement.read(&mut byte))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn idle_http_connections_cannot_exceed_the_listener_limit() {
    connection_limit(false).await;
}

#[tokio::test]
async fn incomplete_tls_handshakes_cannot_exceed_the_listener_limit() {
    connection_limit(true).await;
}
