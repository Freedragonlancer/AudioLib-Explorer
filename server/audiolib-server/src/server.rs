// server/audiolib-server/src/server.rs
//! HTTP/1.1 accept loops (plain and TLS) with a header read timeout and a bounded graceful
//! shutdown.
//!
//! `axum::serve` offers neither, so connections are served with hyper's HTTP/1 builder
//! directly: each connection must deliver its request headers within
//! [`ServeOptions::header_read_timeout`] (a TLS handshake gets the same limit), and on
//! shutdown in-flight requests get at most [`ServeOptions::shutdown_grace`] before the
//! remaining connections are dropped. Every request carries a
//! [`ConnectionInfo`](crate::auth::ConnectionInfo) extension (peer address and listener)
//! used by authentication and the admin guard.
use crate::auth::{ConnectionInfo, Listener};
use axum::{Router, extract::Request};
use hyper::{body::Incoming, server::conn::http1};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{future::Future, pin::pin, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::JoinSet,
};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

/// Connection-level limits for [`serve`].
#[derive(Clone, Copy, Debug)]
pub struct ServeOptions {
    /// Maximum open connections per listener, including incomplete TLS handshakes.
    /// Excess connections are closed immediately; zero is treated as one.
    pub max_connections: usize,
    /// Maximum time for a client to send complete request headers (also bounds idle
    /// keep-alive connections waiting for their next request, and TLS handshakes).
    pub header_read_timeout: Duration,
    /// Maximum wait for in-flight requests after shutdown is requested.
    pub shutdown_grace: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            header_read_timeout: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

/// Serve `app` on `listener` until `shutdown` completes, then stop accepting, let open
/// connections finish their current request for up to `options.shutdown_grace`, and
/// drop whatever is still running.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
    options: ServeOptions,
) {
    serve_inner(listener, app, None, shutdown, options).await;
}

/// [`serve`] over TLS: each accepted connection first completes a TLS handshake with
/// `acceptor` (within `options.header_read_timeout`); requests are marked as coming from
/// the TLS listener.
pub async fn serve_tls(
    listener: TcpListener,
    app: Router,
    acceptor: TlsAcceptor,
    shutdown: impl Future<Output = ()>,
    options: ServeOptions,
) {
    serve_inner(listener, app, Some(acceptor), shutdown, options).await;
}

async fn serve_inner(
    listener: TcpListener,
    app: Router,
    tls: Option<TlsAcceptor>,
    shutdown: impl Future<Output = ()>,
    options: ServeOptions,
) {
    let (close_tx, close_rx) = watch::channel(());
    let slots = Arc::new(Semaphore::new(options.max_connections.max(1)));
    let mut connections = JoinSet::new();
    let mut shutdown = pin!(shutdown);
    loop {
        // Reap completed tasks even under sustained accept pressure; their results
        // must not accumulate after their connection permits have been returned.
        while connections.try_join_next().is_some() {}
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        // E.g. EMFILE: back off briefly instead of spinning.
                        tracing::warn!(%error, "Cannot accept connection");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    tracing::debug!(%peer, "Connection limit reached");
                    drop(stream);
                    continue;
                };
                let _ = stream.set_nodelay(true);
                let info = ConnectionInfo {
                    peer,
                    listener: if tls.is_some() { Listener::Tls } else { Listener::Http },
                };
                let service = TowerToHyperService::new(app.clone().map_request(
                    move |mut request: Request<Incoming>| {
                        request.extensions_mut().insert(info);
                        request
                    },
                ));
                let close = close_rx.clone();
                let tls = tls.clone();
                connections.spawn(async move {
                    // Retain the slot across the handshake and every keep-alive request.
                    let _permit = permit;
                    match tls {
                        None => serve_connection(TokioIo::new(stream), service, options, close).await,
                        Some(acceptor) => {
                            let handshake = tokio::time::timeout(
                                options.header_read_timeout,
                                acceptor.accept(stream),
                            )
                            .await;
                            match handshake {
                                Ok(Ok(stream)) => {
                                    serve_connection(TokioIo::new(stream), service, options, close)
                                        .await;
                                }
                                Ok(Err(error)) => {
                                    tracing::debug!(%peer, %error, "TLS handshake failed");
                                }
                                Err(_) => tracing::debug!(%peer, "TLS handshake timed out"),
                            }
                        }
                    }
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    let _ = close_tx.send(());
    let deadline = tokio::time::sleep(options.shutdown_grace);
    let mut deadline = pin!(deadline);
    loop {
        tokio::select! {
            next = connections.join_next() => {
                if next.is_none() {
                    break;
                }
            }
            () = &mut deadline => {
                tracing::warn!(
                    connections = connections.len(),
                    grace_secs = options.shutdown_grace.as_secs_f64(),
                    "Shutdown grace period elapsed; dropping remaining connections"
                );
                connections.abort_all();
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

async fn serve_connection<I, S>(
    io: I,
    service: S,
    options: ServeOptions,
    mut close: watch::Receiver<()>,
) where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    S: hyper::service::HttpService<Incoming> + Send,
    S::Future: Send + 'static,
    S::ResBody: Send + 'static,
    <S::ResBody as hyper::body::Body>::Data: Send,
    <S::ResBody as hyper::body::Body>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(options.header_read_timeout);
    let mut connection = pin!(builder.serve_connection(io, service));
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(error) = result {
                tracing::debug!(%error, "Connection ended with an error");
            }
        }
        _ = close.changed() => {
            connection.as_mut().graceful_shutdown();
            if let Err(error) = connection.await {
                tracing::debug!(%error, "Connection ended with an error");
            }
        }
    }
}
