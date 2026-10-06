//! Private, explicitly configured DNS and TLS providers.
//!
//! These providers do not register a public Tondo API or a native ABI. Futures
//! are driven by the owning executor; no worker or global provider is installed.

use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
    time::Duration,
};

use hickory_resolver::{
    Resolver,
    config::{
        ConnectionConfig, LookupIpStrategy, NameServerConfig, ResolveHosts, ResolverConfig,
        ResolverOpts, ServerOrderingStrategy,
    },
    proto::rr::Name,
};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore,
    SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{CryptoProvider, aws_lc_rs},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use tondo_stdlib::net::{
    HostName, IpAddress, MAX_RESOLVER_RESULTS, NetError, NetLimits, SocketAddress, TlsError,
    resolver_results,
};

use crate::toolchain::NetworkTarget;

mod dns_runtime;
pub(crate) mod executor;
pub(crate) mod tls_transport;
pub(crate) mod transport;

/// Finite limits on provider-owned state, independent of a caller's read limit.
pub(crate) const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;
pub(crate) const TLS_BUFFER_BYTES: usize = 64 * 1024;
pub(crate) const DNS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const ROOT_BUNDLE_HASH: &str =
    "sha256:142ba280f8d4a0090f7dec142503ce47aa6960752374f31147cb84e1dc797f14";

#[derive(Clone)]
pub(crate) struct DnsConfig {
    config: ResolverConfig,
    options: ResolverOpts,
    next_server: Arc<AtomicU64>,
}

