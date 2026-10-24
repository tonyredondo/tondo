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

    let mut child = Command::new(&executable)
        .env("TONDO_DIAGNOSTICS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
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

    // Off unless asked for.
    let mut child = Command::new(&executable).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    assert!(!listed(child.id()));
    drop(child.stdin.take());
    child.wait().unwrap();
}

const TRACED: &str = r#"import std.console

fn fib(n: Int): Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn crunch(): Int {
    var total = 0
    for i in 0..300 {
        total = total + fib(20 + i % 4)
    }
    total
}

fn main() {
    let job = spawn crunch()
    console.println("{job.wait()}")
}
"#;

#[test]
fn trace_of_a_started_program() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("trace");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("traced.to"), TRACED).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["trace", "--output", "out", "--", "traced.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let timeline: Value = serde_json::from_slice(&std::fs::read(dir.join("out.json")).unwrap()).unwrap();
    let events = timeline["traceEvents"].as_array().unwrap();
    let named = |name: &str| events.iter().any(|e| e["name"] == name);
    assert!(named("crunch") && named("main") && named("spawns crunch"));
    let waits = events.iter().find(|e| e["name"] == "main" && e["args"]["then"] != "finished").unwrap();
    assert_eq!(waits["args"]["then"], "waits at traced.to:17 in main");
    let profile = std::fs::read_to_string(dir.join("out.folded")).unwrap();
    assert!(profile.lines().any(|line| line.contains(";crunch;fib")), "{profile}");
}

const DUMPED: &str = r#"import std.console

fn sleeper(woken: Receiver[Int]) {
    let _ = woken.receive()
}

fn main() {
    var kept: Array[String] = []
    for i in 0..20000 {
        kept.push("entry {i} ".repeat(10))
    }
    let (wake, woken) = channel[Int]()
    let job = spawn sleeper(woken)
    console.println("ready {kept.length()}")
    let _ = console.flush()
    let _ = console.readLine()
    wake.send(0)
    _ = job
}
"#;

#[test]
fn dumps_of_a_running_program_and_at_a_panic() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dumped.to"), DUMPED).unwrap();
    let executable = dir.join("dumped");
    let status = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "dumped.to", "--output"])
        .arg(&executable)
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    let mut child = Command::new(&executable)
        .env("TONDO_DIAGNOSTICS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let pid = child.id().to_string();
    let file = dir.join("running.dump.json");
    let output =
        Command::new(env!("CARGO_BIN_EXE_tondo")).args(["dump", &pid, "--output"]).arg(&file).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    drop(child.stdin.take());
    child.wait().unwrap();

    let read = |view: &[&str]| tondo(&[&["dump", "read", file.to_str().unwrap()], view, &["--json"]].concat());
    let tasks = read(&["tasks"]);
    let task = |runs: &str| tasks.as_array().unwrap().iter().find(|t| t["runs"] == runs).unwrap().clone();
    assert_eq!(task("main")["state"], "waits");
    assert_eq!(task("main")["at"], "dumped.to:16 in main");
    assert_eq!(task("sleeper")["at"], "dumped.to:4 in sleeper");
    assert_eq!(task("sleeper")["spawned"][0], "dumped.to:13 in main");
    // About 2 MiB of strings, from where the program made them.
    let heap = read(&["heap"]);
    let strings = heap["sites"].as_array().unwrap().iter().find(|s| s["kind"] == "String").unwrap();
    assert_eq!(strings["site"], "dumped.to:10 in main");
    assert!(heap["bytes"].as_f64().unwrap() > 500_000.0, "{heap}");

    // At a panic, with TONDO_DUMP.
    std::fs::write(dir.join("panics.to"), "fn main() {\n    let values = [1]\n    let _ = values[3]\n}\n").unwrap();
    let crashes = dir.join("crashes");
    std::fs::create_dir_all(&crashes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "panics.to"])
        .env("TONDO_DUMP", &crashes)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("a dump of the program is in"));
    let dump = std::fs::read_dir(&crashes).unwrap().next().unwrap().unwrap().path();
    let summary = tondo(&["dump", "read", dump.to_str().unwrap(), "--json"]);
    let summary = summary["summary"].as_str().unwrap();
    assert!(summary.contains("dumped at a panic") && summary.contains("1 runs panics.to:3 in main"), "{summary}");
}
