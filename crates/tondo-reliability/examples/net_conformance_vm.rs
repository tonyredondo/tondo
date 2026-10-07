//! Actual public networking calls in the hosted VM with controlled loopback peers.
use std::collections::BTreeSet;
use std::sync::Arc;

use tondo_compiler::driver::{
    BuildTarget, CapabilityName, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition,
    HostProfile, Operation, ResourceLimits, SourceForm, discover_tests, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::test_control::{EnvelopeHandle, EnvelopeLimits, ExecutionPhase};

#[path = "../../tondo-stdlib/examples/support/net_conformance_cases.rs"]
mod cases;
use cases::oracle;

fn record(id: &str, observations: Vec<String>) {
    assert!(cases::CASES.contains(&id));
    let _ = cases::encoded_observations(&observations);
    assert_eq!(observations, cases::kernel(id), "{id}");
    println!(
        "{}",
        serde_json::json!({"id": id, "observations": observations, "envelopes_closed": true})
    );
}

fn request_with_resolver(
    source: &str,
    operation: Operation,
    network: bool,
    resolver: &str,
    clock: bool,
) -> CompilationRequest {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:net-conformance").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("net-conformance.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    let target = if network {
        BuildTarget::vm_hosted()
            .with_network_target(tondo_compiler::toolchain::NetworkTarget {
                resolver_servers: vec![resolver.into()],
            })
            .unwrap()
    } else {
        BuildTarget::vm_hosted()
    };
    let mut capabilities = if network {
        BTreeSet::from([CapabilityName::new("network").unwrap()])
    } else {
        BTreeSet::new()
    };
    if clock {
        capabilities.insert(CapabilityName::new("clock").unwrap());
    }
    CompilationRequest::new(
        operation,
        Edition::V0_1,
        target,
        HostProfile::Hosted,
        capabilities,
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

fn udp_transcript() {
    use std::time::Duration;
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = peer.local_addr().unwrap().port();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let source_address =
        oracle::admission::Address::ipv4([127, 0, 0, 1], i128::from(port)).unwrap();
    let limits = oracle::admission::Limits::create(8, 4, 2).unwrap();
    let actual_address = tondo_stdlib::net::SocketAddress::create(
        tondo_stdlib::net::IpAddress::parse("127.0.0.1").unwrap(),
        i128::from(port),
    )
    .unwrap();
    let mut expected = Vec::new();
    let mut source = format!(
        r#"import std.net
import std.bytes
import std.testing
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => testing.failNow("network")
 }}
}}
fn data(text: String): bytes.Bytes {{
 match bytes.Bytes(text) {{
  ok(item) => item
  err(_) => testing.failNow("bytes")
 }}
}}
test conformance {{
 let ip = value(net.IpAddress.parse("127.0.0.1"))
 let peer = value(net.socketAddress(ip, {port}))
 let options = value(net.options(none, value(net.NetLimits.create(8, 4, 2))))
 let socket = value(net.bind(value(net.socketAddress(ip, 0))))
"#
    );
    for (index, payload) in [b"abc".as_slice(), b"", b"12345", b"ok"]
        .into_iter()
        .enumerate()
    {
        source.push_str(" value(socket.sendTo(data(\"next\"), peer, options))\n");
        let reference =
            oracle::admission::receive_datagram(payload, source_address, payload.len(), limits);
        let kernel = tondo_stdlib::net::Datagram::receive(
            payload.to_vec(),
            actual_address,
            payload.len(),
            tondo_stdlib::net::NetLimits::create(8, 4, 2).unwrap(),
        );
        match reference {
            Ok((bytes, address)) => {
                assert_eq!(address, source_address);
                let kernel = kernel.unwrap();
                assert_eq!(kernel.bytes(), bytes);
                assert_eq!(kernel.source(), actual_address);
                let hex = bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let text = std::str::from_utf8(&bytes).unwrap();
                expected.push(format!("udp:{index}:ok:{hex}"));
                source.push_str(&format!(
                    r#" let packet{index} = value(socket.receiveFrom(options))
 assert(packet{index}.source() == peer)
 assert(packet{index}.bytes().equal(data("{text}")))
 testing.log("udp:{index}:ok:{hex}")
"#
                ));
            }
            Err(oracle::admission::Error::DatagramTooLarge) => {
                assert_eq!(
                    kernel.unwrap_err(),
                    tondo_stdlib::net::NetError::DatagramTooLarge
                );
                expected.push(format!("udp:{index}:err:DatagramTooLarge"));
                source.push_str(&format!(
                    r#" match socket.receiveFrom(options) {{
  err(net.NetError.DatagramTooLarge) => testing.log("udp:{index}:err:DatagramTooLarge")
  _ => testing.failNow("partial datagram published")
 }}
"#
                ));
            }
            other => panic!("unexpected independent fixture {other:?}"),
        }
    }
    source.push_str(" net.UdpSocket.close(socket)\n}\n");
    let peer = std::thread::spawn(move || {
        for payload in [b"abc".as_slice(), b"", b"12345", b"ok"] {
            let mut buffer = [0; 16];
            let (length, source) = peer.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..length], b"next");
            assert_eq!(peer.send_to(payload, source).unwrap(), payload.len());
        }
    });
    let request = request(&source, Operation::Test, true);
    let entries = discover_tests(&request).unwrap();
    if entries.len() != 1 {
        let rejected = execute(request).unwrap();
        panic!("UDP fixture: {}", rejected.diagnostics().human());
    }
    let envelope = EnvelopeHandle::new(
        "net-udp-feasibility",
        EnvelopeLimits::new(1_048_576, 1_048_576, 1_048_576),
    );
    let output = execute(
        request
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone()),
    )
    .unwrap();
    assert_eq!(
        output.status(),
        CompilationStatus::Success,
        "{}",
        output.diagnostics().human()
    );
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    let report = envelope.report().unwrap();
    assert!(report.terminal().is_none());
    assert_eq!(
        report
            .logs()
            .iter()
            .map(|row| row.message().to_owned())
            .collect::<Vec<_>>(),
        expected
    );
    envelope.close().unwrap();
    assert_eq!(envelope.report().unwrap().phase(), ExecutionPhase::Closed);
    peer.join().unwrap();
    record("udp-transcript", expected);
}

