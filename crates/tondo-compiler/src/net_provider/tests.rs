use super::*;
use hickory_resolver::proto::{
    op::{Message, ResponseCode},
    rr::{
        RData, Record, RecordType,
        rdata::{A, AAAA},
    },
};
use rustls::{
    ServerConfig, ServerConnection,
    pki_types::{PrivateKeyDer, pem::PemObject},
    time_provider::TimeProvider,
};
use std::{
    future::Future,
    io::Cursor,
    net::{Ipv4Addr, Ipv6Addr},
};

fn run<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(3), future)
                .await
                .unwrap()
        })
}

fn dns_config(servers: &[SocketAddr]) -> DnsConfig {
    DnsConfig::from_target(&NetworkTarget {
        resolver_servers: servers.iter().map(ToString::to_string).collect(),
    })
    .unwrap()
}

async fn serve_dns(socket: tokio::net::UdpSocket, negative: bool) {
    for _ in 0..2 {
        let mut buffer = [0; 2048];
        let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
        let request = Message::from_vec(&buffer[..length]).unwrap();
        let query = request.queries.first().unwrap().clone();
        assert!(query.name().is_fqdn());
        let mut reply = Message::response(request.id, request.op_code);
        reply.metadata.recursion_desired = true;
        reply.metadata.recursion_available = true;
        reply.add_query(query.clone());
        if negative {
            reply.metadata.response_code = ResponseCode::NXDomain;
        } else {
            match query.query_type() {
                RecordType::A => {
                    for ip in [
                        Ipv4Addr::new(192, 0, 2, 2),
                        Ipv4Addr::new(192, 0, 2, 1),
                        Ipv4Addr::new(192, 0, 2, 2),
                    ] {
                        reply.add_answer(Record::from_rdata(
                            query.name().clone(),
                            0,
                            RData::A(A(ip)),
                        ));
                    }
                }
                RecordType::AAAA => {
                    reply.add_answer(Record::from_rdata(
                        query.name().clone(),
                        0,
                        RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
                    ));
                }
                other => panic!("unexpected local query type: {other}"),
            }
        }
        socket
            .send_to(&reply.to_vec().unwrap(), peer)
            .await
            .unwrap();
    }
}

#[test]
fn dns_local_provider_preserves_address_order_and_deduplicates() {
    run(async {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = dns_config(&[socket.local_addr().unwrap()]);
        let server = tokio::spawn(serve_dns(socket, false));
        let actual = config
            .resolve(
                HostName::parse("example.test").unwrap(),
                443,
                NetLimits::default(),
            )
            .await
            .unwrap();
        server.await.unwrap();
        let expected = ["192.0.2.2", "192.0.2.1", "::1"]
            .map(|ip| SocketAddress::create(IpAddress::parse(ip).unwrap(), 443).unwrap());
        assert_eq!(actual, expected);
    });
}

#[test]
fn dns_local_provider_refuses_whole_oversized_and_negative_results() {
    run(async {
        for negative in [false, true] {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let config = dns_config(&[socket.local_addr().unwrap()]);
            let server = tokio::spawn(serve_dns(socket, negative));
            let result = config
                .resolve(
                    HostName::parse("missing.test.").unwrap(),
                    80,
                    NetLimits::create(1, 1, 1).unwrap(),
                )
                .await;
            assert_eq!(
                result,
                Err(if negative {
                    NetError::ResolveFailed
                } else {
                    NetError::ResourceLimit
                })
            );
            server.await.unwrap();
        }
    });
}

