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
    tondo(&["ps", "--json"])["programs"].as_array().unwrap().iter().any(|p| p["pid"] == pid)
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

const ALLOCATING: &str = r#"import std.console
import std.time

fn chunk(i: Int): String {
    "item {i} ".repeat(50)
}

fn sleeper() {
    time.sleep(time.seconds(10))
}

fn main() {
    var kept: Array[String] = []
    for i in 0..20000 {
        kept.push(chunk(i))
    }
    let jobs = [spawn sleeper(), spawn sleeper()]
    console.println("{kept.length()}")
    time.sleep(time.seconds(10))
    _ = jobs
}
"#;

#[test]
fn profiles_time_memory_and_tasks() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("profile");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("busy.to"), TRACED).unwrap();
    std::fs::write(dir.join("allocating.to"), ALLOCATING).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(&dir).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let show = |args: &[&str]| -> Value {
        serde_json::from_str(&run(&[&["profile", "show"], args, &["--json"]].concat())).unwrap()
    };
    // Processor time, by function and by the program's own lines.
    run(&["profile", "--output", "cpu.pprof", "--", "busy.to"]);
    let cpu = show(&["cpu.pprof"]);
    assert_eq!(
        (&cpu["format"], &cpu["type"], &cpu["unit"]),
        (&json!("tondo-profile-show-0.1/1"), &json!("cpu"), &json!("nanoseconds"))
    );
    let functions: Vec<&str> =
        cpu["functions"].as_array().unwrap().iter().map(|f| f["function"].as_str().unwrap()).collect();
    assert!(functions.contains(&"fib") && functions.contains(&"crunch"), "{cpu}");
    let fib = cpu["lines"].as_array().unwrap().iter().find(|l| l["function"] == "fib").expect("a line of fib");
    assert_eq!((&fib["file"], &fib["line"]), (&json!("busy.to"), &json!(4)), "{cpu}");
    assert!(fib["source"].as_str().unwrap().contains("fib(n - 1) + fib(n - 2)"), "{fib}");
    let text = run(&["profile", "show", "cpu.pprof", "--lines", "--top", "3"]);
    assert!(text.contains("busy.to:4 in fib  if n < 2 { n } else { fib(n - 1) + fib(n - 2) }"), "{text}");
    run(&["profile", "show", "cpu.pprof", "--html", "flame.html"]);
    let page = std::fs::read_to_string(dir.join("flame.html")).unwrap();
    assert!(page.contains("<svg") && page.contains("<title>fib") && !page.contains("<script"), "{page}");
    // Memory, as the program left it: what it holds and what it allocated.
    run(&["profile", "--memory", "--duration", "2", "--output", "memory.pprof", "--", "allocating.to"]);
    let memory = show(&["memory.pprof"]);
    assert_eq!(memory["type"], "inuse_space", "{memory}");
    assert_eq!(memory["types"], json!(["alloc_objects", "alloc_space", "inuse_objects", "inuse_space"]));
    // Twenty thousand strings of about 450 bytes, estimated from samples.
    let held = memory["total"].as_i64().unwrap();
    assert!((4_000_000..20_000_000).contains(&held), "{memory}");
    let line = &memory["lines"][0];
    assert_eq!((&line["file"], &line["line"]), (&json!("allocating.to"), &json!(5)), "{memory}");
    // Where each task waits.
    run(&["profile", "--tasks", "--duration", "1", "--output", "tasks.pprof", "--", "allocating.to"]);
    let tasks = show(&["tasks.pprof"]);
    assert_eq!(tasks["total"], 3, "{tasks}");
    let sleeping = tasks["lines"].as_array().unwrap().iter().find(|l| l["line"] == 9).expect("the sleepers' line");
    assert_eq!(sleeping["flat"], 2, "{tasks}");
    // What goes with what.
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["profile", "--cpu", "--memory", "--", "busy.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("a profile is of one thing"));
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["profile", "show", "cpu.pprof", "--sample", "space"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("the profile has no `space`: it has samples, cpu"));
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
        tasks = read(&["tasks"])["tasks"].clone();
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
    let heap = read(&["heap"])["heap"].clone();
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
    let script = "break debugged.to:13\ncontinue\nlocals\nstep\nstack\nprint user\nframe 1\nprint ada.name\nchildren names\nframe 0\nfinish\ndelete 1\ncontinue\n";
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
    assert_eq!(user["children"], 2);
    // The caller's frame, and parts of its bindings by their paths.
    assert!(lines.iter().any(|l| l["selected"] == json!({ "task": 1, "frame": 1 })), "{lines:?}");
    let name = lines.iter().find(|l| l["name"] == "ada.name").unwrap();
    assert_eq!((&name["value"], &name["type"]), (&json!("\"Ada\""), &Value::Null));
    let parts = lines.iter().find_map(|l| l["children"].as_array()).unwrap();
    let parts: Vec<(&str, &str)> =
        parts.iter().map(|p| (p["path"].as_str().unwrap(), p["value"].as_str().unwrap())).collect();
    assert_eq!(parts, [("names[0]", "\"a\""), ("names[1]", "\"b\"")]);
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

/// The debugger runs one test of a project, stopping in it, and says how
/// it ended.
#[test]
fn debug_adapter_runs_a_test() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("daptests");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.to"), "pub fn add(a: Int, b: Int): Int {\n    a + b\n}\n\nfn main() {}\n")
        .unwrap();
    let tests = "test adds {\n    assert(add(1, 2) == 3)\n}\n\ntest fails {\n    let sum = add(1, 2)\n    assert(sum == 4, \"wrong sum\")\n}\n";
    std::fs::write(dir.join("src/main_test.to"), tests).unwrap();
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
    let mut events = Vec::new();
    let response = |output: &mut BufReader<_>, command: &str, events: &mut Vec<Value>| loop {
        let message = dap_read(output);
        if message["type"] == "response" && message["command"] == command {
            assert_eq!(message["success"], true, "{message}");
            return message;
        }
        events.push(message);
    };
    send("initialize", json!({ "adapterID": "tondo" }));
    response(&mut output, "initialize", &mut events);
    send("launch", json!({ "program": ".", "cwd": dir, "test": "daptests::unit::daptests::fails" }));
    response(&mut output, "launch", &mut events);
    let path = dir.join("src/main_test.to");
    send("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 7 }] }));
    response(&mut output, "setBreakpoints", &mut events);
    send("configurationDone", json!({}));
    response(&mut output, "configurationDone", &mut events);
    let stopped = loop {
        let message = events.pop().unwrap_or_else(|| dap_read(&mut output));
        if message["event"] == "stopped" {
            break message;
        }
    };
    send("stackTrace", json!({ "threadId": stopped["body"]["threadId"] }));
    let trace = response(&mut output, "stackTrace", &mut events);
    assert_eq!(trace["body"]["stackFrames"][0]["line"], 7, "{trace}");
    // The failing assertion stops on its line again as it reports.
    let mut said = String::new();
    send("continue", json!({ "threadId": 1 }));
    loop {
        let message = if events.is_empty() { dap_read(&mut output) } else { events.remove(0) };
        match message["event"].as_str() {
            Some("output") => said.push_str(message["body"]["output"].as_str().unwrap_or_default()),
            Some("stopped") => send("continue", json!({ "threadId": 1 })),
            Some("terminated") => break,
            _ => {}
        }
    }
    assert!(said.contains("the test failed: panic[P0007]: assertion-failed: wrong sum"), "{said}");
    assert!(!said.contains('\x1e'), "{said}");
    send("disconnect", json!({}));
    drop(input);
    child.wait().unwrap();
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
    let thread = stopped["body"]["threadId"].clone();
    send("stackTrace", json!({ "threadId": thread }));
    let trace = response(&mut output, "stackTrace", &mut events);
    assert_eq!(trace["body"]["stackFrames"][0]["line"], 13);
    // The frame's bindings, and the parts of one, opened.
    let variables =
        |send: &mut dyn FnMut(&str, Value), frame: &Value, output: &mut BufReader<_>, events: &mut Vec<Value>| {
            send("scopes", json!({ "frameId": frame }));
            let scopes = response(output, "scopes", events);
            send("variables", json!({ "variablesReference": scopes["body"]["scopes"][0]["variablesReference"] }));
            response(output, "variables", events)["body"]["variables"].clone()
        };
    let frame = trace["body"]["stackFrames"][0]["id"].clone();
    let locals = variables(&mut send, &frame, &mut output, &mut events);
    assert_eq!(locals[0]["name"], "ada");
    let opened = locals[0]["variablesReference"].clone();
    assert_ne!(opened, 0);
    send("variables", json!({ "variablesReference": opened }));
    let parts = response(&mut output, "variables", &mut events)["body"]["variables"].clone();
    let parts: Vec<(&str, &str)> =
        parts.as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap(), p["value"].as_str().unwrap())).collect();
    assert_eq!(parts, [("name", "\"Ada\""), ("age", "36")]);
    send("evaluate", json!({ "expression": "names[1]", "frameId": frame }));
    let evaluated = response(&mut output, "evaluate", &mut events);
    assert_eq!(evaluated["body"]["result"], "\"b\"");
    // Into the call: the caller's frame still shows its bindings.
    send("stepIn", json!({ "threadId": thread }));
    response(&mut output, "stepIn", &mut events);
    until(&mut output, "stopped", &mut events);
    send("stackTrace", json!({ "threadId": thread }));
    let trace = response(&mut output, "stackTrace", &mut events);
    let frames = trace["body"]["stackFrames"].as_array().unwrap().clone();
    assert_eq!((frames[0]["name"].as_str(), frames[1]["name"].as_str()), (Some("older"), Some("main")));
    let outer = variables(&mut send, &frames[1]["id"], &mut output, &mut events);
    assert_eq!(outer[0]["name"], "ada");
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
    let types = tondo(&["dump", "read", file, "objects", "--json"])["objects"].clone();
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
    let object = tondo(&["dump", "read", file, "object", address, "--json"])["object"].clone();
    assert_eq!(object["type"], "String");
    let chain = object["chain"].as_array().unwrap();
    assert_eq!(chain[0], "task 1");
    assert!(chain[1].as_str().unwrap().starts_with("Array[String]"), "{object}");
}