impl DnsConfig {
    pub(crate) fn from_target(target: &NetworkTarget) -> Result<Self, NetError> {
        target.validate().map_err(|_| NetError::InvalidAddress)?;
        let mut servers = Vec::new();
        servers
            .try_reserve_exact(target.resolver_servers.len())
            .map_err(|_| NetError::ResourceLimit)?;
        for endpoint in &target.resolver_servers {
            let address: SocketAddr = endpoint.parse().map_err(|_| NetError::InvalidAddress)?;
            let mut connection = ConnectionConfig::udp();
            connection.port = address.port();
            let mut server = NameServerConfig::udp(address.ip());
            server.connections = vec![connection];
            servers.push(server);
        }
        // Both configuration types are non-exhaustive in the pinned provider.
        let mut options = ResolverOpts::default();
        options.attempts = 0;
        options.timeout = DNS_RESPONSE_TIMEOUT;
        options.cache_size = 0;
        options.use_hosts_file = ResolveHosts::Never;
        options.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        options.num_concurrent_reqs = 1;
        options.max_active_requests = 2;
        options.preserve_intermediates = false;
        options.try_tcp_on_error = false;
        options.server_ordering_strategy = ServerOrderingStrategy::UserProvidedOrder;
        options.os_port_selection = true;
        Ok(Self {
            config: ResolverConfig::from_parts(None, Vec::new(), servers),
            options,
            next_server: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Each operation owns its resolver and background-task set. Dropping the
    /// pending future retires that resolver rather than retaining a shared cache.
    pub(crate) async fn resolve(
        &self,
        host: HostName,
        port: i128,
        limits: NetLimits,
    ) -> Result<Vec<SocketAddress>, NetError> {
        let port = u16::try_from(port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or(NetError::InvalidPort)?;
        let text = std::str::from_utf8(host.as_bytes()).map_err(|_| NetError::InvalidHostName)?;
        let mut name = Name::from_ascii(text).map_err(|_| NetError::InvalidHostName)?;
        name.set_fqdn(true);
        // One declared endpoint per resolution, rotating in declared order.
        // A pool containing all endpoints would silently fail over on errors,
        // even with retries disabled. Both A and AAAA use the selected endpoint.
        let index = self.next_server.fetch_add(1, Ordering::Relaxed)
            % self.config.name_servers.len() as u64;
        let config = ResolverConfig::from_parts(
            None,
            Vec::new(),
            vec![self.config.name_servers[index as usize].clone()],
        );
        let (runtime, owner) = dns_runtime::DnsRuntime::new()?;
        let resolver =
            Resolver::builder_with_config(config, dns_runtime::DnsConnector(runtime.clone()))
                .with_options(self.options.clone())
                .build()
                .map_err(|_| NetError::ResolveFailed)?;
        let mut lookup = Box::pin(resolver.lookup_ip(name));
        let reply = std::future::poll_fn(|context| {
            if let Err(error) = runtime.poll_jobs(context) {
                return Poll::Ready(Err(error));
            }
            if let Poll::Ready(result) = lookup.as_mut().poll(context) {
                return Poll::Ready(result.map_err(|_| NetError::ResolveFailed));
            }
            if let Err(error) = runtime.poll_jobs(context) {
                return Poll::Ready(Err(error));
            }
            lookup
                .as_mut()
                .poll(context)
                .map(|result| result.map_err(|_| NetError::ResolveFailed))
        })
        .await?;
        drop(lookup);
        drop(resolver);
        drop(owner);
        let mut addresses = Vec::new();
        for ip in reply.iter() {
            if addresses.len() == MAX_RESOLVER_RESULTS {
                return Err(NetError::ResourceLimit);
            }
            addresses
                .try_reserve(1)
                .map_err(|_| NetError::ResourceLimit)?;
            addresses.push(SocketAddress::create(IpAddress::from_ip(ip), port.into())?);
        }
        if addresses.is_empty() {
            return Err(NetError::ResolveFailed);
        }
        resolver_results(&addresses, limits)
    }
}

/// Exact DER leaf pin, followed by the same chain, validity, name and handshake
/// signature checks as the ordinary provider. A matching byte string alone is
/// never enough to authenticate a connection.
#[derive(Debug)]
struct PinnedVerifier {
    certificate: CertificateDer<'static>,
    verifier: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if leaf.as_ref() != self.certificate.as_ref() {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ));
        }
        self.verifier
            .verify_server_cert(leaf, intermediates, name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls12_signature(message, cert, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls13_signature(message, cert, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}

fn crypto_provider() -> Arc<CryptoProvider> {
    use aws_lc_rs::cipher_suite::*;
    let mut provider = aws_lc_rs::default_provider();
    provider.cipher_suites = vec![
        TLS13_AES_256_GCM_SHA384,
        TLS13_AES_128_GCM_SHA256,
        TLS13_CHACHA20_POLY1305_SHA256,
        TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    ];
    Arc::new(provider)
}

#[derive(Clone, Debug)]
pub(crate) struct TlsConfig(Arc<ClientConfig>);

impl TlsConfig {
    /// Root material comes only from the pinned crate, never the host filesystem.
    /// `Some` selects an exact leaf pin; `None` selects the versioned bundle.
    pub(crate) fn create(pinned: Option<&[u8]>) -> Result<Self, TlsError> {
        let provider = crypto_provider();
        let mut roots = RootCertStore::empty();
        let verifier: Arc<dyn ServerCertVerifier> = if let Some(bytes) = pinned {
            if bytes.is_empty() {
                return Err(TlsError::InvalidCertificate);
            }
            if bytes.len() > MAX_CERTIFICATE_BYTES {
                return Err(TlsError::ResourceLimit);
            }
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(bytes.len())
                .map_err(|_| TlsError::ResourceLimit)?;
            owned.extend_from_slice(bytes);
            let certificate = CertificateDer::from(owned);
            roots
                .add(certificate.clone())
                .map_err(|_| TlsError::InvalidCertificate)?;
            let verifier =
                WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                    .build()
                    .map_err(|_| TlsError::InvalidCertificate)?;
            Arc::new(PinnedVerifier {
                certificate,
                verifier,
            })
        } else {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .map_err(|_| TlsError::Unsupported)?
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|_| TlsError::Unsupported)?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.resumption = rustls::client::Resumption::disabled();
        config.enable_early_data = false;
        Ok(Self(Arc::new(config)))
    }

    pub(crate) fn connection(&self, host: HostName) -> Result<ClientConnection, TlsError> {
        let name = std::str::from_utf8(host.as_bytes()).map_err(|_| TlsError::InvalidServerName)?;
        // ServerName::try_from also accepts IP literals. This API requires a
        // DNS name for SNI and certificate-name checking.
        let name =
            ServerName::try_from(name.to_owned()).map_err(|_| TlsError::InvalidServerName)?;
        if !matches!(name, ServerName::DnsName(_)) {
            return Err(TlsError::InvalidServerName);
        }
        let mut connection = ClientConnection::new(self.0.clone(), name).map_err(tls_error)?;
        connection.set_buffer_limit(Some(TLS_BUFFER_BYTES));
        Ok(connection)
    }
}

/// Identity of the actual ordered DER trust anchors, with lengths and the root
/// count in the digest. This is not the downloaded package archive checksum.
pub(crate) fn roots_hash() -> String {
    let mut digest = Sha256::new();
    digest.update(b"tondo-webpki-roots/1\0");
    digest.update((webpki_roots::TLS_SERVER_ROOTS.len() as u64).to_le_bytes());
    for root in webpki_roots::TLS_SERVER_ROOTS {
        for field in [
            Some(root.subject.as_ref()),
            Some(root.subject_public_key_info.as_ref()),
            root.name_constraints.as_ref().map(|value| value.as_ref()),
        ] {
            digest.update([u8::from(field.is_some())]);
            if let Some(bytes) = field {
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
        }
    }
    let mut output = String::from("sha256:");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest.finalize() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}

pub(crate) fn tls_error(error: rustls::Error) -> TlsError {
    match error {
        rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented => {
            TlsError::CertificateRejected
        }
        _ => TlsError::HandshakeFailed,
    }
}

pub(crate) fn io_error(error: &std::io::Error) -> NetError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => NetError::ConnectionRefused,
        ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof => NetError::ConnectionReset,
        ErrorKind::NotConnected => NetError::NotConnected,
        ErrorKind::AddrInUse => NetError::AddressInUse,
        ErrorKind::NetworkUnreachable
        | ErrorKind::HostUnreachable
        | ErrorKind::AddrNotAvailable => NetError::Unreachable,
        ErrorKind::TimedOut => NetError::Timeout,
        _ => NetError::Host,
    }
}

#[cfg(test)]
mod tests;