#[test]
fn dns_invalid_port_has_no_io_and_silent_provider_has_no_retry_or_failover() {
    run(async {
        let first = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let second = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut config = dns_config(&[first.local_addr().unwrap(), second.local_addr().unwrap()]);
        // Outlast Hickory's 333 ms UDP retry floor. A shorter probe would
        // miss retries performed beneath ResolverOpts.attempts.
        config.options.timeout = Duration::from_millis(800);
        assert_eq!(
            config
                .resolve(
                    HostName::parse("silent.test").unwrap(),
                    0,
                    NetLimits::default()
                )
                .await,
            Err(NetError::InvalidPort)
        );
        assert_eq!(config.next_server.load(Ordering::Relaxed), 0);
        assert_eq!(
            config
                .resolve(
                    HostName::parse("silent.test").unwrap(),
                    53,
                    NetLimits::default()
                )
                .await,
            Err(NetError::ResolveFailed)
        );
        let mut bytes = [0; 2048];
        for _ in 0..2 {
            first.try_recv_from(&mut bytes).unwrap();
        }
        assert_eq!(
            first.try_recv_from(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            second.try_recv_from(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        // The next explicit call selects the next declared endpoint once.
        let server = tokio::spawn(serve_dns(second, false));
        assert!(
            config
                .resolve(
                    HostName::parse("example.test").unwrap(),
                    53,
                    NetLimits::default()
                )
                .await
                .is_ok()
        );
        server.await.unwrap();
    });
}

#[test]
fn dns_cancellation_immediately_releases_every_query_socket() {
    use std::{future::poll_fn, task::Poll};
    run(async {
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = dns_config(&[server.local_addr().unwrap()]);
        let mut lookup = Box::pin(config.resolve(
            HostName::parse("cancel.test").unwrap(),
            80,
            NetLimits::default(),
        ));
        let mut addresses = Vec::new();
        let mut bytes = [0; 2048];
        poll_fn(|context| {
            assert!(lookup.as_mut().poll(context).is_pending());
            let mut buffer = tokio::io::ReadBuf::new(&mut bytes);
            if let Poll::Ready(Ok(address)) = server.poll_recv_from(context, &mut buffer) {
                addresses.push(address);
                if addresses.len() == 2 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
            }
            Poll::Pending
        })
        .await;
        drop(lookup);
        addresses.sort();
        addresses.dedup();
        assert!(!addresses.is_empty());
        for address in addresses {
            // No yield or background abort is needed before rebinding the
            // source port: dropping the query synchronously drops its sockets.
            let rebound = std::net::UdpSocket::bind(address).unwrap();
            assert_eq!(rebound.local_addr().unwrap(), address);
        }
    });
}

#[test]
fn dns_provider_job_budget_and_terminal_cleanup_are_observable() {
    use super::dns_runtime::DnsRuntime;
    use hickory_resolver::net::runtime::Spawn;
    use std::{
        sync::atomic::AtomicUsize,
        task::{Context, Waker},
    };
    struct PendingJob(Arc<AtomicUsize>);
    impl Future for PendingJob {
        type Output = ();
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }
    impl Drop for PendingJob {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let (mut runtime, owner) = DnsRuntime::new().unwrap();
    for _ in 0..8 {
        runtime.spawn_bg(PendingJob(dropped.clone()));
    }
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(runtime.poll_jobs(&mut context), Ok(()));
    runtime.spawn_bg(PendingJob(dropped.clone()));
    assert_eq!(
        runtime.poll_jobs(&mut context),
        Err(NetError::ResourceLimit)
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    drop(owner);
    assert_eq!(dropped.load(Ordering::Relaxed), 9);
}

fn loopback() -> SocketAddress {
    SocketAddress::create(IpAddress::parse("127.0.0.1").unwrap(), 0).unwrap()
}

#[test]
fn tcp_losing_preparation_preserves_connection_and_bytes_then_reports_eof() {
    use super::transport::{Listener, TcpStream};
    use tondo_stdlib::io::ReadResult;
    run(async {
        let listener = Arc::new(Listener::listen(loopback(), 8).unwrap());
        let address = listener.local_address().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let peer_address = client.local_address().unwrap();
        let loser = listener.prepare_accept().await.unwrap();
        assert!(matches!(
            listener.prepare_accept().await,
            Err(NetError::ResourceLimit)
        ));
        drop(loser);
        let peer = listener.prepare_accept().await.unwrap().commit().unwrap();
        assert_eq!(peer.peer_address().unwrap(), peer_address);
        assert_eq!(client.peer_address().unwrap(), address);
        let (reader, writer) = peer.split();
        let (client_reader, client_writer) = client.split();
        assert_eq!(client_writer.write(b"abc").await.unwrap(), 3);
        let prepared = reader.prepare_read(1, NetLimits::default()).await.unwrap();
        assert!(matches!(
            reader.prepare_read(1, NetLimits::default()).await,
            Err(NetError::ResourceLimit)
        ));
        drop(prepared);
        assert_eq!(
            reader.read(0, NetLimits::default()).await,
            Err(NetError::InvalidLimit)
        );
        assert_eq!(
            reader.read(1, NetLimits::default()).await.unwrap(),
            ReadResult::Data(b"a".to_vec())
        );
        assert_eq!(
            reader.read(2, NetLimits::default()).await.unwrap(),
            ReadResult::Data(b"bc".to_vec())
        );
        client_writer.shutdown().unwrap();
        assert_eq!(
            reader.read(1, NetLimits::default()).await.unwrap(),
            ReadResult::Eof
        );
        assert_eq!(writer.write(&[]).await.unwrap(), 0);
        assert_eq!(writer.write(b"z").await.unwrap(), 1);
        assert_eq!(
            client_reader.read(1, NetLimits::default()).await.unwrap(),
            ReadResult::Data(vec![b'z'])
        );
        drop(reader);
        drop(writer);
        assert_eq!(
            client_reader.read(1, NetLimits::default()).await.unwrap(),
            ReadResult::Eof
        );
    });
}

#[test]
fn tcp_pending_read_cancellation_releases_consumer_and_preserves_next_chunk() {
    use super::transport::{Listener, TcpStream};
    use tondo_stdlib::io::ReadResult;
    run(async {
        let listener = Arc::new(Listener::listen(loopback(), 1).unwrap());
        let client = TcpStream::connect(listener.local_address().unwrap())
            .await
            .unwrap();
        let peer = listener.prepare_accept().await.unwrap().commit().unwrap();
        let (reader, _writer) = peer.split();
        let (_reader, writer) = client.split();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                reader.prepare_read(8, NetLimits::default())
            )
            .await
            .is_err()
        );
        writer.write(b"after cancellation").await.unwrap();
        assert_eq!(
            reader.read(5, NetLimits::default()).await.unwrap(),
            ReadResult::Data(b"after".to_vec())
        );
    });
}

#[test]
fn tcp_argument_failures_precede_io_and_shutdown_is_explicit() {
    use super::transport::{Listener, TcpStream};
    run(async {
        for backlog in [0, -1, i128::MAX] {
            assert!(matches!(
                Listener::listen(loopback(), backlog),
                Err(NetError::InvalidLimit)
            ));
        }
        assert!(matches!(
            TcpStream::connect(loopback()).await,
            Err(NetError::InvalidPort)
        ));
        let listener = Arc::new(Listener::listen(loopback(), 1).unwrap());
        let client = TcpStream::connect(listener.local_address().unwrap())
            .await
            .unwrap();
        let peer = listener.prepare_accept().await.unwrap().commit().unwrap();
        peer.shutdown(std::net::Shutdown::Both).unwrap();
        let (reader, _writer) = client.split();
        assert_eq!(
            reader.read(1, NetLimits::default()).await.unwrap(),
            tondo_stdlib::io::ReadResult::Eof
        );
    });
}

#[test]
fn udp_losing_preparation_preserves_empty_and_oversized_datagrams() {
    use super::transport::UdpSocket;
    run(async {
        let sender = UdpSocket::bind(loopback()).unwrap();
        let receiver = UdpSocket::bind(loopback()).unwrap();
        let source = sender.local_address().unwrap();
        let destination = receiver.local_address().unwrap();
        let limits = NetLimits::create(4, 4, 4).unwrap();
        sender.send(&[], destination, limits).await.unwrap();
        let prepared = receiver.prepare_receive(limits).await.unwrap();
        assert!(matches!(
            receiver.prepare_receive(limits).await,
            Err(NetError::ResourceLimit)
        ));
        drop(prepared);
        let empty = receiver.receive(limits).await.unwrap();
        assert_eq!(empty.bytes(), b"");
        assert_eq!(empty.source(), source);
        sender
            .send(b"oversized", destination, NetLimits::default())
            .await
            .unwrap();
        drop(receiver.prepare_receive(limits).await.unwrap());
        assert_eq!(
            receiver.receive(limits).await,
            Err(NetError::DatagramTooLarge)
        );
        sender.send(b"next", destination, limits).await.unwrap();
        assert_eq!(receiver.receive(limits).await.unwrap().bytes(), b"next");
    });
}

#[test]
fn udp_send_refusals_have_no_datagram_and_pending_cancel_releases_consumer() {
    use super::transport::UdpSocket;
    run(async {
        let sender = UdpSocket::bind(loopback()).unwrap();
        let receiver = UdpSocket::bind(loopback()).unwrap();
        let destination = receiver.local_address().unwrap();
        let limits = NetLimits::create(1, 1, 1).unwrap();
        assert_eq!(
            sender.send(b"too large", destination, limits).await,
            Err(NetError::DatagramTooLarge)
        );
        assert_eq!(
            sender.send(b"a", loopback(), limits).await,
            Err(NetError::InvalidPort)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(5), receiver.prepare_receive(limits))
                .await
                .is_err()
        );
        sender.send(b"a", destination, limits).await.unwrap();
        assert_eq!(receiver.receive(limits).await.unwrap().bytes(), b"a");
    });
}

#[derive(Debug)]
struct FixedTime(u64);

impl TimeProvider for FixedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(self.0)))
    }
}

