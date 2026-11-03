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
    for _ in 0..480 {
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
    // It waits in `receive`, or, sent its value, is ready to go there.
    assert_eq!((&tasks[1]["task"], &tasks[1]["at"]), (&json!(2), &json!("waited.to:8 in waiter")));
    assert!(tasks[1]["why"] == "waits" || tasks[1]["why"] == "ready", "{tasks:?}");
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
    let waits = json!({ "task": 2, "at": "deadlocked.to:3 in waiter", "why": "waits", "for": "a value of a channel that holds 0 of 1" });
    assert_eq!(tasks[1], waits);
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
    let observed = |args: &[&str]| -> Value {
        let output =
            Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).args(["--via", &via, "--json"]).output().unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    };
    let counters = observed(&["counters", &pid, "--count", "1"]);
    let dumped = observed(&["dump", &pid, "--output", dir.join("looped.dump.json").to_str().unwrap()]);
    let listed = observed(&["ps"]);
    let _ = program.kill();
    let _ = program.wait();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stop = lines.iter().find_map(|l| l.get("stopped")).unwrap();
    assert_eq!(stop["at"], "looped.to:6 in main");
    assert_eq!(lines.iter().find(|l| l["name"] == "ticks > 0").unwrap()["value"], "true");
    assert!(counters["counters"].as_array().is_some_and(|c| !c.is_empty()), "{counters}");
    assert!(dumped["summary"].as_str().unwrap().starts_with("looped (pid "), "{dumped}");
    assert!(listed["programs"].as_array().unwrap().iter().any(|p| p["pid"].as_u64() == Some(program_id)), "{listed}");
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
    if !cfg!(target_os = "linux") {
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

/// In a replay, `reverse`, `back` and `why` go back by running the
/// recording again, from a checkpoint where the program stopped before, to
/// the program as it was there, every value of it.
#[test]
fn debugger_goes_back_in_a_replay() {
    if !cfg!(target_os = "linux") {
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
    assert_eq!(value(&mut ask, "total"), total - rolls[2]);
    assert_eq!(value(&mut ask, "i"), 2);
    // Two lines of the task back: the start of the iteration's `let`.
    let stop = ask("back 1", "stopped");
    assert_eq!(stop["stopped"]["at"], "rolled.to:8 in main", "{stop}");
    // And on again, the same.
    ask("next", "stopped");
    ask("next", "stopped");
    assert_eq!(value(&mut ask, "total"), total);
    drop(input);
    child.wait().unwrap();
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
    assert!(tasks.iter().any(|t| t["task"] == 2 && t["why"] == "frozen"), "{tasks:?}");
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
        self.seq += 1;
        let text =
            json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        std::io::Write::write_all(&mut self.input, format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
            .unwrap();
        loop {
            let message = dap_read(&mut self.output);
            if message["type"] == "response" && message["command"] == command {
                return message;
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
