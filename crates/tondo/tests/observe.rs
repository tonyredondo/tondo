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
    let said = || format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{}", said());
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
    // `crunch`, inlined, has no time of its own, so it ranks after every
    // function that has some, which an emulator gives more of.
    let cpu = show(&["cpu.pprof", "--top", "1000"]);
    assert_eq!(
        (&cpu["format"], &cpu["type"], &cpu["unit"]),
        (&json!("tondo-profile-show-0.1/1"), &json!("cpu"), &json!("nanoseconds"))
    );
    let functions: Vec<&str> =
        cpu["functions"].as_array().unwrap().iter().map(|f| f["function"].as_str().unwrap()).collect();
    assert!(functions.contains(&"fib") && functions.contains(&"crunch"), "{cpu}");
    // The runtime's functions have their names too, from the executable's
    // symbols, on macOS as elsewhere.
    assert!(functions.iter().any(|f| f.starts_with("tondo_runtime::")), "{cpu}");
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

const LABELED: &str = r#"import std.profile

fn fib(n: Int): Int {
    if n < 2 {
        return n
    }
    fib(n - 1) + fib(n - 2)
}

// A context's field marked `@label` is a label too (language 11.16).
context Route = {
    @label("endpoint") path: String = ""
}

fn users(): Int {
    with Route { path: "/users" }
    fib(25)
}

fn main() {
    for _ in 0..4000 {
        users()
        profile.withLabels(["endpoint": "/orders"], () { fib(23) })
    }
}
"#;

#[test]
fn profiles_carry_labels() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("profile-labels");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("labeled.to"), LABELED).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(&dir).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    run(&["profile", "--output", "labeled.pprof", "--", "labeled.to"]);
    let shown: Value = serde_json::from_str(&run(&["profile", "show", "labeled.pprof", "--json"])).unwrap();
    let labels: Vec<(&str, &str, i64)> = shown["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["label"].as_str().unwrap(), l["value"].as_str().unwrap(), l["total"].as_i64().unwrap()))
        .collect();
    // fib(25) takes more than twice what fib(23) does.
    let [("endpoint", "/users", users), ("endpoint", "/orders", orders)] = labels[..] else { panic!("{shown}") };
    assert!(users > orders, "{shown}");
    let text = run(&["profile", "show", "labeled.pprof"]);
    assert!(text.contains("by label:\n") && text.contains("%  endpoint=/users\n"), "{text}");
}

const DUMPED: &str = r#"import std.console

context Request = {
    id: String = ""
}

fn sleeper(woken: Receiver[Int]) {
    let _ = woken.receive()
}

fn main() {
    var kept: Array[String] = []
    for i in 0..80000 {
        kept.push("entry {i} ".repeat(10))
    }
    let (wake, woken) = channel[Int]()
    with Request { id: "r9" }
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
    assert_eq!(task("main")["at"], "dumped.to:21 in main");
    assert_eq!(task("sleeper")["at"], "dumped.to:8 in sleeper");
    assert_eq!(task("sleeper")["spawned"][0], "dumped.to:18 in main");
    // The task it starts has its contexts.
    let contexts = json!([{ "name": "Request", "fields": { "id": "\"r9\"" } }]);
    assert_eq!(task("main")["contexts"], contexts);
    assert_eq!(task("sleeper")["contexts"], contexts);
    // About 8 MiB of strings, from where the program made them: with a
    // sample every 512 KiB, a run without one is as good as impossible.
    let heap = read(&["heap"])["heap"].clone();
    let strings = heap["sites"].as_array().unwrap().iter().find(|s| s["kind"] == "String");
    let strings = strings.unwrap_or_else(|| panic!("no strings in {heap}"));
    assert_eq!(strings["site"], "dumped.to:14 in main");
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
    with Trace { depth: 2 }
    let ada = User { name: "Ada", age: 36 }
    let names = ["a", "b"]
    let aged = older(ada)
    console.println("{aged.age} {names.length()}")
}

context Trace = {
    depth: Int = 0
    path: String = "/"
    tags: Array[String] = ["start"]
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
    let script = "break debugged.to:14\ncontinue\nlocals\nstep\nstack\nprint user\ncontext\nframe 1\nprint ada.name\nchildren names\nframe 0\nfinish\ndelete 1\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    assert_eq!(stops[0]["why"], "start");
    assert_eq!(stops[1]["at"], "debugged.to:14 in main");
    assert_eq!(stops[2]["at"], "debugged.to:6 in older");
    let locals = lines.iter().find_map(|l| l["locals"].as_array()).unwrap();
    let shown: Vec<String> = locals.iter().map(|l| format!("{} = {}", l["name"], l["value"])).collect();
    assert_eq!(shown, [r#""ada" = "User { name: \"Ada\", age: 36 }""#, r#""names" = "[\"a\", \"b\"]""#]);
    let stack = lines.iter().find_map(|l| l["stack"].as_array()).unwrap();
    assert_eq!(stack[..2], [json!("debugged.to:6 in older"), json!("debugged.to:14 in main")]);
    let user = lines.iter().find(|l| l["name"] == "user").unwrap();
    assert_eq!(user["value"], "User { name: \"Ada\", age: 36 }");
    assert_eq!(user["children"], 2);
    // The task's contexts: what a `with` set, and where.
    let contexts = lines.iter().find_map(|l| l["contexts"].as_array()).unwrap();
    assert_eq!(contexts[0]["name"], "Trace");
    let fields: Vec<(&str, &str, &Value)> = contexts[0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["value"].as_str().unwrap(), &f["set"]))
        .collect();
    assert_eq!(
        fields,
        [
            ("depth", "2", &json!("debugged.to:11:5 in main")),
            ("path", "\"/\"", &Value::Null),
            ("tags", "[\"start\"]", &Value::Null)
        ]
    );
    assert_eq!(contexts[0]["fields"][2]["children"], 1);
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

const CLOSED: &str = r#"fn total(items: Array[Int]): Array[Int] {
    let offset = 3
    let label = "total"
    items.map((item) { item + offset })
}

fn main() {
    _ = total([1, 2])
}
"#;

/// A closure's frame shows what it captured as its own bindings, and the
/// bindings of the function it is written in, whose frame is below it.
#[test]
fn debugger_shows_what_a_block_can_use() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-closure");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("closed.to"), CLOSED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "closed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break closed.to:4\ncontinue\ncontinue\nlocals\nprint label\ndelete 1\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    assert_eq!(stops[2]["at"], "closed.to:4 in closure in total");
    let locals = lines.iter().find_map(|l| l["locals"].as_array()).unwrap();
    let shown: Vec<String> = locals.iter().map(|l| format!("{} = {} {}", l["name"], l["value"], l["from"])).collect();
    assert_eq!(
        shown,
        [
            r#""item" = "1" null"#,
            r#""offset" = "3" null"#,
            r#""items" = "[1, 2]" "total""#,
            r#""label" = "\"total\"" "total""#
        ]
    );
    let label = lines.iter().find(|l| l["name"] == "label").unwrap();
    assert_eq!(label["value"], "\"total\"");
    // A breakpoint's condition and message read them too.
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "closed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break closed.to:4 if label == \"total\" log {label}: {item}\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let logged: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|l| l["logged"].as_str().map(String::from))
        .collect();
    assert_eq!(logged, ["total: {item?}", "total: 1", "total: 2"]);
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
    let tests = "test adds {\n    assert(add(1, 2) == 3)\n}\n\nsuite sums {\n    let base = 1\n    let unused = 5\n    test fails {\n        let sum = add(base, 2)\n        assert(sum == 4, \"wrong sum\")\n    }\n}\n";
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
    send("launch", json!({ "program": ".", "cwd": dir, "test": "daptests::unit::daptests::sums::fails" }));
    response(&mut output, "launch", &mut events);
    let path = dir.join("src/main_test.to");
    send("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 10 }] }));
    response(&mut output, "setBreakpoints", &mut events);
    // A file of the other system has no code in the program.
    let (here, other) = if cfg!(target_os = "macos") { ("macOS", "linux") } else { ("Linux", "macos") };
    let elsewhere = dir.join(format!("src/only_{other}.to"));
    std::fs::write(&elsewhere, "fn only() {}\n").unwrap();
    send("setBreakpoints", json!({ "source": { "path": elsewhere }, "breakpoints": [{ "line": 1 }] }));
    let set = response(&mut output, "setBreakpoints", &mut events);
    let shown = if other == "macos" { "macOS" } else { "Linux" };
    assert_eq!(
        set["body"]["breakpoints"],
        json!([{ "verified": false, "line": 1, "message": format!("this file is for {shown}; the program runs on {here}") }])
    );
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
    assert_eq!(trace["body"]["stackFrames"][0]["line"], 10, "{trace}");
    assert_eq!(trace["body"]["stackFrames"][1]["name"], "suite sums", "{trace}");
    // The test's bindings, what it captured of its suite among them, then
    // the rest of its suite's, and the contexts.
    send("scopes", json!({ "frameId": trace["body"]["stackFrames"][0]["id"] }));
    let scopes = response(&mut output, "scopes", &mut events)["body"]["scopes"].clone();
    let names: Vec<&str> = scopes.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Locals", "suite sums", "Context"]);
    let mut shown = |reference: &Value| {
        send("variables", json!({ "variablesReference": reference }));
        let variables = response(&mut output, "variables", &mut events)["body"]["variables"].clone();
        variables.as_array().unwrap().iter().map(|v| format!("{} = {}", v["name"], v["value"])).collect::<Vec<_>>()
    };
    assert_eq!(shown(&scopes[0]["variablesReference"]), [r#""sum" = "3""#, r#""base" = "1""#]);
    assert_eq!(shown(&scopes[1]["variablesReference"]), [r#""unused" = "5""#]);
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
    send("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 14 }] }));
    let set = response(&mut output, "setBreakpoints", &mut events);
    assert_eq!(set["body"]["breakpoints"][0]["verified"], true);
    send("configurationDone", json!({}));
    response(&mut output, "configurationDone", &mut events);
    let stopped = until(&mut output, "stopped", &mut events);
    assert_eq!(stopped["body"]["reason"], "breakpoint");
    let thread = stopped["body"]["threadId"].clone();
    send("stackTrace", json!({ "threadId": thread }));
    let trace = response(&mut output, "stackTrace", &mut events);
    assert_eq!(trace["body"]["stackFrames"][0]["line"], 14);
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
    // The console completes bindings, a value's fields and methods.
    let mut complete = |text: &str| {
        let column = text.chars().count() + 1;
        send("completions", json!({ "text": text, "column": column, "frameId": frame }));
        let targets = response(&mut output, "completions", &mut events)["body"]["targets"].clone();
        targets.as_array().unwrap().iter().map(|t| t["label"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(complete("1 + na"), ["names"]);
    assert_eq!(complete("ada.a"), ["age"]);
    assert_eq!(complete("names[0].is"), ["isEmpty()", "isSome()", "isNone()", "isOk()", "isErr()"]);
    // Into the call the line makes, by its target: the caller's frame
    // still shows its bindings.
    send("stepInTargets", json!({ "frameId": frame }));
    let targets = response(&mut output, "stepInTargets", &mut events)["body"]["targets"].clone();
    assert_eq!(targets, json!([{ "id": 0, "label": "older" }]));
    send("stepIn", json!({ "threadId": thread, "targetId": 0 }));
    response(&mut output, "stepIn", &mut events);
    until(&mut output, "stopped", &mut events);
    send("stackTrace", json!({ "threadId": thread }));
    let trace = response(&mut output, "stackTrace", &mut events);
    let frames = trace["body"]["stackFrames"].as_array().unwrap().clone();
    assert_eq!((frames[0]["name"].as_str(), frames[1]["name"].as_str()), (Some("older"), Some("main")));
    let outer = variables(&mut send, &frames[1]["id"], &mut output, &mut events);
    assert_eq!(outer[0]["name"], "ada");
    // The task's contexts, a scope of every frame.
    send("scopes", json!({ "frameId": frames[0]["id"] }));
    let scopes = response(&mut output, "scopes", &mut events)["body"]["scopes"].clone();
    let context = scopes.as_array().unwrap().iter().find(|s| s["name"] == "Context").expect("a Context scope");
    send("variables", json!({ "variablesReference": context["variablesReference"] }));
    let contexts = response(&mut output, "variables", &mut events)["body"]["variables"].clone();
    assert_eq!(contexts[0]["name"], "Trace");
    send("variables", json!({ "variablesReference": contexts[0]["variablesReference"] }));
    let fields = response(&mut output, "variables", &mut events)["body"]["variables"].clone();
    let depth = fields[0]["value"].as_str().unwrap();
    assert!(depth.starts_with("2  (with at ") && depth.ends_with("debugged.to:11:5 in main)"), "{fields}");
    assert_eq!((&fields[1]["name"], &fields[1]["value"]), (&json!("path"), &json!("\"/\"")));
    send("continue", json!({ "threadId": 1 }));
    response(&mut output, "continue", &mut events);
    until(&mut output, "terminated", &mut events);
    send("disconnect", json!({}));
    drop(input);
    child.wait().unwrap();
}

const WATCHED: &str = r#"import std.console

enum Problem {
    Empty
}

fn bump(counter: mut Int) {
    counter += 10
}

fn sum(items: Array[Int]): Int {
    var total = 0
    for item in items {
        if item == 3 {
            bump(mut total)
        }
        total += item
    }
    total
}

fn first(items: Array[Int]): Int ! Problem {
    if items.isEmpty() {
        fail Problem.Empty
    }
    items[0]
}

fn parsed(text: String): Int ! Problem {
    let value = Int.parse(text).mapErr((_) { Problem.Empty })?
    value
}

fn main() {
    var many: Array[Int] = []
    for i in 0..250 {
        many.push(i)
    }
    console.println("{sum([1, 2, 3, 4])}")
    match first([]) {
        ok(n) => console.println("{n}")
        err(_) => console.println("empty")
    }
    match parsed("x") {
        ok(n) => console.println("{n}")
        err(_) => console.println("not a number")
    }
}
"#;

/// Expressions in `print` and in conditions, a breakpoint where a function
/// starts, a page of an array's items, a watch that stops after the line
/// that changed its value, also in a call, and stops where errors start.
#[test]
fn debugger_evaluates_watches_and_catches_errors() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-watch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("summed.to"), WATCHED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "summed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break summed.to:17 if totl > 1\nbreak summed.to:17 if total >\nbreak fn nothing\n\
        break fn sum\ncatch panic error\ncontinue\nprint items.length() * 2 + items[items.length() - 1]\n\
        print items[0] == 1 and not items.isEmpty()\nframe 1\nchildren many 248 5\nframe 0\nnext\nwatch total\ndelete 1\n\
        continue\ncontinue\ncontinue\ndelete 2\ncontinue\ncontinue\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let errors: Vec<&str> = lines.iter().filter_map(|l| l["error"].as_str()).collect();
    assert_eq!(errors.len(), 3, "{lines:?}");
    assert!(errors[0].starts_with("`totl` is not a binding at summed.to:17 in sum; these are: items, total"));
    assert_eq!(errors[1], "the condition: the expression ends too soon");
    assert!(errors[2].starts_with("no function `nothing` has code in the program"), "{}", errors[2]);
    let computed: Vec<(&Value, &Value)> =
        lines.iter().filter(|l| l["name"].is_string()).map(|l| (&l["value"], &l["type"])).collect();
    assert_eq!(computed, [(&json!("12"), &json!("Int")), (&json!("true"), &json!("Bool"))]);
    let page = lines.iter().find(|l| l["children"].is_array()).unwrap();
    assert_eq!(page["total"], 250);
    let paths: Vec<&str> = page["children"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["many[248]", "many[249]"]);
    assert_eq!(lines.iter().find_map(|l| l.get("watch")).unwrap()["value"], "0");
    let stops: Vec<(&str, &str)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), s["why"].as_str().unwrap()))
        .collect();
    assert_eq!(
        stops[1..],
        [
            ("summed.to:12 in sum", "breakpoint 1"),
            ("summed.to:13 in sum", "step"),
            ("summed.to:13 in sum", "watch 2: total changed from 0 to 1 at summed.to:17 in sum"),
            ("summed.to:13 in sum", "watch 2: total changed from 1 to 3 at summed.to:17 in sum"),
            ("summed.to:17 in sum", "watch 2: total changed from 3 to 13 at summed.to:8 in bump"),
            ("summed.to:24 in first", "error: Problem.Empty"),
            ("summed.to:30 in parsed", "error: Problem.Empty"),
        ]
    );
    assert!(lines.iter().any(|l| l.get("exited").is_some()));
}

/// The adapter's function breakpoints, errors as exceptions, pages of an
/// array, expressions, and watches as data breakpoints.
#[test]
fn debug_adapter_watches_and_pages() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-watch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("summed.to"), WATCHED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let (mut seq, mut events) = (0, Vec::new());
    // The body of the response to a request; with no command, of the next
    // stop.
    let mut ask = move |command: &str, arguments: Value| -> Value {
        if command.is_empty() {
            if let Some(at) = events.iter().position(|e: &Value| e["event"] == "stopped") {
                return events.remove(at)["body"].clone();
            }
            loop {
                let message = dap_read(&mut output);
                if message["event"] == "stopped" {
                    return message["body"].clone();
                }
            }
        }
        seq += 1;
        let text = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        std::io::Write::write_all(&mut input, format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
            .unwrap();
        loop {
            let message = dap_read(&mut output);
            if message["type"] == "response" && message["command"] == command {
                assert_eq!(message["success"], true, "{message}");
                return message["body"].clone();
            }
            events.push(message);
        }
    };
    ask("initialize", json!({ "adapterID": "tondo" }));
    ask("launch", json!({ "program": "summed.to", "cwd": dir, "record": 1000 }));
    let set = ask("setFunctionBreakpoints", json!({ "breakpoints": [{ "name": "sum" }] }));
    assert_eq!((&set["breakpoints"][0]["verified"], &set["breakpoints"][0]["line"]), (&json!(true), &json!(12)));
    let caught = json!({ "filters": ["panic"], "filterOptions": [{ "filterId": "error", "condition": "Problem" }] });
    ask("setExceptionBreakpoints", caught);
    ask("configurationDone", json!({}));
    assert_eq!(ask("", Value::Null)["reason"], "breakpoint");
    let trace = ask("stackTrace", json!({ "threadId": 1 }));
    let frames = trace["stackFrames"].as_array().unwrap().clone();
    let scopes = ask("scopes", json!({ "frameId": frames[1]["id"] }));
    let locals = ask("variables", json!({ "variablesReference": scopes["scopes"][0]["variablesReference"] }));
    let many = locals["variables"].as_array().unwrap().iter().find(|v| v["name"] == "many").unwrap().clone();
    assert_eq!(many["indexedVariables"], 250);
    let page = ask(
        "variables",
        json!({ "variablesReference": many["variablesReference"], "filter": "indexed", "start": 248, "count": 2 }),
    );
    let names: Vec<&str> = page["variables"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["[248]", "[249]"]);
    let evaluated = ask("evaluate", json!({ "expression": "items.length() + 1", "frameId": frames[0]["id"] }));
    assert_eq!((&evaluated["result"], &evaluated["type"]), (&json!("5"), &json!("Int")));
    // `total` holds a value from the next line on.
    ask("next", json!({ "threadId": 1 }));
    ask("", Value::Null);
    let trace = ask("stackTrace", json!({ "threadId": 1 }));
    let frames = trace["stackFrames"].as_array().unwrap().clone();
    let scopes = ask("scopes", json!({ "frameId": frames[0]["id"] }));
    let reference = scopes["scopes"][0]["variablesReference"].clone();
    let info = ask("dataBreakpointInfo", json!({ "variablesReference": reference, "name": "total" }));
    assert_eq!(info["description"], "total");
    let set = ask("setDataBreakpoints", json!({ "breakpoints": [{ "dataId": info["dataId"] }] }));
    assert_eq!(set["breakpoints"][0]["verified"], true);
    ask("setFunctionBreakpoints", json!({ "breakpoints": [] }));
    ask("continue", json!({ "threadId": 1 }));
    let stopped = ask("", Value::Null);
    assert_eq!(stopped["reason"], "data breakpoint", "{stopped}");
    let why = stopped["description"].as_str().unwrap();
    assert!(why.starts_with("watch 2: total changed from 0 to 1 at ") && why.ends_with("summed.to:17 in sum"), "{why}");
    // A line back in the history, where `total` changed, and forward to
    // the present again.
    ask("stepBack", json!({ "threadId": 1 }));
    assert_eq!(ask("", Value::Null)["reason"], "step");
    assert_eq!(ask("stackTrace", json!({ "threadId": 1 }))["stackFrames"][0]["line"], 17);
    ask("next", json!({ "threadId": 1 }));
    ask("", Value::Null);
    assert_eq!(ask("stackTrace", json!({ "threadId": 1 }))["stackFrames"][0]["line"], 13);
    // The line that gave `total` its value, of the variable's menu, in the
    // history too; and on again.
    let frame = ask("stackTrace", json!({ "threadId": 1 }))["stackFrames"][0]["id"].clone();
    let why = ask("why", json!({ "expression": "total", "frameId": frame }));
    assert!(why["at"].as_str().unwrap().ends_with("summed.to:17 in sum"), "{why}");
    assert_eq!(why["why"], "`total` became 1 on this line", "{why}");
    ask("", Value::Null);
    ask("next", json!({ "threadId": 1 }));
    ask("", Value::Null);
    assert_eq!(ask("stackTrace", json!({ "threadId": 1 }))["stackFrames"][0]["line"], 13);
    // Beside each line of the call, what it changed the last time it ran.
    let effects = ask("effects", json!({ "frameId": frame }));
    let text = |line: u64| {
        let lines = effects["lines"].as_array().unwrap();
        lines.iter().find(|l| l["line"] == line).map(|l| l["text"].as_str().unwrap().to_string())
    };
    assert_eq!((text(13), text(14), text(17)), (Some("item = 1".into()), None, Some("total = 1".into())), "{effects}");
    assert!(effects["lines"][0]["path"].as_str().unwrap().ends_with("summed.to"), "{effects}");
    ask("setDataBreakpoints", json!({ "breakpoints": [] }));
    // Out of `sum`, its frame's scope shows what it returned.
    ask("stepOut", json!({ "threadId": 1 }));
    assert_eq!(ask("", Value::Null)["reason"], "step");
    let trace = ask("stackTrace", json!({ "threadId": 1 }));
    let scopes = ask("scopes", json!({ "frameId": trace["stackFrames"][0]["id"] }));
    let locals = ask("variables", json!({ "variablesReference": scopes["scopes"][0]["variablesReference"] }));
    assert_eq!(
        (&locals["variables"][0]["name"], &locals["variables"][0]["value"]),
        (&json!("(sum returned)"), &json!("20"))
    );
    // An item changed in place, by what an expression gives.
    let frame = trace["stackFrames"][0]["id"].clone();
    let set = ask("setExpression", json!({ "expression": "many[1]", "value": "many[2] * 10", "frameId": frame }));
    assert_eq!(set["value"], "20");
    ask("continue", json!({ "threadId": 1 }));
    let stopped = ask("", Value::Null);
    assert_eq!((&stopped["reason"], &stopped["text"]), (&json!("exception"), &json!("error: Problem.Empty")));
    let info = ask("exceptionInfo", json!({ "threadId": 1 }));
    assert_eq!(info["exceptionId"], "error");
    ask("disconnect", json!({}));
    drop(ask);
    child.wait().unwrap();
}

const WAITED: &str = r#"import std.console
import std.json

type Point = { x: Int, y: Int }

fn waiter(ready: Sender[Bool], inbox: Receiver[Int]) {
    let label = "waiter"
    let (_, got) = (ready.send(true), inbox.receive())
    console.println("{label} {got}")
}

fn twice(n: Int): Int {
    n * 2
}

fn main() {
    let (ready, started) = channel[Bool](1)
    let (outbox, inbox) = channel[Int](1)
    spawn waiter(ready, inbox)
    _ = started.receive()
    let sent = 1
    let text = json.encode(Point { x: 1, y: 2 })
    let doubled = twice(21)
    outbox.send(sent + doubled)
}
"#;

/// A task that waits shows where and its bindings; steps go over the
/// standard library's and derived code, stop on a line that computes
/// nothing, and say what a function returned.
#[test]
fn debugger_shows_waiting_tasks_and_returned_values() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-wait");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("waited.to"), WAITED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "waited.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break waited.to:21\ncontinue\ntasks\nstack 2\ntask 2\nprint label\ntask 1\nnext\nstep\nstep\nfinish\n\
        continue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let tasks = lines.iter().find_map(|l| l["tasks"].as_array()).unwrap();
    // It waits in `receive`, or, sent its value, is ready to go there; or
    // the stop found it on another worker at the line's probe, before it.
    assert_eq!((&tasks[1]["task"], &tasks[1]["at"]), (&json!(2), &json!("waited.to:8 in waiter")));
    assert!(["waits", "ready", "held"].contains(&tasks[1]["why"].as_str().unwrap_or_default()), "{tasks:?}");
    assert_eq!(lines.iter().find_map(|l| l["stack"].as_array()).unwrap()[0], "waited.to:8 in waiter");
    assert_eq!(lines.iter().find(|l| l["name"] == "label").unwrap()["value"], "\"waiter\"");
    let stops: Vec<(&str, &Value)> =
        lines.iter().filter_map(|l| l.get("stopped")).map(|s| (s["at"].as_str().unwrap(), &s["returned"])).collect();
    assert_eq!(
        stops[1..],
        [
            ("waited.to:21 in main", &Value::Null),
            ("waited.to:22 in main", &Value::Null),
            ("waited.to:23 in main", &Value::Null),
            ("waited.to:13 in twice", &Value::Null),
            ("waited.to:24 in main", &json!({ "function": "twice", "value": "42" })),
        ]
    );
}

const CHANGED: &str = r#"import std.console

enum Role {
    Guest
    Admin
}

type User = { name: String, age: Int, role: Role, scores: Array[Int], pair: (Int, Bool) }

fn main() {
    var count = 1
    var user = User { name: "Ada", age: 36, role: Role.Guest, scores: [1, 2, 3], pair: (1, false) }
    let copy = user.scores
    var role = Role.Guest
    console.println("{count} {user.name} {user.age} {user.role} {user.scores} {copy} {role} {user.pair.1}")
}
"#;

/// `set` changes a binding or a part of one to what an expression gives;
/// an array another value shares is copied first.
#[test]
fn debugger_changes_parts_of_values() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-set");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("changed.to"), CHANGED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "changed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break changed.to:15\ncontinue\nset count count + 41\nset user.age = user.age + 1\n\
        set user.name \"Ada L\"\nset user.role Role.Admin\nset role Admin\nset user.pair.1 true\n\
        set user.scores[count - 42] 9\nset user.nope 1\nset count \"x\"\nset user.scores[3] 1\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let set: Vec<(&str, &str)> = lines
        .iter()
        .filter(|l| l["name"].is_string())
        .map(|l| (l["name"].as_str().unwrap(), l["value"].as_str().unwrap()))
        .collect();
    assert_eq!(
        set,
        [
            ("count", "42"),
            ("user.age", "37"),
            ("user.name", "\"Ada L\""),
            ("user.role", "Role.Admin"),
            ("role", "Role.Admin"),
            ("user.pair.1", "true"),
            ("user.scores[count - 42]", "9")
        ]
    );
    let errors: Vec<&str> = lines.iter().filter_map(|l| l["error"].as_str()).collect();
    assert_eq!(
        errors,
        [
            "`user.nope` has no field `nope`",
            "\"x\" is not a value of type Int",
            "3 is not an index of `user.scores[3]`, which has 3 items"
        ]
    );
    assert!(text.contains("42 Ada L 37 Admin [9, 2, 3] [1, 2, 3] Admin true"), "{text}");
}

const SHARED: &str = r#"import std.console
import std.sync

type Box = { count: Int }

fn worker(state: sync.Mutex[Box], go: Receiver[Bool], done: Sender[Bool]) {
    _ = go.receive()
    for i in 0..3 {
        state.update((box) { box.count += 10 })
    }
    done.send(true)
}

fn main() {
    let state = sync.Mutex(Box { count: 1 })
    let (go, going) = channel[Bool](1)
    let (done, finished) = channel[Bool](1)
    spawn worker(state, going, done)
    go.send(true)
    _ = finished.receive()
    var total = 0
    for i in 0..10 {
        total += i
    }
    console.println("{total} {state.get().count}")
}
"#;

/// A `std.sync` cell shows its value, and a watch of it stops the task
/// that changed it; a watch's condition and count of hits choose which
/// changes stop.
#[test]
fn debugger_watches_shared_values() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-shared");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shared.to"), SHARED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "shared.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // The worker changes the cell only once main sends `go`, after the
    // watch is set.
    let script = "break shared.to:19\ncontinue\nprint state\nwatch state.value.count hits 2\ndelete 1\ncontinue\n\
        delete 2\nbreak shared.to:23\ncontinue\nwatch total if total > 10\ndelete 3\ncontinue\ndelete 4\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let state = lines.iter().find(|l| l["name"] == "state").unwrap();
    assert_eq!(state["value"], "sync.Mutex { value: Box { count: 1 } }");
    let watch = lines.iter().find_map(|l| l.get("watch")).unwrap();
    assert_eq!((&watch["value"], &watch["everywhere"]), (&json!("1"), &json!(true)));
    let stops: Vec<(&str, &str, &Value)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), s["why"].as_str().unwrap(), &s["task"]))
        .collect();
    assert_eq!(
        stops[2..],
        [
            ("shared.to:8 in worker", "watch 2: state.value.count changed from 11 to 21", &json!(2)),
            ("shared.to:23 in main", "breakpoint 3", &json!(1)),
            ("shared.to:22 in main", "watch 4: total changed from 10 to 15 at shared.to:23 in main", &json!(1)),
        ]
    );
    assert!(text.contains("45 31"), "{text}");
}