fn certificate() -> CertificateDer<'static> {
    CertificateDer::from_pem_slice(include_bytes!("fixtures/localhost-cert.pem")).unwrap()
}

fn server(version: &'static rustls::SupportedProtocolVersion) -> ServerConnection {
    let key =
        PrivateKeyDer::from_pem_slice(include_bytes!("fixtures/localhost-test-key.pem")).unwrap();
    let config = ServerConfig::builder_with_provider(crypto_provider())
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate()], key)
        .unwrap();
    ServerConnection::new(Arc::new(config)).unwrap()
}

fn handshake(
    client: &mut ClientConnection,
    server: &mut ServerConnection,
) -> Result<(), rustls::Error> {
    for _ in 0..16 {
        if client.wants_write() {
            let mut bytes = Vec::new();
            client.write_tls(&mut bytes).unwrap();
            server.read_tls(&mut Cursor::new(bytes)).unwrap();
            server.process_new_packets()?;
        }
        if server.wants_write() {
            let mut bytes = Vec::new();
            server.write_tls(&mut bytes).unwrap();
            client.read_tls(&mut Cursor::new(bytes)).unwrap();
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(());
        }
    }
    panic!("local handshake exceeded its step bound")
}

fn pinned_config(time: u64) -> TlsConfig {
    let mut config = TlsConfig::create(Some(certificate().as_ref())).unwrap();
    Arc::make_mut(&mut config.0).time_provider = Arc::new(FixedTime(time));
    config
}