fn tcp_transcript() {
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let mut model = oracle::stream::Stream::new(128).unwrap();
    model.feed(b"a").unwrap();
    model.feed(b"bc").unwrap();
    model.peer_fin();
    let limits = oracle::admission::Limits::create(8, 4, 2).unwrap();
    let mut expected = Vec::new();
    let mut source = format!(
        r#"import std.net
import std.bytes
import std.io
import std.testing
fn value[T](result: T ! net.NetError): T {{
 match result {{
  ok(item) => item
  err(_) => testing.failNow("network")
 }}
}}
fn data(text: String): bytes.Bytes {{
 match bytes.Bytes(text) {{
  ok(item) => item
  err(_) => testing.failNow("bytes")
 }}
}}
test conformance {{
 let ip = value(net.IpAddress.parse("127.0.0.1"))
 let address = value(net.socketAddress(ip, {port}))
 let options = value(net.options(none, value(net.NetLimits.create(8, 4, 2))))
 let stream = value(net.connect(address, options))
 assert(value(stream.peerAddress()) == address)
 let (reader, writer) = net.TcpStream.split(stream)
"#
    );
    for (index, byte) in b"abc".iter().enumerate() {
        let id = model.prepare(1, limits, None, 7, 0).unwrap();
        assert_eq!(
            model.finish(id, true, false, 0),
            Ok(oracle::stream::Completion::Committed(
                oracle::admission::Read::Data(vec![*byte])
            ))
        );
        let kernel = tondo_stdlib::net::tcp_read_result(vec![*byte], 1, false).unwrap();
        let tondo_stdlib::io::ReadResult::Data(bytes) = kernel else {
            panic!("kernel lost final data")
        };
        assert_eq!(bytes, [*byte]);
        let text = char::from(*byte);
        expected.push(format!("tcp:{index}:data:{byte:02x}"));
        source.push_str(&format!(
            r#" match value(reader.read(1, options)) {{
  io.ReadResult.Data(chunk) => {{
   assert(chunk.equal(data("{text}")))
   testing.log("tcp:{index}:data:{byte:02x}")
  }}
  _ => testing.failNow("early stream EOF")
 }}
"#
        ));
    }
    let id = model.prepare(1, limits, None, 7, 0).unwrap();
    assert_eq!(
        model.finish(id, true, false, 0),
        Ok(oracle::stream::Completion::Committed(
            oracle::admission::Read::Eof
        ))
    );
    assert!(matches!(
        tondo_stdlib::net::tcp_read_result(vec![], 1, true).unwrap(),
        tondo_stdlib::io::ReadResult::Eof
    ));
    expected.push("tcp:3:eof".into());
    source.push_str(" match value(reader.read(1, options)) {\n io.ReadResult.Eof => testing.log(\"tcp:3:eof\")\n _ => testing.failNow(\"missing stream EOF\")\n }\n");
    for (index, byte) in b"xyz".iter().enumerate() {
        assert_eq!(model.write(&[*byte], 1), Ok(Some(1)));
        assert_eq!(tondo_stdlib::net::tcp_write_result(1, 1), Ok(1));
        expected.push(format!("tcp:write:{index}:1"));
        let text = char::from(*byte);
        source.push_str(&format!(" assert(value(writer.write(data(\"{text}\"), options)) == 1)\n testing.log(\"tcp:write:{index}:1\")\n"));
    }
    model.shutdown_writer().unwrap();
    model.close_scope();
    assert!(!model.snapshot().transport_live);
    assert_eq!(model.snapshot().sent, b"xyz");
    source.push_str(" value(writer.flush(options))\n value(writer.shutdown(options))\n net.TcpReadHalf.close(reader)\n net.TcpWriteHalf.close(writer)\n}\n");
    let peer = std::thread::spawn(move || {
        let stop = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            assert!(Instant::now() < stop, "bounded TCP peer accept");
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now()
                }
                Err(error) => panic!("controlled TCP accept: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket.write_all(b"a").unwrap();
        socket.write_all(b"bc").unwrap();
        socket.shutdown(std::net::Shutdown::Write).unwrap();
        let mut received = Vec::new();
        let mut buffer = [0; 4];
        loop {
            let length = socket.read(&mut buffer).unwrap();
            if length == 0 {
                break;
            }
            assert!(received.len() + length <= 3);
            received.extend_from_slice(&buffer[..length]);
        }
        assert_eq!(received, b"xyz");
    });
    let request = request(&source, Operation::Test, true);
    let entries = discover_tests(&request).unwrap();
    if entries.len() != 1 {
        let rejected = execute(request).unwrap();
        panic!("TCP fixture: {}", rejected.diagnostics().human());
    }
    let envelope = EnvelopeHandle::new(
        "net-tcp-feasibility",
        EnvelopeLimits::new(1_048_576, 1_048_576, 1_048_576),
    );
    let output = execute(
        request
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone()),
    )
    .unwrap();
    assert_eq!(
        output.status(),
        CompilationStatus::Success,
        "{}",
        output.diagnostics().human()
    );
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    let report = envelope.report().unwrap();
    assert!(report.terminal().is_none());
    assert_eq!(
        report
            .logs()
            .iter()
            .map(|row| row.message().to_owned())
            .collect::<Vec<_>>(),
        expected
    );
    envelope.close().unwrap();
    assert_eq!(envelope.report().unwrap().phase(), ExecutionPhase::Closed);
    peer.join().unwrap();
    record("tcp-transcript", expected);
}

