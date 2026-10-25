//! Live builds (toolchain 7.10): a running program's code changes with
//! `tondo reload`, under the debugger with `reload`, and by itself with
//! `tondo run --watch`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const SERVER: &str = r#"import std.console

type Reply = { text: String, size: Int }

fn reply(line: String): Reply {
    Reply { text: "v1 {line}", size: line.length() }
}

var seen = 0
for {
    match console.readLine() {
        some(line) => {
            seen += 1
            let r = reply(line)
            console.println("{r.text} {r.size} after {seen}")
        }
        none => break
    }
}
"#;

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A program and the lines it writes, as they come.
struct Running {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<String>,
    errors: mpsc::Receiver<String>,
}

impl Running {
    fn start(dir: &Path, args: &[&str]) -> Running {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = |stream: Box<dyn std::io::Read + Send>| {
            let (send, lines) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = send.send(line);
                }
            });
            lines
        };
        let input = child.stdin.take().unwrap();
        let out = lines(Box::new(child.stdout.take().unwrap()));
        let errors = lines(Box::new(child.stderr.take().unwrap()));
        Running { child, input, lines: out, errors }
    }

    /// The next line it writes for `sent`.
    fn answer(&mut self, sent: &str) -> String {
        writeln!(self.input, "{sent}").unwrap();
        self.input.flush().unwrap();
        match self.lines.recv_timeout(Duration::from_secs(60)) {
            Ok(line) => line,
            Err(_) => {
                let errors: Vec<String> = self.errors.try_iter().collect();
                panic!("no answer to {sent}; the error stream said {errors:?}")
            }
        }
    }

    /// The process ID of the program: the child itself, which `tondo run`
    /// became.
    fn pid(&self) -> String {
        self.child.id().to_string()
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reload(dir: &Path, pid: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(["reload", pid]).current_dir(dir).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

fn edit(path: &Path, old: &str, new: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(old), "{old}");
    std::fs::write(path, text.replace(old, new)).unwrap();
}

#[test]
fn a_running_program_changes() {
    let dir = dir("live_reload");
    let file = dir.join("server.to");
    std::fs::write(&file, SERVER).unwrap();
    let mut server = Running::start(&dir, &["run", "server.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1 after 1");

    edit(&file, "\"v1 {line}\"", "\"v2 {line.toUpper()}\"");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("1 changed (reply)"), "{said}");
    assert_eq!(server.answer("b"), "v2 B 1 after 2");

    // Files that do not compile leave it as it was.
    edit(&file, "\"v2 {line.toUpper()}\"", "\"v3\" +");
    let (done, said) = reload(&dir, &server.pid());
    assert!(!done && said.contains("error[") && said.contains("the program goes on as it was"), "{said}");
    assert_eq!(server.answer("c"), "v2 C 1 after 3");

    // A type with other fields converts the values frames hold.
    edit(&file, "\"v3\" +", "\"v3 {line}\"");
    edit(&file, "type Reply = { text: String, size: Int }", "type Reply = { text: String, size: Int, more: Bool }");
    edit(&file, "size: line.length() }", "size: line.length(), more: true }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`Reply.more` is new: values alive get false"), "{said}");

    // A new type and function come in.
    edit(&file, "type Reply = { text: String, size: Int, more: Bool }", "type Reply = { text: String, size: Int }");
    edit(&file, "size: line.length(), more: true }", "size: twice(line.length()) }");
    edit(&file, "var seen = 0", "fn twice(n: Int): Int {\n    n * 2\n}\n\nvar seen = 0");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("added (twice)"), "{said}");
    assert_eq!(server.answer("dd"), "v3 dd 4 after 4");
    server.stop();
}

const SHARED: &str = r#"import std.console

type State = { count: Int }

let shared = Ref(State { count: 0 })
var seen = 0
for {
    match console.readLine() {
        some(line) => {
            seen += 1
            console.println("v1 {line} {shared.value.count} {seen}")
        }
        none => break
    }
}
"#;

const LOOP: &str = r#"import std.console

fn label(n: Int): String {
    let text = "old {n}"
    text
}

for i in 0..3 {
    console.println(label(i))
}
"#;