async fn tls_peer(
    mut socket: tokio::net::TcpStream,
    version: &'static rustls::SupportedProtocolVersion,
    graceful: bool,
) {
    use std::io::{Read, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut connection = server(version);
    let mut request = Vec::new();
    let mut ciphertext = [0; 32 * 1024];
    loop {
        if connection.wants_write() {
            let mut output = Vec::new();
            connection.write_tls(&mut output).unwrap();
            socket.write_all(&output).await.unwrap();
        }
        let mut plaintext = [0; 4];
        match connection.reader().read(&mut plaintext) {
            Ok(length) => request.extend_from_slice(&plaintext[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("local TLS peer read failed: {error}"),
        }
        if request.len() >= 4 {
            break;
        }
        let length = socket.read(&mut ciphertext).await.unwrap();
        assert_ne!(length, 0);
        connection
            .read_tls(&mut Cursor::new(&ciphertext[..length]))
            .unwrap();
        connection.process_new_packets().unwrap();
    }
    assert_eq!(request, b"ping");
    connection.writer().write_all(b"pong").unwrap();
    if graceful {
        connection.send_close_notify();
    }
    let mut output = Vec::new();
    connection.write_tls(&mut output).unwrap();
    socket.write_all(&output).await.unwrap();
}

#[test]
fn tls_loopback_split_reader_and_writer_make_progress_without_holding_a_wait_lock() {
    use super::{tls_transport::TlsStream, transport::TcpStream};
    use std::{future::poll_fn, task::Poll};
    use tondo_stdlib::io::ReadResult;
    run(async {
        for (version, graceful) in [
            (&rustls::version::TLS12, true),
            (&rustls::version::TLS13, true),
            (&rustls::version::TLS12, false),
            (&rustls::version::TLS13, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = transport::tondo_address(listener.local_addr().unwrap()).unwrap();
            let peer = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                tls_peer(socket, version, graceful).await;
            });
            let tcp = TcpStream::connect(address).await.unwrap();
            let stream = TlsStream::connect(
                tcp,
                HostName::parse("localhost").unwrap(),
                &pinned_config(1_798_761_600),
            )
            .await
            .unwrap();
            let (reader, writer) = stream.split();
            assert_eq!(
                reader.read(0, NetLimits::default()).await,
                Err(TlsError::Transport(NetError::InvalidLimit))
            );
            assert_eq!(
                reader.read(i128::MAX, NetLimits::default()).await,
                Err(TlsError::ResourceLimit)
            );
            let mut reading = Box::pin(reader.read(2, NetLimits::default()));
            // Start a reader while the peer waits for a request. Its pending
            // state must leave the session available to this writer.
            poll_fn(|context| {
                assert!(reading.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
            assert_eq!(writer.write(b"ping").unwrap(), 4);
            writer.flush().await.unwrap();
            assert_eq!(reading.await.unwrap(), ReadResult::Data(b"po".to_vec()));
            assert_eq!(
                reader.read(2, NetLimits::default()).await.unwrap(),
                ReadResult::Data(b"ng".to_vec())
            );
            let terminal = reader.read(2, NetLimits::default()).await;
            if graceful {
                assert_eq!(terminal, Ok(ReadResult::Eof));
                writer.shutdown().await.unwrap();
                assert_eq!(writer.write(b"late"), Err(TlsError::Closed));
            } else {
                assert_eq!(
                    terminal,
                    Err(TlsError::Transport(NetError::ConnectionReset))
                );
            }
            peer.await.unwrap();
        }
    });
}

#[test]
fn tls_cancelled_handshake_closes_its_consumed_tcp_transport() {
    use super::{tls_transport::TlsStream, transport::TcpStream};
    use tokio::io::AsyncReadExt;
    run(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = transport::tondo_address(listener.local_addr().unwrap()).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let (mut peer, _) = listener.accept().await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                TlsStream::connect(
                    tcp,
                    HostName::parse("localhost").unwrap(),
                    &pinned_config(1_798_761_600)
                )
            )
            .await
            .is_err()
        );
        // Discard the already sent ClientHello, then observe terminal EOF.
        let mut bytes = [0; 2048];
        loop {
            if peer.read(&mut bytes).await.unwrap() == 0 {
                break;
            }
        }
    });
}

#[test]
fn tls_exact_pin_authenticates_real_tls12_and_tls13_handshakes() {
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let config = pinned_config(1_798_761_600); // 2027-01-01 UTC.
        let mut client = config
            .connection(HostName::parse("localhost").unwrap())
            .unwrap();
        let mut peer = server(version);
        handshake(&mut client, &mut peer).unwrap();
        assert_eq!(client.protocol_version(), Some(version.version));
        let payload = b"verified local TLS payload";
        use std::io::{Read, Write};
        client.writer().write_all(payload).unwrap();
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        peer.read_tls(&mut Cursor::new(bytes)).unwrap();
        peer.process_new_packets().unwrap();
        let mut output = [0; 64];
        let length = peer.reader().read(&mut output).unwrap();
        assert_eq!(&output[..length], payload);
    }
}

#[test]
fn tls_matching_pin_still_refuses_wrong_name_and_expired_certificate() {
    for (name, time) in [("wrong.test", 1_798_761_600), ("localhost", 2_524_608_000)] {
        let config = pinned_config(time);
        let mut client = config.connection(HostName::parse(name).unwrap()).unwrap();
        assert_eq!(
            tls_error(handshake(&mut client, &mut server(&rustls::version::TLS13)).unwrap_err()),
            TlsError::CertificateRejected
        );
    }
    let mut bundle = TlsConfig::create(None).unwrap();
    Arc::make_mut(&mut bundle.0).time_provider = Arc::new(FixedTime(1_798_761_600));
    let mut client = bundle
        .connection(HostName::parse("localhost").unwrap())
        .unwrap();
    assert_eq!(
        tls_error(handshake(&mut client, &mut server(&rustls::version::TLS13)).unwrap_err()),
        TlsError::CertificateRejected
    );
}

#[test]
fn tls_configuration_refuses_invalid_der_and_non_sni_names_before_handshake() {
    for bytes in [&[][..], b"not DER", &[0x30, 0xff]] {
        assert!(matches!(
            TlsConfig::create(Some(bytes)),
            Err(TlsError::InvalidCertificate)
        ));
    }
    assert!(matches!(
        TlsConfig::create(Some(&vec![0; MAX_CERTIFICATE_BYTES + 1])),
        Err(TlsError::ResourceLimit)
    ));
    let config = TlsConfig::create(None).unwrap();
    assert!(matches!(
        config.connection(HostName::parse("127.0.0.1").unwrap()),
        Err(TlsError::InvalidServerName)
    ));
    assert_eq!(roots_hash(), ROOT_BUNDLE_HASH);
    assert_eq!(
        io_error(&std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "private OS detail"
        )),
        NetError::ConnectionRefused
    );
}