const CAUGHT: &str = r#"enum Problem {
    Empty
}

enum Other {
    Bad
}

fn first(items: Array[Int]): Int ! Problem {
    if items.isEmpty() {
        fail Problem.Empty
    }
    items[0]
}

fn second(): Int ! Other {
    fail Other.Bad
}

fn main() {
    _ = first([]).unwrapOr(0)
    _ = second().unwrapOr(0)
    let values = [1, 2]
    _ = values[5]
}
"#;

/// `catch` stops at the errors of the types it names and the panics of
/// the codes or names it names.
#[test]
fn debugger_catches_chosen_errors_and_panics() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-caught");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("caught.to"), CAUGHT).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "caught.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "catch panic P0005\ncatch error\ncatch panic bounds error Other\ncatch panic\ncatch nothing\n\
        catch panic bounds error Other\ncontinue\ncontinue\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let caught: Vec<&Value> = lines.iter().filter_map(|l| l.get("catch")).collect();
    assert_eq!(
        caught,
        [
            &json!(["panic P0005"]),
            &json!(["error"]),
            &json!(["panic bounds", "error Other"]),
            &json!(["panic"]),
            &json!(["panic bounds", "error Other"])
        ][..]
    );
    assert!(lines.iter().any(|l| l["error"].as_str().is_some_and(|e| e.starts_with("catch takes"))));
    let stops: Vec<(&str, &str)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), s["why"].as_str().unwrap()))
        .collect();
    assert_eq!(stops[1], ("caught.to:17 in second", "error: Other.Bad"));
    assert_eq!(stops[2].0, "caught.to:24 in main");
    assert!(stops[2].1.starts_with("panic[P0001]: bounds"), "{stops:?}");
}

const DEADLOCKED: &str = r#"fn waiter(inbox: Receiver[Int]) {
    let label = "waiter"
    _ = inbox.receive()
}

fn main() {
    let (outbox, inbox) = channel[Int](1)
    let (other, never) = channel[Int](1)
    spawn waiter(never)
    _ = inbox.receive()
    outbox.send(1)
    other.send(1)
}
"#;

/// A deadlock stops the program before it ends, where each task waits,
/// with their bindings.
#[test]
fn debugger_stops_at_a_deadlock() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-deadlock");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deadlocked.to"), DEADLOCKED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "deadlocked.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "continue\ntasks\ntask 2\nprint label\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stop = lines.iter().filter_map(|l| l.get("stopped")).nth(1).unwrap();
    assert_eq!(stop["at"], "deadlocked.to:10 in main");
    assert!(stop["why"].as_str().unwrap().starts_with("panic[P0013]: deadlock"), "{stop}");
    let tasks = lines.iter().find_map(|l| l["tasks"].as_array()).unwrap();
    let waits = json!({ "task": 2, "at": "deadlocked.to:3 in waiter", "why": "waits", "for": "a value of a channel that holds 0 of 1", "on": [] });
    // How long it has waited, which changes.
    let mut task = tasks[1].clone();
    assert!(task.as_object_mut().unwrap().remove("ms").unwrap().is_u64(), "{task}");
    assert_eq!(task, waits);
    assert_eq!(lines.iter().find(|l| l["name"] == "label").unwrap()["value"], "\"waiter\"");
    // Going on, the program ends with the deadlock.
    assert!(String::from_utf8_lossy(&output.stderr).contains("panic[P0013]: deadlock"));
}

const INTO: &str = r#"fn twice(n: Int): Int {
    n * 2
}

fn plus(a: Int, b: Int): Int {
    a + b
}

fn main() {
    let total = plus(twice(1), twice(2)) + [1, 2].map((x) { x * 3 }).length()
    _ = total
}
"#;

/// `step into` goes into the call of a name the line makes, over the
/// others, or on to the next line when the line makes none.
#[test]
fn debugger_steps_into_a_chosen_call() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-into");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("into.to"), INTO).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "into.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break into.to:10\ncontinue\ndelete 1\nstep into plus\nfinish\nstep into nothing\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<(&str, &Value)> =
        lines.iter().filter_map(|l| l.get("stopped")).map(|s| (s["at"].as_str().unwrap(), &s["returned"])).collect();
    assert_eq!(stops[2], ("into.to:6 in plus", &json!({ "function": "twice", "value": "4" })));
    assert_eq!(stops.last().unwrap().0, "into.to:11 in main");
}

/// A program another machine runs, reached by a command that runs
/// `tondo relay` there: here, `env`, as `ssh host` would.
#[test]
fn debugger_reaches_a_program_through_a_relay() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-relay");
    std::fs::create_dir_all(&dir).unwrap();
    let looped = "import std.time\n\nfn main() {\n    var ticks = 0\n    for {\n        ticks += 1\n        time.sleep(time.milliseconds(10))\n    }\n}\n";
    std::fs::write(dir.join("looped.to"), looped).unwrap();
    let executable = dir.join("looped");
    let status = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "looped.to", "--debug", "--output"])
        .arg(&executable)
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    let mut program = Command::new(&executable).env("TONDO_DIAGNOSTICS", "1").spawn().unwrap();
    while !listed(program.id()) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let tools = Path::new(env!("CARGO_BIN_EXE_tondo")).parent().unwrap();
    let via = format!("env PATH={}", tools.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--via", &via, &program.id().to_string()])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break looped.to:6\ncontinue\nprint ticks > 0\ndelete 1\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    // The other tools that look at a running program reach it so too.
    let (program_id, pid) = (u64::from(program.id()), program.id().to_string());
    // What a tool gives, or what it said when it gave nothing.
    let observed = |args: &[&str]| -> Value {
        let output =
            Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).args(["--via", &via, "--json"]).output().unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            let said = [output.stdout, output.stderr].concat();
            Value::String(format!("{}: {}", output.status, String::from_utf8_lossy(&said)))
        })
    };
    let counters = observed(&["counters", &pid, "--count", "1"]);
    let dumped = observed(&["dump", &pid, "--output", dir.join("looped.dump.json").to_str().unwrap()]);
    let listed = observed(&["ps"]);
    let _ = program.kill();
    let _ = program.wait();
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    let lines: Vec<Value> = said.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stop = lines.iter().find_map(|l| l.get("stopped")).unwrap_or_else(|| panic!("{said}"));
    assert_eq!(stop["at"], "looped.to:6 in main");
    let printed = lines.iter().find(|l| l["name"] == "ticks > 0").unwrap_or_else(|| panic!("{said}"));
    assert_eq!(printed["value"], "true");
    assert!(counters["counters"].as_array().is_some_and(|c| !c.is_empty()), "{counters}");
    assert!(dumped["summary"].as_str().is_some_and(|s| s.starts_with("looped (pid ")), "{dumped}");
    let programs = listed["programs"].as_array();
    assert!(programs.is_some_and(|p| p.iter().any(|p| p["pid"].as_u64() == Some(program_id))), "{listed}");
}