#[test]
fn edit_and_continue() {
    let dir = dir("live_debug");
    let file = dir.join("loop.to");
    std::fs::write(&file, LOOP).unwrap();
    let mut debugger = Running::start(&dir, &["debug", "--live", "--json", "loop.to"]);
    let said = |debugger: &mut Running, until: &str| -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let line = debugger.lines.recv_timeout(Duration::from_secs(60)).expect("the debugger's answer");
            let done = line.contains(until);
            lines.push(line);
            if done {
                return lines;
            }
        }
    };
    let send = |debugger: &mut Running, command: &str| {
        writeln!(debugger.input, "{command}").unwrap();
        debugger.input.flush().unwrap();
    };
    said(&mut debugger, "\"start\"");
    send(&mut debugger, "break loop.to:5");
    said(&mut debugger, "breakpoint");
    send(&mut debugger, "continue");
    said(&mut debugger, "\"stopped\"");
    // Stopped in `label`: its frame goes on in the new code with the text
    // the old one made, and the next call runs the new code all through,
    // where the breakpoint is too.
    edit(&file, "\"old {n}\"", "\"new {n * 10}\"");
    send(&mut debugger, "reload");
    let reloaded = said(&mut debugger, "reloaded");
    assert!(reloaded.last().unwrap().contains("\"changed\":[\"label\"]"), "{reloaded:?}");
    send(&mut debugger, "continue");
    let next = said(&mut debugger, "\"stopped\"");
    assert!(next.iter().any(|l| l == "old 0"), "{next:?}");
    send(&mut debugger, "delete 1");
    said(&mut debugger, "deleted");
    send(&mut debugger, "continue");
    let rest = said(&mut debugger, "exited");
    assert!(rest.iter().any(|l| l == "new 10") && rest.iter().any(|l| l == "new 20"), "{rest:?}");
    debugger.stop();
}

const MODES: &str = r#"import std.console

enum Mode {
    Quiet
    Loud
}

let _mode = Ref(Mode.Quiet)
var seen = 0
for {
    match console.readLine() {
        some(line) => {
            seen += 1
            console.println("v1 {line} {seen}")
        }
        none => break
    }
}
"#;

#[test]
fn watching_reloads_what_is_saved() {
    let dir = dir("live_watch");
    let file = dir.join("server.to");
    std::fs::write(&file, MODES).unwrap();
    let mut server = Running::start(&dir, &["run", "server.to", "--watch"]);
    assert_eq!(server.answer("a"), "v1 a 1");
    edit(&file, "\"v1 {line}", "\"v2 {line}");
    let reloaded = server.errors.recv_timeout(Duration::from_secs(60)).expect("a reload");
    assert!(reloaded.contains("reloaded: 1 changed (main)"), "{reloaded}");
    assert_eq!(server.answer("b"), "v2 b 2");
    // A change it cannot make in place runs the program again: a `Ref`
    // keeps a value of a variant the new code does not have.
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("    Quiet\n", "").replace("Ref(Mode.Quiet)", "Ref(Mode.Loud)")).unwrap();
    let restarted = server.errors.recv_timeout(Duration::from_secs(60)).expect("a restart");
    assert!(restarted.contains("running it again"), "{restarted}");
    assert_eq!(server.answer("c"), "v2 c 1");
    server.stop();
}

const HANDLER: &str = r#"import std.console

type Reply = { text: String }

fn handle(line: String): String {
    let reply = Reply { text: "v1 {line}" }
    reply.text
}

fn serve(handler: fn(String): String) {
    for {
        match console.readLine() {
            some(line) => console.println(handler(line))
            none => break
        }
    }
}

serve((line) { "{handle(line)}!" })
"#;

