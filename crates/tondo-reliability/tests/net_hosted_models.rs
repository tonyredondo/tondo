use tondo_reliability::net_model::{admission as reference, stream as model};

use std::{
    collections::BTreeSet,
    io::{Read as _, Write as _},
    sync::Arc,
    time::Duration,
};
use tondo_compiler::driver::{
    BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::toolchain::NetworkTarget;

#[test]
fn public_datagram_transcript_matches_atomic_reference_admission() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let peer = std::thread::spawn(move || {
        for bytes in [b"abc".as_slice(), b"", b"12345", b"ok"] {
            let mut request = [0; 16];
            let (length, source) = socket.recv_from(&mut request).unwrap();
            assert_eq!(&request[..length], b"next");
            assert_eq!(socket.send_to(bytes, source).unwrap(), bytes.len());
        }
    });
    let limits = reference::Limits::create(8, 4, 2).unwrap();
    let source_address = reference::Address::ipv4([127, 0, 0, 1], port as i128).unwrap();
    let mut source = format!(
        r#"import std.net
import std.bytes
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => panic("network")
 }}
}}
fn data(text: String): bytes.Bytes {{
 match bytes.Bytes(text) {{
  ok(item) => item
  err(_) => panic("bytes")
 }}
}}
fn main() {{
 let ip = value(net.IpAddress.parse("127.0.0.1"))
 let peer = value(net.socketAddress(ip, {port}))
 let options = value(net.options(none, value(net.NetLimits.create(8, 4, 2))))
 let socket = value(net.bind(value(net.socketAddress(ip, 0))))
"#
    );
    for (index, bytes) in [b"abc".as_slice(), b"", b"12345", b"ok"]
        .into_iter()
        .enumerate()
    {
        source.push_str(" value(socket.sendTo(data(\"next\"), peer, options))\n");
        let expected = reference::receive_datagram(bytes, source_address, bytes.len(), limits);
        match expected {
            Ok((bytes, _)) => {
                let text = std::str::from_utf8(&bytes).unwrap();
                source.push_str(&format!(
                    r#" let packet{index} = value(socket.receiveFrom(options))
 assert(packet{index}.source() == peer)
 assert(packet{index}.bytes().equal(data("{text}")))
"#
                ));
            }
            Err(reference::Error::DatagramTooLarge) => source.push_str(
                r#" match socket.receiveFrom(options) {
  err(net.NetError.DatagramTooLarge) => ()
  _ => panic("partial datagram was published")
 }
"#,
            ),
            other => panic!("unexpected independent datagram outcome: {other:?}"),
        }
    }
    source.push_str(" net.UdpSocket.close(socket)\n}\n");
    let output = execute(request(&source)).unwrap();
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    peer.join().unwrap();
}

fn request(text: &str) -> CompilationRequest {
    request_with_resolver(text, "127.0.0.1:5353")
}

fn request_with_resolver(text: &str, resolver: &str) -> CompilationRequest {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:net-model").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("net-model.to").unwrap(),
            Arc::<[u8]>::from(text.as_bytes()),
        ))
        .unwrap();
    CompilationRequest::new(
        Operation::Run,
        Edition::V0_1,
        BuildTarget::vm_hosted()
            .with_network_target(NetworkTarget {
                resolver_servers: vec![resolver.into()],
            })
            .unwrap(),
        HostProfile::Hosted,
        BTreeSet::from([CapabilityName::new("network").unwrap()]),
        DiagnosticFormat::Json,
        SourceForm::Module,
        ResourceLimits {
            max_vm_steps: 100_000,
            max_vm_heap_bytes: 1_048_576,
            ..ResourceLimits::default()
        },
        PackageGraph::loose(&sources, root).unwrap(),
        sources,
        root,
    )
    .unwrap()
}

/// Fixed controlled-peer DNS reply, independent of the production wire parser.
fn dns_reply(query: &[u8]) -> Vec<u8> {
    assert!(query.len() >= 17);
    assert_eq!(&query[4..6], &[0, 1]);
    let terminator = 12 + query[12..].iter().position(|byte| *byte == 0).unwrap();
    let end = terminator + 5;
    assert!(end <= query.len());
    assert_eq!(&query[12..=terminator], b"\x07example\x04test\0");
    let kind = u16::from_be_bytes([query[terminator + 1], query[terminator + 2]]);
    let addresses: Vec<Vec<u8>> = match kind {
        1 => vec![vec![192, 0, 2, 2], vec![192, 0, 2, 1], vec![192, 0, 2, 2]],
        28 => {
            let mut bytes = vec![0; 16];
            bytes[15] = 1;
            vec![bytes]
        }
        _ => panic!("unexpected controlled DNS question {kind}"),
    };
    let mut reply = query[..2].to_vec();
    reply.extend_from_slice(&[0x81, 0x80, 0, 1, 0, addresses.len() as u8, 0, 0, 0, 0]);
    reply.extend_from_slice(&query[12..end]);
    for address in addresses {
        reply.extend_from_slice(&[0xc0, 0x0c]);
        reply.extend_from_slice(&kind.to_be_bytes());
        reply.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, address.len() as u8]);
        reply.extend_from_slice(&address);
    }
    reply
}

