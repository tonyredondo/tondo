//! `tondo debug` (toolchain 7.6): the debugger of debug builds, over the
//! diagnostics channel. A person types commands at its prompt; a script or
//! a model gives them on its input, one a line, and with `--json` reads
//! each answer and each event as a line of JSON.

use crate::observe::{self, Program};
use serde_json::Value;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// What `tondo debug` debugs: an executable it starts, with its arguments,
/// or a running debug build, by its process ID or name.
pub enum Debugged {
    Started(PathBuf, Vec<std::ffi::OsString>),
    Running(String),
    /// A test of a project: its tests' executable, the test's node, which
    /// runs alone, and the variables that give it the project's stores.
    Test(PathBuf, u32, Vec<(&'static str, PathBuf)>),
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub const HELP: &str = "\
break <file>:<line> [if <path> <op> <value>] [hits <n>|>= <n>|% <n>] [log <message with {paths}>]
                                                stop where that line starts, or write a message there (b)
delete <id>                                     remove a breakpoint
continue                                        go on until a breakpoint (c)
step                                            to the next line, into calls (s)
next                                            to the next line of this function (n)
finish                                          until this function returns (f)
pause                                           stop every task, as Ctrl-C does
catch panic|none                                stop where a task panics and no recover stops it (the default), or not
set <name> <value>                              change a binding of the frame where the task stopped: a number, Bool, Char, String
locals                                          the bindings that hold a value (l)
print <path>                                    a binding or a part of it: user.name, items[2], map[\"key\"] (p)
children <path>                                 the parts of a binding or of a part
stack [<task>]                                  where the stopped task is, or another (bt)
frame <n>                                       look at frame n of the stack: locals, print
task <id>                                       look at another stopped task, from its innermost frame
tasks                                           where each stopped task is
reload                                          a live build: its code as its files are now (r)
restart                                         a live build: the stopped function again, from its start
quit                                            end the program and the debugger (q)";

/// The debugger, until its input ends or `quit`; gives the exit status of
/// a program it started.
pub fn debug(debugged: Debugged, json: bool) -> Result<i32, String> {
    let (program, child) = match debugged {
        Debugged::Test(executable, node, environment) => {
            use std::os::unix::process::CommandExt;
            let plan = std::env::temp_dir().join(format!("tondo-debug-plan-{}", std::process::id()));
            std::fs::write(&plan, node.to_string()).map_err(|e| format!("cannot write {}: {e}", plan.display()))?;
            let mut child = std::process::Command::new(&executable)
                .env("TONDO_DEBUG", "1")
                .env("TONDO_TEST_PLAN", &plan)
                .envs(environment)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            // What the test writes, with the runner's frames as what they
            // say: how the test ended, and its logs.
            for (stream, stdout) in [
                (child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), true),
                (child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), false),
            ] {
                let Some(stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut stream = std::io::BufReader::new(stream);
                    let mut line = String::new();
                    while matches!(stream.read_line(&mut line), Ok(n) if n > 0) {
                        let shown = match line.strip_prefix('\x1e') {
                            Some(frame) => crate::dap::test_frame(frame, &mut stream, Some(node)),
                            None => Some(std::mem::take(&mut line)),
                        };
                        match (shown, stdout) {
                            (Some(text), true) => print!("{text}"),
                            (Some(text), false) => eprint!("{text}"),
                            _ => {}
                        }
                        line.clear();
                    }
                });
            }
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Started(executable, arguments) => {
            use std::os::unix::process::CommandExt;
            // Its own process group, so that Ctrl-C pauses it instead of
            // ending it; and no input, which the debugger reads.
            let child = std::process::Command::new(&executable)
                .args(&arguments)
                .env("TONDO_DEBUG", "1")
                .stdin(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Running(name) => (observe::connect(&name)?, None),
    };
    let pid = program.pid;
    let Program { mut writer, reader, .. } = program;
    // The program may stand still at a breakpoint for as long as it takes.
    let _ = reader.get_ref().set_read_timeout(None);
    writeln!(writer, "debug").map_err(|e| e.to_string())?;
    // A thread reads what the program says: answers, and events whenever
    // it stops or ends.
    let (lines, said) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if lines.send(value).is_err() {
                break;
            }
        }
    });
    let mut session = Session { said, writer, json, events: Vec::new(), ended: false };
    let attached = session.answer()?;
    if let Some(error) = attached["error"].as_str() {
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Err(match error.contains("--debug") {
            true => format!("{error}: debug it as `tondo debug <file.to>`, or run it with `tondo run --debug`"),
            false => error.to_string(),
        });
    }
    session.show(&attached);
    if child.is_some() {
        session.wait_event();
    }
    unsafe { libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t) };
    let terminal = std::io::stdin().is_terminal();
    if terminal && !json {
        println!("type `help` for the commands");
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut quit = false;
    while !session.ended {
        if terminal && !json {
            print!("(tondo) ");
            let _ = std::io::stdout().flush();
        }
        let mut line = String::new();
        if input.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        INTERRUPTED.store(false, Ordering::SeqCst);
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let word = match word {
            "b" => "break",
            "c" => "continue",
            "s" => "step",
            "n" => "next",
            "f" => "finish",
            "l" => "locals",
            "p" => "print",
            "bt" | "backtrace" => "stack",
            "frame" => "select",
            "r" => "reload",
            "q" | "exit" => "quit",
            "" => continue,
            other => other,
        };
        match word {
            "help" | "h" => println!("{HELP}"),
            "quit" => {
                quit = true;
                break;
            }
            "reload" => {
                let reloaded = crate::live::reload(pid, &mut |request| {
                    session.send(request)?;
                    session.answer()
                });
                match (reloaded, json) {
                    (Ok(reloaded), true) => println!("{}", serde_json::json!({ "reloaded": reloaded.json() })),
                    (Ok(reloaded), false) => println!("{}", reloaded.summary()),
                    (Err(refused), true) => println!("{}", serde_json::json!({ "error": refused.message() })),
                    (Err(refused), false) => println!("{}", refused.message()),
                }
            }
            "break" | "delete" | "continue" | "step" | "next" | "finish" | "pause" | "locals" | "print" | "stack"
            | "tasks" | "restart" | "select" | "children" | "task" | "catch" | "set" => {
                // `task <id>` looks at that task's innermost frame.
                let request = match word {
                    "task" => format!("select {rest} 0"),
                    _ => format!("{word} {rest}"),
                };
                session.send(request.trim_end())?;
                let answer = session.answer()?;
                session.show(&answer);
                if answer["running"] == true {
                    session.wait_event();
                }
            }
            other => {
                let message = format!("unknown command `{other}`: `help` lists them");
                match json {
                    true => println!("{}", serde_json::json!({ "error": message })),
                    false => println!("{message}"),
                }
            }
        }
    }
    // The input ended: the program goes on without the debugger, which
    // closes its side of the connection for good. `quit` ends the program
    // it started.
    let _ = session.writer.shutdown(std::net::Shutdown::Both);
    drop(session);
    let Some(mut child) = child else { return Ok(0) };
    if quit {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(1))
}

struct Session {
    said: mpsc::Receiver<Value>,
    writer: std::os::unix::net::UnixStream,
    json: bool,
    /// Events that came while an answer was awaited.
    events: Vec<Value>,
    ended: bool,
}

impl Session {
    fn send(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.writer, "{command}").map_err(|_| "the program ended".to_string())
    }