/// A closure made before a reload runs its new body, and a type no
/// running frame holds changes in place.
#[test]
fn closures_and_types_change() {
    let dir = dir("live_closures");
    let file = dir.join("handler.to");
    std::fs::write(&file, HANDLER).unwrap();
    let mut server = Running::start(&dir, &["run", "handler.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a!");
    edit(&file, "{handle(line)}!", "{handle(line)}?");
    edit(&file, "type Reply = { text: String }", "type Reply = { text: String, size: Int }");
    edit(
        &file,
        "Reply { text: \"v1 {line}\" }\n    reply.text",
        "Reply { text: \"v2 {line}\", size: 2 }\n    \"{reply.text} {reply.size}\"",
    );
    let (done, said) = reload(&dir, &server.pid());
    assert!(done, "{said}");
    assert_eq!(server.answer("b"), "v2 b 2?");
    server.stop();
}

const HOLDER: &str = r#"import std.console

type State = { count: Int }

var state = State { count: 0 }
for {
    match console.readLine() {
        some(line) => {
            state = State { count: state.count + 1 }
            console.println("{line} {state.count}")
        }
        none => break
    }
}
"#;

/// A type of another shape converts the values a running frame holds, by
/// field name, as the frame moves to the new code; new fields start empty.
#[test]
fn values_a_running_frame_holds_convert() {
    let dir = dir("live_holder");
    let file = dir.join("holder.to");
    std::fs::write(&file, HOLDER).unwrap();
    let mut server = Running::start(&dir, &["run", "holder.to", "--live"]);
    assert_eq!(server.answer("a"), "a 1");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, label: String }");
    edit(&file, "State { count: 0 }", "State { count: 0, label: \"\" }");
    edit(&file, "State { count: state.count + 1 }", "State { count: state.count + 1, label: \"{state.label}+\" }");
    edit(&file, "\"{line} {state.count}\"", "\"{line} {state.count} {state.label}\"");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State.label` is new: values alive get \"\""), "{said}");
    assert_eq!(server.answer("b"), "b 2 +");
    // A new field without an empty value leaves the frame on the old code.
    edit(&file, "type State = {", "enum Mode {\n    Fast\n    Slow\n}\n\ntype State = { mode: Mode,");
    edit(&file, "State { count: 0,", "State { mode: Mode.Fast, count: 0,");
    edit(&file, "State { count: state.count + 1,", "State { mode: Mode.Slow, count: state.count + 1,");
    let (done, said) = reload(&dir, &server.pid());
    assert!(
        done && said.contains("`State.mode` is new and `Mode` has no empty value to start with: make it `Mode?`"),
        "{said}"
    );
    assert_eq!(server.answer("c"), "c 3 ++");
    server.stop();
}

const NESTED: &str = r#"import std.console

type Entry = { name: String, hits: Int }
type State = { entries: Array[Entry], index: Map[String, Entry], total: Int, note: String }

enum Mode {
    Idle
    Busy(Int)
}

var state = State { entries: [], index: [:], total: 0, note: "x" }
var mode = Mode.Idle
for {
    match console.readLine() {
        some(line) => {
            if line == "busy" {
                mode = Mode.Busy(5)
            }
            let entry = Entry { name: line, hits: 1 }
            state.entries.push(entry)
            state.index[line] = entry
            state.total += 1
            let shown = match mode {
                Mode.Idle => "idle"
                Mode.Busy(_) => "busy"
            }
            console.println("{line} {state.total} {state.index.length()} {shown}")
        }
        none => break
    }
}
"#;

/// Values inside collections and other values convert too; a value of a
/// variant the new code has not leaves its frame on the old code, which
/// the program's error stream says.
#[test]
fn collections_and_variants_convert() {
    let dir = dir("live_nested");
    let file = dir.join("nested.to");
    std::fs::write(&file, NESTED).unwrap();
    let mut server = Running::start(&dir, &["run", "nested.to", "--live"]);
    assert_eq!(server.answer("a"), "a 1 1 idle");
    edit(&file, "hits: Int }", "hits: Int, tags: Array[String] }");
    edit(&file, "total: Int, note: String }", "total: Int, seen: Set[String] }");
    edit(&file, "total: 0, note: \"x\" }", "total: 0, seen: Set[] }");
    edit(&file, "hits: 1 }", "hits: 1, tags: [\"t\"] }");
    edit(
        &file,
        "            state.total += 1\n",
        "            state.total += 1\n            state.seen.insert(line)\n",
    );
    edit(
        &file,
        "{state.index.length()}",
        "{state.index.length()} {state.entries[0].tags.length()} {state.seen.length()}",
    );
    let (done, said) = reload(&dir, &server.pid());
    assert!(
        done && said.contains("`Entry.tags` is new: values alive get []")
            && said.contains("`State.note` is gone")
            && said.contains("`State.seen` is new: values alive get Set[]"),
        "{said}"
    );
    assert_eq!(server.answer("b"), "b 2 2 0 1 idle");
    assert_eq!(server.answer("busy"), "busy 3 3 0 2 busy");
    edit(&file, "    Busy(Int)\n", "    Paused\n");
    edit(&file, "mode = Mode.Busy(5)", "mode = Mode.Paused");
    edit(&file, "Mode.Busy(_) => \"busy\"", "Mode.Paused => \"paused\"");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`Mode.Busy` is gone: a frame holding one stays on the old code"), "{said}");
    assert_eq!(server.answer("c"), "c 4 4 0 3 busy");
    let told = server.errors.recv_timeout(Duration::from_secs(60)).expect("why the frame stays");
    assert!(
        told.contains("one of its `Mode` values does not convert: it is of a variant the new code does not have"),
        "{told}"
    );
    server.stop();
}

#[test]
fn values_kept_in_a_ref_convert_in_place() {
    let dir = dir("live_shared");
    let file = dir.join("shared.to");
    std::fs::write(&file, SHARED).unwrap();
    let mut server = Running::start(&dir, &["run", "shared.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 0 1");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, more: Bool }");
    edit(&file, "State { count: 0 }", "State { count: 0, more: true }");
    edit(&file, "\"v1 {line} {shared.value.count}", "\"v2 {line} {shared.value.count} {shared.value.more}");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State.more` is new: values alive get false"), "{said}");
    assert_eq!(server.answer("b"), "v2 b 0 false 2");
    server.stop();
}