const STOPPING: &str = "import std.console

fn pick(values: Array[Int], at: Int): Int {
    return values[at]
}

fn main() {
    var total = 0
    for i in 0..4 {
        total = total + i
    }
    console.println(\"{total}\")
    console.println(\"{pick([1, 2], total)}\")
}
";

/// A logpoint writes and goes on; a count of hits stops at the one asked
/// for; a binding changes; and a panic stops where the program's code
/// panicked, before it unwinds.
#[test]
fn debugger_logs_counts_changes_and_stops_at_panics() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-stops");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stopping.to"), STOPPING).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "stopping.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break stopping.to:10 log at {i}: {total}\nbreak stopping.to:10 hits 3\ncontinue\nset total 40\ncontinue\nlocals\nframe 1\nprint total\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let logged: Vec<&str> = lines.iter().filter_map(|l| l["logged"].as_str()).collect();
    assert_eq!(logged, ["at 0: 0", "at 1: 0", "at 2: 1", "at 3: 42"], "{lines:?}");
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    assert_eq!(
        (stops[1]["why"].as_str(), stops[1]["at"].as_str()),
        (Some("breakpoint 2"), Some("stopping.to:10 in main"))
    );
    let set = lines.iter().find(|l| l["name"] == "total").unwrap();
    assert_eq!(set["value"], "40");
    // 40, then + 2 and + 3: `pick` reads past the end, and the panic
    // stops there.
    let panic = stops[2]["why"].as_str().unwrap();
    assert!(panic.starts_with("panic[P0001]: bounds: index 45 is out of bounds"), "{panic}");
    assert_eq!(stops[2]["at"], "stopping.to:4 in pick");
    let locals = lines.iter().find_map(|l| l["locals"].as_array()).unwrap();
    assert_eq!(locals.iter().find(|l| l["name"] == "at").unwrap()["value"], "45");
    let total = lines.iter().filter(|l| l["name"] == "total").nth(1).unwrap();
    assert_eq!(total["value"], "45");
    assert!(lines.iter().any(|l| l.get("exited").is_some()));
}