    /// The answer to the last command; events that come first wait.
    fn answer(&mut self) -> Result<Value, String> {
        loop {
            let value = self.said.recv().map_err(|_| "the program ended".to_string())?;
            if value.get("stopped").is_some() || value.get("exited").is_some() {
                self.events.push(value);
                continue;
            }
            // A logpoint's message, as it comes.
            if value.get("logged").is_some() {
                self.show(&value);
                continue;
            }
            return Ok(value);
        }
    }

    /// Waits for the program to stop or end, pausing it on Ctrl-C.
    fn wait_event(&mut self) {
        let mut paused = false;
        loop {
            if let Some(event) = (!self.events.is_empty()).then(|| self.events.remove(0)) {
                self.show(&event);
                return;
            }
            if INTERRUPTED.swap(false, Ordering::SeqCst) && !paused {
                paused = self.send("pause").is_ok();
            }
            match self.said.recv_timeout(Duration::from_millis(100)) {
                Ok(value) if value.get("stopped").is_some() || value.get("exited").is_some() => {
                    self.events.push(value);
                }
                Ok(value) if value.get("logged").is_some() => self.show(&value),
                // The answer to `pause`.
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.events.push(serde_json::json!({ "exited": true }));
                }
            }
        }
    }

    /// Prints an answer or an event, as JSON or in words.
    fn show(&mut self, value: &Value) {
        if value.get("exited").is_some() {
            self.ended = true;
        }
        if self.json {
            println!("{value}");
            return;
        }
        if let Some(error) = value["error"].as_str() {
            println!("error: {error}");
        } else if let Some(stop) = value.get("stopped") {
            let at = stop["at"].as_str().unwrap_or_default();
            match stop["why"].as_str().unwrap_or_default() {
                "start" => println!("the program waits before main: set breakpoints, then `continue`"),
                why => {
                    println!("task {} stopped at {at} ({why})", stop["task"]);
                    if let Some(source) = source_line(at) {
                        println!("{source}");
                    }
                }
            }
        } else if value.get("exited").is_some() {
            println!("the program ended");
        } else if let Some(breakpoint) = value.get("breakpoint") {
            println!(
                "breakpoint {} at {}:{}",
                breakpoint["id"],
                breakpoint["file"].as_str().unwrap_or_default(),
                breakpoint["line"]
            );
        } else if let Some(locals) = value["locals"].as_array() {
            if locals.is_empty() {
                println!("no binding holds a value here");
            }
            for local in locals {
                println!("{}", binding(local));
            }
        } else if value.get("value").is_some() {
            println!("{}", binding(value));
        } else if let Some(frames) = value["stack"].as_array() {
            for (index, frame) in frames.iter().enumerate() {
                println!("#{index} {}", frame.as_str().unwrap_or_default());
            }
        } else if let Some(tasks) = value["tasks"].as_array() {
            for task in tasks {
                println!(
                    "task {} at {} ({})",
                    task["task"],
                    task["at"].as_str().unwrap_or_default(),
                    task["why"].as_str().unwrap_or_default()
                );
            }
        } else if let Some(logged) = value["logged"].as_str() {
            println!("{logged}");
        } else if let Some(id) = value.get("deleted") {
            println!("breakpoint {id} removed");
        } else if let Some(parts) = value["children"].as_array() {
            for part in parts {
                println!(
                    "{} = {}",
                    part["path"].as_str().unwrap_or_default(),
                    part["value"].as_str().unwrap_or_default()
                );
            }
        } else if let Some(selected) = value.get("selected") {
            println!("task {}, frame {}", selected["task"], selected["frame"]);
        } else if value.get("debugging").is_some() || value.get("running").is_some() || value.get("pausing").is_some() {
        } else {
            println!("{value}");
        }
    }
}

fn binding(value: &Value) -> String {
    let (name, value_text) = (value["name"].as_str().unwrap_or_default(), value["value"].as_str().unwrap_or_default());
    match value["type"].as_str() {
        Some(ty) => format!("{name}: {ty} = {value_text}"),
        None => format!("{name} = {value_text}"),
    }
}

/// The line of source a place names, `file:line in function`, as the
/// debugger prints it under a stop.
fn source_line(at: &str) -> Option<String> {
    let (place, _) = at.split_once(" in ")?;
    let (file, line) = place.rsplit_once(':')?;
    let line: usize = line.parse().ok()?;
    let text = std::fs::read_to_string(file).ok()?;
    let source = text.lines().nth(line.checked_sub(1)?)?;
    Some(format!("{line:>5} | {source}"))
}
