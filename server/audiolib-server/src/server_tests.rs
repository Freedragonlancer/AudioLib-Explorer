// server/audiolib-server/src/server_tests.rs
//! Connection admission limits include idle HTTP sockets and pending TLS handshakes, and a
//! peer that stops reading a response loses it after the write stall timeout.
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
        write_stall_timeout: Duration::from_secs(60),
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

/// An endless response body; the guard is dropped with the body.
struct Endless(#[allow(dead_code)] Arc<()>);

impl futures_core::Stream for Endless {
    type Item = Result<axum::body::Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(Ok(axum::body::Bytes::from_static(&[0; 64 * 1024]))))
    }
}

#[tokio::test]
async fn a_peer_that_stops_reading_releases_the_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let options = ServeOptions {
        max_connections: 4,
        header_read_timeout: Duration::from_secs(5),
        shutdown_grace: Duration::from_millis(100),
        write_stall_timeout: Duration::from_millis(300),
    };
    let guard = Arc::new(());
    let body_guard = guard.clone();
    let app = Router::new().route(
        "/",
        get(move || {
            let guard = body_guard.clone();
            async move { axum::body::Body::from_stream(Endless(guard)) }
        }),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(serve_inner(
        listener,
        app,
        None,
        async {
            let _ = stopped.await;
        },
        options,
    ));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    // The route keeps one clone; a third exists while the body is being sent. The
    // client never reads, so the socket buffers fill up and writes stall.
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&guard) < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        while Arc::strong_count(&guard) > 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stalled response body was kept");
    drop(client);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}
