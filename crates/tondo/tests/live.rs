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

    // A type with other fields would not fit the values of the old one.
    edit(&file, "\"v3\" +", "\"v3 {line}\"");
    edit(&file, "type Reply = { text: String, size: Int }", "type Reply = { text: String, size: Int, more: Bool }");
    edit(&file, "size: line.length() }", "size: line.length(), more: true }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(!done && said.contains("`Reply` changed what it is made of"), "{said}");

    // A new type and function come in.
    edit(&file, "type Reply = { text: String, size: Int, more: Bool }", "type Reply = { text: String, size: Int }");
    edit(&file, "size: line.length(), more: true }", "size: twice(line.length()) }");
    edit(&file, "var seen = 0", "fn twice(n: Int): Int {\n    n * 2\n}\n\nvar seen = 0");
    let (done, said) = reload(&dir, &server.pid());
    assert!(done && said.contains("added (twice)"), "{said}");
    assert_eq!(server.answer("dd"), "v3 dd 4 after 4");
    server.stop();
}

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
    // Stopped in `label`: its frame finishes with the old code, and the
    // next call runs the new one, where the breakpoint is too.
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

#[test]
fn watching_reloads_what_is_saved() {
    let dir = dir("live_watch");
    let file = dir.join("server.to");
    std::fs::write(&file, SERVER).unwrap();
    let mut server = Running::start(&dir, &["run", "server.to", "--watch"]);
    assert_eq!(server.answer("a"), "v1 a 1 after 1");
    edit(&file, "\"v1 {line}\"", "\"v2 {line}\"");
    let reloaded = server.errors.recv_timeout(Duration::from_secs(60)).expect("a reload");
    assert!(reloaded.contains("reloaded: 1 changed (reply)"), "{reloaded}");
    assert_eq!(server.answer("b"), "v2 b 1 after 2");
    // A change it cannot make in place runs the program again.
    edit(&file, "type Reply = { text: String, size: Int }", "type Reply = { text: String, size: Int, more: Bool }");
    edit(&file, "size: line.length() }", "size: line.length(), more: true }");
    let restarted = server.errors.recv_timeout(Duration::from_secs(60)).expect("a restart");
    assert!(restarted.contains("running it again"), "{restarted}");
    assert_eq!(server.answer("c"), "v2 c 1 after 1");
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

#[test]
fn a_type_a_running_frame_holds_does_not_change() {
    let dir = dir("live_holder");
    let file = dir.join("holder.to");
    std::fs::write(&file, HOLDER).unwrap();
    let mut server = Running::start(&dir, &["run", "holder.to", "--live"]);
    assert_eq!(server.answer("a"), "a 1");
    edit(&file, "type State = { count: Int }", "type State = { count: Int, more: Bool }");
    edit(&file, "State { count: 0 }", "State { count: 0, more: true }");
    edit(&file, "State { count: state.count + 1 }", "State { count: state.count + 1, more: true }");
    let (done, said) = reload(&dir, &server.pid());
    assert!(!done && said.contains("may be alive in `main` (holder.to:"), "{said}");
    assert_eq!(server.answer("b"), "b 2");
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