const CHANNEL: &str = r#"import std.console
import std.sync

type State = { count: Int }

fn worker(inbox: Receiver[State], total: sync.Mutex[State]) {
    for state in inbox {
        total.set(State { count: total.get().count + state.count })
        console.println("v1 got {state.count} total {total.get().count}")
    }
}

let total: sync.Mutex[State] = sync.Mutex(State { count: 0 })
let (sender, receiver) = channel[State](8)
spawn worker(receiver, total)
for {
    match console.readLine() {
        some(line) => sender.send(State { count: line.length() })
        none => break
    }
}
sender.close()
"#;

#[test]
fn channels_and_std_sync_values_convert_in_place() {
    let dir = dir("live_channel");
    let file = dir.join("channel.to");
    std::fs::write(&file, CHANNEL).unwrap();
    let mut server = Running::start(&dir, &["run", "channel.to", "--live"]);
    assert_eq!(server.answer("ab"), "v1 got 2 total 2");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, tag: String }");
    edit(&file, "state.count })", "state.count, tag: state.tag })");
    edit(&file, "\"v1 got {state.count}", "\"v2 got {state.count} [{state.tag}]");
    edit(&file, "State { count: 0 }", "State { count: 0, tag: \"\" }");
    edit(&file, "line.length() }", "line.length(), tag: line }");
    // `main` waits for a line in a statement that sends a `State`: the
    // statement starts again in the new code.
    let (done, said) = reload(&dir, &server.pid());
    assert!(done, "{said}");
    assert_eq!(server.answer("xyz"), "v2 got 3 [xyz] total 5");
    server.stop();
}

const BORROWED: &str = r#"import std.console

type State = { count: Int }

fn watch(state: State) {
    for {
        match console.readLine() {
            some(line) => console.println("v1 {line} {state.count}")
            none => break
        }
    }
}

let shared = Ref(State { count: 7 })
watch(shared.value)
"#;