#[test]
fn public_dns_transcript_matches_ordered_finite_reference() {
    for result_limit in [2, 3] {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let resolver = socket.local_addr().unwrap().to_string();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let peer = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut buffer = [0; 2048];
                let (length, source) = socket.recv_from(&mut buffer).unwrap();
                let reply = dns_reply(&buffer[..length]);
                socket.send_to(&reply, source).unwrap();
            }
        });
        let a = reference::Address::ipv4([192, 0, 2, 2], 443).unwrap();
        let b = reference::Address::ipv4([192, 0, 2, 1], 443).unwrap();
        let mut ip6 = [0; 16];
        ip6[15] = 1;
        let c = reference::Address::V6(ip6, 443);
        let expected = reference::resolve(
            &[a, b, a, c],
            reference::Limits::create(8, 8, result_limit).unwrap(),
        );
        let mut source = format!(
            r#"import std.net
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => panic("network")
 }}
}}
fn main() {{
 let options = value(net.options(none, value(net.NetLimits.create(8, 8, {result_limit}))))
 let result = net.resolve(value(net.hostName("example.test")), 443, options)
"#
        );
        match expected {
            Ok(addresses) => {
                assert_eq!(addresses, [a, b, c]);
                let expressions = addresses.into_iter().map(|address| {
                    let (text, port) = match address {
                        reference::Address::V4(bytes, port) => (format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3]), port),
                        reference::Address::V6(bytes, port) => {
                            assert_eq!(bytes, ip6);
                            ("::1".to_owned(), port)
                        }
                    };
                    format!("value(net.socketAddress(value(net.IpAddress.parse(\"{text}\")), {port}))")
                }).collect::<Vec<_>>().join(", ");
                source.push_str(&format!(
                    " let expected = [{expressions}]\n assert(value(result) == expected)\n"
                ));
            }
            Err(reference::Error::ResourceLimit) => source.push_str(
                r#" match result {
  err(net.NetError.ResourceLimit) => ()
  _ => panic("partial DNS result was published")
 }
"#,
            ),
            other => panic!("unexpected DNS model outcome: {other:?}"),
        }
        source.push_str("}\n");
        let output = execute(request_with_resolver(&source, &resolver)).unwrap();
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
        peer.join().unwrap();
    }
}

#[test]
fn public_stream_transcripts_match_model_despite_peer_fragmentation() {
    for fragments in [
        vec!["abc"],
        vec!["a", "bc"],
        vec!["ab", "c"],
        vec!["", "a", "b", "c"],
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let peer_fragments = fragments.clone();
        let peer = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(start.elapsed() < Duration::from_secs(3));
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("controlled accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            for fragment in peer_fragments {
                stream.write_all(fragment.as_bytes()).unwrap();
            }
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut observed = Vec::new();
            stream.read_to_end(&mut observed).unwrap();
            assert_eq!(observed, b"xyz");
        });
        let mut expected = model::Stream::new(128).unwrap();
        for fragment in fragments {
            expected.feed(fragment.as_bytes()).unwrap();
        }
        expected.peer_fin();
        let mut source = format!(
            r#"import std.net
import std.bytes
import std.io
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => panic("network")
 }}
}}
fn data(text: String): bytes.Bytes {{
 match bytes.Bytes(text) {{
  ok(item) => item
  err(_) => panic("bytes")
 }}
}}
fn main() {{
 let address = value(net.socketAddress(value(net.IpAddress.parse("127.0.0.1")), {port}))
 let options = value(net.options(none, net.NetLimits.defaults()))
 let stream = value(net.connect(address, options))
 let (reader, writer) = net.TcpStream.split(stream)
"#
        );
        for byte in b"abc" {
            let id = expected
                .prepare(
                    1,
                    reference::Limits::create(128, 128, 32).unwrap(),
                    None,
                    7,
                    0,
                )
                .unwrap();
            assert_eq!(
                expected.finish(id, true, false, 0),
                Ok(model::Completion::Committed(reference::Read::Data(vec![
                    *byte
                ])))
            );
            source.push_str(&format!(
                r#" match value(reader.read(1, options)) {{
  io.ReadResult.Data(item) => assert(item.equal(data("{}")))
  io.ReadResult.Eof => panic("early EOF")
 }}
"#,
                *byte as char
            ));
        }
        let id = expected
            .prepare(
                1,
                reference::Limits::create(128, 128, 32).unwrap(),
                None,
                7,
                0,
            )
            .unwrap();
        assert_eq!(
            expected.finish(id, true, false, 0),
            Ok(model::Completion::Committed(reference::Read::Eof))
        );
        assert_eq!(expected.write(b"xyz", 3), Ok(Some(3)));
        source.push_str(
            r#" match value(reader.read(1, options)) {
  io.ReadResult.Eof => ()
  _ => panic("missing EOF")
 }
 assert(value(writer.write(data("xyz"), options)) == 3)
 value(writer.shutdown(options))
 net.TcpReadHalf.close(reader)
 net.TcpWriteHalf.close(writer)
}