fn values() {
    let (source, expected) = cases::values();
    let observed = run(&source, "127.0.0.1:9");
    assert_eq!(observed, expected);
    record("values-and-keys", observed);
}

fn run(source: &str, resolver: &str) -> Vec<String> {
    run_request(request_with_resolver(
        source,
        Operation::Test,
        true,
        resolver,
        false,
    ))
}

fn request(source: &str, operation: Operation, network: bool) -> CompilationRequest {
    request_with_resolver(source, operation, network, "127.0.0.1:9", false)
}

fn run_request(request: CompilationRequest) -> Vec<String> {
    let entries = discover_tests(&request).unwrap();
    if entries.len() != 1 {
        let rejected = execute(request).unwrap();
        panic!("fixture discovery: {}", rejected.diagnostics().human());
    }
    let envelope = EnvelopeHandle::new(
        "net-conformance",
        EnvelopeLimits::new(1_048_576, 1_048_576, 1_048_576),
    );
    let output = execute(
        request
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone()),
    )
    .unwrap();
    assert_eq!(
        output.status(),
        CompilationStatus::Success,
        "{}",
        output.diagnostics().human()
    );
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    assert!(envelope.report().unwrap().terminal().is_none());
    let observed = envelope
        .report()
        .unwrap()
        .logs()
        .iter()
        .map(|row| row.message().to_owned())
        .collect();
    envelope.close().unwrap();
    assert_eq!(envelope.report().unwrap().phase(), ExecutionPhase::Closed);
    observed
}

