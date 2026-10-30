//! `std.net` over TLS (14.4.6 of the standard library), with certificates
//! the test makes.

use std::path::Path;
use std::process::Command;

fn run(name: &str, program: &str, files: &[(&str, String)]) -> (String, String, bool) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, text) in files {
        std::fs::write(dir.join(path), text).unwrap();
    }
    std::fs::write(dir.join("main.to"), program).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "main.to"])
        .current_dir(&dir)
        .env("TONDO_RT_CHECK_LEAKS", "1")
        .output()
        .unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.success())
}

#[test]
fn connections_over_tls() {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let authority = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let certificate =
        CertificateParams::new(vec!["localhost".to_string()]).unwrap().signed_by(&key, &authority).unwrap();
    let program = r#"
import std.console
import std.net

fn echo(listener: net.Listener) {
    for connection in listener {
        match connection.readLine() {
            ok(some(line)) => _ = connection.write("secure {line}\n")
            ok(none) => {}
            err(error) => console.println("server: {error}")
        }
        _ = connection.close()
    }
}

fn main(): !net.NetError {
    let listener = net.listen("127.0.0.1", 0, tls: net.Tls.files("server.pem", "server.key"))?
    let port = listener.address().port
    spawn echo(listener)

    let stream = net.connectTls("localhost", port, trust: "authority.pem")?
    stream.write("hello\n")?
    console.println("{stream.readLine()?}")

    // The system does not trust the authority, and the name must match.
    let attempts: Array[(String, String?)] = [("localhost", none), ("127.0.0.1", some("authority.pem"))]
    for (host, trust) in attempts {
        match net.connectTls(host, port, trust: trust) {
            ok(_) => console.println("unexpected connection")
            err(net.NetError.Tls(why)) => console.println("refused {host}: {why}")
            err(error) => console.println("{error}")
        }
    }

    // Plain text to a TLS listener fails its handshake on the server.
    let plain = net.connect("127.0.0.1", port)?
    plain.write("not a handshake\n")?
    _ = plain.readAll()

    match net.connectTls("localhost", port, trust: "missing.pem") {
        ok(_) => console.println("unexpected connection")
        err(error) => console.println("{"{error}".startsWith("TLS: the trusted certificates `missing.pem`")}")
    }
    match net.listen("127.0.0.1", 0, tls: net.Tls.files("missing.pem", "server.key")) {
        ok(_) => console.println("unexpected listener")
        err(error) => console.println("{"{error}".startsWith("TLS: the certificate `missing.pem`")}")
    }
    listener.close()
}
"#;
    let files =
        [("authority.pem", authority.pem()), ("server.pem", certificate.pem()), ("server.key", key.serialize_pem())];
    let (stdout, stderr, success) = run("net-tls", program, &files);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "some(secure hello)", "{stdout}{stderr}");
    assert!(lines.iter().filter(|l| l.starts_with("refused ")).count() == 2, "{stdout}{stderr}");
    assert!(lines.iter().filter(|l| l.starts_with("server: TLS: ")).count() >= 1, "{stdout}{stderr}");
    assert_eq!(lines.iter().filter(|l| **l == "true").count(), 2, "{stdout}{stderr}");
    assert!(success, "{stderr}");
}

/// `Stream.startTls` against a server that starts plain and answers a
/// `STARTTLS` line with TLS, as SMTP, PostgreSQL and MySQL do.
#[test]
fn starting_tls_on_a_plain_connection() {
    use rcgen::{CertificateParams, KeyPair};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::Arc;
    let key = KeyPair::generate().unwrap();
    let certificate = CertificateParams::new(vec!["localhost".to_string()]).unwrap().self_signed(&key).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (der, key_der) = (certificate.der().clone(), key.serialize_der());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).unwrap();
        assert_eq!(line, "STARTTLS\n");
        stream.write_all(b"GO\n").unwrap();
        let key = rustls_pki_types::PrivateKeyDer::Pkcs8(key_der.into());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der], key)
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(connection, stream);
        let mut line = String::new();
        BufReader::new(&mut tls).read_line(&mut line).unwrap();
        tls.write_all(format!("secure {line}").as_bytes()).unwrap();
        tls.flush().unwrap();
    });
    let program = format!(
        r#"
import std.console
import std.net

fn main(): !net.NetError {{
    let plain = net.connect("127.0.0.1", {port})?
    plain.write("STARTTLS\n")?
    console.println("{{plain.readLine()?}}")
    let secure = plain.startTls("localhost", trust: "server.pem")?
    secure.write("hello\n")?
    console.println("{{secure.readLine()?}}")
    console.println("{{plain.readLine().isErr()}}")
}}
"#
    );
    let (stdout, stderr, success) = run("net-starttls", &program, &[("server.pem", certificate.pem())]);
    server.join().unwrap();
    assert_eq!(stdout, "some(GO)\nsome(secure hello)\ntrue\n", "{stderr}");
    assert!(success, "{stderr}");
}