#[test]
fn a_value_a_ref_lends_does_not_convert() {
    let dir = dir("live_borrowed");
    let file = dir.join("lent.to");
    std::fs::write(&file, BORROWED).unwrap();
    let mut server = Running::start(&dir, &["run", "lent.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 7");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, more: Bool }");
    edit(&file, "State { count: 7 }", "State { count: 7, more: true }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(!done && said.contains("a task is in a call at `main` (lent.to:15) whose statement may use"), "{said}");
    assert_eq!(server.answer("b"), "v1 b 7");
    server.stop();
}

#[test]
fn a_stopped_frame_restarts() {
    let dir = dir("live_restart");
    let file = dir.join("loop.to");
    std::fs::write(&file, LOOP).unwrap();
    let mut debugger = Running::start(&dir, &["debug", "--live", "--json", "loop.to"]);
    let said = |debugger: &mut Running, until: &str| -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let line = debugger.lines.recv_timeout(Duration::from_secs(60)).expect("the debugger's answer");
            let done = line.contains(until);
            lines.push(line);
            if done {
                return lines;
            }
        }
    };
    let send = |debugger: &mut Running, command: &str| {
        writeln!(debugger.input, "{command}").unwrap();
        debugger.input.flush().unwrap();
    };
    said(&mut debugger, "\"start\"");
    send(&mut debugger, "break loop.to:5");
    said(&mut debugger, "breakpoint");
    send(&mut debugger, "continue");
    said(&mut debugger, "\"stopped\"");
    edit(&file, "\"old {n}\"", "\"new {n * 10}\"");
    send(&mut debugger, "reload");
    said(&mut debugger, "reloaded");
    // The frame returns and its function runs again, from its first line,
    // with its new code.
    send(&mut debugger, "restart");
    let restarted = said(&mut debugger, "\"stopped\"");
    assert!(
        restarted.last().unwrap().contains("\"why\":\"restart\"") && restarted.last().unwrap().contains("loop.to:4"),
        "{restarted:?}"
    );
    send(&mut debugger, "delete 1");
    said(&mut debugger, "deleted");
    send(&mut debugger, "continue");
    let rest = said(&mut debugger, "exited");
    assert!(rest.iter().any(|l| l == "new 0") && !rest.iter().any(|l| l == "old 0"), "{rest:?}");
    debugger.stop();
}

/// A running frame moves to the new code at its next line, with the values
/// it has; one that needs a value the old code does not have there stays
/// until it reaches a line where it has all it needs.
#[test]
fn running_frames_move_to_the_new_code() {
    let dir = dir("live_moves");
    let file = dir.join("server.to");
    std::fs::write(&file, SERVER).unwrap();
    let mut server = Running::start(&dir, &["run", "server.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1 after 1");
    edit(&file, "after {seen}\")", "after {seen}!\")");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("1 changed (main); frames move to the new code at their next statement"), "{said}");
    assert_eq!(server.answer("b"), "v1 b 1 after 2!");
    edit(&file, "            seen += 1\n", "            seen += 1\n            let tenfold = seen * 10\n");
    edit(&file, "after {seen}!\")", "after {seen} {tenfold}\")");
    let (done, said) = reload(&dir, &server.pid());
    assert!(
        done && said
            .contains("a frame of `main` stays on the old code at lines 15–16 of server.to: `tenfold` is new there"),
        "{said}"
    );
    assert_eq!(server.answer("c"), "v1 c 1 after 3 30");
    server.stop();
}

const CLOSURE: &str = r#"import std.console

let serve = () {
    var count = 0
    for {
        match console.readLine() {
            some(line) => {
                count += 1
                console.println("v1 {line} {count}")
            }
            none => break
        }
    }
}
serve()
"#;

#[test]
fn a_closure_s_frame_moves() {
    let dir = dir("live_closure_moves");
    let file = dir.join("closure.to");
    std::fs::write(&file, CLOSURE).unwrap();
    let mut server = Running::start(&dir, &["run", "closure.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1");
    edit(&file, "\"v1 {line} {count}\"", "\"v2 {line} {count * 100}\"");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("1 changed (closure in main)"), "{said}");
    assert_eq!(server.answer("b"), "v2 b 200");
    server.stop();
}

const COUNTING: &str = r#"import std.console

var total = 0
for i in 0..4 {
    total += i
    console.println("{i} total {total}")
}
"#;