/// The Debug Adapter Protocol for one test: requests in order, and what
/// comes back.
struct Adapter {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: BufReader<std::process::ChildStdout>,
    seq: u64,
    events: Vec<Value>,
}

impl Adapter {
    fn start() -> Adapter {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("dap")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Adapter { child, input, output, seq: 0, events: Vec::new() }
    }

    fn ask(&mut self, command: &str, arguments: Value) -> Value {
        self.seq += 1;
        let text =
            json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        std::io::Write::write_all(&mut self.input, format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
            .unwrap();
        loop {
            let message = dap_read(&mut self.output);
            if message["type"] == "response" && message["command"] == command {
                assert_eq!(message["success"], true, "{message}");
                return message["body"].clone();
            }
            self.events.push(message);
        }
    }

    fn until(&mut self, event: &str) -> Value {
        if let Some(at) = self.events.iter().position(|e| e["event"] == event) {
            return self.events.remove(at)["body"].clone();
        }
        loop {
            let message = dap_read(&mut self.output);
            if message["event"] == event {
                return message["body"].clone();
            }
        }
    }

    fn stop(mut self) {
        self.ask("disconnect", json!({}));
        drop(self.input);
        self.child.wait().unwrap();
    }
}

#[test]
fn debug_adapter_stops_at_panics_and_opens_dumps() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-panics");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stopping.to"), STOPPING).unwrap();
    let mut adapter = Adapter::start();
    let capabilities = adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    assert_eq!(capabilities["exceptionBreakpointFilters"][0]["filter"], "panic");
    adapter.ask("launch", json!({ "program": "stopping.to", "cwd": dir }));
    adapter.until("initialized");
    adapter.ask("setExceptionBreakpoints", json!({ "filters": ["panic"] }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    assert_eq!(stopped["reason"], "exception");
    assert!(stopped["text"].as_str().unwrap().contains("index 6 is out of bounds"), "{stopped}");
    let info = adapter.ask("exceptionInfo", json!({ "threadId": stopped["threadId"] }));
    assert_eq!(info["exceptionId"], "panic");
    adapter.ask("continue", json!({ "threadId": stopped["threadId"] }));
    adapter.until("terminated");
    adapter.stop();

    // A dump of a program that panicked: its task and where it was.
    let crashes = dir.join("crashes");
    let _ = std::fs::remove_dir_all(&crashes);
    std::fs::create_dir_all(&crashes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "stopping.to"])
        .env("TONDO_DUMP", &crashes)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let dump = std::fs::read_dir(&crashes).unwrap().next().unwrap().unwrap().path();
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "dump": dump, "cwd": dir }));
    adapter.until("initialized");
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let threads = adapter.ask("threads", json!({}));
    assert_eq!(threads["threads"].as_array().unwrap().len(), 1, "{threads}");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    let frames = trace["stackFrames"].as_array().unwrap();
    let pick = frames.iter().find(|f| f["name"] == "pick").unwrap_or_else(|| panic!("{trace}"));
    assert_eq!((pick["line"].as_u64(), pick["source"]["name"].as_str()), (Some(4), Some("stopping.to")));
    let scopes = adapter.ask("scopes", json!({ "frameId": pick["id"] }));
    let task = adapter.ask("variables", json!({ "variablesReference": scopes["scopes"][0]["variablesReference"] }));
    assert_eq!(task["variables"][0]["name"], "state");
    adapter.stop();
}
