//! `tondo debug` (toolchain 7.6): the debugger of debug builds, over the
//! diagnostics channel. A person types commands at its prompt; a script or
//! a model gives them on its input, one a line, and with `--json` reads
//! each answer and each event as a line of JSON.

use crate::observe::{self, Program};
use serde_json::Value;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// What `tondo debug` debugs: an executable it starts, with its arguments,
/// or a running debug build, by its process ID or name.
pub enum Debugged {
    Started(PathBuf, Vec<std::ffi::OsString>),
    Running(String),
    /// A running program on another machine, or in a container, by the
    /// command that reaches it (`--via`) and its name there.
    Remote(String, String),
    /// A test of a project: its tests' executable, the test's node, which
    /// runs alone, the variables that give it the project's stores, and the
    /// seed of the order of its tasks (`--schedule`).
    Test(PathBuf, u32, Vec<(&'static str, PathBuf)>, Option<u64>),
    /// A run `tondo run --record` recorded in this directory, replayed.
    Replayed(PathBuf),
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub const HELP: &str = "\
break <file>:<line> [if <condition>] [hits <n>|>= <n>|% <n>] [log <message with {expressions}>]
                                                stop where that line starts, or write a message there (b); in a
                                                live build the condition is compiled, and may call functions
break fn <name> ...                             the same where each function of that name starts: Stack.push, push
break <place> task <id> ...                     the same for one task only
break <place> ... capture                       capture the stack and the bindings there, and go on: a running
                                                server's state, without stopping it (hits 1: the first time only)
break on spawn|send|receive|wait <ms>           stop a task as it starts, sends or receives on a channel, or once
                                                it waited longer than that for a lock, a channel or a task
freeze <task>, thaw <task>                      the task stays held while the others go on, or goes on again
retainers <path>                                what keeps a value of the heap alive: the stacks that point to it,
                                                and the shortest chains of objects from a stack to it
memory                                          the objects by where they were made, and how each place grew since
                                                the last `memory`
watch <expression> [if <condition>] [hits <n>]  stop on the line after the one that changes its value, in this
                                                frame or a call it makes, until the frame returns; a std.sync
                                                cell's value, as state.value, in whichever task changes it
delete <id>                                     remove a breakpoint or a watch
continue                                        go on until a breakpoint (c)
step                                            to the next line, into calls (s)
step into <name>                                into the call of that name the line makes: plus, items.map
next                                            to the next line of this function (n)
until <line>|<file>:<line>                      on until that line, or the next stop
snapshot                                        why the task stopped, each frame with its bindings, where it was
                                                spawned, the other tasks and the last lines of its history
finish                                          until this function returns (f)
steps program|all                               steps go over the standard library's code (the default), or into it
record [<lines>|off]                            keep each line the tasks run, with the bindings as it started (10000)
back [<n> | until <condition>]                  back in the stopped task's history, n lines or to the nearest where
                                                the condition held: locals, print, stack show it
forward [<n> | until <condition>]               forward in it, as steps do there; past the last, the present
reverse                                         back to the last line at a breakpoint, or the first kept
why <expression>                                back to the line that gave the expression its value
                                                (in a replay, `tondo debug --replay`, back, reverse and why go
                                                back to the whole program as it was there)
pause                                           stop every task, as Ctrl-C does
catch panic [<code>...] error [<type>...]|none  stop where a task panics and no recover stops it (the default), where
                                                an error starts (a fail, or a ? on what std failed with), both, or none;
                                                only those codes or names (P0001, bounds) and types (ConfigError)
set <target> [=] <expression>                   change a binding of the frame where the task stopped, or a part of it:
                                                set count count + 1, set user.age = 37, set items[i] 0, set role Role.Admin
locals                                          the bindings that hold a value here, and those of the
                                                functions this one is written in, as a test's suites (l)
context                                         the task's contexts, and the `with` that set each field
print <expression>                              a binding or a part of it: user.name, items[i + 1], map[\"key\"], or what
                                                an expression computes from them: total / count, name.length() > 3 (p)
children <path> [<start> <count>]               the parts of a binding or of a part, all or those from start on
stack [<task>]                                  where the stopped task is, or another (bt)
frame <n>                                       look at frame n of the stack: locals, print
task <id>                                       look at another stopped task, from its innermost frame
tasks                                           where each stopped task is
reload                                          a live build: its code as its files are now (r)
goto <line>                                     a live build: the stopped frame goes on at that line of its function
call <expression>                               a live build: what an expression gives that calls the program's
                                                code, run by the stopped task: total(items), user.describe()
restart                                         a live build: the stopped function again, from its start
quit                                            end the program and the debugger (q)";

/// What a test writes, with the runner's frames as what they say: how the
/// test ended, and its logs.
fn tested(child: &mut std::process::Child, node: u32) {
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
}

/// The test a recording of `tondo test --record` is of.
pub(crate) fn replayed_test(dir: &Path) -> Option<u32> {
    let read = std::fs::read(dir.join("run.json")).ok()?;
    let run: Value = serde_json::from_slice(&read).ok()?;
    run["test"].as_u64().map(|node| node as u32)
}

/// The program of a recording, `tondo run --record`'s directory, to replay
/// under the debugger: with the arguments and the environment it had, in
/// the order of tasks of its seed. The debugger finds it where its own
/// temporary directory is.
pub fn replayed(dir: &Path) -> Result<(PathBuf, std::process::Command), String> {
    let read = std::fs::read(dir.join("run.json"))
        .map_err(|e| format!("{} is not a recording of `tondo run --record`: {e}", dir.display()))?;
    let run: Value = serde_json::from_slice(&read).map_err(|e| format!("{}: {e}", dir.display()))?;
    let executable = dir.join("program");
    let arguments: Vec<&str> = run["arguments"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let environment = run["environment"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k, v.as_str()?)));
    let mut command = std::process::Command::new(&executable);
    command
        .args(&arguments)
        .env_clear()
        .envs(environment)
        .env_remove("TONDO_RT_RECORD")
        .env("TMPDIR", std::env::temp_dir())
        .env("TONDO_RT_REPLAY", dir.join("calls"))
        .env("TONDO_RT_SCHEDULE", run["seed"].as_str().unwrap_or("0"))
        .env("TONDO_DEBUG", "1");
    if let Some(directory) = run["directory"].as_str().map(PathBuf::from).filter(|d| d.is_dir()) {
        command.current_dir(directory);
    }
    Ok((executable, command))
}

/// The debugger, until its input ends or `quit`; gives the exit status of
/// a program it started.
pub fn debug(debugged: Debugged, json: bool) -> Result<i32, String> {
    let replay = match &debugged {
        Debugged::Replayed(dir) => Some(dir.clone()),
        _ => None,
    };
    let (program, mut child) = match debugged {
        Debugged::Test(executable, node, environment, schedule) => {
            use std::os::unix::process::CommandExt;
            let plan = std::env::temp_dir().join(format!("tondo-debug-plan-{}", std::process::id()));
            std::fs::write(&plan, node.to_string()).map_err(|e| format!("cannot write {}: {e}", plan.display()))?;
            let mut child = std::process::Command::new(&executable)
                .env("TONDO_DEBUG", "1")
                .env("TONDO_TEST_PLAN", &plan)
                .envs(environment)
                .envs(schedule.map(|seed| ("TONDO_RT_SCHEDULE", format!("{seed:x}"))))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            tested(&mut child, node);
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
        Debugged::Replayed(dir) => {
            use std::os::unix::process::CommandExt;
            let (executable, mut command) = replayed(&dir)?;
            // A test's run, whose frames say how it ended.
            let test = replayed_test(&dir);
            if test.is_some() {
                command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            }
            let mut child = command
                .stdin(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            if let Some(node) = test {
                tested(&mut child, node);
            }
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Running(name) => (observe::connect(&name)?, None),
        Debugged::Remote(via, name) => (observe::via(&via, &name)?, None),
    };
    let pid = program.pid;
    let (writer, said) = attach(program)?;
    let mut session = Session { said, writer, json, events: Vec::new(), ended: false, task: 0 };
    // A replay goes back by running its recording again (`back`, `reverse`,
    // `why`): it keeps the commands that set how the program stops.
    let mut replaying = replay.map(|dir| Replaying {
        test: replayed_test(&dir),
        dir,
        settings: Vec::new(),
        current: child.take().map(Running::Started),
        checkpoints: Vec::new(),
        piped: false,
    });
    let attached = session.answer()?;
    if let Some(error) = attached["error"].as_str() {
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(running) = replaying.and_then(|r| r.current) {
            running.end();
        }
        return Err(match error.contains("--debug") {
            true => format!("{error}: debug it as `tondo debug <file.to>`, or run it with `tondo run --debug`"),
            false => error.to_string(),
        });
    }
    session.show(&attached);
    if child.is_some() || replaying.is_some() {
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
            // What an expression gives, calling the program's code: a live
            // build's patch.
            "call" => {
                let mut ask = |request: &str| {
                    session.send(request)?;
                    session.answer()
                };
                let place =
                    ask("stack").ok().and_then(|s| s["stack"][0].as_str().map(String::from)).unwrap_or_default();
                let locals = ask("locals").unwrap_or_default();
                let bindings: Vec<(String, String)> = locals["locals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| {
                        (l["name"].as_str().unwrap_or_default().into(), l["type"].as_str().unwrap_or_default().into())
                    })
                    .collect();
                match crate::live::call(pid, &mut ask, &place, rest, &bindings) {
                    Ok(value) => session.show(&serde_json::json!({ "name": rest, "value": value })),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // The stopped frame goes on at another line of its function.
            "goto" => {
                let Ok(line) = rest.parse::<u64>() else {
                    session.show(
                        &serde_json::json!({ "error": "goto takes a line of the stopped function, as `goto 12`" }),
                    );
                    continue;
                };
                let mut ask = |request: &str| {
                    session.send(request)?;
                    session.answer()
                };
                let place =
                    ask("stack").ok().and_then(|s| s["stack"][0].as_str().map(String::from)).unwrap_or_default();
                let moved =
                    crate::live::goto(pid, &mut ask, &place, line).and_then(|hex| ask(&format!("goto {line} {hex}")));
                match moved {
                    // Where it goes on, which the debugger tells next.
                    Ok(answer) if answer["running"] == true => {
                        session.show(&answer);
                        session.wait_event();
                    }
                    Ok(answer) => session.show(&answer),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // `why`, in the history: the line that gave the value, and
            // where what that line reads got its own.
            "why" if replaying.is_none() => {
                let mut ask_why = || -> Result<Value, String> {
                    ask(&mut session, &format!("why {rest}"))?;
                    let mut stop = session.next_event()?;
                    let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
                    stop["stopped"]["causes"] = causes_here(&mut session, &at);
                    Ok(stop)
                };
                match ask_why() {
                    Ok(stop) => session.show(&stop),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // A replay goes back by running its recording again.
            "back" | "reverse" | "why" if replaying.is_some() => {
                let replaying = replaying.as_mut().expect("a replay");
                match replaying.go_back(&mut session, word, rest) {
                    Ok(stop) => session.show(&stop),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            "break" | "delete" | "continue" | "step" | "next" | "finish" | "pause" | "locals" | "print" | "stack"
            | "why" | "tick" | "tasks" | "restart" | "select" | "children" | "task" | "catch" | "set" | "context"
            | "watch" | "steps" | "record" | "back" | "forward" | "reverse" | "until" | "snapshot" | "freeze"
            | "thaw" | "retainers" | "memory" => {
                // `task <id>` looks at that task's innermost frame.
                let request = match word {
                    "task" => format!("select {rest} 0"),
                    _ => format!("{word} {rest}"),
                };
                session.send(request.trim_end())?;
                let mut answer = session.answer()?;
                // How the program stops, which a replay run again takes too.
                if let Some(replaying) = replaying.as_mut()
                    && matches!(word, "break" | "delete" | "catch" | "steps")
                    && answer.get("error").is_none()
                {
                    replaying.settings.push(request.trim_end().to_string());
                }
                if word == "break" {
                    let mut ask = |request: &str| {
                        session.send(request)?;
                        session.answer()
                    };
                    crate::live::compile_condition(pid, &mut ask, &mut answer);
                }
                session.show(&answer);
                if answer["running"] == true {
                    session.wait_event();
                    // A replay keeps a checkpoint where it stops.
                    if let Some(replaying) = replaying.as_mut().filter(|_| !session.ended) {
                        replaying.keep(&mut |command| ask(&mut session, command));
                    }
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
    // A replay's checkpoints end with it; its process too, at `quit`, or
    // it goes on to its end.
    if let Some(replaying) = replaying {
        for (_, pid) in replaying.checkpoints {
            Running::Forked(pid).end();
        }
        match replaying.current {
            Some(Running::Started(started)) => child = Some(started),
            Some(Running::Forked(pid)) => {
                if quit {
                    Running::Forked(pid).end();
                }
                while unsafe { libc::kill(pid as i32, 0) } == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
                return Ok(0);
            }
            None => {}
        }
    }
    let Some(mut child) = child else { return Ok(0) };
    if quit {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(1))
}

/// Talks to a program the debugger attaches to: what it says comes on the
/// channel, which a thread fills.
fn attach(program: Program) -> Result<(std::os::unix::net::UnixStream, mpsc::Receiver<Value>), String> {
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
    Ok((writer, said))
}

/// A replay under the debugger: its recording, the commands that set how
/// the program stops, the process the debugger talks to, and checkpoints:
/// copies of a replay where it stopped, by the probe they are at.
pub(crate) struct Replaying {
    dir: PathBuf,
    settings: Vec<String>,
    current: Option<Running>,
    checkpoints: Vec<(u64, u32)>,
    /// What a replay started from the start writes goes to a pipe the
    /// caller reads, as the editor's adapter does, not to this process's.
    piped: bool,
    /// The test a recording of `tondo test --record` is of, whose frames
    /// say how it ended.
    pub(crate) test: Option<u32>,
}

/// A process of a replay: one the debugger started, or one a checkpoint
/// forked, which only ends when it is ended.
pub(crate) enum Running {
    Started(std::process::Child),
    Forked(u32),
}

impl Running {
    pub(crate) fn pid(&self) -> u32 {
        match self {
            Running::Started(child) => child.id(),
            Running::Forked(pid) => *pid,
        }
    }

    pub(crate) fn end(self) {
        match self {
            Running::Started(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Ended at once, it leaves its sockets behind.
            Running::Forked(pid) => {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                let _ = std::fs::remove_file(observe::directory().join(pid.to_string()));
                let _ = std::fs::remove_file(observe::directory().join(format!("{pid}.checkpoint")));
            }
        }
    }
}

/// The checkpoints a replay keeps; past them, the oldest go.
const CHECKPOINTS: usize = 32;

impl Replaying {
    /// The recording run again, attached and stopped: from its start, or
    /// from a checkpoint, with the debugger's settings; what it writes
    /// shows from probe `shown` on.
    fn start(&self, from: Option<(u64, u32)>, shown: u64, json: bool) -> Result<(Running, Session), String> {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        let (executable, mut command) = replayed(&self.dir)?;
        // One that only looks writes nothing.
        let output = || match (shown, self.piped || self.test.is_some()) {
            (u64::MAX, _) => std::process::Stdio::null(),
            (_, true) => std::process::Stdio::piped(),
            (_, false) => std::process::Stdio::inherit(),
        };
        let mut running = match from {
            None => Running::Started(
                command
                    .env("TONDO_RT_SHOWN_FROM", shown.to_string())
                    .stdin(std::process::Stdio::null())
                    .stdout(output())
                    .stderr(output())
                    .process_group(0)
                    .spawn()
                    .map_err(|e| format!("cannot start {}: {e}", executable.display()))?,
            ),
            Some((_, checkpoint)) => {
                let path = observe::directory().join(format!("{checkpoint}.checkpoint"));
                let mut stream = std::os::unix::net::UnixStream::connect(&path)
                    .map_err(|e| format!("the checkpoint {checkpoint} is gone: {e}"))?;
                writeln!(stream, "resume {shown}").map_err(|e| e.to_string())?;
                let mut line = String::new();
                std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
                Running::Forked(line.trim().parse().map_err(|_| format!("the checkpoint {checkpoint} did not fork"))?)
            }
        };
        if let (Running::Started(child), Some(node), false) = (&mut running, self.test, self.piped) {
            tested(child, node);
        }
        let pid = running.pid();
        let (writer, said) = match observe::started(&executable, pid).and_then(attach) {
            Ok(attached) => attached,
            Err(message) => {
                running.end();
                return Err(message);
            }
        };
        let mut session = Session { said, writer, json, events: Vec::new(), ended: false, task: 0 };
        session.answer()?;
        // A replay from the start waits there; a checkpoint's stopped where
        // it was taken.
        if from.is_none() {
            session.next_event()?;
        }
        for setting in &self.settings {
            session.send(setting)?;
            session.answer()?;
        }
        Ok((running, session))
    }

    /// A checkpoint where the program stopped, kept for going back.
    pub(crate) fn keep(&mut self, ask: &mut dyn FnMut(&str) -> Result<Value, String>) {
        let Ok(answer) = ask("checkpoint") else { return };
        let (Some(tick), Some(pid)) = (answer["checkpoint"]["tick"].as_u64(), answer["checkpoint"]["pid"].as_u64())
        else {
            return;
        };
        // One kept there already is the same.
        if self.checkpoints.iter().any(|&(t, _)| t == tick) {
            Running::Forked(pid as u32).end();
            return;
        }
        self.checkpoints.push((tick, pid as u32));
        self.checkpoints.sort_unstable();
        while self.checkpoints.len() > CHECKPOINTS {
            let (_, oldest) = self.checkpoints.remove(0);
            Running::Forked(oldest).end();
        }
    }

    /// Where a replay may start to reach probe `tick`: the checkpoints
    /// before it, the nearest first, then the start. A breakpoint that
    /// counts its hits counts them from the start.
    fn bases(&self, tick: u64, at: bool) -> Vec<Option<(u64, u32)>> {
        let counted = self.settings.iter().any(|s| s.contains(" hits "));
        let mut bases: Vec<Option<(u64, u32)>> = match counted {
            true => Vec::new(),
            false => self
                .checkpoints
                .iter()
                .rev()
                .filter(|&&(t, _)| t < tick || (at && t == tick))
                .map(|&c| Some(c))
                .collect(),
        };
        bases.push(None);
        bases
    }

    /// `back [<n>]`, `reverse` or `why <expression>` in a replay: a replay
    /// from the nearest checkpoint, or an earlier one, runs to where the
    /// program is, keeping where the stopped task's lines start, where
    /// breakpoints would stop, or the history; another runs to the probe
    /// that gives, and takes over.
    fn go_back(&mut self, session: &mut Session, word: &str, rest: &str) -> Result<Value, String> {
        let tick = ask(session, "tick")?["tick"].as_u64().unwrap_or(0);
        let (to, why, causes) = self.find(word, rest, tick, session.task)?;
        let (running, mut next, mut stop) = self.reach(to, why, session.json)?;
        if !causes.is_null() {
            stop["stopped"]["causes"] = causes;
        }
        if let Some(old) = self.current.replace(running) {
            old.end();
        }
        std::mem::swap(session, &mut next);
        self.keep(&mut |command| ask(session, command));
        Ok(stop)
    }

    /// The probe `back`, `reverse` or `why` goes to from probe `tick`,
    /// where `task` stopped, and why: what a replay from the nearest
    /// checkpoint before, or an earlier one, finds on its way there.
    pub(crate) fn find(&self, word: &str, rest: &str, tick: u64, task: u64) -> Result<(u64, String, Value), String> {
        let mut failed = String::new();
        for base in self.bases(tick, false) {
            let (running, mut scan) = self.start(base, u64::MAX, true)?;
            let scanned = self.scan(&mut scan, word, rest, tick, task);
            let _ = scan.send("quit");
            running.end();
            match scanned {
                Ok(target) => return Ok(target),
                Err(message) => failed = message,
            }
        }
        Err(failed)
    }

    /// A replay stopped at probe `to`, from the nearest checkpoint, and the
    /// stop, which says `why`.
    pub(crate) fn reach(&self, to: u64, why: String, json: bool) -> Result<(Running, Session, Value), String> {
        let base = self.bases(to, true).into_iter().next().flatten();
        let (running, mut next) = self.start(base, to, json)?;
        let stop = match to {
            _ if base.is_some_and(|(t, _)| t == to) => {
                let snapshot = ask(&mut next, "snapshot")?;
                let at = snapshot["snapshot"]["frames"][0]["at"].clone();
                serde_json::json!({ "stopped": { "at": at, "task": snapshot["snapshot"]["task"].clone(), "why": why } })
            }
            0 => {
                let snapshot = ask(&mut next, "snapshot")?;
                let at = snapshot["snapshot"]["frames"][0]["at"].clone();
                serde_json::json!({ "stopped": { "at": at, "task": snapshot["snapshot"]["task"].clone(), "why": why } })
            }
            _ => {
                ask(&mut next, &format!("seek {to}"))?;
                let mut stop = next.next_event()?;
                stop["stopped"]["why"] = why.into();
                stop
            }
        };
        Ok((running, next, stop))
    }

    /// A replay of the recording in `dir`, as the editor's adapter has one:
    /// what its replays write goes to pipes, and the adapter ends its first
    /// process.
    pub(crate) fn for_editor(dir: PathBuf) -> Replaying {
        let test = replayed_test(&dir);
        Replaying { dir, settings: Vec::new(), current: None, checkpoints: Vec::new(), piped: true, test }
    }

    /// A command that sets how the program stops, which replays take too.
    pub(crate) fn set(&mut self, command: &str) {
        self.settings.push(command.to_string());
    }

    /// The process the debugger talks to from now on, the old one ended.
    pub(crate) fn take_over(&mut self, running: Running) {
        if let Some(old) = self.current.replace(running) {
            old.end();
        }
    }

    /// Ends the replay's processes: its checkpoints, and the one it talks
    /// to unless `keep`.
    pub(crate) fn end(&mut self) {
        for (_, pid) in std::mem::take(&mut self.checkpoints) {
            Running::Forked(pid).end();
        }
        if let Some(current) = self.current.take() {
            current.end();
        }
    }

    /// What a replay that started before probe `tick` finds on its way
    /// there, for `word`: the probe to go back to, and why.
    fn scan(
        &self,
        scan: &mut Session,
        word: &str,
        rest: &str,
        tick: u64,
        task: u64,
    ) -> Result<(u64, String, Value), String> {
        if word == "why" {
            ask(scan, "record 1000000")?;
        }
        let scanning = match word {
            "back" => format!("scan {tick} {task}"),
            _ => format!("scan {tick}"),
        };
        ask(scan, &scanning)?;
        scan.next_event()?;
        let scanned = ask(scan, "scanned")?;
        let ticks = |key: &str| -> Vec<u64> {
            scanned[key].as_array().into_iter().flatten().filter_map(Value::as_u64).filter(|&t| t < tick).collect()
        };
        match word {
            "back" => {
                let count = rest.trim().parse::<usize>().unwrap_or(1).max(1);
                let lines = ticks("lines");
                let back = lines.len().checked_sub(count).map(|at| lines[at]);
                back.map(|to| (to, format!("{count} lines back"), Value::Null))
                    .ok_or(format!("task {task} has no line that many lines back"))
            }
            "reverse" => match ticks("hits").last() {
                Some(&to) => Ok((to, "reverse".into(), Value::Null)),
                None => Err("no breakpoint stops before here".into()),
            },
            _ => {
                ask(scan, &format!("why {rest}"))?;
                let stop = scan.next_event()?;
                let to = stop["stopped"]["tick"].as_u64().ok_or("the history has no line of that")?;
                let causes = causes_here(scan, stop["stopped"]["at"].as_str().unwrap_or_default());
                Ok((to, stop["stopped"]["because"].as_str().unwrap_or("why").to_string(), causes))
            }
        }
    }
}

/// Where each binding a line reads got its value, as the history says,
/// for the line `at` the debugger shows: the causes of what it gives.
fn causes_here(session: &mut Session, at: &str) -> Value {
    let Some(text) = line_text(at) else { return Value::Array(Vec::new()) };
    let Ok(locals) = ask(session, "locals") else { return Value::Array(Vec::new()) };
    let bound: Vec<&str> =
        locals["locals"].as_array().into_iter().flatten().filter_map(|l| l["name"].as_str()).collect();
    let mut read: Vec<String> = read_by(&text).into_iter().filter(|n| bound.contains(&n.as_str())).collect();
    read.dedup();
    if read.is_empty() {
        return Value::Array(Vec::new());
    }
    ask(session, &format!("causes {}", read.join(" "))).map_or(Value::Array(Vec::new()), |a| a["causes"].clone())
}

/// The text of the line of `file:line in function`.
fn line_text(at: &str) -> Option<String> {
    let (place, _) = at.split_once(" in ")?;
    let (file, line) = place.rsplit_once(':')?;
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().nth(line.parse::<usize>().ok()?.checked_sub(1)?).map(String::from)
}

/// The names a line of code reads, as its text has them: what follows an
/// assignment's `=`, and its target too in a compound one, as `+=`; all of
/// a line that assigns nothing. A name after a `.`, a field's or a
/// method's, or before a `(`, a function's, is not one.
fn read_by(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut from = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        let (before, after) = (i.checked_sub(1).map_or(b' ', |b| bytes[b]), bytes.get(i + 1).copied().unwrap_or(b' '));
        if byte == b'=' && !matches!(after, b'=' | b'>') && !matches!(before, b'=' | b'!' | b'<' | b'>') {
            from = if matches!(before, b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^') { 0 } else { i + 1 };
            break;
        }
    }
    let mut names = Vec::new();
    let rest = &text[from..];
    let mut at = 0;
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    while at < chars.len() {
        let (start, c) = chars[at];
        if !(c.is_alphabetic() || c == '_') {
            at += 1;
            continue;
        }
        let mut end = at;
        while end < chars.len() && (chars[end].1.is_alphanumeric() || chars[end].1 == '_') {
            end += 1;
        }
        let word = &rest[start..chars.get(end).map_or(rest.len(), |&(i, _)| i)];
        let after = rest[chars.get(end).map_or(rest.len(), |&(i, _)| i)..].trim_start();
        let dotted = rest[..start].ends_with('.');
        if !dotted && !after.starts_with('(') && !names.iter().any(|n| n == word) {
            names.push(word.to_string());
        }
        at = end;
    }
    names
}

/// The answer to a command, or the error it is.
fn ask(session: &mut Session, command: &str) -> Result<Value, String> {
    session.send(command)?;
    let answer = session.answer()?;
    match answer["error"].as_str() {
        Some(error) => Err(error.to_string()),
        None => Ok(answer),
    }
}

pub(crate) struct Session {
    pub(crate) said: mpsc::Receiver<Value>,
    pub(crate) writer: std::os::unix::net::UnixStream,
    json: bool,
    /// Events that came while an answer was awaited.
    events: Vec<Value>,
    ended: bool,
    /// The task of the last stop.
    task: u64,
}

impl Session {
    fn send(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.writer, "{command}").map_err(|_| "the program ended".to_string())
    }

    /// The next stop or end, which it does not show.
    fn next_event(&mut self) -> Result<Value, String> {
        if !self.events.is_empty() {
            return Ok(self.events.remove(0));
        }
        loop {
            let value = self.said.recv().map_err(|_| "the program ended".to_string())?;
            if value.get("stopped").is_some() || value.get("exited").is_some() {
                return Ok(value);
            }
        }
    }

    /// The answer to the last command; events that come first wait.
    fn answer(&mut self) -> Result<Value, String> {
        loop {
            let value = self.said.recv().map_err(|_| "the program ended".to_string())?;
            if value.get("stopped").is_some() || value.get("exited").is_some() {
                self.events.push(value);
                continue;
            }
            // A logpoint's message, a capture or a watch that ended, as it
            // comes.
            if value.get("logged").is_some() || value.get("unwatched").is_some() || value.get("captured").is_some() {
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
                Ok(value)
                    if value.get("logged").is_some()
                        || value.get("unwatched").is_some()
                        || value.get("captured").is_some() =>
                {
                    self.show(&value)
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
        if let Some(task) = value["stopped"]["task"].as_u64() {
            self.task = task;
        }
        let causes = || {
            for cause in value["stopped"]["causes"].as_array().into_iter().flatten() {
                let back = cause["back"].as_u64().unwrap_or(0);
                println!(
                    "  `{}` was {}, from {}, {back} line{} back",
                    cause["name"].as_str().unwrap_or_default(),
                    cause["value"].as_str().unwrap_or_default(),
                    cause["at"].as_str().unwrap_or_default(),
                    if back == 1 { "" } else { "s" }
                );
            }
        };
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
                "history" => match stop["back"].as_u64().unwrap_or(0) {
                    0 => println!("back in the present, at {at}"),
                    back => {
                        println!("{back} lines back in task {}'s history, at {at}", stop["task"]);
                        if let Some(because) = stop["because"].as_str() {
                            println!("{because}");
                        }
                        causes();
                        if let Some(source) = source_line(at) {
                            println!("{source}");
                        }
                    }
                },
                why => {
                    println!("task {} stopped at {at} ({why})", stop["task"]);
                    causes();
                    if let Some(returned) = stop.get("returned") {
                        let function = returned["function"].as_str().unwrap_or_default();
                        println!("{function} returned {}", returned["value"].as_str().unwrap_or_default());
                    }
                    if let Some(source) = source_line(at) {
                        println!("{source}");
                    }
                }
            }
        } else if value.get("exited").is_some() {
            println!("the program ended");
        } else if let Some(breakpoint) = value.get("breakpoint") {
            println!(
                "breakpoint {} at {}:{}{}",
                breakpoint["id"],
                breakpoint["file"].as_str().unwrap_or_default(),
                breakpoint["line"],
                if breakpoint["compiled"] == true { ", its condition compiled" } else { "" }
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
        } else if let Some(contexts) = value["contexts"].as_array() {
            for context in contexts {
                println!("{}", context["name"].as_str().unwrap_or_default());
                for field in context["fields"].as_array().into_iter().flatten() {
                    let set = match (field["set"].as_str(), field["inherited"].as_bool()) {
                        (Some(at), Some(true)) => format!("  (with at {at}, in the task that started this one)"),
                        (Some(at), _) => format!("  (with at {at})"),
                        (None, _) => String::new(),
                    };
                    let name = field["name"].as_str().unwrap_or_default();
                    println!("  {name} = {}{set}", field["value"].as_str().unwrap_or_default());
                }
            }
        } else if let Some(frames) = value["stack"].as_array() {
            for (index, frame) in frames.iter().enumerate() {
                println!("#{index} {}", frame.as_str().unwrap_or_default());
            }
            for place in value["spawned"].as_array().into_iter().flatten() {
                println!("   spawned at {}", place.as_str().unwrap_or_default());
            }
        } else if let Some(snapshot) = value.get("snapshot") {
            println!("task {} stopped: {}", snapshot["task"], snapshot["why"].as_str().unwrap_or_default());
            for (index, frame) in snapshot["frames"].as_array().into_iter().flatten().enumerate() {
                println!("#{index} {}", frame["at"].as_str().unwrap_or_default());
                for local in frame["locals"].as_array().into_iter().flatten() {
                    println!("    {}", binding(local));
                }
            }
            for place in snapshot["spawned"].as_array().into_iter().flatten() {
                println!("   spawned at {}", place.as_str().unwrap_or_default());
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
        } else if let Some(retainers) = value.get("retainers") {
            println!("{}", retainers["object"].as_str().unwrap_or_default());
            let stacks: Vec<String> =
                retainers["stacks"].as_array().into_iter().flatten().map(|t| t.to_string()).collect();
            if !stacks.is_empty() {
                println!("  the stacks of task {} point to it", stacks.join(", task "));
            }
            for chain in retainers["chains"].as_array().into_iter().flatten() {
                let links: Vec<&str> = chain.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                println!("  {}", links.join("\n    -> "));
            }
        } else if let Some(memory) = value.get("memory") {
            println!("{} objects, {} bytes", memory["objects"], memory["bytes"]);
            for place in memory["places"].as_array().into_iter().flatten() {
                let more = place["more"].as_i64().unwrap_or(0);
                let grew = if more == 0 {
                    String::new()
                } else {
                    format!("  ({more:+} objects, {:+} bytes)", place["more_bytes"].as_i64().unwrap_or(0))
                };
                println!(
                    "  {:>8} bytes in {:>5} objects  {}{grew}",
                    place["bytes"].as_u64().unwrap_or(0),
                    place["objects"].as_u64().unwrap_or(0),
                    place["at"].as_str().unwrap_or_default()
                );
            }
        } else if let Some(frozen) = value["frozen"].as_array() {
            let tasks: Vec<String> = frozen.iter().map(|t| t.to_string()).collect();
            match tasks.is_empty() {
                true => println!("no task is frozen"),
                false => println!("frozen: task {}", tasks.join(", task ")),
            }
        } else if let Some(lines) = value.get("recording") {
            match lines.as_u64() {
                Some(0) => println!("the history is off"),
                _ => println!("the history keeps the last {lines} lines"),
            }
        } else if let Some(captured) = value.get("captured") {
            println!("captured at {} (task {})", captured["at"].as_str().unwrap_or_default(), captured["task"]);
            for local in captured["locals"].as_array().into_iter().flatten() {
                println!("    {}", binding(local));
            }
        } else if let Some(watch) = value.get("watch") {
            println!("watch {} = {}", watch["id"], watch["value"].as_str().unwrap_or_default());
        } else if let Some(id) = value.get("unwatched") {
            println!("watch {id} ended: {}", value["why"].as_str().unwrap_or_default());
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
    let mut from = value["from"].as_str().map_or(String::new(), |from| format!("  (of {from})"));
    if value["changed"] == true {
        from.push_str("  (changed)");
    }
    match value["type"].as_str() {
        Some(ty) => format!("{name}: {ty} = {value_text}{from}"),
        None => format!("{name} = {value_text}{from}"),
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