/// Stopped under the debugger, a frame goes on in the new code, and the
/// stack shows it once.
#[test]
fn a_stopped_frame_goes_on_in_the_new_code() {
    let dir = dir("live_stopped_moves");
    let file = dir.join("count.to");
    std::fs::write(&file, COUNTING).unwrap();
    let mut debugger = Running::start(&dir, &["debug", "--live", "--json", "count.to"]);
    let said = |debugger: &mut Running, until: &str| -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let line = debugger.lines.recv_timeout(Duration::from_secs(60)).expect("the debugger's answer");
            let done = line.contains(until);
            lines.push(line);
            if done {
                return lines;
            }
        }
    };
    let send = |debugger: &mut Running, command: &str| {
        writeln!(debugger.input, "{command}").unwrap();
        debugger.input.flush().unwrap();
    };
    said(&mut debugger, "\"start\"");
    send(&mut debugger, "break count.to:6");
    said(&mut debugger, "breakpoint");
    send(&mut debugger, "continue");
    said(&mut debugger, "\"stopped\"");
    send(&mut debugger, "continue");
    let first = said(&mut debugger, "\"stopped\"");
    assert!(first.iter().any(|l| l == "0 total 0"), "{first:?}");
    edit(&file, "\"{i} total {total}\"", "\"{i} sum {total * 100}\"");
    send(&mut debugger, "reload");
    let reloaded = said(&mut debugger, "reloaded");
    assert!(reloaded.last().unwrap().contains("\"changed\":[\"main\"]"), "{reloaded:?}");
    send(&mut debugger, "continue");
    let next = said(&mut debugger, "\"stopped\"");
    assert!(next.iter().any(|l| l == "1 sum 100"), "{next:?}");
    send(&mut debugger, "stack");
    let stack = said(&mut debugger, "\"stack\"");
    assert_eq!(stack.last().unwrap(), "{\"stack\":[\"count.to:6 in main\"]}");
    send(&mut debugger, "delete 1");
    said(&mut debugger, "deleted");
    send(&mut debugger, "continue");
    let rest = said(&mut debugger, "exited");
    assert!(rest.iter().any(|l| l == "2 sum 300") && rest.iter().any(|l| l == "3 sum 600"), "{rest:?}");
    debugger.stop();
}

const SERVE: &str = r#"import std.console

type Config = { prefix: String }

fn serve(config: Config) {
    var seen = 0
    for {
        match console.readLine() {
            some(line) => {
                seen += 1
                console.println("{config.prefix} {line} {seen}")
            }
            none => break
        }
    }
}

serve(Config { prefix: "v1" })
"#;

/// A frame of a function whose parameters' type changes shape moves to the
/// function it is now, with its parameters converted, even several reloads
/// later.
#[test]
fn a_frame_moves_with_its_parameters_converted() {
    let dir = dir("live_serve");
    let file = dir.join("serve.to");
    std::fs::write(&file, SERVE).unwrap();
    let mut server = Running::start(&dir, &["run", "serve.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1");
    edit(&file, "type Config = { prefix: String }", "type Config = { prefix: String, suffix: String }");
    edit(&file, "{seen}\")", "{seen}[{config.suffix}]\")");
    edit(&file, "Config { prefix: \"v1\" }", "Config { prefix: \"v1\", suffix: \"s\" }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("2 changed (main, serve)") && said.contains("`Config.suffix` is new"), "{said}");
    edit(&file, "[{config.suffix}]", "<{config.suffix}>");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && !said.contains("finish on the old code"), "{said}");
    assert_eq!(server.answer("b"), "v1 b 2<>");
    server.stop();
}

/// A number converts as `T(x)` does: always when the new type holds every
/// value of the old, and otherwise when the value fits.
#[test]
fn numbers_convert() {
    let dir = dir("live_numbers");
    let file = dir.join("holder.to");
    std::fs::write(&file, HOLDER).unwrap();
    let mut server = Running::start(&dir, &["run", "holder.to", "--live"]);
    assert_eq!(server.answer("a"), "a 1");
    edit(&file, "count: Int }", "count: Float }");
    edit(&file, "count: 0 }", "count: 0.0 }");
    edit(&file, "state.count + 1 }", "state.count + 0.5 }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State.count` is `Float` now: values alive convert"), "{said}");
    assert_eq!(server.answer("b"), "b 1.5");
    assert_eq!(server.answer("c"), "c 2.0");
    edit(&file, "count: Float }", "count: Int8 }");
    edit(&file, "count: 0.0 }", "count: 0 }");
    edit(&file, "state.count + 0.5 }", "state.count + 1 }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State.count` is `Int8` now: values alive convert when they fit"), "{said}");
    assert_eq!(server.answer("d"), "d 3");
    server.stop();
}

const CLOSURE_STATE: &str = r#"import std.console

type Config = { prefix: String }

let config = Config { prefix: "v1" }
let serve = () {
    var seen = 0
    for {
        match console.readLine() {
            some(line) => {
                seen += 1
                console.println("{config.prefix} {line} {seen}")
            }
            none => break
        }
    }
}
serve()
"#;