/// `record` keeps each line a task runs with its bindings; `back`,
/// `reverse` and `forward` go through them, and the present comes back.
#[test]
fn debugger_goes_back_in_history() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-history");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("summed.to"), WATCHED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "summed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "record 1000\nbreak summed.to:17\ncontinue\ncontinue\nback\nprint item\nreverse\nprint total\nstack\n\
        forward 100\nprint total\nrecord off\ndelete 1\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<(&str, &Value)> =
        lines.iter().filter_map(|l| l.get("stopped")).map(|s| (s["at"].as_str().unwrap(), &s["back"])).collect();
    assert_eq!(
        stops[1..4],
        [
            ("summed.to:17 in sum", &Value::Null),
            ("summed.to:17 in sum", &Value::Null),
            ("summed.to:14 in sum", &json!(1))
        ]
    );
    // Back to the breakpoint's line before, then forward to the present.
    assert!(stops[4].0 == "summed.to:17 in sum" && stops[4].1.as_u64() > Some(1), "{stops:?}");
    assert_eq!(stops[5], ("summed.to:17 in sum", &json!(0)));
    let printed: Vec<(&str, &str)> = lines
        .iter()
        .filter(|l| l["name"].is_string())
        .map(|l| (l["name"].as_str().unwrap(), l["value"].as_str().unwrap()))
        .collect();
    assert_eq!(printed, [("item", "2"), ("total", "0"), ("total", "1")]);
    let stack = lines.iter().find_map(|l| l["stack"].as_array()).unwrap();
    assert_eq!(stack[..2], [json!("summed.to:17 in sum"), json!("summed.to:39 in main")]);
}

const JUMPED: &str = r#"import std.console

fn main() {
    var count = 0
    count += 1
    count += 10
    let label = "done"
    console.println("{count} {label}")
}
"#;

/// In a live build, `goto` moves the stopped frame to another line of its
/// function, when what that line uses holds a value; it goes on there.
#[test]
fn debugger_moves_a_frame_to_another_line() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-goto");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("jumped.to"), JUMPED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--live", "jumped.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break jumped.to:6\ncontinue\ngoto 5\nstack\nnext\nprint count\ngoto 8\ndelete 1\ngoto 4\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<(&str, &str)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), s["why"].as_str().unwrap()))
        .collect();
    assert_eq!(
        stops[1..],
        [
            ("jumped.to:6 in main", "breakpoint 1"),
            ("jumped.to:5 in main", "goto"),
            ("jumped.to:6 in main", "breakpoint 1"),
            ("jumped.to:4 in main", "goto"),
        ]
    );
    assert_eq!(lines.iter().find_map(|l| l["stack"].as_array()).unwrap()[0], "jumped.to:5 in main");
    assert_eq!(lines.iter().find(|l| l["name"] == "count").unwrap()["value"], "2");
    let error = lines.iter().find_map(|l| l["error"].as_str()).unwrap();
    assert_eq!(error, "the frame does not go to line 8: `label` holds no value at line 6 yet");
    // From line 4 again, `count` starts over.
    assert!(text.contains("11 done"), "{text}");
    // The editor's `goto`, to a target of the line.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "jumped.to", "cwd": dir, "live": true }));
    let path = dir.join("jumped.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 6 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let targets = adapter.ask("gotoTargets", json!({ "source": { "path": path }, "line": 5 }));
    adapter.ask("goto", json!({ "threadId": 1, "targetId": targets["targets"][0]["id"] }));
    assert_eq!(adapter.until("stopped")["reason"], "goto");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["stackFrames"][0]["line"], 5);
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const CALLING: &str = r#"import std.console

type User = { name: String, age: Int }

fn describe(user: User): String {
    "{user.name} ({user.age})"
}

fn twice(n: Int): Int {
    n * 2
}

fn main() {
    let ada = User { name: "Ada", age: 36 }
    let count = 21
    console.println(describe(ada))
    console.println("{twice(count)}")
}
"#;

/// In a live build, `call` runs an expression that calls the program's
/// code, on the stopped task, through a patch; the editor's console does
/// for an expression the debugger does not read.
#[test]
fn debugger_calls_the_programs_code() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-call");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("calling.to"), CALLING).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--live", "calling.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script =
        "break calling.to:16\ncontinue\ncall twice(count) + 1\ncall describe(ada)\ncall nothing(count)\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let called: Vec<(&str, &str)> = lines
        .iter()
        .filter(|l| l["name"].is_string())
        .map(|l| (l["name"].as_str().unwrap(), l["value"].as_str().unwrap()))
        .collect();
    assert_eq!(called, [("twice(count) + 1", "43"), ("describe(ada)", "Ada (36)")]);
    let error = lines.iter().find_map(|l| l["error"].as_str()).unwrap();
    assert!(error.contains("unknown name `nothing`"), "{error}");
    // What the program printed, between the debugger's answers, as it
    // flushes its output when it stops or when it ends.
    let printed: Vec<&str> =
        text.lines().filter(|l| serde_json::from_str::<Value>(l).map_or(true, |v| !v.is_object())).collect();
    assert_eq!(printed, ["Ada (36)", "42"], "{text}");
    // The editor's console.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "calling.to", "cwd": dir, "live": true }));
    let path = dir.join("calling.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 16 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    let evaluated =
        adapter.ask("evaluate", json!({ "expression": "twice(count)", "frameId": frame, "context": "repl" }));
    assert_eq!(evaluated["result"], "42");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const CONDITIONED: &str = r#"import std.console

type Order = { id: Int, total: Int }

fn big(order: Order): Bool {
    order.total > 200
}

fn main() {
    var sum = 0
    for i in 0..12 {
        let order = Order { id: i, total: i * 30 }
        sum += order.total
        console.println("{order.id}")
    }
    console.println("{sum}")
}
"#;

/// In a live build, a breakpoint's condition is compiled, and may call the
/// program's code; after a reload that changes its function, it is compiled
/// again for the new code.
#[test]
fn debugger_compiles_conditions() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-condition");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("conditioned.to"), CONDITIONED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--live", "conditioned.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut until = |key: &str| loop {
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0, "the debugger ended before `{key}`");
        let value: Value = serde_json::from_str(&line).unwrap_or_default();
        if value.get(key).is_some() || value.get("error").is_some() {
            return value;
        }
    };
    let mut send = |line: &str| std::io::Write::write_all(&mut input, format!("{line}\n").as_bytes()).unwrap();
    until("stopped");
    send("break conditioned.to:13 if nothing(order)");
    let refused = until("breakpoint");
    assert!(refused["error"].as_str().unwrap().contains("unknown name `nothing`"), "{refused}");
    send("break conditioned.to:13 if big(order) and sum < 2000");
    let set = until("breakpoint");
    assert_eq!(set["breakpoint"]["compiled"], true, "{set}");
    send("continue");
    until("stopped");
    send("print order.id");
    assert_eq!(until("value")["value"], "7");
    // `main` changes, and `big` with it: the condition is compiled again
    // for the new code of its line, and `big` says it is bigger now.
    let source = CONDITIONED.replace("> 200", "> 250").replace("\"{order.id}\"", "\"order {order.id}\"");
    std::fs::write(dir.join("conditioned.to"), source).unwrap();
    send("reload");
    let reloaded = until("reloaded");
    assert!(reloaded["reloaded"]["changed"].as_array().unwrap().contains(&json!("main")), "{reloaded}");
    send("continue");
    until("stopped");
    send("print order.id");
    assert_eq!(until("value")["value"], "9");
    send("print sum");
    assert_eq!(until("value")["value"], "1080");
    send("quit");
    child.wait().unwrap();
    // The editor's conditions too.
    std::fs::write(dir.join("conditioned.to"), CONDITIONED).unwrap();
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "conditioned.to", "cwd": dir, "live": true }));
    let path = dir.join("conditioned.to");
    let wanted = json!([{ "line": 13, "condition": "big(order)" }, { "line": 14, "condition": "nothing(order)" }]);
    let set = adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": wanted }));
    assert_eq!(set["breakpoints"][0]["verified"], true, "{set}");
    assert_eq!(set["breakpoints"][1]["verified"], false, "{set}");
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    let id = adapter.ask("evaluate", json!({ "expression": "order.id", "frameId": frame }));
    assert_eq!(id["result"], "7");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const TABLED: &str = r#"import std.console

type User = { name: String, age: Int }

fn main() {
    var users: Array[User] = []
    for i in 0..3 {
        users.push(User { name: "user {i}", age: 30 + i })
    }
    let ages: Map[String, Int] = ["Ada": 36]
    console.println("{users.length()} {ages.length()}")
    console.println("{users.length()}")
}
"#;

/// The editor's table of a value: a row for each part of a page, a column
/// for each field, and only of the stop it was asked at.
#[test]
fn debugger_gives_a_values_table() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-table");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tabled.to"), TABLED).unwrap();
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "tabled.to", "cwd": dir }));
    let path = dir.join("tabled.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 11 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let scopes = adapter.ask("scopes", json!({ "frameId": trace["stackFrames"][0]["id"] }));
    let locals = scopes["scopes"][0]["variablesReference"].clone();
    let variables = adapter.ask("variables", json!({ "variablesReference": locals }));
    let reference = |name: &str| {
        let found = variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == name).unwrap();
        found["variablesReference"].clone()
    };
    let (users, ages) = (reference("users"), reference("ages"));
    let table = adapter.ask("table", json!({ "variablesReference": users, "start": 1, "count": 5 }));
    assert_eq!(table["columns"], json!(["name", "age"]));
    assert_eq!(table["total"], 3);
    let first = &table["rows"][0];
    assert_eq!((first["name"].as_str(), first["cells"]["name"].as_str()), (Some("[1]"), Some("\"user 1\"")));
    assert_eq!(table["rows"][1]["cells"]["age"], "32");
    let map = adapter.ask("table", json!({ "variablesReference": ages }));
    assert_eq!(map["rows"], json!([{ "name": "[\"Ada\"]", "cells": { "value": "36" } }]));
    // The next page of a table is of its stop.
    let stop = table["stop"].clone();
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 12 }] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("stopped");
    let refused = adapter.refused("table", json!({ "variablesReference": users, "start": 2, "stop": stop }));
    assert!(refused.contains("went on"), "{refused}");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const RECORDED: &str = r#"import std.console
import std.fs
import std.random
import std.time

fn main() {
    var dice = random.Random.new()
    let rolled = dice.int(0..1000000)
    let name = console.readLine().unwrapOr("nobody")
    let path = "{name}.txt"
    fs.writeText(path, "{rolled} at {time.unixTime().nanoseconds()}").expect("written")
    let kept = fs.readText(path).expect("read")
    fs.remove(path).expect("removed")
    console.println("{name} {kept}")
}
"#;

/// On Linux, a run is recorded and replayed under the debugger the same
/// each time, its random numbers, time, input and files as they were, and
/// the replay touches no file.
#[test]
fn debugger_replays_a_recorded_run() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-replay");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("recorded.to"), RECORDED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "recorded.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"ada\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(printed.starts_with("ada "), "{printed}");
    for _ in 0..2 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["debug", "--json", "--replay", "run"])
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut text = String::new();
        let mut until = |key: &str| loop {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0, "the debugger ended before `{key}`: {text}");
            text.push_str(&line);
            let value: Value = serde_json::from_str(&line).unwrap_or_default();
            assert!(value.get("error").is_none(), "{text}");
            if value.get(key).is_some() {
                return value;
            }
        };
        let mut send = |line: &str| std::io::Write::write_all(&mut input, format!("{line}\n").as_bytes()).unwrap();
        until("stopped");
        send("break recorded.to:13");
        until("breakpoint");
        send("continue");
        until("stopped");
        // The program wrote the file in the recorded run only.
        assert!(!dir.join("ada.txt").exists());
        send("print kept");
        let kept = until("value")["value"].as_str().unwrap().trim_matches('"').to_string();
        assert_eq!(format!("ada {kept}\n"), printed);
        send("continue");
        until("exited");
        drop(input);
        child.wait().unwrap();
        assert!(text.contains(&printed), "{text}");
    }
}

const ROLLED: &str = r#"import std.console
import std.random

fn main() {
    var dice = random.Random.new()
    var total = 0
    for i in 0..6 {
        let roll = dice.int(1..7)
        total += roll
        console.println("{i}: {roll}")
    }
    console.println("total {total}")
}
"#;

const SENT: &str = r#"fn produce(out: Sender[Int], done: Receiver[Int]) {
    var n = 3
    for i in 0..3 {
        n = n * 2 + i
        out.send(n)
    }
    _ = done.receive()
}

fn main() {
    let (out, inbox) = channel[Int](1)
    let (finish, done) = channel[Int](1)
    spawn produce(out, done)
    var total = 0
    for _ in 0..3 {
        let got = inbox.receive().unwrapOr(0)
        total += got
    }
    _ = total
    finish.send(0)
}
"#;

/// In a replay, what a line received comes from the line of the task that
/// sent it, which `why` and `origins` follow into that task.
#[test]
fn a_replay_follows_values_through_channels() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-sent");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sent.to"), SENT).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "sent.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(recorded.status.success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--replay", "run"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break sent.to:17\ncontinue\ncontinue\nwhy got\norigins\ntick\ncontinue\ndiff 3\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned();
    let lines: Vec<Value> = output.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let why = lines.iter().filter_map(|l| l.get("stopped")).nth(3).unwrap_or_else(|| panic!("{output}"));
    assert_eq!(why["at"], "sent.to:16 in main", "{output}");
    let received = why["causes"].as_array().unwrap().iter().find(|c| c["received"] == true);
    assert_eq!(received.map(|c| &c["at"]), Some(&json!("sent.to:5 in produce")), "{output}");
    // The origins go on in the task that sent it.
    let origins = lines.iter().find_map(|l| l.get("origins")).unwrap_or_else(|| panic!("{output}"));
    assert!(origins.to_string().contains("sent.to:4 in produce"), "{origins}");
    // What changed since probe 3, in each task.
    let diff = lines.iter().find_map(|l| l.get("diff")).unwrap_or_else(|| panic!("{output}"));
    let changed: Vec<&str> = diff["bindings"].as_array().unwrap().iter().filter_map(|b| b["name"].as_str()).collect();
    assert!(changed.contains(&"total") && changed.contains(&"n"), "{diff}");
}

/// A recording keeps the program's sources, so it runs again elsewhere, as
/// on another machine, with the lines it shows and reads.
#[test]
fn a_recording_replays_elsewhere() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-elsewhere");
    let _ = std::fs::remove_dir_all(&dir);
    let (here, elsewhere) = (dir.join("here"), dir.join("elsewhere"));
    std::fs::create_dir_all(&here).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(here.join("rolled.to"), ROLLED).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "rolled.to"])
        .current_dir(&here)
        .output()
        .unwrap();
    assert!(recorded.status.success());
    std::fs::rename(here.join("run"), elsewhere.join("run")).unwrap();
    std::fs::remove_dir_all(&here).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--replay", "run"])
        .current_dir(&elsewhere)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break rolled.to:12\ncontinue\nwhy total\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned();
    assert!(output.contains("    9 |         total += roll"), "{output}");
    assert!(output.contains("`roll` was "), "{output}");
}

/// Two threads of C that call the program in whichever order they get to
/// it, with what each sees of the other in what it prints.
const RACING: &str = r#"import std.console
import std.ffi
import std.sync

extern fn pthread_create(
    thread: Pointer[UInt64],
    attributes: Pointer[ffi.Void]?,
    start: extern fn(Pointer[ffi.Void]?): Pointer[ffi.Void]?,
    argument: Pointer[ffi.Void]?,
): ffi.CInt

extern fn pthread_join(thread: UInt64, result: Pointer[Pointer[ffi.Void]?]?): ffi.CInt

unsafe fn worker(argument: Pointer[ffi.Void]?): Pointer[ffi.Void]? {
    let order = ffi.fromContext[sync.Mutex[Array[Int]]](argument?)
    for _ in 0..50 {
        let next = order.update((seen) {
            seen.push(seen.length())
            seen.length()
        })
        _ = next
    }
    none
}

fn main() {
    let order = sync.Mutex(Array.new[Int]())
    unsafe {
        let context = ffi.context(order)
        let first = ffi.allocate[UInt64]()
        let second = ffi.allocate[UInt64]()
        _ = pthread_create(first, none, worker, context)
        _ = pthread_create(second, none, worker, context)
        _ = pthread_join(first.read(), none)
        _ = pthread_join(second.read(), none)
        ffi.freeContext[sync.Mutex[Array[Int]]](context)
        ffi.free(first)
        ffi.free(second)
    }
    console.println("{order.get().length()}")
}
"#;

/// A replay runs the threads of C again and holds their calls to the
/// order the recording has, and `back` goes on working there.
#[test]
fn a_replay_holds_the_threads_of_c_to_their_order() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-racing");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("racing.to"), RACING).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "racing.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&recorded.stdout).into_owned();
    assert!(recorded.status.success(), "{stdout}{}", String::from_utf8_lossy(&recorded.stderr));
    assert_eq!(stdout, "100\n");
    let run: Value = serde_json::from_slice(&std::fs::read(dir.join("run/run.json")).unwrap()).unwrap();
    for _ in 0..3 {
        // A replay that hung would hold the tests for hours.
        let mut replayed = Command::new(dir.join("run/program"))
            .env("TONDO_RT_REPLAY", dir.join("run/calls"))
            .env("TONDO_RT_SCHEDULE", run["seed"].as_str().unwrap())
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while replayed.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = replayed.kill();
                panic!("the replay hung: {:?}", replayed.wait_with_output().unwrap());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let replayed = replayed.wait_with_output().unwrap();
        let said = String::from_utf8_lossy(&replayed.stderr).into_owned();
        assert_eq!(String::from_utf8_lossy(&replayed.stdout), stdout, "{said}");
        assert!(replayed.status.success(), "{said}");
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--replay", "run"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // A search to the end goes past the tasks of those calls, whose stacks go
    // with them.
    let script = "break racing.to:21\ncontinue\ncontinue\nprint next\nback\nprint next\nfirst next > 40\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned();
    assert!(output.contains("racing.to:21"), "{output}");
    assert!(output.contains("next: Int = 2"), "{output}");
    assert!(output.matches("stopped at").count() >= 3, "{output}");
    assert!(output.contains("the first time `next > 40` holds"), "{output}");
}

