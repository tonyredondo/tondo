//! `tondo ps` and `tondo counters` look at a running program through its
//! socket, and the socket goes when the program ends.

use serde_json::{Value, json};
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
    let missing: Vec<&str> = ["crunch", "main", "spawns crunch"].into_iter().filter(|n| !named(n)).collect();
    let names: Vec<&Value> = events.iter().map(|e| &e["name"]).collect();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(missing.is_empty() && !said.contains("the program"), "missing {missing:?} among {names:?}; {said}");
    let waits = events.iter().find(|e| e["name"] == "main" && e["args"]["then"] != "finished").unwrap();
    assert_eq!(waits["args"]["then"], "waits at traced.to:17 in main");
    let profile = std::fs::read_to_string(dir.join("out.folded")).unwrap();
    assert!(profile.lines().any(|line| line.contains(";crunch;fib")), "{profile}");
    // A program that does not end well says how it ended.
    std::fs::write(dir.join("fails.to"), "fn main() {\n    panic(\"no\")\n}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["trace", "--output", "fails", "--", "fails.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && said.contains("; the program exited with status 101"), "{said}");
}

const DUMPED: &str = r#"import std.console

fn sleeper(woken: Receiver[Int]) {
    let _ = woken.receive()
}

fn main() {
    var kept: Array[String] = []
    for i in 0..80000 {
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
    let read = |view: &[&str]| tondo(&[&["dump", "read", file.to_str().unwrap()], view, &["--json"]].concat());
    // The tasks get to their waits soon after the program says it is ready.
    let mut tasks = Value::Null;
    for _ in 0..50 {
        let output =
            Command::new(env!("CARGO_BIN_EXE_tondo")).args(["dump", &pid, "--output"]).arg(&file).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        tasks = read(&["tasks"]);
        let waits = |runs: &str| tasks.as_array().unwrap().iter().any(|t| t["runs"] == runs && t["state"] == "waits");
        if waits("main") && waits("sleeper") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(child.stdin.take());
    child.wait().unwrap();

    let task = |runs: &str| {
        let found = tasks.as_array().unwrap().iter().find(|t| t["runs"] == runs);
        found.unwrap_or_else(|| panic!("no task runs {runs} in {tasks}")).clone()
    };
    assert_eq!(task("main")["state"], "waits");
    assert_eq!(task("main")["at"], "dumped.to:16 in main");
    assert_eq!(task("sleeper")["at"], "dumped.to:4 in sleeper");
    assert_eq!(task("sleeper")["spawned"][0], "dumped.to:13 in main");
    // About 8 MiB of strings, from where the program made them: with a
    // sample every 512 KiB, a run without one is as good as impossible.
    let heap = read(&["heap"]);
    let strings = heap["sites"].as_array().unwrap().iter().find(|s| s["kind"] == "String");
    let strings = strings.unwrap_or_else(|| panic!("no strings in {heap}"));
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

const DEBUGGED: &str = r#"import std.console

type User = { name: String, age: Int }

fn older(user: User): User {
    let next = User { name: user.name, age: user.age + 1 }
    next
}

fn main() {
    let ada = User { name: "Ada", age: 36 }
    let names = ["a", "b"]
    let aged = older(ada)
    console.println("{aged.age} {names.length()}")
}
"#;

#[test]
fn debugger_session() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("debugged.to"), DEBUGGED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "debugged.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break debugged.to:13\ncontinue\nlocals\nstep\nstack\nprint user\nfinish\ndelete 1\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    assert_eq!(stops[0]["why"], "start");
    assert_eq!(stops[1]["at"], "debugged.to:13 in main");
    assert_eq!(stops[2]["at"], "debugged.to:6 in older");
    let locals = lines.iter().find_map(|l| l["locals"].as_array()).unwrap();
    let shown: Vec<String> = locals.iter().map(|l| format!("{} = {}", l["name"], l["value"])).collect();
    assert_eq!(shown, [r#""ada" = "User { name: \"Ada\", age: 36 }""#, r#""names" = "[\"a\", \"b\"]""#]);
    let stack = lines.iter().find_map(|l| l["stack"].as_array()).unwrap();
    assert_eq!(stack[..2], [json!("debugged.to:6 in older"), json!("debugged.to:13 in main")]);
    let user = lines.iter().find(|l| l["name"] == "user").unwrap();
    assert_eq!(user["value"], "User { name: \"Ada\", age: 36 }");
    assert!(lines.iter().any(|l| l.get("exited").is_some()));
}

/// One message of the Debug Adapter Protocol.
fn dap_read(reader: &mut impl BufRead) -> Value {
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        length = line.strip_prefix("Content-Length: ").unwrap().parse().unwrap();
    }
    let mut body = vec![0; length];
    std::io::Read::read_exact(reader, &mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn debug_adapter() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("debugged.to"), DEBUGGED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut seq = 0;
    let mut send = |command: &str, arguments: Value| {
        seq += 1;
        let text = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        std::io::Write::write_all(&mut input, format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
            .unwrap();
    };
    // Reads until the response to `command`, keeping the events.
    let mut events = Vec::new();
    let response = |output: &mut BufReader<_>, command: &str, events: &mut Vec<Value>| loop {
        let message = dap_read(output);
        if message["type"] == "response" && message["command"] == command {
            assert_eq!(message["success"], true, "{message}");
            return message;
        }
        events.push(message);
    };
    let until = |output: &mut BufReader<_>, event: &str, events: &mut Vec<Value>| {
        if let Some(at) = events.iter().position(|e| e["event"] == event) {
            return events.remove(at);
        }
        loop {
            let message = dap_read(output);
            if message["event"] == event {
                return message;
            }
        }
    };
    send("initialize", json!({ "adapterID": "tondo" }));
    response(&mut output, "initialize", &mut events);
    send("launch", json!({ "program": "debugged.to", "cwd": dir }));
    response(&mut output, "launch", &mut events);
    until(&mut output, "initialized", &mut events);
    let path = dir.join("debugged.to");
    send("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 13 }] }));
    let set = response(&mut output, "setBreakpoints", &mut events);
    assert_eq!(set["body"]["breakpoints"][0]["verified"], true);
    send("configurationDone", json!({}));
    response(&mut output, "configurationDone", &mut events);
    let stopped = until(&mut output, "stopped", &mut events);
    assert_eq!(stopped["body"]["reason"], "breakpoint");
    send("stackTrace", json!({ "threadId": stopped["body"]["threadId"] }));
    let trace = response(&mut output, "stackTrace", &mut events);
    assert_eq!(trace["body"]["stackFrames"][0]["line"], 13);
    send("variables", json!({ "variablesReference": 1 }));
    let variables = response(&mut output, "variables", &mut events);
    assert_eq!(variables["body"]["variables"][0]["name"], "ada");
    send("continue", json!({ "threadId": 1 }));
    response(&mut output, "continue", &mut events);
    until(&mut output, "terminated", &mut events);
    send("disconnect", json!({}));
    drop(input);
    child.wait().unwrap();
}

#[test]
fn every_object_of_a_debug_build() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("objects");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dumped.to"), DUMPED).unwrap();
    let executable = dir.join("dumped");
    let status = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "dumped.to", "--debug", "--output"])
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
    let file = dir.join("objects.dump.json");
    let pid = child.id().to_string();
    let output =
        Command::new(env!("CARGO_BIN_EXE_tondo")).args(["dump", &pid, "--output"]).arg(&file).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    drop(child.stdin.take());
    child.wait().unwrap();

    let file = file.to_str().unwrap();
    let types = tondo(&["dump", "read", file, "objects", "--json"]);
    let strings = types["types"].as_array().unwrap().iter().find(|t| t["type"] == "String").unwrap();
    assert!(strings["objects"].as_u64().unwrap() >= 20000, "{types}");
    // One of the strings the array keeps, and what keeps it.
    let dump: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let entry = dump["objects"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["text"] == "entry 7 entry 7 entry 7 entry 7 entry 7 entry 7 entry 7 entry 7 entry 7 entry 7 ");
    let address = entry.unwrap()["address"].as_str().unwrap();
    let object = tondo(&["dump", "read", file, "object", address, "--json"]);
    assert_eq!(object["type"], "String");
    let chain = object["chain"].as_array().unwrap();
    assert_eq!(chain[0], "task 1");
    assert!(chain[1].as_str().unwrap().starts_with("Array[String]"), "{object}");
}
