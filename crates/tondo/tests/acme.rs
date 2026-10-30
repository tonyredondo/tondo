//! `std.acme` (14.39 of the standard library) against Pebble, the ACME
//! server of Let's Encrypt for tests, when `TONDO_TEST_ACME` gives its
//! directory and `TONDO_TEST_ACME_TRUST` the authority of its own
//! certificate, as CI does on Linux: a server obtains a certificate for
//! `localhost`, answering the check on port 5001 itself, and serves with it.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const PROGRAM: &str = r#"
import std.acme
import std.console
import std.env
import std.http
import std.time

fn main(): !http.HttpError {
    let directory = env.get("ACME_DIRECTORY").unwrapOr("")
    let trust = env.get("ACME_TRUST").unwrapOr("")
    var app = http.App.new()
    app.get("/", (_) { http.Reply.text("secure hello") })
    let tls = acme.tls(["localhost"], email: "me@example.com", directory: directory, cache: "cache", trust: some(trust))
    let server = app.start(host: "127.0.0.1", port: 5001, tls: tls)?
    // The authority's root, which signs what it issues.
    let roots = http.Client.new(trust: some(trust)).get(directory.replace("14000/dir", "15000/roots/0"))?.text()?
    let client = http.Client.new(trust: some(roots))
    for _ in 0..300 {
        match client.get("https://localhost:5001/") {
            ok(reply) => {
                console.println("{reply.status()} {reply.text()?}")
                break
            }
            err(_) => time.sleep(time.milliseconds(200))
        }
    }
    server.stop()
}
"#;

#[test]
fn obtains_and_keeps_a_certificate() {
    let (Ok(directory), Ok(trust)) = (std::env::var("TONDO_TEST_ACME"), std::env::var("TONDO_TEST_ACME_TRUST")) else {
        return;
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("acme");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.to"), PROGRAM).unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["run", "main.to"])
            .current_dir(&dir)
            .env("ACME_DIRECTORY", &directory)
            .env("ACME_TRUST", &trust)
            .env("TONDO_RT_CHECK_LEAKS", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "200 secure hello\n", "{stderr}");
        assert!(output.status.success(), "{stderr}");
    };
    // Pebble may still be starting.
    let started = Instant::now();
    while std::net::TcpStream::connect("127.0.0.1:14000").is_err() {
        assert!(started.elapsed() < Duration::from_secs(120), "Pebble does not listen");
        std::thread::sleep(Duration::from_millis(500));
    }
    run();
    // A second start shows the certificate it kept, without asking again.
    let kept = dir.join("cache/localhost/localhost.pem");
    let issued = std::fs::metadata(&kept).unwrap().modified().unwrap();
    run();
    assert_eq!(std::fs::metadata(&kept).unwrap().modified().unwrap(), issued);
}