/// Two recorded runs of a program, one that went wrong: where they part,
/// each task by itself, what differs there, and where that came from.
#[test]
fn debugger_compares_two_recorded_runs() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-compare");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A clock differs in every run, and decides nothing.
    let program = "import std.console\nimport std.random\nimport std.time\n\nfn total(): Int {\n    var dice = random.Random.new()\n    \
                   let rate = dice.int(1..=4)\n    rate * 6\n}\n\nfn main() {\n    let began = time.utcNow()\n    let sum = total()\n    if sum > 12 {\n        \
                   panic(\"over budget\")\n    }\n    console.println(\"fine {began}\")\n}\n";
    std::fs::write(dir.join("flaky.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    // A run of each kind: one in two is, and the seed of each run differs.
    let (mut good, mut bad) = (None, None);
    for run in 0..200 {
        let name = format!("run{run}");
        let ran = Command::new(tondo).args(["run", "--record", &name, "flaky.to"]).current_dir(&dir).output().unwrap();
        match ran.status.success() {
            true => good = good.or(Some(name)),
            false => bad = bad.or(Some(name)),
        }
        if good.is_some() && bad.is_some() {
            break;
        }
    }
    let (good, bad) = (good.unwrap(), bad.unwrap());
    let compared = |first: &str, second: &str| {
        let args = ["debug", "--compare", first, second, "--json"];
        let output = Command::new(tondo).args(args).current_dir(&dir).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let parted = compared(&good, &bad);
    if parted["parted"] != true {
        // What each replay does, when they seem the same.
        for run in [&good, &bad] {
            let mut debugger = Command::new(tondo)
                .args(["debug", "--replay", run])
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let commands = "continue\nstack\nquit\n";
            std::io::Write::write_all(&mut debugger.stdin.take().unwrap(), commands.as_bytes()).unwrap();
            eprintln!("{run}: {:?}", debugger.wait_with_output().unwrap());
        }
    }
    assert_eq!(parted["parted"], true, "{parted}");
    assert_eq!(parted["after"], "flaky.to:14 in main", "{parted}");
    assert_eq!(parted["first"]["at"], "flaky.to:17 in main", "{parted}");
    assert_eq!(parted["second"]["at"], "flaky.to:15 in main", "{parted}");
    let read: Vec<&Value> = parted["bindings"].as_array().unwrap().iter().filter(|b| b["read"] == true).collect();
    assert_eq!(read.len(), 1, "{parted}");
    assert_eq!(read[0]["name"], "sum", "{parted}");
    assert!(parted["bindings"].to_string().contains("began"), "{parted}");
    // Of what made it, only what differs: the die, and what threw it.
    assert_eq!(parted["origins"].as_array().unwrap().len(), 1, "{parted}");
    assert_eq!(parted["origins"][0]["causes"][0]["name"], "rate", "{parted}");
    assert_eq!(parted["origins"][0]["causes"][0]["at"], "flaky.to:7 in total", "{parted}");
    // The recordings keep what it reads, wherever it runs.
    let elsewhere = Command::new(tondo)
        .args(["debug", "--compare", &good, &bad].map(|arg| match arg.starts_with('-') || arg == "debug" {
            true => arg.to_string(),
            false => dir.join(arg).display().to_string(),
        }))
        .args(["--json"])
        .current_dir("/")
        .output()
        .unwrap();
    let elsewhere: Value = serde_json::from_slice(&elsewhere.stdout).unwrap();
    assert_eq!(elsewhere["origins"][0]["causes"][0]["name"], "rate", "{elsewhere}");
    assert_eq!(compared(&good, &good)["parted"], false);
    // Another build's lines are not these.
    let mut other = std::fs::read(dir.join(&good).join("program")).unwrap();
    other.push(0);
    std::fs::create_dir_all(dir.join("other")).unwrap();
    std::fs::write(dir.join("other/program"), other).unwrap();
    let refused = Command::new(tondo).args(["debug", "--compare", &good, "other"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&refused.stderr).contains("different builds"));
}

/// A recording of a crash is what a search is for: the replay crashes where
/// the run did, and says what it followed all the same.
#[test]
fn a_replay_searches_the_recording_of_a_crash() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-crash");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.ffi\n\nfn main() {\n    var total = 0\n    for i in 0..5 {\n        total += i\n    }\n    \
                   unsafe {\n        ffi.allocate[Int]().offset(1000000000).write(total)\n    }\n}\n";
    std::fs::write(dir.join("crash.to"), program).unwrap();
    let run = |command: &str| {
        Command::new("sh").args(["-c", &format!("ulimit -c 0; {command}")]).current_dir(&dir).output().unwrap()
    };
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let recorded = run(&format!("{tondo} run --record rec crash.to"));
    assert!(!recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let replayed = run(&format!(
        "printf 'break crash.to:6\\ncontinue\\nfirst total > 3\\nlast total > 3\\n' | {tondo} debug --replay rec"
    ));
    let output = String::from_utf8_lossy(&replayed.stdout).into_owned();
    assert!(output.contains("the first time `total > 3` holds"), "{output}");
    assert!(output.contains("the last time `total > 3` holds"), "{output}");
}

/// lldb stops a debug build at a line of its DWARF, on Linux and macOS,
/// and the `tondo` command that `tondo dump plugin lldb` writes shows the
/// bindings of the frame it shows.
#[test]
fn lldb_stops_at_a_line_and_shows_the_bindings() {
    if Command::new("lldb").arg("--version").output().is_err() {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-lldb");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program =
        "fn square(n: Int): Int {\n    let result = n * n\n    result\n}\n\nfn main() {\n    _ = square(3)\n}\n";
    std::fs::write(dir.join("app.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let status = Command::new(tondo).args(["build", "app.to", "--debug", "--output", "app"]).current_dir(&dir).status();
    assert!(status.unwrap().success());
    let plugin = Command::new(tondo).args(["dump", "plugin", "lldb"]).env("XDG_CACHE_HOME", &dir).output().unwrap();
    let load = String::from_utf8_lossy(&plugin.stdout).trim().to_string();
    let ran = Command::new("lldb")
        .args(["--batch", "-o", &load, "-o", "breakpoint set --file app.to --line 2", "-o", "run", "-o", "bt"])
        .args(["-o", "tondo locals", "-o", "up", "-o", "tondo print 3 * 2", "--", "./app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&ran.stdout).into_owned() + &String::from_utf8_lossy(&ran.stderr);
    // macOS lets no session without a person debug, as one over SSH.
    if shown.contains("cannot get permission to debug") {
        return;
    }
    // Nor does an emulator let lldb trace a process.
    if shown.trim().is_empty() {
        return;
    }
    assert!(shown.contains("square at app.to:2"), "{shown}");
    assert!(shown.contains("main at app.to:7"), "{shown}");
    assert!(shown.contains("n: Int = 3"), "{shown}");
    assert!(shown.contains("3 * 2 = 6"), "{shown}");
}

/// A program that watches files and profiles itself, whose watch and
/// samples come from threads and signals of the runtime.
const OBSERVED: &str = r#"import std.console
import std.fs
import std.path
import std.profile
import std.time

fn spin(): Int {
    var total = 0
    for i in 0..3000000 {
        total += i % 7
    }
    total
}

fn main() {
    let dir = fs.realPath(fs.createTemporaryDirectory()?)?
    let changes = fs.watch([dir], settle: time.milliseconds(100))?
    fs.writeText(path.join(dir, "a.txt"), "a")?
    for change in changes.receive(timeout: time.seconds(10)).unwrapOr([]) {
        console.println("{change.path.removePrefix(dir)} {change.kind}")
    }
    let cpu = profile.startCpu()
    let total = spin()
    let taken = cpu.stop()
    console.println("{total} {taken.folded().contains("spin")} {taken.folded().split("\n").length()}")
}
"#;

/// A replay gives the program what a watch and the profiler gave it in
/// the recorded run, without watching or sampling.
#[test]
fn a_replay_gives_watches_and_profiles_back() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-watched");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("watched.to"), OBSERVED).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "watched.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&recorded.stdout).into_owned();
    assert!(recorded.status.success(), "{stdout}{}", String::from_utf8_lossy(&recorded.stderr));
    assert!(stdout.contains("/a.txt Created") && stdout.contains(" true "), "{stdout}");
    let run: Value = serde_json::from_slice(&std::fs::read(dir.join("run/run.json")).unwrap()).unwrap();
    let replayed = Command::new(dir.join("run/program"))
        .env("TONDO_RT_REPLAY", dir.join("run/calls"))
        .env("TONDO_RT_SCHEDULE", run["seed"].as_str().unwrap())
        .current_dir(&dir)
        .output()
        .unwrap();
    let again = String::from_utf8_lossy(&replayed.stdout).into_owned();
    assert_eq!(again, stdout, "{}", String::from_utf8_lossy(&replayed.stderr));
}

/// A child of a recorded run that is a debug build of Tondo records its
/// own run, which a replay of its parent names, to replay it too.
#[test]
fn a_replay_names_the_recordings_of_children() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-children");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let child =
        "import std.console\nimport std.env\n\nfn main() {\n    console.println(\"child got {env.args()}\")\n}\n";
    std::fs::write(dir.join("child.to"), child).unwrap();
    let parent = "import std.console\nimport std.process\n\nfn main() {\n    let out = process.command(\"./child\", \"hi\").output()?\n    console.println(\"parent read {out.text()?}\")\n}\n";
    std::fs::write(dir.join("parent.to"), parent).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "child.to", "--output", "child", "--debug"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "parent.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&recorded.stdout).into_owned();
    assert!(stdout.contains("parent read child got [hi]"), "{stdout}");
    let mut replay = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--replay", "run"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(replay.stdin.as_mut().unwrap(), b"follow\ncontinue\n").unwrap();
    drop(replay.stdin.take());
    let output = String::from_utf8_lossy(&replay.wait_with_output().unwrap().stdout).into_owned();
    let recording = output.split("`tondo debug --replay ").nth(1).and_then(|r| r.split('`').next());
    let recording = recording.unwrap_or_else(|| panic!("{output}"));
    assert!(Path::new(recording).join("sources").is_dir(), "{recording}");
    let mut replay = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--replay", recording])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(replay.stdin.as_mut().unwrap(), b"break child.to:5\ncontinue\nlocals\ncontinue\n")
        .unwrap();
    drop(replay.stdin.take());
    let output = String::from_utf8_lossy(&replay.wait_with_output().unwrap().stdout).into_owned();
    assert!(output.contains("child.to:5") && output.contains("child got [hi]"), "{output}");
}

/// In a replay, `reverse`, `back` and `why` go back by running the
/// recording again, from a checkpoint where the program stopped before, to
/// the program as it was there, every value of it.
#[test]
fn debugger_goes_back_in_a_replay() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-back");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rolled.to"), ROLLED).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "rolled.to"])
        .current_dir(&dir)
        .output();
    let recorded = String::from_utf8_lossy(&recorded.unwrap().stdout).into_owned();
    let rolls: Vec<i64> = recorded.lines().take(6).map(|l| l.split(": ").nth(1).unwrap().parse().unwrap()).collect();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--replay", "run"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut text = String::new();
    let mut ask = |line: &str, key: &str| {
        if !line.is_empty() {
            std::io::Write::write_all(&mut input, format!("{line}\n").as_bytes()).unwrap();
        }
        loop {
            let mut read = String::new();
            assert!(output.read_line(&mut read).unwrap() > 0, "the debugger ended: {text}");
            text.push_str(&read);
            let value: Value = serde_json::from_str(&read).unwrap_or_default();
            assert!(value.get("error").is_none(), "{text}");
            if value.get(key).is_some() {
                return value;
            }
        }
    };
    ask("", "stopped");
    ask("break rolled.to:10", "breakpoint");
    for _ in 0..4 {
        ask("continue", "stopped");
    }
    let value = |ask: &mut dyn FnMut(&str, &str) -> Value, name: &str| -> i64 {
        ask(&format!("print {name}"), "value")["value"].as_str().unwrap().parse().unwrap()
    };
    assert_eq!(value(&mut ask, "i"), 3);
    // The breakpoint's stop before this one.
    let stop = ask("reverse", "stopped");
    assert_eq!(stop["stopped"]["why"], "reverse", "{stop}");
    assert_eq!(value(&mut ask, "i"), 2);
    // The line that gave `total` its value: the `+=` of this iteration.
    let total = value(&mut ask, "total");
    let stop = ask("why total", "stopped");
    assert_eq!(stop["stopped"]["at"], "rolled.to:9 in main", "{stop}");
    // What that line reads, and where it got it.
    let causes = stop["stopped"]["causes"].as_array().unwrap();
    let roll = causes.iter().find(|c| c["name"] == "roll").unwrap_or_else(|| panic!("{stop}"));
    assert_eq!(
        (roll["value"].as_str().unwrap(), &roll["at"]),
        (rolls[2].to_string().as_str(), &json!("rolled.to:8 in main"))
    );
    assert_eq!(value(&mut ask, "total"), total - rolls[2]);
    assert_eq!(value(&mut ask, "i"), 2);
    // Two lines of the task back: the start of the iteration's `let`.
    let stop = ask("back 1", "stopped");
    assert_eq!(stop["stopped"]["at"], "rolled.to:8 in main", "{stop}");
    // And on again, the same.
    ask("next", "stopped");
    ask("next", "stopped");
    assert_eq!(value(&mut ask, "total"), total);
    // Each value `total` had, from the line that gave it, over the run.
    let timeline = ask("timeline total", "timeline");
    let values: Vec<i64> = timeline["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(values, [0, rolls[0], rolls[0] + rolls[1], rolls[0] + rolls[1] + rolls[2]], "{timeline}");
    let ran = ask("ran rolled.to:9 roll", "ran");
    assert_eq!(ran["times"], 3, "{ran}");
    // Where `total` came from, back to the dice it rolled.
    let origins = ask("origins total", "origins");
    let tree = origins["origins"].to_string();
    assert_eq!(origins["origins"]["at"], "rolled.to:9 in main", "{origins}");
    assert!(tree.contains(r#""name":"roll""#) && tree.contains(r#""name":"dice""#), "{origins}");
    assert!(tree.contains("rolled.to:5 in main"), "{origins}");
    // The line after which a condition first held over the run, and the
    // last time another did.
    let stop = ask(&format!("first total > {}", rolls[0]), "stopped");
    assert_eq!(stop["stopped"]["at"], "rolled.to:9 in main", "{stop}");
    assert_eq!((value(&mut ask, "i"), value(&mut ask, "total")), (1, rolls[0]));
    let stop = ask(&format!("last total == {}", rolls[0]), "stopped");
    assert_eq!(stop["stopped"]["at"], "rolled.to:9 in main", "{stop}");
    assert_eq!((value(&mut ask, "i"), value(&mut ask, "total")), (0, 0));
    // And back to where the second came from.
    let tick = timeline["timeline"][2]["tick"].as_u64().unwrap();
    ask(&format!("seek {tick}"), "stopped");
    assert_eq!((value(&mut ask, "i"), value(&mut ask, "total")), (1, rolls[0]));
    drop(input);
    child.wait().unwrap();
    // The editor's step back and reverse continue.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "replay": "run", "cwd": dir }));
    let path = dir.join("rolled.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 10 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let evaluated = |adapter: &mut Adapter, expression: &str| {
        let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
        let frame = trace["stackFrames"][0]["id"].clone();
        let line = trace["stackFrames"][0]["line"].clone();
        let value = adapter.ask("evaluate", json!({ "expression": expression, "frameId": frame }));
        (value["result"].as_str().unwrap().to_string(), line)
    };
    for _ in 0..2 {
        adapter.ask("continue", json!({ "threadId": 1 }));
        adapter.until("stopped");
    }
    assert_eq!(evaluated(&mut adapter, "i").0, "2");
    adapter.ask("reverseContinue", json!({ "threadId": 1 }));
    adapter.until("stopped");
    assert_eq!(evaluated(&mut adapter, "i"), ("1".to_string(), json!(10)));
    adapter.ask("stepBack", json!({ "threadId": 1 }));
    adapter.until("stopped");
    assert_eq!(evaluated(&mut adapter, "i"), ("1".to_string(), json!(9)));
    let origins = adapter.ask("origins", json!({ "expression": "total" }));
    assert!(origins["origins"]["causes"].as_array().is_some_and(|c| !c.is_empty()), "{origins}");
    // To the line after which `total` last had the first roll.
    let condition = format!("total == {}", rolls[0]);
    let found = adapter.ask("when", json!({ "condition": condition, "which": "last" }));
    assert!(found["why"].as_str().unwrap().starts_with("the last time `total =="), "{found}");
    adapter.until("stopped");
    assert_eq!(evaluated(&mut adapter, "total"), ("0".to_string(), json!(9)));
    let diff = adapter.ask("diff", json!({ "from": 1 }));
    assert!(diff["diff"]["bindings"].as_array().is_some_and(|b| !b.is_empty()), "{diff}");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const CHANGED_ELSEWHERE: &str = r#"import std.console
import std.sync

type Counter = { hits: Int }

fn Counter.bump(mut self) {
    self.hits += 10
}

fn add(items: var Array[Int]) {
    items.push(7)
}

fn record(stats: sync.Mutex[Counter]) {
    stats.update((s) { s.hits += 1 })
}

fn main() {
    var counter = Counter { hits: 0 }
    counter.bump()
    var items = [1, 2]
    add(var items)
    let stats = sync.Mutex(Counter { hits: 0 })
    var group = Group.new[Unit]()
    for _ in 0..3 {
        group.add(spawn record(stats))
    }
    group.wait()
    console.println("{counter.hits} {items} {stats.get().hits}")
}
"#;

#[test]
fn debugger_finds_who_changed_a_value_in_a_replay() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-changed");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("changed.to"), CHANGED_ELSEWHERE).unwrap();
    let recorded = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--record", "run", "changed.to"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&recorded.stdout).lines().next(), Some("10 [1, 2, 7] 3"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "--replay", "run"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut text = String::new();
    let mut ask = |line: &str, key: &str| {
        if !line.is_empty() {
            std::io::Write::write_all(&mut input, format!("{line}\n").as_bytes()).unwrap();
        }
        loop {
            let mut read = String::new();
            assert!(output.read_line(&mut read).unwrap() > 0, "the debugger ended: {text}");
            text.push_str(&read);
            let value: Value = serde_json::from_str(&read).unwrap_or_default();
            assert!(value.get("error").is_none(), "{text}");
            if value.get(key).is_some() {
                return value;
            }
        }
    };
    ask("", "stopped");
    ask("break changed.to:29", "breakpoint");
    ask("continue", "stopped");
    // Each change of the shared value, made by the other tasks.
    let timeline = ask("timeline stats", "timeline");
    let changes = timeline["timeline"].as_array().unwrap();
    let at: Vec<&str> = changes.iter().map(|c| c["at"].as_str().unwrap()).collect();
    assert_eq!(
        at,
        [
            "changed.to:23 in main",
            "changed.to:15 in closure in record",
            "changed.to:15 in closure in record",
            "changed.to:15 in closure in record"
        ],
        "{timeline}"
    );
    // The line of the function that changed the value, wherever it is.
    for (expression, line) in [
        ("counter.hits", "changed.to:7 in Counter.bump"),
        ("items", "changed.to:11 in add"),
        ("stats", "changed.to:15 in closure in record"),
    ] {
        let stop = ask(&format!("why {expression}"), "stopped");
        assert_eq!(stop["stopped"]["at"], line, "{stop}");
        ask("continue", "stopped");
    }
    drop(input);
    child.wait().unwrap();
    // The editor's `why`, of a variable's menu: the program as it was.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "replay": "run", "cwd": dir }));
    let path = dir.join("changed.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 29 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    // The tasks, and a value over the run, whose changes it goes to.
    let tasks = adapter.ask("tasks", json!({}));
    assert!(tasks["tasks"].as_array().unwrap().iter().any(|t| t["task"] == 1), "{tasks}");
    let timeline = adapter.ask("timeline", json!({ "expression": "counter.hits", "frameId": frame }));
    let last = timeline["changes"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((&last["value"], &last["at"]), (&json!("10"), &json!("changed.to:7 in Counter.bump")), "{timeline}");
    adapter.ask("seek", json!({ "tick": last["tick"] }));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["stackFrames"][0]["line"], 7, "{trace}");
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    let why = adapter.ask("why", json!({ "expression": "items", "frameId": frame }));
    assert_eq!(why["at"], "changed.to:11 in add", "{why}");
    let causes = why["causes"].as_array().unwrap();
    assert!(causes.iter().any(|c| c["name"] == "items" && c["at"] == "changed.to:22 in main"), "{why}");
    let stop = adapter.until("stopped");
    assert_eq!(stop["threadId"], 1, "{stop}");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["stackFrames"][0]["line"], 11, "{trace}");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

/// With `followChildren`, a child the program starts is debugged too: the
/// adapter asks the editor for a session of its own, which attaches to the
/// child, a debug build of Tondo that waited for it.
#[test]
fn debug_adapter_follows_children() {
    if !cfg!(unix) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-children");
    std::fs::create_dir_all(&dir).unwrap();
    let child = "import std.console\n\nfn main() {\n    let greeting = \"hi\"\n    console.println(greeting)\n}\n";
    std::fs::write(dir.join("child.to"), child).unwrap();
    let parent =
        "import std.process\n\nfn main(): !process.ProcessError {\n    _ = process.command(\"./child\").run()?\n}\n";
    std::fs::write(dir.join("parent.to"), parent).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "child.to", "--debug", "--output", "child"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(built.success());
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": dir.join("parent.to"), "cwd": dir, "followChildren": true }));
    adapter.ask("configurationDone", json!({}));
    let asked = adapter.asked("startDebugging");
    assert_eq!(asked["request"], "attach", "{asked}");
    let pid = asked["configuration"]["processId"].clone();
    assert!(asked["configuration"]["name"].as_str().unwrap().starts_with("child ("), "{asked}");
    let mut followed = Adapter::start();
    followed.ask("initialize", json!({ "adapterID": "tondo" }));
    followed.ask("attach", json!({ "processId": pid, "cwd": dir }));
    let path = dir.join("child.to");
    followed.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 5 }] }));
    followed.ask("configurationDone", json!({}));
    followed.until("stopped");
    let trace = followed.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["stackFrames"][0]["line"], 5, "{trace}");
    followed.ask("continue", json!({ "threadId": 1 }));
    followed.until("terminated");
    followed.stop();
    adapter.until("terminated");
    adapter.stop();
}

