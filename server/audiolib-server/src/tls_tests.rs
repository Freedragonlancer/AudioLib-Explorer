// server/audiolib-server/src/tls_tests.rs
//! Real TLS renewal regression: partial replacements keep the old usable certificate,
//! complete renewal reaches new handshakes and preserves established connections.
use super::*;
use axum::routing::get;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsConnector, client::TlsStream};

async fn connect(
    port: u16,
    certificates: &[CertificateDer<'static>],
) -> TlsStream<tokio::net::TcpStream> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate.clone()).unwrap();
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    // Every assertion exercises a fresh handshake, not a resumed session.
    config.resumption = rustls::client::Resumption::disabled();
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap()
}

async fn request(stream: &mut TlsStream<tokio::net::TcpStream>) {
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(stream.read_u8().await.unwrap());
    }
    assert!(headers.starts_with(b"HTTP/1.1 200"), "{headers:?}");
    let mut body = [0; 2];
    stream.read_exact(&mut body).await.unwrap();
    assert_eq!(&body, b"ok");
}

#[tokio::test]
async fn renewed_pem_files_reload_without_disconnecting_existing_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let first = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let second = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let certificates = [first.cert.der().clone(), second.cert.der().clone()];
    let cert = directory.path().join("cert.pem");
    let key = directory.path().join("key.pem");
    fs::write(&cert, first.cert.pem()).unwrap();
    fs::write(&key, first.signing_key.serialize_pem()).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = crate::ServerConfig::from_toml_str(
        &format!("bind = '127.0.0.1:0'\nlibrary_db = 'unused.db'\ntls_cert_path = '{}'\ntls_key_path = '{}'\ntls_port = {port}", cert.display(), key.display()),
        directory.path(),
    ).unwrap();
    let security = Security::from_config(&config, None);
    let (stop, stopped) = watch::channel(false);
    let manager = spawn_manager_with_interval(
        security.clone(),
        Router::new().route("/", get(|| async { "ok" })),
        "127.0.0.1".parse().unwrap(),
        ServeOptions::default(),
        stopped,
        Duration::from_millis(20),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while security.tls_status().state != "listening" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut established = connect(port, &certificates).await;
    assert_eq!(
        established.get_ref().1.peer_certificates().unwrap()[0],
        certificates[0]
    );
    request(&mut established).await;

    // ACME clients may replace the certificate and key in separate operations.
    fs::write(&cert, second.cert.pem()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while security.tls_status().message.is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(security.tls_status().state, "listening");
    let mut during_replacement = connect(port, &certificates).await;
    assert_eq!(
        during_replacement.get_ref().1.peer_certificates().unwrap()[0],
        certificates[0]
    );
    request(&mut during_replacement).await;
    drop(during_replacement);

    fs::write(&key, second.signing_key.serialize_pem()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let renewed = connect(port, &certificates).await;
            if renewed.get_ref().1.peer_certificates().unwrap()[0] == certificates[1] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(security.tls_status().message.is_none());
    request(&mut established).await;
    drop(established);
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), manager)
        .await
        .unwrap()
        .unwrap();
}
