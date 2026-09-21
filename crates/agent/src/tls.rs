//! Serving the agent's API over TLS.
//!
//! A reverse proxy stays the recommendation for anything with a domain name —
//! Caddy and Traefik renew certificates, and the agent does not want to. What
//! this is for is the case that has no proxy and no public name: a NAS or a
//! home server whose operator should still not be putting a filesystem
//! inventory on the wire in plaintext. The certificate is supplied by the
//! operator; the agent neither generates nor renews one.
//!
//! **Handshakes happen off the accept path.** `axum::serve::Listener::accept`
//! is awaited one connection at a time, so performing the handshake inside it
//! serialises every new connection behind the slowest one: a peer that opens a
//! socket and never sends a ClientHello would block all other clients until
//! its timeout expired. So the TCP accept loop and the handshakes run in their
//! own tasks and only finished streams are queued for axum to pick up. This is
//! also why `accept` here cannot fail — by then the handshake is already done.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::{server::TlsStream, TlsAcceptor};

/// HTTP/1.1 only, because that is the only protocol this server speaks: axum
/// is built here without its `http2` feature. Advertising `h2` would let a
/// client negotiate a protocol the connection then cannot serve.
const ALPN_HTTP_1_1: &[u8] = b"http/1.1";

/// How long one connection may take to complete its handshake.
///
/// Bounded because a socket that never finishes otherwise holds a task and a
/// file descriptor for as long as the peer cares to keep it open, and this
/// listener is reachable by anyone who can reach the port.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many completed handshakes may wait to be handed to axum.
///
/// Deliberately small. The channel is not a buffer for load; it exists only to
/// decouple concurrent handshakes from an `accept` that yields one connection
/// at a time. Once it is full the acceptor stops taking new sockets, which is
/// the backpressure we want rather than an unbounded queue of live TLS
/// sessions.
const QUEUE_DEPTH: usize = 64;

/// Build the TLS configuration from an operator-supplied certificate and key.
///
/// The certificate file is a PEM chain, leaf first, as TLS requires; a
/// self-signed certificate is a chain of one and is the expected case here.
pub fn server_config(cert_file: &Path, key_file: &Path) -> Result<Arc<rustls::ServerConfig>> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_file)
        .with_context(|| format!("opening TLS certificate {}", cert_file.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("reading TLS certificate {}", cert_file.display()))?;
    // An empty but readable PEM file is the likeliest operator mistake here
    // (a truncated copy, a wrong path that happens to exist), and rustls's own
    // error for it does not say which file was empty.
    anyhow::ensure!(
        !certs.is_empty(),
        "{} contains no certificate; it must hold a PEM chain, leaf first",
        cert_file.display()
    );

    let key = PrivateKeyDer::from_pem_file(key_file)
        .with_context(|| format!("reading TLS private key {}", key_file.display()))?;

    // The provider is named rather than left to `ServerConfig::builder`, which
    // resolves it from whichever provider features happen to be compiled in
    // and panics if that is ambiguous. A second provider arriving through some
    // future dependency would turn this into a crash at startup instead of a
    // build error, and startup is the worst place to find out.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("selecting TLS protocol versions")?
        .with_no_client_auth()
        // Fails when the key does not match the certificate, which is worth
        // saying plainly: it is the mistake an operator makes when copying two
        // files from different places.
        .with_single_cert(certs, key)
        .with_context(|| {
            format!(
                "the key in {} does not match the certificate in {}",
                key_file.display(),
                cert_file.display()
            )
        })?;
    config.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    Ok(Arc::new(config))
}

/// A listener that yields connections whose handshake has already completed.
pub struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local_addr: SocketAddr,
}