/// In a replay the editor opens the recording of a child in a session of
/// its own, which replays it.
#[test]
fn debug_adapter_opens_the_recordings_of_children() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-recorded-children");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let child = "import std.console\n\nfn main() {\n    let greeting = \"hi\"\n    console.println(greeting)\n}\n";
    std::fs::write(dir.join("child.to"), child).unwrap();
    let parent =
        "import std.process\n\nfn main(): !process.ProcessError {\n    _ = process.command(\"./child\").run()?\n}\n";
    std::fs::write(dir.join("parent.to"), parent).unwrap();
    for args in [&["build", "child.to", "--debug", "--output", "child"][..], &["run", "--record", "run", "parent.to"]] {
        let ran = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(&dir).output().unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    }
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "replay": "run", "cwd": dir, "followChildren": true }));
    adapter.ask("configurationDone", json!({}));
    let asked = adapter.asked("startDebugging");
    assert_eq!(asked["request"], "launch", "{asked}");
    assert!(asked["configuration"]["name"].as_str().unwrap().starts_with("child ("), "{asked}");
    let mut followed = Adapter::start();
    followed.ask("initialize", json!({ "adapterID": "tondo" }));
    followed.ask("launch", json!({ "replay": asked["configuration"]["replay"], "cwd": dir }));
    let path = dir.join("child.to");
    followed.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 5 }] }));
    followed.ask("configurationDone", json!({}));
    followed.until("stopped");
    let trace = followed.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["stackFrames"][0]["line"], 5, "{trace}");
    followed.ask("continue", json!({ "threadId": 1 }));
    followed.until("terminated");
    followed.stop();
    adapter.until("terminated");
    adapter.stop();
}