/// A closure's frame moves when its function's closures are numbered
/// otherwise, and when what it captures changes shape.
#[test]
fn a_closure_s_frame_moves_when_its_identity_changes() {
    let dir = dir("live_closure_identity");
    let file = dir.join("serve.to");
    std::fs::write(&file, CLOSURE_STATE).unwrap();
    let mut server = Running::start(&dir, &["run", "serve.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1");
    edit(&file, "let serve = () {", "let bye = () { console.println(\"bye\") }\nlet serve = () {");
    edit(&file, "{seen}\")", "{seen}!\")");
    edit(&file, "serve()\n", "serve()\nbye()\n");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && !said.contains("finish on the old code"), "{said}");
    assert_eq!(server.answer("b"), "v1 b 2!");
    edit(&file, "type Config = { prefix: String }", "type Config = { prefix: String, suffix: String }");
    edit(&file, "Config { prefix: \"v1\" }", "Config { prefix: \"v1\", suffix: \"s\" }");
    edit(&file, "{seen}!\")", "{seen}[{config.suffix}]\")");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`Config.suffix` is new"), "{said}");
    assert_eq!(server.answer("c"), "v1 c 3[]");
    server.stop();
}

const LAMBDA: &str = r#"import std.console

fn serve(handler: fn(String): String) {
    for {
        match console.readLine() {
            some(line) => console.println(handler(line))
            none => break
        }
    }
}

serve((line) { "v1 {line}" })
"#;

