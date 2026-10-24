//! `tondo ps` and `tondo counters` look at a running program through its
//! socket, and the socket goes when the program ends.

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

const PROGRAM: &str = r#"import std.console
import std.diagnostics
import std.net

fn main(): Unit ! net.NetError {
    let orders = diagnostics.counter("orders")
    orders.add()
    orders.add(41)
    diagnostics.gauge("queue length").set(5)
    let listener = net.listen("127.0.0.1", 0)?
    console.println("ready")
    let _ = console.flush()
    let _ = console.readLine()
    listener.close()
}
"#;

fn tondo(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn listed(pid: u32) -> bool {
    tondo(&["ps", "--json"]).as_array().unwrap().iter().any(|p| p["pid"] == pid)
}

#[test]
fn ps_and_counters() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("observe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("watched.to"), PROGRAM).unwrap();
    let executable = dir.join("watched");
    let status = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "watched.to", "--output"])
        .arg(&executable)
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());

    let mut child = Command::new(&executable).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    assert!(listed(child.id()));

    // By name, as one program has it.
    let sample = tondo(&["counters", "watched", "--count", "1", "--interval", "0.1", "--json"]);
    let value = |name: &str| {
        let counters = sample["counters"].as_array().unwrap();
        counters.iter().find(|c| c["name"] == name).map(|c| c["value"].as_i64().unwrap())
    };
    assert_eq!(sample["pid"], child.id());
    assert_eq!(value("program.orders"), Some(42));
    assert_eq!(value("program.queue length"), Some(5));
    assert_eq!(value("sockets.listening"), Some(1));
    assert_eq!(value("tasks.live"), Some(1));
    assert!(value("memory.held").unwrap() > 0);

    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(!listed(child.id()));

    // Turned off.
    let mut child = Command::new(&executable)
        .env("TONDO_DIAGNOSTICS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    assert!(!listed(child.id()));
    drop(child.stdin.take());
    child.wait().unwrap();
}