fn capabilities() {
    let mut records = Vec::new();
    for (form, source) in [
        ("import", "import std.net\nfn main() {}\n"),
        (
            "alias",
            "import std.net\nfn main() { let parse = net.IpAddress.parse\n }\n",
        ),
        (
            "defer",
            "import std.net\nfn close(stream: net.TcpStream) { defer net.TcpStream.close(stream)\n }\nfn main() {}\n",
        ),
    ] {
        for network in [false, true] {
            let output = execute(request(source, Operation::Check, network)).unwrap();
            if network {
                assert_eq!(
                    output.status(),
                    CompilationStatus::Success,
                    "{}",
                    output.diagnostics().human()
                );
            } else {
                assert_eq!(output.status(), CompilationStatus::Rejected);
                assert!(
                    output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .any(|d| d.code() == "E1008"
                            && d.message().contains("capability `network` is missing"))
                );
                assert!(
                    output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .all(|d| d.code() != "E0004")
                );
            }
            records.push(serde_json::json!({"form":form,"network":network,"result":if network {"accepted"} else {"E1008"}}));
        }
    }
    println!(
        "{}",
        serde_json::json!({"id":"vm-capabilities","observations":records})
    );
}

const PRELUDE: &str = r#"import std.net
import std.bytes
import std.io
import std.testing
fn value[T](result: T ! net.NetError): T {
 match result {
  ok(item) => item
  err(_) => testing.failNow("network")
 }
}
fn data(text: String): bytes.Bytes {
 match bytes.Bytes(text) {
  ok(item) => item
  err(_) => testing.failNow("bytes")
 }
}
"#;

