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
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub const HELP: &str = "\
break <file>:<line> [if <name> <op> <value>]   stop where that line starts (b)
delete <id>                                     remove a breakpoint
continue                                        go on until a breakpoint (c)
step                                            to the next line, into calls (s)
next                                            to the next line of this function (n)
finish                                          until this function returns (f)
pause                                           stop every task, as Ctrl-C does
locals                                          the bindings that hold a value (l)
print <name>                                    one binding (p)
stack                                           where the stopped task is (bt)
tasks                                           where each stopped task is
reload                                          a live build: its code as its files are now (r)
restart                                         a live build: the stopped function again, from its start
quit                                            end the program and the debugger (q)";

/// The debugger, until its input ends or `quit`; gives the exit status of
/// a program it started.
pub fn debug(debugged: Debugged, json: bool) -> Result<i32, String> {
    let (program, child) = match debugged {
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
                    (Ok(reloaded), true) => println!(
                        "{}",
                        serde_json::json!({ "reloaded": { "changed": reloaded.changed, "added": reloaded.added } })
                    ),
                    (Ok(reloaded), false) => println!("{}", reloaded.summary()),
                    (Err(refused), true) => println!("{}", serde_json::json!({ "error": refused.message() })),
                    (Err(refused), false) => println!("{}", refused.message()),
                }
            }
            "break" | "delete" | "continue" | "step" | "next" | "finish" | "pause" | "locals" | "print" | "stack"
            | "tasks" | "restart" => {
                session.send(format!("{word} {rest}").trim_end())?;
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
        } else if let Some(id) = value.get("deleted") {
            println!("breakpoint {id} removed");
        } else if value.get("debugging").is_some() || value.get("running").is_some() || value.get("pausing").is_some() {
        } else {
            println!("{value}");
        }
    }
}

fn binding(value: &Value) -> String {
    format!(
        "{}: {} = {}",
        value["name"].as_str().unwrap_or_default(),
        value["type"].as_str().unwrap_or_default(),
        value["value"].as_str().unwrap_or_default()
    )
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
