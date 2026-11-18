//! TLS plumbing: reading the ClientHello ahead, replaying it, and the upstream client config.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::Acceptor;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

const MAX_HELLO: usize = 64 * 1024;
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// What a client sent first after `CONNECT`.
pub(crate) enum Hello {
    /// A TLS ClientHello; `bytes` must be replayed into whatever handles the stream next.
    Tls { sni: Option<String>, bytes: Vec<u8> },
    /// Something else (raw TCP, plain HTTP, ...).
    Other(Vec<u8>),
}

/// Reads just enough to parse the ClientHello, without answering it.
pub(crate) async fn read_client_hello<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Hello> {
    let mut bytes = Vec::with_capacity(2048);
    let mut acceptor = Acceptor::default();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes[0] != TLS_HANDSHAKE_RECORD {
            return Ok(Hello::Other(bytes));
        }
        let mut fresh = &chunk[..n];
        while !fresh.is_empty() {
            if acceptor.read_tls(&mut fresh)? == 0 {
                break;
            }
        }
        match acceptor.accept() {
            Ok(Some(accepted)) => {
                let sni = accepted.client_hello().server_name().map(str::to_string);
                return Ok(Hello::Tls { sni, bytes });
            }
            Ok(None) if bytes.len() < MAX_HELLO => continue, // ClientHello spans more reads
            Ok(None) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ClientHello too large",
                ));
            }
            Err((e, _)) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }
}

/// A stream that first serves bytes already read from it (the ClientHello).
pub(crate) struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.pos < this.prefix.len() {
            let n = buf.remaining().min(this.prefix.len() - this.pos);
            buf.put_slice(&this.prefix[this.pos..this.pos + n]);
            this.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// TLS settings for connections from shiftWire to target servers.
pub(crate) fn upstream_config(verify: bool, provider: Arc<CryptoProvider>) -> ClientConfig {
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions");
    if verify {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        // Pentest targets (staging, internal hosts) routinely have invalid certificates.
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
            .with_no_client_auth()
    }
}

/// Accepts any server certificate, but still checks the handshake signatures.
#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