fn dns_reply(query: &[u8]) -> Vec<u8> {
    assert!(query.len() >= 17);
    assert_eq!(&query[4..6], &[0, 1]);
    let terminator = 12 + query[12..].iter().position(|byte| *byte == 0).unwrap();
    let end = terminator + 5;
    assert!(end <= query.len());
    assert_eq!(&query[12..=terminator], b"\x07example\x04test\0");
    let kind = u16::from_be_bytes([query[terminator + 1], query[terminator + 2]]);
    let addresses = match kind {
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

fn dns_transcript() {
    use std::time::Duration;
    let expected = cases::kernel("dns-transcript");
    let mut observations = Vec::new();
    for limit in [2, 3] {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let resolver = socket.local_addr().unwrap().to_string();
        let peer = std::thread::spawn(move || {
            let mut kinds = Vec::new();
            for _ in 0..2 {
                let mut bytes = [0; 2048];
                let (length, source) = socket.recv_from(&mut bytes).unwrap();
                let terminator = 12 + bytes[12..length].iter().position(|b| *b == 0).unwrap();
                kinds.push(u16::from_be_bytes([
                    bytes[terminator + 1],
                    bytes[terminator + 2],
                ]));
                let reply = dns_reply(&bytes[..length]);
                assert_eq!(socket.send_to(&reply, source).unwrap(), reply.len());
            }
            assert_eq!(kinds, [1, 28]);
        });
        let mut source = format!(
            "{PRELUDE}\ntest conformance {{\n let options=value(net.options(none,value(net.NetLimits.create(8,4,{limit}))))\n let result=net.resolve(value(net.hostName(\"example.test\")),443,options)\n"
        );
        if limit == 2 {
            source.push_str(" match result {\n err(net.NetError.ResourceLimit) => testing.log(\"dns:2:err:ResourceLimit\")\n _ => testing.failNow(\"partial DNS array\")\n }\n");
        } else {
            source.push_str(" let expected=[value(net.socketAddress(value(net.IpAddress.parse(\"192.0.2.2\")),443)),value(net.socketAddress(value(net.IpAddress.parse(\"192.0.2.1\")),443)),value(net.socketAddress(value(net.IpAddress.parse(\"::1\")),443))]\n assert(value(result)==expected)\n testing.log(\"dns:3:ordered-v4-v6\")\n");
        }
        source.push_str("}\n");
        observations.extend(run(&source, &resolver));
        peer.join().unwrap();
    }
    assert_eq!(observations, expected);
    record("dns-transcript", observations);
}

fn tls_transcripts() {
    use oracle::tls::{Failure, Phase, Tls, Verdict};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};
    let certificate = CertificateDer::from_pem_slice(include_bytes!(
        "../../tondo-compiler/src/net_provider/fixtures/localhost-cert.pem"
    ))
    .unwrap();
    let pin = certificate
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    for (label, version) in [
        ("tls12", &rustls::version::TLS12),
        ("tls13", &rustls::version::TLS13),
    ] {
        for refused in [false, true] {
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
                let stop = Instant::now() + Duration::from_secs(5);
                let socket = loop {
                    assert!(Instant::now() < stop, "bounded TLS accept");
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::yield_now()
                        }
                        Err(e) => panic!("TLS accept: {e}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(config)).unwrap(),
                    socket,
                );
                let mut bytes = [0; 4];
                let result = stream.read_exact(&mut bytes);
                if refused {
                    assert!(result.is_err(), "rejected client sent application bytes");
                } else {
                    result.unwrap();
                    assert_eq!(&bytes, b"ping");
                    assert_eq!(stream.conn.protocol_version(), Some(version.version));
                    stream.write_all(b"pong").unwrap();
                    stream.conn.send_close_notify();
                    stream.flush().unwrap();
                }
                let mut remaining = [0; 2048];
                for step in 0..32 {
                    if stream.sock.read(&mut remaining).unwrap() == 0 {
                        return;
                    }
                    assert!(step < 31, "TLS peer did not observe client cleanup");
                }
            });
            let mut model = Tls::new();
            model.start(true, None, 0).unwrap();
            let name = if refused { "wrong.test" } else { "localhost" };
            let mut source = format!(
                r#"import std.encoding
{PRELUDE}
fn tlsValue[T](result: T ! net.TlsError): T {{
 match result {{
  ok(item) => item
  err(_) => testing.failNow("TLS")
 }}
}}
test conformance {{
 let certificate=match encoding.HexOptions.lower(encoding.EncodingLimits.defaults()).decode(data("{pin}")) {{
  ok(item) => item
  err(_) => testing.failNow("certificate")
 }}
 let config=tlsValue(net.tlsConfig(net.TlsVerification.PinnedCertificate(certificate)))
 let options=value(net.options(none,net.NetLimits.defaults()))
 let address=value(net.socketAddress(value(net.IpAddress.parse("127.0.0.1")),{port}))
 let tcp=value(net.connect(address,options))
 let result=net.TlsStream.connect(tcp,value(net.hostName("{name}")),config,options)
"#
            );
            let expected = if refused {
                assert_eq!(
                    model.finish(Verdict::CertificateRejected, 0, false),
                    Err(Failure::CertificateRejected)
                );
                source.push_str(" match result {\n err(net.TlsError.CertificateRejected) => testing.log(\"refused:CertificateRejected\")\n ok(opened) => { net.TlsStream.close(opened)\n testing.failNow(\"published refused TLS owner\") }\n err(_) => testing.failNow(\"incorrect TLS error\")\n }\n");
                vec!["refused:CertificateRejected".to_owned()]
            } else {
                model.finish(Verdict::Verified, 0, false).unwrap();
                model.split().unwrap();
                source.push_str(
                    r#" let (reader,writer)=net.TlsStream.split(tlsValue(result))
 assert(tlsValue(writer.write(data("ping"),options))==4)
 testing.log("write:4")
 tlsValue(writer.flush(options))
 match tlsValue(reader.read(2,options)) {
  io.ReadResult.Data(chunk) => assert(chunk.equal(data("po")))
  _ => testing.failNow("first TLS fragment")
 }
 testing.log("read:706f")
 match tlsValue(reader.read(2,options)) {
  io.ReadResult.Data(chunk) => assert(chunk.equal(data("ng")))
  _ => testing.failNow("final TLS fragment")
 }
 testing.log("read:6e67")
 match tlsValue(reader.read(2,options)) {
  io.ReadResult.Eof => testing.log("eof:close-notify")
  _ => testing.failNow("missing close_notify")
 }
 tlsValue(writer.shutdown(options))
 net.TlsReadHalf.close(reader)
 net.TlsWriteHalf.close(writer)
"#,
                );
                model.close_half(true).unwrap();
                assert!(model.transport_live());
                model.close_half(false).unwrap();
                ["write:4", "read:706f", "read:6e67", "eof:close-notify"]
                    .map(str::to_owned)
                    .to_vec()
            };
            source.push_str("}\n");
            let observations = run(&source, "127.0.0.1:9");
            assert_eq!(observations, expected);
            peer.join().unwrap();
            assert_eq!(model.phase(), Phase::Closed);
            assert!(!model.transport_live());
            let id = format!("{label}-{}", if refused { "refusal" } else { "transcript" });
            println!(
                "{}",
                serde_json::json!({"id":id,"observations":observations,"envelopes_closed":true})
            );
        }
    }
}

fn lifecycle() {
    let source = format!(
        r#"{PRELUDE}
fn receive(socket: ref net.UdpSocket, options: net.NetOptions): net.Datagram ! net.NetError selectable {{
 socket.receiveFrom(options)
}}
test conformance {{
 let ip=value(net.IpAddress.parse("127.0.0.1"))
 let ephemeral=value(net.socketAddress(ip,0))
 let listener=value(net.listen(ephemeral,1))
 let address=value(listener.localAddress())
 assert(address!=ephemeral)
 let options=value(net.options(none,net.NetLimits.defaults()))
 let client=value(net.connect(address,options))
 let accepted=value(listener.accept(options))
 assert(value(client.peerAddress())==address)
 assert(value(accepted.localAddress())==address)
 assert(value(accepted.peerAddress())==value(client.localAddress()))
 value(client.shutdown(net.Shutdown.Write,options))
 net.TcpStream.close(client)
 net.TcpStream.close(accepted)
 net.TcpListener.close(listener)
 testing.log("listener:assigned-port-closed")
 testing.log("listener:accepted-address-laws-closed")
 let sender=value(net.bind(ephemeral))
 let first=value(net.bind(ephemeral))
 let second=value(net.bind(ephemeral))
 value(sender.sendTo(data("x"),value(first.localAddress()),options))
 value(sender.sendTo(data("x"),value(second.localAddress()),options))
 let winner=select {{
  let result=receive(ref first,options) => {{ assert(value(result).bytes().equal(data("x")))
   0 }}
  let result=receive(ref second,options) => {{ assert(value(result).bytes().equal(data("x")))
   1 }}
 }}
 let retained=if winner==0 {{
  select {{
   let result=second.receiveFrom(options) => {{ assert(value(result).bytes().equal(data("x")))
    true }}
   else => false
  }}
 }} else {{
  select {{
   let result=first.receiveFrom(options) => {{ assert(value(result).bytes().equal(data("x")))
    true }}
   else => false
  }}
 }}
 assert(retained)
 testing.log("select:losing-datagram-retained")
 net.UdpSocket.close(sender)
 net.UdpSocket.close(first)
 net.UdpSocket.close(second)
}}
"#
    );
    let observations = run(&source, "127.0.0.1:9");
    assert_eq!(
        observations,
        [
            "listener:assigned-port-closed",
            "listener:accepted-address-laws-closed",
            "select:losing-datagram-retained"
        ]
    );
    println!(
        "{}",
        serde_json::json!({"id":"vm-lifecycle","observations":observations,"envelopes_closed":true})
    );
}

fn deadlines() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    let resolver = socket.local_addr().unwrap().to_string();
    let source = format!(
        r#"import std.time
{PRELUDE}
test conformance {{
 let instant=match time.now() {{
  ok(item) => item
  err(_) => testing.failNow("clock")
 }}
 let options=value(net.options(some(instant),net.NetLimits.defaults()))
 let host=value(net.hostName("example.test"))
 match net.resolve(host,0,options) {{
  err(net.NetError.InvalidPort) => testing.log("deadline:validation-first")
  _ => testing.failNow("validation lost precedence")
 }}
 match net.resolve(host,443,options) {{
  err(net.NetError.Timeout) => testing.log("deadline:expired-no-DNS")
  _ => testing.failNow("expired DNS result published")
 }}
}}
"#
    );
    let observations = run_request(request_with_resolver(
        &source,
        Operation::Test,
        true,
        &resolver,
        true,
    ));
    assert_eq!(
        observations,
        ["deadline:validation-first", "deadline:expired-no-DNS"]
    );
    let mut bytes = [0; 2048];
    assert_eq!(
        socket.recv_from(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    println!(
        "{}",
        serde_json::json!({"id":"vm-deadlines","observations":observations,"envelopes_closed":true})
    );
}

fn main() {
    // The fresh reference process has no OS provider; VM peers stay loopback only.
    values();
    tcp_transcript();
    udp_transcript();
    dns_transcript();
    tls_transcripts();
    lifecycle();
    deadlines();
    capabilities();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_values_and_capabilities_are_real_compiler_observations() {
        values();
        capabilities();
    }

    #[test]
    fn controlled_transports_preserve_bytes_errors_and_owners() {
        tcp_transcript();
        udp_transcript();
        dns_transcript();
        tls_transcripts();
        lifecycle();
        deadlines();
    }

    #[test]
    #[should_panic(expected = "unregistered network case")]
    fn unknown_case_never_selects_an_implicit_fixture() {
        cases::kernel("unregistered");
    }
}
