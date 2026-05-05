//! Per-connection peer info + per-request auth context.
//! PeerInfo is injected by the custom accept loop (Task 12); tonic's
//! ConnectInfoLayer propagates it into request extensions via the
//! Connected trait impl on PeerAwareStream.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// RAII guard that decrements the active-connection counter when dropped.
///
/// Created when a connection is successfully handed to tonic; ensures that
/// `active_conns` is decremented even if tonic drops the stream mid-flight
/// without going through a normal shutdown path.
pub(crate) struct ActiveConnGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveConnGuard {
    pub(crate) fn new(counter: Arc<AtomicUsize>) -> Self {
        Self { counter }
    }
}

impl Drop for ActiveConnGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub remote_addr: SocketAddr,
    pub peer_cert_der: Option<Vec<u8>>,
    pub cert_subject_cn: Option<String>,
    pub tls_version: String,
}

/// Authenticated caller identity, inserted by AuthLayer into request extensions.
/// Task 11 (audit_bridge) reads this for the audit entry.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub auth_type: AuthType,
    pub client_id: String,
    pub jti: Option<String>,
    pub command_id: String, // ulid
}

#[derive(Debug, Clone, Copy)]
pub enum AuthType {
    Jwt,
    Mtls,
    JwtAndMtls,
}

impl AuthType {
    /// Stable string label used in metrics + audit `auth_type` fields.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthType::Jwt => "jwt",
            AuthType::Mtls => "mtls",
            AuthType::JwtAndMtls => "jwt+mtls",
        }
    }
}

/// Wraps a TLS stream + PeerInfo; implements Connected so tonic's
/// ConnectInfoLayer exposes PeerInfo in every request's extensions.
///
/// Optionally holds an `ActiveConnGuard` to decrement `active_conns` when
/// the stream is dropped (RAII — no manual decrement needed on success path).
pub(crate) struct PeerAwareStream<S> {
    inner: S,
    peer_info: PeerInfo,
    _active_conns_guard: Option<ActiveConnGuard>,
}

impl<S> PeerAwareStream<S> {
    pub fn new(inner: S, peer_info: PeerInfo, guard: Option<ActiveConnGuard>) -> Self {
        Self {
            inner,
            peer_info,
            _active_conns_guard: guard,
        }
    }
    // peer_info() getter omitted — tonic's ConnectInfoLayer reads PeerInfo via
    // the `Connected` trait impl below; no direct accessor is needed.
}

impl<S: AsyncRead + Unpin> AsyncRead for PeerAwareStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PeerAwareStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<S: Send + 'static> tonic::transport::server::Connected for PeerAwareStream<S> {
    type ConnectInfo = PeerInfo;
    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer_info.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
    use tonic::transport::server::Connected as _;

    fn mk_peer() -> PeerInfo {
        PeerInfo {
            remote_addr: "127.0.0.1:5001".parse().unwrap(),
            peer_cert_der: None,
            cert_subject_cn: None,
            tls_version: "TLSv1.3".into(),
        }
    }

    #[test]
    fn connect_info_returns_peer() {
        let (a, _b) = duplex(64);
        let stream = PeerAwareStream::new(a, mk_peer(), None);
        let info = stream.connect_info();
        assert_eq!(info.remote_addr.port(), 5001);
    }

    #[tokio::test]
    async fn async_read_delegates() {
        let (a, mut b) = duplex(64);
        let mut stream = PeerAwareStream::new(a, mk_peer(), None);
        b.write_all(b"hello").await.unwrap();
        drop(b);
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn async_write_delegates() {
        let (a, mut b) = duplex(64);
        let mut stream = PeerAwareStream::new(a, mk_peer(), None);
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut buf = Vec::new();
        b.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
    }

    /// Verify that dropping a `PeerAwareStream` with a guard decrements the counter.
    #[test]
    fn active_conn_guard_decrements_on_stream_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicUsize::new(100));
        let (a, _b) = duplex(64);
        let guard = ActiveConnGuard::new(counter.clone());
        let stream = PeerAwareStream::new(a, mk_peer(), Some(guard));
        drop(stream);
        assert_eq!(
            counter.load(Ordering::Relaxed),
            99,
            "guard must decrement on drop"
        );
    }

    /// 100 accepted+closed connections must leave active_conns at 0.
    #[test]
    fn active_conn_guard_100_accepts_leave_zero() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..100 {
            counter.fetch_add(1, Ordering::Relaxed);
            let (a, _b) = duplex(64);
            let guard = ActiveConnGuard::new(counter.clone());
            let stream = PeerAwareStream::new(a, mk_peer(), Some(guard));
            drop(stream);
        }
        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "100 accepts+closes must leave active_conns=0"
        );
    }

    #[test]
    fn auth_context_clone_works() {
        let ctx = AuthContext {
            auth_type: AuthType::Jwt,
            client_id: "u1".into(),
            jti: Some("j1".into()),
            command_id: "01HXYZ".into(),
        };
        let cloned = ctx.clone();
        assert_eq!(ctx.client_id, cloned.client_id);
    }
}
