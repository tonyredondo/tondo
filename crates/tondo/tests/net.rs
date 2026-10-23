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