"#,
        );
        let output = execute(request(&source)).unwrap();
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
        peer.join().unwrap();
        expected.close_scope();
        assert!(expected.consistent());
        assert_eq!(expected.snapshot().observed, 3);
        assert!(!expected.snapshot().transport_live);
    }
}

#[test]
fn public_tls_transcripts_match_explicit_verdict_and_half_owner_model() {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use tondo_reliability::net_model::tls::{Failure, Phase, Tls, Verdict};

    let certificate = CertificateDer::from_pem_slice(include_bytes!(
        "../../tondo-compiler/src/net_provider/fixtures/localhost-cert.pem"
    ))
    .unwrap();
    let pin: String = certificate
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        for rejected in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let key = PrivateKeyDer::from_pem_slice(include_bytes!(
                "../../tondo-compiler/src/net_provider/fixtures/localhost-test-key.pem"
            ))
            .unwrap();
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_protocol_versions(&[version])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key)
            .unwrap();
            let peer = std::thread::spawn(move || {
                let start = std::time::Instant::now();
                let socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(start.elapsed() < Duration::from_secs(3));
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(error) => panic!("controlled TLS accept failed: {error}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut bytes = [0; 4];
                let read = stream.read_exact(&mut bytes);
                if rejected {
                    assert!(read.is_err(), "a rejected client sent application bytes");
                } else {
                    read.unwrap();
                    assert_eq!(&bytes, b"ping");
                    stream.write_all(b"pong").unwrap();
                    stream.conn.send_close_notify();
                    stream.flush().unwrap();
                }
                // The client must release its consumed transport on either
                // error or last-half close; neither may leave a socket behind.
                let mut remaining = [0; 2048];
                for step in 0..32 {
                    if stream.sock.read(&mut remaining).unwrap() == 0 {
                        return;
                    }
                    assert!(step < 31, "client cleanup exceeded the retained byte bound");
                }
            });
            let mut model = Tls::new();
            model.start(true, None, 0).unwrap();
            let name = if rejected { "wrong.test" } else { "localhost" };
            let mut source = format!(
                r#"import std.net
import std.bytes
import std.encoding
import std.io
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => panic("network")
 }}
}}
fn tlsValue[T](result: T ! net.TlsError): T {{
 match result {{
  ok(item) => item
  err(_) => panic("TLS")
 }}
}}
fn data(text: String): bytes.Bytes {{
 match bytes.Bytes(text) {{
  ok(item) => item
  err(_) => panic("bytes")
 }}
}}
fn main() {{
 let certificate = match encoding.HexOptions.lower(encoding.EncodingLimits.defaults()).decode(data("{pin}")) {{
  ok(item) => item
  err(_) => panic("certificate")
 }}
 let config = tlsValue(net.tlsConfig(net.TlsVerification.PinnedCertificate(certificate)))
 let options = value(net.options(none, net.NetLimits.defaults()))
 let address = value(net.socketAddress(value(net.IpAddress.parse("127.0.0.1")), {port}))
 let tcp = value(net.connect(address, options))
 let result = net.TlsStream.connect(tcp, value(net.hostName("{name}")), config, options)
"#
            );
            if rejected {
                assert_eq!(
                    model.finish(Verdict::CertificateRejected, 0, false),
                    Err(Failure::CertificateRejected)
                );
                source.push_str(
                    r#" match result {
  err(net.TlsError.CertificateRejected) => ()
  ok(opened) => {
   net.TlsStream.close(opened)
   panic("rejected owner was published")
  }
  err(_) => panic("incorrect TLS rejection")
 }
"#,
                );
            } else {
                model.finish(Verdict::Verified, 0, false).unwrap();
                model.split().unwrap();
                source.push_str(
                    r#" let (reader, writer) = net.TlsStream.split(tlsValue(result))
 assert(tlsValue(writer.write(data("ping"), options)) == 4)
 tlsValue(writer.flush(options))
 match tlsValue(reader.read(2, options)) {
  io.ReadResult.Data(item) => assert(item.equal(data("po")))
  _ => panic("missing first TLS fragment")
 }
 match tlsValue(reader.read(2, options)) {
  io.ReadResult.Data(item) => assert(item.equal(data("ng")))
  _ => panic("missing final TLS fragment")
 }
 match tlsValue(reader.read(2, options)) {
  io.ReadResult.Eof => ()
  _ => panic("missing close_notify")
 }
 tlsValue(writer.shutdown(options))
 net.TlsReadHalf.close(reader)
 net.TlsWriteHalf.close(writer)
"#,
                );
                model.close_half(true).unwrap();
                assert!(model.transport_live());
                model.close_half(false).unwrap();
            }
            source.push_str("}\n");
            let result = execute(request(&source)).unwrap();
            assert_eq!(result.exit_code(), 0, "{}", result.diagnostics().human());
            peer.join().unwrap();
            assert_eq!(model.phase(), Phase::Closed);
            assert!(!model.transport_live());
        }
    }
}