#[test]
fn a_dump_keeps_the_bindings_of_its_frames() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-bindings");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "type User = {\n    name: String\n    tags: Array[String]\n}\n\nfn check(user: User, limit: Int) {\n    let over = user.tags.length() - limit\n    if over > 0 {\n        panic(\"too many by {over}\")\n    }\n}\n\nfn main() {\n    let list = [User { name: \"ana\", tags: [\"a\", \"b\", \"c\"] }]\n    var checked = 0\n    for user in list {\n        check(user, 1)\n        checked = checked + 1\n    }\n}\n";
    std::fs::write(dir.join("told.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo)
        .args(["build", "told.to", "--debug", "--output", "told"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(dir.join("told")).env("TONDO_DUMP", &dir).current_dir(&dir).output().unwrap();
    assert!(!ran.status.success());
    let dump = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .find(|p| p.to_string_lossy().ends_with(".dump.json"))
        .unwrap();
    let read = |view: &[&str]| {
        let args = [&["dump", "read", dump.to_str().unwrap()], view].concat();
        let output = Command::new(tondo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let locals = read(&["locals"]);
    assert!(locals.starts_with("told.to:9 in check"), "{locals}");
    assert!(locals.contains("over: Int = 2"), "{locals}");
    assert!(locals.contains("limit: Int = 1"), "{locals}");
    assert_eq!(read(&["print", "user.name"]).trim(), "\"ana\"");
    assert_eq!(read(&["print", "user.tags[2]"]).trim(), "\"c\"");
    // The frame below it, by its task.
    let below = read(&["locals", "1", "1"]);
    assert!(below.contains("checked: Int = 0"), "{below}");
    assert_eq!(read(&["print", "list[0].tags[1]", "1", "1"]).trim(), "\"b\"");
    // In the editor, as it opens the dump.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "dump": dump, "cwd": dir }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    let frames = trace["stackFrames"].as_array().unwrap();
    let frame = frames.iter().find(|f| f["name"] == "check").unwrap_or_else(|| panic!("{trace}"))["id"].clone();
    let scopes = adapter.ask("scopes", json!({ "frameId": frame }));
    assert_eq!(scopes["scopes"][0]["name"], "Bindings", "{scopes}");
    let reference = scopes["scopes"][0]["variablesReference"].clone();
    let variables = adapter.ask("variables", json!({ "variablesReference": reference }));
    let user = variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == "user").unwrap().clone();
    let parts = adapter.ask("variables", json!({ "variablesReference": user["variablesReference"] }));
    assert!(parts.to_string().contains("\\\"ana\\\""), "{parts}");
    let value = adapter.ask("evaluate", json!({ "expression": "user.tags[0]", "frameId": frame }));
    assert_eq!(value["result"], "\"a\"", "{value}");
    adapter.stop();
}

#[test]
fn a_dump_of_a_build_without_probes_has_its_bindings() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-release");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Small functions, which the optimizer inlines: each frame has its own.
    let panics = "import std.console\n\nfn work(rows: Array[Int], scale: Int): Int {\n    var sum = 0\n    for row in rows {\n        sum = sum + row * scale\n    }\n    deep(sum, rows.length())\n}\n\nfn deep(value: Int, count: Int): Int {\n    let limit = 100\n    if value > limit {\n        panic(\"value {value} over {limit} with {count} rows\")\n    }\n    value\n}\n\nfn main() {\n    let rows = [1, 2, 3, 40]\n    let factor = 7\n    let done = work(rows, factor)\n    console.println(\"{done} {rows.length()} {factor}\")\n}\n";
    std::fs::write(dir.join("panics.to"), panics).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo).args(["build", "panics.to", "--output", "panics"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(dir.join("panics")).env("TONDO_DUMP", &dir).current_dir(&dir).output().unwrap();
    assert!(!ran.status.success());
    let dump = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .find(|p| p.to_string_lossy().ends_with(".dump.json"))
        .unwrap();
    let read = |view: &[&str]| {
        let args = [&["dump", "read", dump.to_str().unwrap()], view].concat();
        let output = Command::new(tondo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let deep = read(&["locals", "1", "0"]);
    assert!(deep.starts_with("panics.to:14 in deep"), "{deep}");
    assert!(deep.contains("value: Int = 322"), "{deep}");
    assert!(deep.contains("limit: Int = 100"), "{deep}");
    let work = read(&["locals", "1", "1"]);
    assert!(work.starts_with("panics.to:8 in work"), "{work}");
    assert!(work.contains("sum: Int = 322"), "{work}");
    assert_eq!(read(&["print", "factor", "1", "2"]).trim(), "7");
    // A fault in the program's code, which says its registers.
    let faults = "import std.ffi\n\nextern fn strlen(text: Pointer[UInt8]?): ffi.CSize\n\nfn measure(text: Pointer[UInt8]?, tries: Int): Int {\n    var total = tries\n    for i in 0..3 {\n        total = total + i\n    }\n    _ = unsafe { strlen(text) }\n    total\n}\n\nfn main() {\n    let text: Pointer[UInt8]? = none\n    let first = 40\n    let got = measure(text, first)\n    panic(\"{got} {first}\")\n}\n";
    std::fs::write(dir.join("faults.to"), faults).unwrap();
    let built =
        Command::new(tondo).args(["build", "faults.to", "--output", "faults"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    std::fs::remove_file(&dump).unwrap();
    let ran = Command::new(dir.join("faults")).env("TONDO_DUMP", &dir).current_dir(&dir).output().unwrap();
    assert!(!ran.status.success());
    let dump = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .find(|p| p.to_string_lossy().ends_with(".dump.json"))
        .unwrap();
    let args = ["dump", "read", dump.to_str().unwrap(), "tasks"];
    let tasks = Command::new(tondo).args(args).output().unwrap();
    assert!(String::from_utf8_lossy(&tasks.stdout).contains("in measure"), "{tasks:?}");
}

/// Runs git in `dir`, which must work.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.email=a@example.com", "-c", "user.name=a", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn a_dump_says_what_keeps_memory_alive() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-retained");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("debug")).unwrap();
    std::fs::create_dir_all(dir.join("release")).unwrap();
    let program = "import std.console\n\nfn fill(count: Int): Array[String] {\n    var rows: Array[String] = []\n    \
        for i in 0..count {\n        rows.push(\"row {i} with text that takes some room in memory, more than a little\")\n    \
        }\n    rows\n}\n\nfn main() {\n    let rows = fill(3000)\n    console.println(\"{rows.length()}\")\n    \
        panic(\"look\")\n}\n";
    std::fs::write(dir.join("keeps.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    for (name, flags) in [("debug", &["--debug"][..]), ("release", &[][..])] {
        let output = format!("keeps-{name}");
        let built = Command::new(tondo_bin)
            .args(["build", "keeps.to", "--output", &output])
            .args(flags)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let ran = Command::new(dir.join(&output))
            .env("TONDO_DUMP", dir.join(name))
            .env("TONDO_HEAP_RATE", "4096")
            .output()
            .unwrap();
        assert!(!ran.status.success());
    }
    // The array keeps its strings, which each take much less.
    let dump = std::fs::read_dir(dir.join("debug")).unwrap().next().unwrap().unwrap().path();
    let objects = tondo(&["dump", "read", dump.to_str().unwrap(), "objects", "--json"]);
    let largest = &objects["objects"]["largest"][0];
    assert!(largest["type"].as_str().unwrap().starts_with("Array"), "{largest}");
    assert!(largest["retained"].as_u64().unwrap() > 3000 * 60, "{largest}");
    assert!(largest["retained"].as_u64() > largest["size"].as_u64(), "{largest}");
    // In a build without objects, the samples say the same by type, and
    // more of them say it closer.
    let dump = std::fs::read_dir(dir.join("release")).unwrap().next().unwrap().unwrap().path();
    let heap = tondo(&["dump", "read", dump.to_str().unwrap(), "heap", "--json"]);
    let kinds = heap["heap"]["kinds"].as_array().unwrap();
    let strings = kinds.iter().find(|k| k["kind"] == "String").unwrap_or_else(|| panic!("{heap}"));
    assert!(strings["objects"].as_f64().unwrap() > 1500.0, "{strings}");
}

#[test]
fn the_debugger_stops_at_locks_and_recovered_panics_and_cancels_tasks() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-events");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.sync\nimport std.time\n\n\
        fn hold(lock: sync.Mutex[Int], ready: Sender[Bool], go: Receiver[Bool]) {\n    lock.update((value) {\n        \
        ready.send(true)\n        _ = go.receive()\n        value = value + 1\n    })\n}\n\n\
        fn sleeper() {\n    time.sleep(time.seconds(60))\n}\n\n\
        fn main() {\n    let lock = sync.Mutex(0)\n    let (ready, started) = channel[Bool](1)\n    \
        let (go, going) = channel[Bool](1)\n    let holder = spawn hold(lock, ready, going)\n    \
        _ = started.receive()\n    go.send(true)\n    lock.update((value) { value = value + 1 })\n    \
        holder.wait()\n    let caught = recover { panic(\"caught here\") }\n    \
        console.println(\"{caught.isErr()}\")\n    let job = spawn sleeper()\n    time.sleep(time.milliseconds(1))\n    \
        let long = \"x\".repeat(10000)\n    console.println(\"{long.length()}\")\n    _ = job.wait()\n    \
        console.println(\"ended\")\n}\n";
    std::fs::write(dir.join("events.to"), program).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "events.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let line = program.lines().position(|l| l.contains("long.length()")).unwrap() + 1;
    let script = format!(
        "catch none\nbreak on lock\nbreak on recover\nbreak events.to:{line}\ncontinue\ncontinue\ncontinue\ncancel 3\ncontinue\n"
    );
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    let why = |index: usize| stops[index]["why"].as_str().unwrap_or_default();
    assert!(why(1).starts_with("wants a std.sync lock task 2 holds, taken at"), "{stops:?}");
    assert!(why(2).contains("caught here"), "{stops:?}");
    assert_eq!(stops[3]["at"].as_str().unwrap(), format!("events.to:{line} in main"), "{stops:?}");
    // The task it cancelled does not sleep its minute.
    assert!(text.contains("ended"), "{text}");

    // In the editor: the lines that stop, the files, all of a value and
    // the task it cancels.
    let mut adapter = Adapter::start();
    let capabilities = adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    assert_eq!(capabilities["supportsTerminateThreadsRequest"], true);
    adapter.ask("launch", json!({ "program": "events.to", "cwd": dir }));
    adapter.until("initialized");
    let source = json!({ "name": "events.to", "path": dir.join("events.to") });
    adapter.ask("setBreakpoints", json!({ "source": source, "breakpoints": [{ "line": line }] }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let located = adapter.ask("breakpointLocations", json!({ "source": source, "line": 1, "endLine": line }));
    let located: Vec<u64> =
        located["breakpoints"].as_array().unwrap().iter().map(|b| b["line"].as_u64().unwrap()).collect();
    assert!(located.contains(&(line as u64)) && !located.contains(&1), "{located:?}");
    let loaded = adapter.ask("loadedSources", json!({}));
    assert!(
        loaded["sources"].as_array().unwrap().iter().any(|s| s["path"].as_str().unwrap().ends_with("events.to")),
        "{loaded}"
    );
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    let frame = &trace["stackFrames"][0]["id"];
    let whole = adapter.ask("evaluate", json!({ "expression": "long", "frameId": frame, "context": "clipboard" }));
    assert_eq!(whole["result"].as_str().unwrap().len(), 10002, "{whole}");
    adapter.ask("terminateThreads", json!({ "threadIds": [3] }));
    adapter.ask("continue", json!({ "threadId": stopped["threadId"] }));
    adapter.until("terminated");
    adapter.stop();
}

#[test]
fn dumps_leave_by_a_command_and_group_by_their_failure() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-group");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    std::fs::create_dir_all(dir.join("away")).unwrap();
    // The value in the message changes; the failure is the same. Another
    // argument fails elsewhere.
    let program = "import std.env\n\nfn first(which: String) {\n    _ = recover { panic(\"it went on\") }\n    \
        panic(\"it fell with {which}\")\n}\n\n\
        fn second(items: Array[Int]): Int {\n    items[items.length()]\n}\n\n\
        fn main() {\n    let which = env.args()[0]\n    if which == \"other\" {\n        \
        _ = second([1])\n    }\n    first(which)\n}\n";
    std::fs::write(dir.join("falls.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo_bin).args(["build", "falls.to", "--output", "falls"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    for which in ["a", "b", "other"] {
        let ran = Command::new(dir.join("falls"))
            .arg(which)
            .env("TONDO_DUMP", dir.join("dumps"))
            .env("TONDO_DUMP_COMMAND", "cp \"$TONDO_DUMP_FILE\" away/")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(!ran.status.success());
    }
    // Each dump left before its program did.
    assert_eq!(std::fs::read_dir(dir.join("away")).unwrap().count(), 3);
    let groups = tondo(&["dump", "group", dir.join("away").to_str().unwrap(), "--json"]);
    let list = groups["groups"].as_array().unwrap();
    let counts: Vec<u64> = list.iter().map(|g| g["count"].as_u64().unwrap()).collect();
    assert_eq!(counts, [2, 1], "{groups}");
    // The page says the same, with what can be followed.
    let dump = list[0]["file"].as_str().unwrap();
    let page = dir.join("page.html");
    let report = Command::new(tondo_bin).args(["dump", "report", dump, "--output"]).arg(&page).output().unwrap();
    assert!(report.status.success(), "{report:?}");
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.contains("data-file=\"falls.to\" data-line=\"5\""), "{html}");
    // Its events have the panic that a `recover` stopped too.
    assert!(html.contains("it went on"), "{html}");
    assert!(html.contains("signature: "), "{html}");
    // What it is not told to run does not stop a dump, and a percentage is
    // a limit it can read.
    let bad = Command::new(dir.join("falls"))
        .arg("a")
        .env("TONDO_DUMP", dir.join("dumps"))
        .env("TONDO_DUMP_WHEN", "memory=95%")
        .env("TONDO_DUMP_COMMAND", "exit 3")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&bad.stderr);
    assert!(said.contains("TONDO_DUMP_COMMAND ended with"), "{said}");
    assert!(!said.contains("TONDO_DUMP_WHEN"), "{said}");
}

#[test]
fn a_build_says_where_its_files_came_from_and_brings_them_back() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sources");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    // A library and a service in a directory of a repository, both pushed.
    for bare in ["lib.git", "app.git"] {
        git(&dir, &["init", "--quiet", "--bare", bare]);
    }
    git(&dir, &["clone", "--quiet", "lib.git", "lib"]);
    let lib = dir.join("lib");
    std::fs::create_dir_all(lib.join("src")).unwrap();
    std::fs::write(lib.join("tondo.toml"), "[package]\nname = \"greet\"\nversion = \"1.0.0\"\n").unwrap();
    let library = "pub fn hello(name: String): String {\n    panic(\"boom {name}\")\n}\n";
    std::fs::write(lib.join("src/greet.to"), library).unwrap();
    git(&lib, &["add", "-A"]);
    git(&lib, &["commit", "--quiet", "-m", "one"]);
    git(&lib, &["push", "--quiet", "origin", "HEAD"]);
    let head = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&lib).output().unwrap();
    let commit = String::from_utf8_lossy(&head.stdout).trim().to_string();
    git(&dir, &["clone", "--quiet", "app.git", "app"]);
    let app = dir.join("app/services/api");
    std::fs::create_dir_all(app.join("src")).unwrap();
    let manifest = format!(
        "[package]\nname = \"api\"\nversion = \"0.1.0\"\n\n[dependencies]\ngreet = {{ git = \"file://{}\", rev = \"{commit}\" }}\n",
        dir.join("lib.git").display()
    );
    std::fs::write(app.join("tondo.toml"), manifest).unwrap();
    let main = "import greet\n\nfn main() {\n    greet.hello(\"ada\")\n}\n";
    std::fs::write(app.join("src/main.to"), main).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let cache = dir.join("cache");
    let tondo_in = |args: &[&str]| {
        let mut command = Command::new(tondo_bin);
        command.args(args).current_dir(&app).env("XDG_CACHE_HOME", &cache);
        command.output().unwrap()
    };
    assert!(tondo_in(&["check", "."]).status.success());
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "--quiet", "-m", "one"]);
    git(&app, &["push", "--quiet", "origin", "HEAD"]);
    let built = tondo_in(&["build", ".", "--output", "api"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    Command::new(app.join("api")).env("TONDO_DUMP", dir.join("dumps")).output().unwrap();
    let dump = std::fs::read_dir(dir.join("dumps")).unwrap().next().unwrap().unwrap().path();
    let dump = dump.to_str().unwrap();
    let summary = tondo_in(&["dump", "read", dump]);
    assert!(String::from_utf8_lossy(&summary.stdout).contains("built from "), "{summary:?}");
    // A file that changed since comes as it was, from its repository, and
    // one of a dependency that is not here too, from a cache that starts
    // with none of it.
    std::fs::write(app.join("src/main.to"), format!("// changed\n{main}")).unwrap();
    let cache_of_builds = cache.join("tondo/deps");
    let _ = std::fs::remove_dir_all(&cache_of_builds);
    for (file, text) in [("src/main.to", main), ("greet/src/greet.to", library)] {
        let found = tondo_in(&["source", file, "--from", dump, "--json"]);
        assert!(found.status.success(), "{}", String::from_utf8_lossy(&found.stderr));
        let found: Value = serde_json::from_slice(&found.stdout).unwrap();
        assert_eq!(found["state"], "fetched", "{found}");
        assert_eq!(std::fs::read_to_string(found["path"].as_str().unwrap()).unwrap(), text);
    }
    let found = tondo_in(&["source", "<std>/http.to", "--from", dump, "--json"]);
    assert!(found.status.success(), "{}", String::from_utf8_lossy(&found.stderr));
    // The executable says the same, and with `--no-source-links` nothing.
    let found = tondo_in(&["source", "greet/src/greet.to", "--from", "api", "--json"]);
    assert!(found.status.success(), "{}", String::from_utf8_lossy(&found.stderr));
    assert!(tondo_in(&["build", ".", "--no-source-links", "--output", "quiet"]).status.success());
    assert!(!tondo_in(&["source", "src/main.to", "--from", "quiet"]).status.success());
    let bytes = std::fs::read(app.join("quiet")).unwrap();
    assert!(!bytes.windows(7).any(|window| window == b"app.git"));

    // The editor opens what is not here as if it were, in a dump.
    let mut adapter = Adapter::start_in(&cache);
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "dump": dump, "cwd": app }));
    adapter.until("initialized");
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    let frames = trace["stackFrames"].as_array().unwrap();
    let hello = frames.iter().find(|f| f["source"]["name"] == "greet/src/greet.to");
    let source = &hello.unwrap_or_else(|| panic!("{trace}"))["source"];
    let reference = source["sourceReference"].as_u64().unwrap_or_else(|| panic!("{trace}"));
    let opened = adapter.ask("source", json!({ "source": source, "sourceReference": reference }));
    assert_eq!(opened["content"], library, "{opened}");
    // The standard library's come with `tondo`.
    let standard = frames.iter().find(|f| f["source"]["name"].as_str().is_some_and(|n| n.starts_with("<std>/")));
    if let Some(frame) = standard {
        let source = &frame["source"];
        let opened = adapter.ask("source", json!({ "source": source, "sourceReference": source["sourceReference"] }));
        assert!(opened["content"].as_str().unwrap().contains("fn "), "{opened}");
    }
    adapter.stop();

    // And a breakpoint stops in it, in a program that runs.
    std::fs::write(app.join("src/main.to"), main).unwrap();
    let mut adapter = Adapter::start_in(&cache);
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": ".", "cwd": app }));
    adapter.until("initialized");
    let source = json!({ "name": "greet/src/greet.to", "path": "greet/src/greet.to", "sourceReference": 1 });
    let set = adapter.ask("setBreakpoints", json!({ "source": source, "breakpoints": [{ "line": 2 }] }));
    assert_eq!(set["breakpoints"][0]["verified"], true, "{set}");
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    assert_eq!(trace["stackFrames"][0]["source"]["name"], "greet/src/greet.to", "{trace}");
    adapter.stop();
}

#[test]
fn a_call_in_a_replay_changes_nothing() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-call");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.sync\n\ntype Order = { id: Int, hours: Int }\n\n\
        fn isLate(order: Order): Bool {\n    order.hours > 24\n}\n\n\
        fn bump(cell: sync.Atomic[Int]): Int {\n    _ = cell.add(1)\n    cell.load()\n}\n\n\
        fn main() {\n    let order = Order { id: 1, hours: 30 }\n    let cell = sync.Atomic(10)\n    \
        console.println(\"{order.id}\")\n    console.println(\"end {cell.load()}\")\n}\n";
    std::fs::write(dir.join("late.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let ran =
        Command::new(tondo_bin).args(["run", "--record", "recording", "late.to"]).current_dir(&dir).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let mut child = Command::new(tondo_bin)
        .args(["debug", "--json", "--replay", "recording"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let line = program.lines().position(|l| l.contains("end {cell")).unwrap() + 1;
    let script = format!(
        "break late.to:{line}\ncontinue\ncall isLate(order)\ncall bump(cell)\ncall bump(cell)\ntick\nprint cell\nback\nquit\n"
    );
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let value = |name: &str| lines.iter().filter(|l| l["name"] == name).map(|l| l["value"].clone()).collect::<Vec<_>>();
    assert_eq!(value("isLate(order)"), [json!("true")], "{lines:?}");
    // Each call starts from the run as it was.
    assert_eq!(value("bump(cell)"), [json!("11"), json!("11")], "{lines:?}");
    assert_eq!(value("cell"), [json!("sync.Atomic { value: 10 }")], "{lines:?}");
    // And the run goes on: back to the line before.
    let stops: Vec<&Value> = lines.iter().filter_map(|l| l.get("stopped")).collect();
    assert!(stops.last().unwrap()["at"].as_str().unwrap().contains(&format!("late.to:{}", line - 1)), "{lines:?}");
}

#[test]
fn a_task_that_waits_shows_what_its_registers_held() {
    // The tables of the runtime's frames are those of ELF.
    if !cfg!(target_os = "linux") {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-waiting");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    let program = "import std.console\nimport std.time\n\n\
        fn consume(job: Int, limit: Int, inbox: Receiver[Int]): Int {\n    var seen = job * 3\n    \
        var count = limit + 5\n    for {\n        let next = inbox.receive()\n        match next {\n            \
        some(value) => {\n                seen = seen + value\n                count = count + 1\n                \
        if value > 100 {\n                    console.println(\"{seen} {count} {limit}\")\n                }\n            \
        }\n            none => {\n                return seen + count\n            }\n        }\n    }\n}\n\n\
        fn main() {\n    let (sender, inbox) = channel[Int]()\n    let job = spawn consume(7, 40, inbox)\n    \
        sender.send(5)\n    sender.send(6)\n    time.sleep(time.milliseconds(200))\n    \
        panic(\"look at the task that waits\")\n    job.wait()\n}\n";
    std::fs::write(dir.join("waits.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo_bin).args(["build", "waits.to", "--output", "waits"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    Command::new(dir.join("waits")).env("TONDO_DUMP", dir.join("dumps")).output().unwrap();
    let dump = std::fs::read_dir(dir.join("dumps")).unwrap().next().unwrap().unwrap().path();
    let read = Command::new(tondo_bin).args(["dump", "read"]).arg(&dump).args(["locals", "2"]).output().unwrap();
    let locals = String::from_utf8_lossy(&read.stdout);
    assert!(locals.contains("seen: Int = 32"), "{locals}");
    assert!(locals.contains("count: Int = 47"), "{locals}");
    assert!(locals.contains("limit: Int = 40"), "{locals}");
}

#[test]
fn what_is_secret_stays_out_of_dumps_and_the_debugger() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("secret");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\n\n@secret\ntype Token = String\n\ntype Wrapped = Token\n\n\
        @secret\ntype Pair[T] = { first: T, second: T }\n\n\
        type User = { name: String, @secret password: String, token: Token, tokens: Array[Token] }\n\n\
        enum Login {\n    Anonymous,\n    Named { user: String, @secret pin: Int },\n}\n\n\
        context Auth = {\n    who: String = \"nobody\"\n    @secret session: String = \"\"\n}\n\n\
        fn work(user: User, _wrapped: Wrapped, pair: Pair[Int], login: Login) {\n    \
        with Auth { who: \"ximena\", session: \"s3cr3t\" }\n    console.println(user.password)\n    \
        console.println(\"{pair.first}\")\n    \
        panic(\"stop here\")\n}\n\n\
        fn main() {\n    let tail = \"ter2\"\n    let user = User { name: \"ximena\", password: \"hun{tail}\", \
        token: Token(\"abc{tail}\"), tokens: [Token(\"k1{tail}\")] }\n    \
        work(user, Wrapped(Token(\"xyz{tail}\")), Pair { first: 1, second: 2 }, \
        Login.Named { user: \"ximena\", pin: 987654321 })\n}\n";
    std::fs::write(dir.join("secret.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    // The program itself sees its values.
    let ran = Command::new(tondo).args(["run", "secret.to"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&ran.stdout).contains("hunter2\n1\n"), "{ran:?}");
    let leaks = |text: &str| {
        ["hunter2", "s3cr3t", "987654321", "xyzter2", "abcter2", "k1ter2"].iter().any(|s| text.contains(s))
    };
    // In every build, and in the texts of a debug build's objects.
    for (name, flags) in [("release", &[][..]), ("debug", &["--debug"][..])] {
        let built = Command::new(tondo)
            .args(["build", "secret.to", "--output", name])
            .args(flags)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let dumps = dir.join(format!("{name}-dumps"));
        std::fs::create_dir_all(&dumps).unwrap();
        let ran = Command::new(dir.join(name)).env("TONDO_DUMP", &dumps).output().unwrap();
        assert!(!ran.status.success());
        let dump = std::fs::read_dir(&dumps).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(&dump).unwrap();
        assert!(!leaks(&text), "{text}");
        let read = Command::new(tondo).args(["dump", "read"]).arg(&dump).arg("locals").output().unwrap();
        let locals = String::from_utf8_lossy(&read.stdout);
        assert!(locals.contains("password: <secret>, token: <secret>, tokens: [<secret>]"), "{locals}");
        assert!(locals.contains("login: Login = Login.Named(\"ximena\", <secret>)"), "{locals}");
        // Nothing of it at all when the service says so.
        let dumps = dir.join(format!("{name}-off"));
        std::fs::create_dir_all(&dumps).unwrap();
        Command::new(dir.join(name)).env("TONDO_DUMP", &dumps).env("TONDO_DUMP_VALUES", "off").output().unwrap();
        let dump = std::fs::read_dir(&dumps).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(&dump).unwrap();
        assert!(!text.contains("ximena") && !text.contains("<secret>"), "{text}");
        let read = Command::new(tondo).args(["dump", "read"]).arg(&dump).arg("locals").output().unwrap();
        assert!(String::from_utf8_lossy(&read.stderr).contains("TONDO_DUMP_VALUES=off"), "{read:?}");
    }
    // The debugger shows the same, and a part cannot be opened.
    let mut child = Command::new(tondo)
        .args(["debug", "secret.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let line = program.lines().position(|l| l.contains("panic(")).unwrap() + 1;
    let script =
        format!("break secret.to:{line}\ncontinue\nlocals\nprint user.password\nprint user.tokens[0]\ncontext\nquit\n");
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    // What the program wrote comes first.
    let text = text.split_once("stopped at").unwrap().1;
    assert!(!leaks(text), "{text}");
    assert!(text.contains("user.password = <secret>"), "{text}");
    assert!(text.contains("user.tokens[0] = <secret>"), "{text}");
    assert!(text.contains("session = <secret>"), "{text}");
}

#[test]
fn a_dump_says_who_waits_for_whom() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-why");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    // One task holds a lock while it waits, and five wait for the lock.
    let waits = "import std.console\nimport std.sync\nimport std.time\n\n\
        fn hold(lock: sync.Mutex[Int], gate: Receiver[Int]) {\n    lock.update((value) {\n        \
        _ = gate.receive()\n        value = value + 1\n    })\n}\n\n\
        fn want(lock: sync.Mutex[Int]) {\n    lock.update((value) { value = value + 1 })\n}\n\n\
        fn main() {\n    let order = \"order-77\"\n    let lock = sync.Mutex(0)\n    \
        let (sender, gate) = channel[Int]()\n    let holder = spawn hold(lock, gate)\n    \
        time.sleep(time.milliseconds(50))\n    var group = Group.new[Unit]()\n    for _ in 0..5 {\n        \
        group.add(spawn want(lock))\n    }\n    time.sleep(time.milliseconds(200))\n    \
        console.println(\"ready\")\n    if order != \"\" {\n        panic(\"look now\")\n    }\n    \
        sender.close()\n    holder.wait()\n    group.wait()\n}\n";
    std::fs::write(dir.join("waits.to"), waits).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo_bin)
        .args(["build", "waits.to", "--debug", "--output", "waits"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    Command::new(dir.join("waits")).env("TONDO_DUMP", dir.join("dumps")).output().unwrap();
    let dump = std::fs::read_dir(dir.join("dumps")).unwrap().next().unwrap().unwrap().path();
    let read = |view: &[&str]| tondo(&[&["dump", "read", dump.to_str().unwrap()], view, &["--json"]].concat());
    let why = &read(&["why"])["why"];
    assert_eq!(why["holding"][0]["task"], 2, "{why}");
    assert_eq!(why["holding"][0]["holds_up"].as_array().unwrap().len(), 5, "{why}");
    assert_eq!(why["longest"]["task"], 2, "{why}");
    assert!(why["longest"]["ms"].as_u64().unwrap() >= 200, "{why}");
    assert_eq!(why["crowded"][0]["count"], 5, "{why}");
    let stacks = read(&["stacks"]);
    assert_eq!(stacks["stacks"][0]["count"], 5, "{stacks}");
    assert!(stacks["stacks"][0]["stack"][0].as_str().unwrap().contains("in want"), "{stacks}");
    // The text of a value, with where it is.
    let found = read(&["find", "ORDER-77"]);
    let value = found["found"].as_array().unwrap().iter().find(|f| f["in"] == "value");
    assert_eq!(value.unwrap()["path"], "order", "{found}");
    let none = read(&["find", "nothing-of-this"]);
    assert_eq!(none["found"], json!([]), "{none}");
    // Two tasks that wait for each other are a cycle.
    std::fs::create_dir_all(dir.join("deadlock")).unwrap();
    let cases = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/cases/run/panic_deadlock_locks.to");
    let built = Command::new(tondo_bin).args(["build", cases, "--output"]).arg(dir.join("cycle")).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    Command::new(dir.join("cycle")).env("TONDO_DUMP", dir.join("deadlock")).output().unwrap();
    let dump = std::fs::read_dir(dir.join("deadlock")).unwrap().next().unwrap().unwrap().path();
    let why = tondo(&["dump", "read", dump.to_str().unwrap(), "why", "--json"]);
    let cycle = why["why"]["cycles"][0].as_array().unwrap();
    assert_eq!((cycle.len(), &cycle[0]["task"], &cycle[1]["task"]), (2, &json!(1), &json!(2)), "{why}");
    // The debugger shows the same of a program that stands still.
    let mut child = Command::new(tondo_bin)
        .args(["debug", "waits.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let line = waits.lines().position(|l| l.contains("panic(")).unwrap() + 1;
    let script = format!("break waits.to:{line}\ncontinue\nblocked\nquit\n");
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("task 2 holds up 5 tasks (3, 4, 5, 6, 7)"), "{text}");
}

#[test]
fn dumps_come_on_a_schedule_and_differ() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-schedule");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    let program = "import std.time\n\nfn main() {\n    var items: Array[String] = []\n    var round = 0\n    for round < 45 {\n        for i in 0..500 {\n            items.push(\"item {round} {i}\")\n        }\n        round = round + 1\n        time.sleep(time.milliseconds(100))\n    }\n}\n";
    std::fs::write(dir.join("grows.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo)
        .args(["build", "grows.to", "--debug", "--output", "grows"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(dir.join("grows"))
        .env("TONDO_DUMP", dir.join("dumps"))
        .env("TONDO_DUMP_WHEN", "every=1s,keep=2")
        .output()
        .unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let mut dumps: Vec<_> = std::fs::read_dir(dir.join("dumps")).unwrap().flatten().map(|f| f.path()).collect();
    dumps.sort_by_key(|dump| dump.metadata().unwrap().modified().unwrap());
    assert_eq!(dumps.len(), 2, "{dumps:?}");
    let diff = Command::new(tondo).arg("dump").arg("diff").args(&dumps).output().unwrap();
    let text = String::from_utf8_lossy(&diff.stdout);
    assert!(diff.status.success(), "{}", String::from_utf8_lossy(&diff.stderr));
    // What it says depends on where each dump found the task.
    assert!(text.contains(" later:"), "{text}");
    let wrong = Command::new(dir.join("grows"))
        .env("TONDO_DUMP", dir.join("dumps"))
        .env("TONDO_DUMP_WHEN", "sometimes")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("every=5m"), "{wrong:?}");
}

#[test]
fn a_signal_asks_for_a_dump_and_dumps_show_what_grows() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-trend");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    std::fs::create_dir_all(dir.join("small")).unwrap();
    let program = "import std.console\nimport std.time\n\nfn main() {\n    var kept: Array[String] = []\n    \
        for round in 0..300 {\n        for i in 0..400 {\n            \
        kept.push(\"value {round} {i} with more text in it, as a cache that never lets go of what it holds\")\n        \
        }\n        time.sleep(time.milliseconds(10))\n    }\n    console.println(\"{kept.length()}\")\n}\n";
    std::fs::write(dir.join("leaks.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo_bin).args(["build", "leaks.to", "--output", "leaks"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let child = Command::new(dir.join("leaks"))
        .env("TONDO_DUMP", dir.join("dumps"))
        .env("TONDO_DUMP_WHEN", "every=200ms,keep=100")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The first dump says it is well under way.
    while std::fs::read_dir(dir.join("dumps")).unwrap().next().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let pid = child.id().to_string();
    assert!(Command::new("kill").args(["-QUIT", &pid]).status().unwrap().success());
    let output = child.wait_with_output().unwrap();
    // It went on to its end.
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "120000\n");
    let dumps: Vec<_> = std::fs::read_dir(dir.join("dumps")).unwrap().flatten().map(|f| f.path()).collect();
    let asked = dumps.iter().find(|d| d.to_string_lossy().contains("-quit-1.dump.json.gz"));
    let asked = asked.unwrap_or_else(|| panic!("{dumps:?}"));
    let summary = tondo(&["dump", "read", asked.to_str().unwrap(), "--json"]);
    assert!(summary["summary"].as_str().unwrap().contains("dumped on a signal"), "{summary}");
    let trend = tondo(&["dump", "trend", dir.join("dumps").to_str().unwrap(), "--json"]);
    assert!(trend["dumps"].as_u64().unwrap() >= 3, "{trend}");
    assert_eq!(trend["held"]["rising"], true, "{trend}");
    let first = trend["memory"][0]["place"].as_str().unwrap_or_else(|| panic!("{trend}"));
    assert!(first.contains("leaks.to:8"), "{trend}");
    // Fewer dumps than `keep` fit in the room they are given.
    let ran = Command::new(dir.join("leaks"))
        .env("TONDO_DUMP", dir.join("small"))
        .env("TONDO_DUMP_WHEN", "every=200ms,size=1k")
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(std::fs::read_dir(dir.join("small")).unwrap().count(), 1);
}

#[test]
fn a_dump_of_a_server_has_its_requests_in_flight() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dump-requests");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dumps")).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let program = format!(
        "import std.http\nimport std.time\n\nfn slow(id: Int): String {{\n    time.sleep(time.seconds(60))\n    \
         \"done {{id}}\"\n}}\n\nfn main(): !http.HttpError {{\n    var app = http.App.new()\n    \
         app.get(\"/slow/:id\", slow)\n    app.listen({port})?\n}}\n"
    );
    std::fs::write(dir.join("server.to"), program).unwrap();
    let tondo_bin = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo_bin).args(["build", "server.to", "--output", "server"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut server =
        Command::new(dir.join("server")).env("TONDO_DUMP", dir.join("dumps")).stderr(Stdio::piped()).spawn().unwrap();
    // The client holds its connection open while the handler sleeps.
    let client = (0..200).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::net::TcpStream::connect(("127.0.0.1", port)).ok()
    });
    let mut client = client.unwrap();
    std::io::Write::write_all(&mut client, b"GET /slow/77?token=hidden HTTP/1.1\r\nHost: here\r\n\r\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let pid = server.id().to_string();
    assert!(Command::new("kill").args(["-QUIT", &pid]).status().unwrap().success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let dump = loop {
        let found = std::fs::read_dir(dir.join("dumps")).unwrap().flatten().next();
        if let Some(found) = found.filter(|_| std::time::Instant::now() < deadline) {
            break found.path();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // A file is there before all of it; a dump that reads is all of it.
    let mut requests = Value::Null;
    for _ in 0..100 {
        let read =
            Command::new(tondo_bin).args(["dump", "read"]).arg(&dump).args(["requests", "--json"]).output().unwrap();
        if read.status.success() {
            requests = serde_json::from_slice(&read.stdout).unwrap();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = server.kill();
    let output = server.wait_with_output().unwrap();
    let request = &requests["requests"][0];
    assert_eq!(request["method"], "GET", "{requests}");
    assert_eq!(request["path"], "/slow/77", "{requests}");
    assert_ne!(request["task"], 0, "{requests}");
    assert!(request["ms"].as_u64().unwrap() >= 400, "{requests}");
    // A breakpoint on requests stops the task that takes one.
    let debugged = dir.join("debugged");
    std::fs::create_dir_all(&debugged).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let debug_port = listener.local_addr().unwrap().port();
    drop(listener);
    let program = format!(
        "import std.http\n\nfn main(): !http.HttpError {{\n    var app = http.App.new()\n    \
         app.get(\"/one\", () {{ \"done\" }})\n    app.listen({debug_port})?\n}}\n"
    );
    std::fs::write(debugged.join("server.to"), program).unwrap();
    let mut debugger = Command::new(tondo_bin)
        .args(["debug", "--json", "server.to"])
        .current_dir(&debugged)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut commands = debugger.stdin.take().unwrap();
    std::io::Write::write_all(&mut commands, b"break on request\ncontinue\n").unwrap();
    let connected = (0..600).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::net::TcpStream::connect(("127.0.0.1", debug_port)).ok()
    });
    let mut connected = connected.unwrap();
    std::io::Write::write_all(&mut connected, b"GET /one HTTP/1.1\r\nHost: here\r\n\r\n").unwrap();
    let mut said = BufReader::new(debugger.stdout.take().unwrap());
    let mut stop = String::new();
    while !stop.contains("serves GET /one") {
        stop.clear();
        assert!(said.read_line(&mut stop).unwrap() > 0, "the debugger ended before the request");
    }
    assert!(stop.contains("server.to:5"), "{stop}");
    drop(commands);
    let _ = debugger.kill();
    let _ = debugger.wait();
    // What came before it, in order.
    let events = tondo(&["dump", "read", dump.to_str().unwrap(), "events", "--json"]);
    let what: Vec<&str> = events["events"].as_array().unwrap().iter().map(|e| e["what"].as_str().unwrap()).collect();
    let came = what.iter().position(|w| *w == "request").unwrap_or_else(|| panic!("{events}"));
    assert!(what[came..].contains(&"spawned"), "{events}");
    assert!(events["events"][came]["text"].as_str().unwrap().contains("GET /slow/77"), "{events}");
    // What breaks the copy that shows the values says nothing here.
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!said.contains("memory allocation") && !said.contains("panicked"), "{said}");
}

#[test]
fn a_line_stops_once_however_many_blocks_it_takes() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-one-stop");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.http\n\nfn main(): !http.HttpError {\n    var app = http.App.new()\n    app.get(\n        \"/hit\",\n        (_) {\n            let s = 1\n            http.Reply.text(\"hit {s}\")\n        },\n    )\n    let reply = app.client().get(\"/hit\")?\n    console.println(\"{reply.status()}\")\n}\n";
    std::fs::write(dir.join("once.to"), program).unwrap();
    let mut debugger = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "once.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let commands = "break once.to:10\ncontinue\ncontinue\ncontinue\ncontinue\nquit\n";
    std::io::Write::write_all(&mut debugger.stdin.take().unwrap(), commands.as_bytes()).unwrap();
    let text = String::from_utf8(debugger.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(text.matches("stopped at once.to:10").count(), 1, "{text}");
    assert!(text.contains("200"), "{text}");
    // Nor does a step from it stay on it.
    let mut debugger = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "once.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let commands = "break once.to:10\ncontinue\nnext\nquit\n";
    std::io::Write::write_all(&mut debugger.stdin.take().unwrap(), commands.as_bytes()).unwrap();
    let text = String::from_utf8(debugger.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(text.matches("stopped at once.to:10").count(), 1, "{text}");
}

#[test]
fn a_recording_keeps_the_end_of_a_run() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-last");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let body = "import std.console\nimport std.time\n\nfn main() {\n    let start = time.now()\n    var round = 0\n    for start.elapsed() < time.milliseconds(1000) {\n        round = round + 1\n        if round % 2000 == 0 {\n            time.sleep(time.milliseconds(1))\n        }\n    }\n    console.println(\"rounds {round}\")\n";
    std::fs::write(dir.join("crashes.to"), format!("{body}    panic(\"the end\")\n}}\n")).unwrap();
    std::fs::write(dir.join("ends.to"), format!("{body}}}\n")).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let run = |name: &str, file: &str| {
        let args = ["run", "--record", name, "--last", "200ms", "--hold", "2m", file];
        Command::new(tondo).args(args).current_dir(&dir).output().unwrap()
    };
    // On a busy machine a run may not get as far as dropping its start.
    let ran = (0..5)
        .map(|_| run("crash", "crashes.to"))
        .find(|_| !dir.join("crash/calls").exists())
        .expect("a run that kept only its end");
    assert!(!ran.status.success());
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("the end of the run is held in crash for 2m"), "{said}");
    // The start of the run went, with what recorded it.
    assert!(!dir.join("crash/calls").exists());
    // What it held is on disk, for after the copies.
    let report = std::fs::read_to_string(dir.join("crash/report.txt")).unwrap();
    assert!(report.contains("the run ended in task 1, at crashes.to:14 in main"), "{report}");
    assert!(report.contains("round = "), "{report}");
    let held = std::fs::read_to_string(dir.join("crash/held")).unwrap();
    let first: u64 = held.split(' ').next().unwrap().parse().unwrap();
    assert!(first > 0, "{held}");
    // Twice: a copy stays for the next one.
    for _ in 0..2 {
        let mut debugger = Command::new(tondo)
            .args(["debug", "--replay", "crash"])
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let commands = "continue\nprint round\nback\ntick\nfirst round > 0\nquit\n";
        std::io::Write::write_all(&mut debugger.stdin.take().unwrap(), commands.as_bytes()).unwrap();
        let output = debugger.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(&format!("the recording keeps the run from probe {first} on")), "{text}");
        assert!(text.contains("the end"), "{text}");
        assert!(text.contains("round: Int = "), "{text}");
    }
    // The editor starts from it too, and goes on to the panic.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "replay": "crash", "cwd": dir }));
    adapter.ask("setExceptionBreakpoints", json!({ "filters": ["panic"] }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    assert_eq!(stopped["reason"], "exception", "{stopped}");
    adapter.ask("stepBack", json!({ "threadId": stopped["threadId"] }));
    adapter.until("stopped");
    adapter.stop();
    for pid in held.lines().filter_map(|line| line.split_once(' ')?.1.parse::<i32>().ok()) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    // One that goes well leaves nothing behind.
    let ran = run("end", "ends.to");
    assert!(ran.status.success());
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("the run went well"), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("end/held")).unwrap(), "");
    let debugger = Command::new(tondo).args(["debug", "--replay", "end"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&debugger.stderr).contains("are gone"), "{debugger:?}");
}

#[test]
fn debug_adapter_cancels_a_long_search() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-cancel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\n\nfn main() {\n    var total = 0\n    for i in 0..3000000 {\n        total = total + i\n    }\n    console.println(\"{total}\")\n}\n";
    std::fs::write(dir.join("long.to"), program).unwrap();
    let args = ["run", "--record", "run", "long.to"];
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(&dir).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo", "supportsProgressReporting": true }));
    adapter.ask("launch", json!({ "replay": "run", "cwd": dir }));
    let path = dir.join("long.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 8 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    adapter.send("timeline", json!({ "expression": "total", "frameId": frame }));
    let started = adapter.until("progressStart");
    assert_eq!(started["cancellable"], true, "{started}");
    let began = std::time::Instant::now();
    adapter.send("cancel", json!({ "progressId": started["progressId"] }));
    let answer = adapter.response("timeline");
    assert_eq!((&answer["success"], &answer["message"]), (&json!(false), &json!("cancelled")), "{answer}");
    assert!(began.elapsed().as_secs() < 60, "{:?}", began.elapsed());
    adapter.until("progressEnd");
    // Where it stopped, the session goes on.
    let again = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(again["stackFrames"][0]["line"], 8, "{again}");
    adapter.stop();
    // A replay on its way to a far probe stops where it got to.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "replay": "run", "cwd": dir }));
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 4 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    adapter.ask("seek", json!({ "tick": 5_500_000 }));
    adapter.ask("pause", json!({ "threadId": 1 }));
    let stopped = adapter.until("stopped");
    assert_eq!(stopped["reason"], "pause", "{stopped}");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    assert_eq!(trace["stackFrames"][0]["source"]["name"], "long.to", "{trace}");
    adapter.stop();
}

#[test]
fn debug_adapter_reads_and_writes_memory_and_shows_instructions() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-memory");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\n\nfn twice(n: Int): Int {\n    n * 2\n}\n\nfn main() {\n    var count = 21\n    let text = \"tondo\"\n    count = twice(count)\n    let pair = Pair { left: 5, right: 9 }\n    console.println(\"{count} {text} {pair.right}\")\n}\n\ntype Pair = {\n    left: Int\n    right: Int\n}\n";
    let path = dir.join("memory.to");
    std::fs::write(&path, program).unwrap();
    let mut adapter = Adapter::start();
    let capabilities = adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    assert_eq!(capabilities["supportsDisassembleRequest"], true, "{capabilities}");
    adapter.ask("launch", json!({ "program": "memory.to", "cwd": dir }));
    adapter
        .ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 4 }, { "line": 12 }] }));
    adapter.ask("configurationDone", json!({}));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let pc = |at: usize| trace["stackFrames"][at]["instructionPointerReference"].as_str().unwrap().to_string();
    // The instructions around a stop, of the line it stopped at.
    let shown = adapter
        .ask("disassemble", json!({ "memoryReference": pc(0), "instructionOffset": -4, "instructionCount": 12 }));
    let list = shown["instructions"].as_array().unwrap();
    assert_eq!(list.len(), 12, "{shown}");
    let at = list.iter().position(|i| i["address"] == pc(0)).unwrap_or_else(|| panic!("{} in {shown}", pc(0)));
    assert_eq!(list[at]["line"], 4, "{shown}");
    assert!(!list[at]["instruction"].as_str().unwrap().is_empty(), "{shown}");
    // Exactly where it stopped: after the call of the probe.
    let before = list[at - 1]["instruction"].as_str().unwrap();
    assert!(before.starts_with("call") || before.starts_with("blr"), "{shown}");
    let addresses: Vec<u64> =
        list.iter().map(|i| u64::from_str_radix(&i["address"].as_str().unwrap()[2..], 16).unwrap()).collect();
    assert!(addresses.windows(2).all(|pair| pair[0] < pair[1]), "{shown}");
    // A caller's, at the call.
    let there = adapter.ask("disassemble", json!({ "memoryReference": pc(1), "instructionCount": 1 }));
    assert_eq!(there["instructions"][0]["line"], 10, "{there}");
    // Where a binding's bytes are.
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": 1 }));
    let frame = trace["stackFrames"][0]["id"].clone();
    let scopes = adapter.ask("scopes", json!({ "frameId": frame }));
    let reference = scopes["scopes"][0]["variablesReference"].clone();
    let variables = adapter.ask("variables", json!({ "variablesReference": reference }));
    let named =
        |name: &str| variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == name).unwrap().clone();
    let count = named("count");
    assert_eq!(count["value"], "42", "{variables}");
    let memory = count["memoryReference"].clone();
    let read = adapter.ask("readMemory", json!({ "memoryReference": memory, "count": 8 }));
    assert_eq!(read["data"], "KgAAAAAAAAA=", "{read}");
    // A string's, on the heap.
    let text = adapter.ask("readMemory", json!({ "memoryReference": named("text")["memoryReference"], "count": 256 }));
    assert_ne!(text["data"], read["data"], "{text}");
    // A part's, as it opens and as an expression.
    let parts = adapter.ask("variables", json!({ "variablesReference": named("pair")["variablesReference"] }));
    let right = parts["variables"].as_array().unwrap().iter().find(|v| v["name"] == "right").unwrap();
    let read = adapter.ask("readMemory", json!({ "memoryReference": right["memoryReference"], "count": 8 }));
    assert_eq!(read["data"], "CQAAAAAAAAA=", "{read}");
    let part = adapter.ask("evaluate", json!({ "expression": "pair.right", "frameId": frame, "context": "watch" }));
    assert_eq!(part["memoryReference"], right["memoryReference"], "{part}");
    // Writing, and what the program then has.
    let wrote = adapter.ask("writeMemory", json!({ "memoryReference": memory, "data": "BwAAAAAAAAA=" }));
    assert_eq!(wrote["bytesWritten"], 8, "{wrote}");
    adapter.until("invalidated");
    let printed = adapter.ask("evaluate", json!({ "expression": "count", "frameId": frame, "context": "watch" }));
    assert_eq!(printed["result"], "7", "{printed}");
    // What is not there is not read, and does not crash the program.
    let nothing = adapter.ask("readMemory", json!({ "memoryReference": "0x10", "count": 16 }));
    assert_eq!((&nothing["unreadableBytes"], &nothing["data"]), (&json!(16), &json!("")), "{nothing}");
    assert!(
        adapter.refused("writeMemory", json!({ "memoryReference": "0x10", "data": "AAAA" })).contains("0 of the 3")
    );
    adapter.ask("continue", json!({ "threadId": 1 }));
    let output = adapter.until("output");
    assert!(output["output"].as_str().unwrap().contains("7 tondo 9"), "{output}");
    adapter.until("terminated");
    adapter.stop();
}

const SPAWNING: &str = r#"import std.console

fn work(start: Int, done: Sender[Int]) {
    var total = start
    for i in 0..5 {
        total += i
    }
    done.send(total)
}

fn launch(done: Sender[Int]) {
    spawn work(100, done)
}

fn main() {
    let (done, results) = channel[Int](2)
    launch(done)
    let got = results.receive()
    console.println("{got}")
}
"#;

/// A task's stack goes on with where it was spawned; `locals` says what
/// changed; a breakpoint can be one task's; the history is searched by a
/// condition; `until` runs to a line; `snapshot` gives a stop at once.
#[test]
fn debugger_follows_tasks_and_searches_history() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-spawn");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("spawning.to"), SPAWNING).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "spawning.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "record\nbreak spawning.to:6 task 1\nbreak spawning.to:6 task 2\ncontinue\nstack\nlocals\nnext\nnext\n\
        locals\nback until total == 100\nforward 100\ndelete 2\nuntil 8\nsnapshot\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<(&str, &str, &Value)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), s["why"].as_str().unwrap(), &s["back"]))
        .collect();
    assert_eq!(
        stops[1..],
        [
            ("spawning.to:6 in work", "breakpoint 2", &Value::Null),
            ("spawning.to:5 in work", "step", &Value::Null),
            ("spawning.to:6 in work", "breakpoint 2", &Value::Null),
            ("spawning.to:5 in work", "history", &json!(1)),
            ("spawning.to:6 in work", "history", &json!(0)),
            ("spawning.to:8 in work", "until", &Value::Null),
        ]
    );
    let stack = lines.iter().find(|l| l.get("stack").is_some()).unwrap();
    assert_eq!(stack["spawned"], json!(["spawning.to:12 in launch", "spawning.to:17 in main"]));
    let locals: Vec<&Vec<Value>> = lines.iter().filter_map(|l| l["locals"].as_array()).collect();
    let changed = |locals: &Vec<Value>| -> Vec<String> {
        locals.iter().filter(|l| l["changed"] == true).map(|l| l["name"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!((changed(locals[0]), changed(locals[1])), (vec![], vec!["i".to_string()]));
    let snapshot = lines.iter().find_map(|l| l.get("snapshot")).unwrap();
    assert_eq!((&snapshot["task"], &snapshot["why"]), (&json!(2), &json!("until")));
    assert_eq!(snapshot["frames"][0]["at"], "spawning.to:8 in work");
    let total = snapshot["frames"][0]["locals"].as_array().unwrap().iter().find(|l| l["name"] == "total").unwrap();
    assert_eq!(total["value"], "110");
    assert!(snapshot["before"].as_array().unwrap().len() >= 5, "{snapshot}");
    // The editor shows where the task was spawned after its frames.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "spawning.to", "cwd": dir }));
    let path = dir.join("spawning.to");
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [{ "line": 6 }] }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    let trace = adapter.ask("stackTrace", json!({ "threadId": stopped["threadId"] }));
    let names: Vec<&str> =
        trace["stackFrames"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["work", "spawned at", "launch", "main"]);
    adapter.ask("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": [] }));
    adapter.ask("continue", json!({ "threadId": 1 }));
    adapter.until("terminated");
    adapter.stop();
}

const COUNTED: &str = r#"import std.console

fn count(name: String, done: Sender[String]) {
    var total = 0
    for i in 0..3 {
        total += i
    }
    done.send("{name} {total}")
}

fn main() {
    let (done, finished) = channel[String](1)
    spawn count("a", done.copy())
    spawn count("b", done)
    console.println(finished.receive().unwrapOr(""))
    console.println(finished.receive().unwrapOr(""))
}
"#;

/// Breakpoints at events: a task starts, a channel is used, a wait is long;
/// a frozen task stays while the others go on.
#[test]
fn debugger_stops_at_events_and_freezes_tasks() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-events");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("counted.to"), COUNTED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "counted.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break on spawn\ncontinue\ndelete 1\nfreeze 2\nbreak on send\ncontinue\ntasks\ndelete 2\nthaw 2\n\
        break on wait 0\ncontinue\ndelete 3\nbreak on nothing\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stops: Vec<(&str, &Value, &str)> = lines
        .iter()
        .filter_map(|l| l.get("stopped"))
        .map(|s| (s["at"].as_str().unwrap(), &s["task"], s["why"].as_str().unwrap()))
        .collect();
    // The first of the two tasks to start, as the workers take them.
    let (at, task, why) = stops[1];
    assert!(matches!(task.as_u64(), Some(2 | 3)), "{stops:?}");
    let spawned = format!("task {task} starts, spawned at counted.to:{} in main", 11 + task.as_u64().unwrap());
    assert_eq!((at, why), ("counted.to:4 in count", spawned.as_str()));
    // Task 2 frozen at its first line, task 3 sends.
    assert_eq!(stops[2], ("counted.to:8 in count", &json!(3), "sends on a channel that holds 0 of 1"));
    let tasks = lines.iter().find_map(|l| l["tasks"].as_array()).unwrap();
    // Stands frozen, if it started: task 3 may have come first, and it then
    // stops at its first line once it runs.
    assert!(task != 2 || tasks.iter().any(|t| t["task"] == 2 && t["why"] == "frozen"), "{tasks:?}");
    // Thawed, a task waits on the channel: main for a value, or task 2 for
    // room, whichever ends its wait first.
    let waited = stops[3].2;
    let on_channel = waited.ends_with(" ms to receive from a channel") || waited.ends_with(" ms to send on a channel");
    assert!(waited.starts_with("waited ") && on_channel, "{stops:?}");
    assert!(lines.iter().any(|l| l["error"].as_str().is_some_and(|e| e.starts_with("break on takes"))));
    assert!(text.contains("a 3\n") && text.contains("b 3\n"), "{text}");
    // The editor's event filters.
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "counted.to", "cwd": dir }));
    adapter.ask("setExceptionBreakpoints", json!({ "filters": ["spawn"] }));
    adapter.ask("configurationDone", json!({}));
    let stopped = adapter.until("stopped");
    assert!(matches!(stopped["threadId"].as_u64(), Some(2 | 3)), "{stopped}");
    adapter.ask("setExceptionBreakpoints", json!({ "filters": [] }));
    adapter.ask("continue", json!({ "threadId": stopped["threadId"] }));
    adapter.until("terminated");
    adapter.stop();
}

const RETAINED: &str = r#"import std.console

type Entry = { name: String, data: Array[Int] }

fn fill(cache: Ref[Array[Entry]], count: Int): Array[Entry] {
    var more = cache.value
    for i in 0..count {
        more.push(Entry { name: "entry {i}", data: [i, i + 1] })
    }
    more
}

fn main() {
    let first = fill(Ref([]), 3)
    let kept = Ref(first)
    let second = fill(kept, 50)
    console.println("{second.length()}")
}
"#;

/// `retainers` says what keeps a value alive, and `memory` where the
/// objects were made and how that grew since it last counted.
#[test]
fn debugger_tells_what_holds_memory() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-memory");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("retained.to"), RETAINED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "retained.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break retained.to:16\ncontinue\nmemory\nretainers first\nnext\nmemory\nretainers nothing\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let counted: Vec<&Value> = lines.iter().filter_map(|l| l.get("memory")).collect();
    assert!(counted[0]["objects"].as_u64() > Some(0), "{:?}", counted[0]);
    let places = counted[0]["places"].as_array().unwrap();
    assert!(places.iter().any(|p| p["at"] == "retained.to:8 in fill"), "{places:?}");
    // A line later, a place that made more says so.
    let grown = counted[1]["places"].as_array().unwrap();
    assert!(grown.iter().any(|p| p["more"].as_i64() > Some(0)), "{grown:?}");
    let held = lines.iter().find_map(|l| l.get("retainers")).unwrap();
    assert!(held["object"].as_str().unwrap().ends_with("made at retained.to:8 in fill"), "{held}");
    assert_eq!(held["stacks"], json!([1]));
    assert!(lines.iter().any(|l| l["error"].as_str().is_some_and(|e| e.contains("nothing"))));
}

const VIEWED: &str = r#"import std.console
import std.debug

type Stack = { top: Int?, below: Array[Int] }

impl debug.View[Array[Int]] for Stack {
    fn view(self): Array[Int] {
        match self.top {
            some(top) => self.below.concat([top])
            none => self.below
        }
    }
}

type Holder = { name: String, stack: Stack }

fn main() {
    let stack = Stack { top: some(3), below: [1, 2] }
    let holder = Holder { name: "h", stack: stack }
    console.println("{stack.below.length()} {holder.name}")
}
"#;

/// A type with a `debug.View` shows as its view, also inside another
/// value, and its value as it is is at hand as `raw()`.
#[test]
fn debugger_shows_views() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-views");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("viewed.to"), VIEWED).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "viewed.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script =
        "break viewed.to:20\ncontinue\nlocals\nprint stack[2]\nprint stack.raw().top\nchildren stack\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let locals = lines.iter().find_map(|l| l["locals"].as_array()).unwrap();
    assert_eq!((&locals[0]["value"], &locals[0]["viewed"]), (&json!("[1, 2, 3]"), &json!(true)));
    assert_eq!(locals[1]["value"], "Holder { name: \"h\", stack: [1, 2, 3] }");
    let printed: Vec<(&str, &str)> = lines
        .iter()
        .filter(|l| l["name"].is_string())
        .map(|l| (l["name"].as_str().unwrap(), l["value"].as_str().unwrap()))
        .collect();
    assert_eq!(printed, [("stack[2]", "3"), ("stack.raw().top", "some(3)")]);
    let parts = lines.iter().find_map(|l| l["children"].as_array()).unwrap();
    assert_eq!(
        (&parts[0]["path"], &parts[0]["value"]),
        (&json!("stack.raw()"), &json!("Stack { top: some(3), below: [1, 2] }"))
    );
    assert_eq!(parts.len(), 4);
}

/// A breakpoint that captures gives the stack and the bindings where its
/// condition holds, and the program goes on.
#[test]
fn debugger_captures_without_stopping() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-capture");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("spawning.to"), SPAWNING).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["debug", "--json", "spawning.to"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let script = "break spawning.to:6 if i == 2 capture\ncontinue\n";
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let captured: Vec<&Value> = lines.iter().filter_map(|l| l.get("captured")).collect();
    assert_eq!(captured.len(), 1, "{lines:?}");
    assert_eq!((&captured[0]["at"], &captured[0]["task"]), (&json!("spawning.to:6 in work"), &json!(2)));
    let total = captured[0]["locals"].as_array().unwrap().iter().find(|l| l["name"] == "total").unwrap();
    assert_eq!(total["value"], "101");
    // It never stopped: the program ran to its end.
    assert_eq!(lines.iter().filter(|l| l.get("stopped").is_some()).count(), 1);
    assert!(text.contains("some(110)"), "{text}");
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
        Adapter::with(&[])
    }

    /// With the cache of `tondo` in a directory of its own.
    fn start_in(cache: &Path) -> Adapter {
        Adapter::with(&[("XDG_CACHE_HOME", cache.as_os_str())])
    }

    fn with(variables: &[(&str, &std::ffi::OsStr)]) -> Adapter {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("dap")
            .envs(variables.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Adapter { child, input, output, seq: 0, events: Vec::new() }
    }

    fn ask(&mut self, command: &str, arguments: Value) -> Value {
        let message = self.respond(command, arguments);
        assert_eq!(message["success"], true, "{message}");
        message["body"].clone()
    }

    /// Why the adapter refused a request.
    fn refused(&mut self, command: &str, arguments: Value) -> String {
        let message = self.respond(command, arguments);
        assert_eq!(message["success"], false, "{message}");
        message["message"].as_str().unwrap_or_default().to_string()
    }

    fn respond(&mut self, command: &str, arguments: Value) -> Value {
        self.send(command, arguments);
        self.response(command)
    }

    /// A request, whose response comes later.
    fn send(&mut self, command: &str, arguments: Value) {
        self.seq += 1;
        let text =
            json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        std::io::Write::write_all(&mut self.input, format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
            .unwrap();
    }

    fn response(&mut self, command: &str) -> Value {
        loop {
            let message = dap_read(&mut self.output);
            if message["type"] == "response" && message["command"] == command {
                return message;
            }
            self.events.push(message);
        }
    }

    /// The arguments of a request the adapter makes of the editor.
    fn asked(&mut self, command: &str) -> Value {
        // It may have come while the answer to another request was awaited.
        if let Some(at) = self.events.iter().position(|e| e["type"] == "request" && e["command"] == command) {
            return self.events.remove(at)["arguments"].clone();
        }
        loop {
            let message = dap_read(&mut self.output);
            if message["type"] == "request" && message["command"] == command {
                return message["arguments"].clone();
            }
            // No more requests come: waiting would hang the tests.
            assert!(message["event"] != "terminated", "the session ended before `{command}`: {:?}", self.events);
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
            // No more events come: waiting would hang the tests.
            assert!(message["event"] != "terminated", "the session ended before `{event}`: {:?}", self.events);
            self.events.push(message);
        }
    }

    fn stop(mut self) {
        self.ask("disconnect", json!({}));
        drop(self.input);
        self.child.wait().unwrap();
    }
}

const TICKING: &str = "enum Mode {
    Quiet
    Loud
}

fn word(): String {
    \"v1\"
}

let _mode = Ref(Mode.Quiet)
var turns = 0
for {
    turns = (turns + word().length()) % 1000
}
";

/// Debug Live: the request `reload` says what changed, or why it did not,
/// and whether the change needs the program started again.
#[test]
fn debug_adapter_reloads_a_live_build() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dap-live");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ticking.to");
    std::fs::write(&file, TICKING).unwrap();
    let mut adapter = Adapter::start();
    adapter.ask("initialize", json!({ "adapterID": "tondo" }));
    adapter.ask("launch", json!({ "program": "ticking.to", "cwd": dir, "live": true }));
    adapter.until("initialized");
    adapter.ask("configurationDone", json!({}));
    std::fs::write(&file, TICKING.replace("\"v1\"", "\"v2\"")).unwrap();
    let reloaded = adapter.ask("reload", json!({}));
    assert_eq!(reloaded["changed"], json!(["word"]), "{reloaded}");
    std::fs::write(&file, TICKING.replace("\"v1\"\n}", "\"v2\"\n")).unwrap();
    let refused = adapter.ask("reload", json!({}));
    assert_eq!(refused["restart"], false, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("not reloaded"), "{refused}");
    std::fs::write(&file, TICKING.replace("    Quiet\n", "").replace("Ref(Mode.Quiet)", "Ref(Mode.Loud)")).unwrap();
    let refused = adapter.ask("reload", json!({}));
    assert_eq!(refused["restart"], true, "{refused}");
    adapter.stop();
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