impl TlsListener {
    /// Take ownership of a bound TCP listener and start accepting on it.
    ///
    /// The address is captured now rather than asked for later, because
    /// `local_addr` on the trait cannot fail usefully once the socket has been
    /// moved into the acceptor task.
    pub fn spawn(listener: TcpListener, tls: Arc<rustls::ServerConfig>) -> Result<Self> {
        let local_addr = listener
            .local_addr()
            .context("reading the listener's local address")?;
        let (tx, ready) = mpsc::channel(QUEUE_DEPTH);
        tokio::spawn(accept_loop(listener, TlsAcceptor::from(tls), tx));
        Ok(TlsListener { ready, local_addr })
    }
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ready: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            // One failed accept is not a reason to stop serving: a peer that
            // hung up mid-handshake, or a momentary descriptor shortage, would
            // otherwise take the agent's API down until someone restarted it.
            Err(err) => {
                eprintln!("tls: accept failed: {err}");
                continue;
            }
        };
        // Set before the handshake rather than on the finished TLS stream, so
        // that the handshake round trips are not waiting on Nagle either.
        let _ = stream.set_nodelay(true);

        let acceptor = acceptor.clone();
        let ready = ready.clone();
        tokio::spawn(async move {
            let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream));
            let stream = match handshake.await {
                Ok(Ok(stream)) => stream,
                // A failed or abandoned handshake is the peer's problem, and
                // it is the ordinary shape of a port scan or of a client that
                // does not trust our certificate. Not worth a line each.
                Ok(Err(_)) | Err(_) => return,
            };
            // The receiver is gone only when the server is shutting down.
            let _ = ready.send((stream, peer)).await;
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(pair) => pair,
            // The acceptor task ended, which it only does if it was dropped.
            // The trait gives `accept` no way to report that, and returning
            // anything would mean inventing a connection, so park: graceful
            // shutdown is already racing this future and will win.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `server_config` is the one place an operator's mistake turns into a
    /// message they have to act on, so the messages are asserted, not just the
    /// failure.
    #[test]
    fn a_missing_certificate_file_names_the_path() {
        let err = server_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
        )
        .expect_err("a missing certificate must fail");
        let text = format!("{err:#}");
        assert!(
            text.contains("/nonexistent/cert.pem"),
            "the error must name the file, got: {text}"
        );
    }

    #[test]
    fn an_empty_certificate_file_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        std::fs::write(&cert, "").unwrap();
        std::fs::write(&key, "").unwrap();

        let err = server_config(&cert, &key).expect_err("an empty chain must fail");
        let text = format!("{err:#}");
        assert!(
            text.contains("no certificate"),
            "the error must say the chain is empty, got: {text}"
        );
    }

    #[test]
    fn alpn_offers_http_1_1_and_nothing_else() {
        let (cert, key) = crate::tls::test_support::self_signed("localhost");
        let dir = tempfile::tempdir().unwrap();
        let cert_file = dir.path().join("cert.pem");
        let key_file = dir.path().join("key.pem");
        std::fs::write(&cert_file, cert).unwrap();
        std::fs::write(&key_file, key).unwrap();

        let config = server_config(&cert_file, &key_file).expect("a fresh pair must load");
        // h2 must not be offered: axum is built without its http2 feature, so a
        // client that negotiated it would get a connection the server cannot
        // speak on.
        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn a_key_that_does_not_match_the_certificate_says_so() {
        let (cert, _) = crate::tls::test_support::self_signed("localhost");
        let (_, other_key) = crate::tls::test_support::self_signed("localhost");
        let dir = tempfile::tempdir().unwrap();
        let cert_file = dir.path().join("cert.pem");
        let key_file = dir.path().join("key.pem");
        std::fs::write(&cert_file, cert).unwrap();
        std::fs::write(&key_file, other_key).unwrap();

        let err = server_config(&cert_file, &key_file).expect_err("a mismatched pair must fail");
        let text = format!("{err:#}");
        assert!(
            text.contains("does not match"),
            "the error must name the mismatch, got: {text}"
        );
    }
}

/// Certificate generation for tests only.
///
/// A certificate checked into the repository expires, and then CI fails on a
/// date nobody chose; generating one per run has no expiry to go stale. This
/// lives behind `cfg(test)` so `rcgen` stays a dev-dependency and never enters
/// the shipped binary.
#[cfg(test)]
pub(crate) mod test_support {
    /// A self-signed certificate and its key, both PEM.
    ///
    /// The name goes in as a subject alternative name: rustls does not fall
    /// back to the common name, so a certificate without a SAN is rejected by
    /// every client and the test would be testing the wrong failure.
    pub fn self_signed(name: &str) -> (String, String) {
        let key = rcgen::KeyPair::generate().expect("generating a key pair");
        let cert = rcgen::CertificateParams::new(vec![name.to_string()])
            .expect("a name must be a valid SAN")
            .self_signed(&key)
            .expect("self-signing");
        (cert.pem(), key.serialize_pem())
    }
}