/// A closure made before a reload runs its new body even when another
/// closure comes before it in its function now.
#[test]
fn a_closure_numbered_otherwise_runs_its_new_body() {
    let dir = dir("live_lambda");
    let file = dir.join("lambda.to");
    std::fs::write(&file, LAMBDA).unwrap();
    let mut server = Running::start(&dir, &["run", "lambda.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a");
    edit(
        &file,
        "serve((line) { \"v1 {line}\" })",
        "let bye = () { console.println(\"bye\") }\nserve((line) { \"v2 {line}\" })\nbye()",
    );
    let (done, said) = reload(&dir, &server.pid());
    assert!(done, "{said}");
    assert_eq!(server.answer("b"), "v2 b");
    server.stop();
}

const RENAMED: &str = r#"import std.console

enum Mode {
    Idle
    Busy(Int)
}

type State = { total: Int, mode: Mode }

fn show(state: State): String {
    let mode = match state.mode {
        Mode.Idle => "idle"
        Mode.Busy(n) => "busy {n}"
    }
    "{state.total} {mode}"
}

var state = State { total: 0, mode: Mode.Busy(5) }
for {
    match console.readLine() {
        some(line) => {
            state = State { total: state.total + 1, mode: state.mode }
            console.println("{line} {show(state)}")
        }
        none => break
    }
}
"#;

/// A field, a variant or a type renamed keeps the values of the old name.
#[test]
fn renames_keep_values() {
    let dir = dir("live_renamed");
    let file = dir.join("renamed.to");
    std::fs::write(&file, RENAMED).unwrap();
    let mut server = Running::start(&dir, &["run", "renamed.to", "--live"]);
    assert_eq!(server.answer("a"), "a 1 busy 5");
    let rename = |old: &str, new: &str| {
        let text = std::fs::read_to_string(&file).unwrap();
        let mut out = String::new();
        let mut word = String::new();
        for c in text.chars().chain(['\n']) {
            if c.is_alphanumeric() {
                word.push(c);
                continue;
            }
            out.push_str(if word == old { new } else { &word });
            word.clear();
            out.push(c);
        }
        out.pop();
        std::fs::write(&file, out).unwrap();
    };
    rename("total", "count");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State.total` is `count` now"), "{said}");
    assert_eq!(server.answer("b"), "b 2 busy 5");
    rename("Busy", "Working");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`Mode.Busy` is `Working` now"), "{said}");
    assert_eq!(server.answer("c"), "c 3 busy 5");
    rename("State", "AppState");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("`State` is `AppState` now") && said.contains("2 changed (main, show)"), "{said}");
    assert_eq!(server.answer("d"), "d 4 busy 5");
    server.stop();
}

const LENT: &str = r#"import std.console

type State = { count: Int }

fn serve(state: mut State) {
    for {
        match console.readLine() {
            some(line) => {
                if line == "end" {
                    return
                }
                state = State { count: state.count + 1 }
                console.println("{line} {state.count}")
            }
            none => return
        }
    }
}

fn collect(): State {
    var state = State { count: 100 }
    serve(mut state)
    state
}

let result = collect()
console.println("done {result.count}")
"#;

/// A frame with a value lent to it, or whose result, of a type of another
/// shape, moves: the value goes back to its caller converted, and so does
/// the result, as nothing is lost.
#[test]
fn values_go_back_converted() {
    let dir = dir("live_lent");
    let file = dir.join("lent.to");
    std::fs::write(&file, LENT).unwrap();
    let mut server = Running::start(&dir, &["run", "lent.to", "--live"]);
    assert_eq!(server.answer("a"), "a 101");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, label: String }");
    edit(&file, "State { count: state.count + 1 }", "State { count: state.count + 1, label: \"{state.label}+\" }");
    edit(&file, "\"{line} {state.count}\"", "\"{line} {state.count} [{state.label}]\"");
    edit(&file, "State { count: 100 }", "State { count: 100, label: \"\" }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(
        done && said.contains("3 changed (collect, main, serve)") && !said.contains("finish on the old code"),
        "{said}"
    );
    assert_eq!(server.answer("b"), "b 102 [+]");
    assert_eq!(server.answer("end"), "done 102");
    server.stop();
}

/// A task waiting for a line holds no thread, and its wait ends at once
/// when it is cancelled, as a reload ends it to start its statement again.
#[test]
fn a_wait_for_input_ends_when_cancelled() {
    let dir = dir("live_input");
    let file = dir.join("input.to");
    let program = "import std.console\nimport std.time\n\nlet job = spawn console.readLine()\ntime.sleep(time.milliseconds(50))\njob.cancel()\nconsole.println(\"cancelled\")\n";
    std::fs::write(&file, program).unwrap();
    let server = Running::start(&dir, &["run", "input.to"]);
    let line = server.lines.recv_timeout(Duration::from_secs(60)).expect("the program goes on");
    assert_eq!(line, "cancelled");
    server.stop();
}

/// A frame still on the code two reloads ago moves to the newest.
#[test]
fn a_frame_moves_past_several_reloads() {
    let dir = dir("live_several");
    let file = dir.join("server.to");
    std::fs::write(&file, SERVER).unwrap();
    let mut server = Running::start(&dir, &["run", "server.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1 after 1");
    for (old, new) in [("after {seen}", "then {seen}"), ("then {seen}", "now {seen}")] {
        edit(&file, old, new);
        let (done, said) = reload(&dir, &server.pid());
        assert!(done && said.contains("1 changed (main)"), "{said}");
    }
    assert_eq!(server.answer("b"), "v1 b 1 now 2");
    server.stop();
}

const LOCKED: &str = r#"import std.console
import std.sync

type State = { hits: Int }

let state: sync.Mutex[State] = sync.Mutex(State { hits: 0 })
for {
    match console.readLine() {
        some(line) => {
            let said = state.update((s) {
                s.hits += 1
                "v1 {line} {s.hits}"
            })
            console.println(said)
        }
        none => break
    }
}
"#;

/// A place inside a `lock` keeps its frame on the old code, and the report
/// says so, rather than naming the lock's own value.
#[test]
fn a_frame_inside_a_lock_stays() {
    let dir = dir("live_locked");
    let file = dir.join("locked.to");
    std::fs::write(&file, LOCKED).unwrap();
    let mut server = Running::start(&dir, &["run", "locked.to", "--live"]);
    assert_eq!(server.answer("a"), "v1 a 1");
    edit(&file, "type State = { hits: Int }", "type State = { hits: Int, last: String }");
    edit(&file, "State { hits: 0 }", "State { hits: 0, last: \"\" }");
    edit(&file, "\"v1 {line} {s.hits}\"", "\"v2 {line} {s.hits} [{s.last}]\"");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("it is inside a `lock` or borrows a value there"), "{said}");
    assert!(!said.contains("is not a type of the new code"), "{said}");
    assert_eq!(server.answer("b"), "v2 b 2 []");
    server.stop();
}
