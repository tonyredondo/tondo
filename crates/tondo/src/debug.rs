//! `tondo debug` (toolchain 7.6): the debugger of debug builds, over the
//! diagnostics channel. A person types commands at its prompt; a script or
//! a model gives them on its input, one a line, and with `--json` reads
//! each answer and each event as a line of JSON.

use crate::observe::{self, Program};
use serde_json::Value;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
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
    Test(PathBuf, u32, Vec<(&'static str, PathBuf)>, Option<String>),
    /// A run `tondo run --record` recorded in this directory, replayed.
    Replayed(PathBuf),
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The processes a debugger started or forked for itself, each a pid or a
/// process group as its negative, which end with it when SIGTERM or SIGHUP
/// end it, as a script, an editor or a closed terminal does, as `quit` ends
/// them: in slots a signal's handler reads. Not a program it attached to,
/// which goes on, nor a copy a recording holds.
static OWNED: [AtomicI32; 64] = [const { AtomicI32::new(0) }; 64];

/// The debugger ends `process`, a pid or `-group`, when it is ended.
pub(crate) fn own(process: i32) {
    for slot in &OWNED {
        if slot.compare_exchange(0, process, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return;
        }
    }
}

/// `process` ended, and its pid may be another's.
pub(crate) fn disown(process: i32) {
    for slot in &OWNED {
        let _ = slot.compare_exchange(process, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

extern "C" fn ending(signal: libc::c_int) {
    for slot in &OWNED {
        let process = slot.load(Ordering::SeqCst);
        if process != 0 {
            unsafe { libc::kill(process, libc::SIGKILL) };
        }
    }
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// SIGTERM and SIGHUP end what the debugger owns, and then the debugger.
pub(crate) fn end_owned_on_signals() {
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(signal, ending as *const () as libc::sighandler_t) };
    }
}

extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub const HELP: &str = "\
break <file>:<line> [if <condition>] [hits <n>|>= <n>|% <n>] [log <message with {expressions}>]
                                                stop where that line starts, or write a message there (b); in a
                                                live build the condition is compiled, and may call functions
break fn <name> ...                             the same where each function of that name starts: Stack.push, push
break <place> task <id> ...                     the same for one task only
break <place> request <text> ...                the same in the tasks of a request, as `request /orders`
break <place> ... capture                       capture the stack and the bindings there, and go on: a running
                                                server's state, without stopping it (hits 1: the first time only)
break on spawn|send|receive|wait <ms>           stop a task as it starts, sends or receives on a channel, or once
                                                it waited longer than that for a lock, a channel or a task
freeze <task>, thaw <task>                      the task stays held while the others go on, or goes on again
retainers <path>                                what keeps a value of the heap alive: the stacks that point to it,
                                                and the shortest chains of objects from a stack to it
disassemble [<frame>]                           the instructions around where a frame is, by line
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
follow [off]                                    debug the children the program starts too: a debug build waits for it
record [<lines>|off]                            keep each line the tasks run, with the bindings as it started (10000)
back [<n> | until <condition>]                  back in the stopped task's history, n lines or to the nearest where
                                                the condition held: locals, print, stack show it
forward [<n> | until <condition>]               forward in it, as steps do there; past the last, the present
reverse                                         back to the last line at a breakpoint, or the first kept
why <expression>                                back to the line that gave the expression its value, and where
                                                what that line reads got its own
timeline <expression>                           each value it took in the history, with the line that gave it
ran <file>:<line> [<expression>]                each time a task started that line, with the expression's value
seek <probe>                                    a replay: to that probe, which timeline and ran give
first <condition>, last <condition>            a replay: to the line after which the condition held, the first
                                                or the last time in the whole run
diff <probe> [<probe>]                          a replay: what changed between two probes, or one and now: the
                                                bindings of each task's frames, the tasks, and the memory by place
origins [<expression>]                          a replay: where the value came from, line by line, and what each
                                                of those lines read, back to what the program read; without an
                                                expression, what the line where it stopped reads
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
checkpoints [<ms>]                              a replay: the probes of the copies it keeps; how often it keeps one
restart                                         a live build: the stopped function again, from its start
quit                                            end the program and the debugger (q)";

/// What a test writes, with the runner's frames as what they say: how the
/// test ended, and its logs.
fn tested(
    out: Option<impl std::io::Read + Send + 'static>,
    error: Option<impl std::io::Read + Send + 'static>,
    node: u32,
) {
    for (stream, stdout) in [
        (out.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), true),
        (error.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), false),
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

/// Sends a line to a checkpoint with descriptors along, which the replay it
/// forks writes its output to.
fn given(stream: &std::os::unix::net::UnixStream, line: &str, fds: &[i32]) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut data = line.as_bytes().to_vec();
    let mut part = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    let size = std::mem::size_of_val(fds) as u32;
    let mut control = vec![0u8; unsafe { libc::CMSG_SPACE(size) } as usize];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(header).cast::<i32>(), fds.len());
    }
    match unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) } {
        -1 => Err(std::io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// The sources of the recording a replay runs, which its lines are read
/// from as they were when it was recorded, wherever it runs.
static SOURCES: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// A replay reads the sources its recording kept.
pub(crate) fn read_sources_of(recording: &Path) {
    *SOURCES.lock().unwrap() = Some(recording.join("sources"));
}

/// The recording's copy of `file`, a path as the program's stops name it.
pub(crate) fn kept_source(file: &str) -> Option<PathBuf> {
    let sources = SOURCES.lock().unwrap();
    Some(sources.as_ref()?.join(file.trim_start_matches('/'))).filter(|kept| kept.is_file())
}

/// Where the editor finds `file`: in `root`, or the recording's copy when
/// it is not there, as on another machine.
pub(crate) fn source_for_editor(root: &Path, file: &str) -> PathBuf {
    let here = root.join(file);
    match here.is_file() {
        true => here,
        false => kept_source(file).unwrap_or(here),
    }
}

/// Copies into `<recording>/sources` the program's source files, as the
/// lines of its DWARF name them, so that a replay elsewhere shows them.
pub fn keep_sources(executable: &Path, recording: &Path) {
    use object::{Object, ObjectSection};
    let Ok(bytes) = std::fs::read(executable) else { return };
    let Ok(file) = object::File::parse(bytes.as_slice()) else { return };
    let section = |id: gimli::SectionId| -> Result<gimli::EndianSlice<'_, gimli::LittleEndian>, gimli::Error> {
        let data = file.section_by_name(id.name()).and_then(|s| s.data().ok()).unwrap_or(&[]);
        Ok(gimli::EndianSlice::new(data, gimli::LittleEndian))
    };
    let Ok(dwarf) = gimli::Dwarf::load(section) else { return };
    let mut units = dwarf.units();
    while let Ok(Some(header)) = units.next() {
        let Ok(unit) = dwarf.unit(header) else { continue };
        let Some(lines) = unit.line_program.as_ref() else { continue };
        let directory = unit.comp_dir.map(|d| PathBuf::from(d.to_string_lossy().into_owned())).unwrap_or_default();
        for entry in lines.header().file_names() {
            let Ok(name) = dwarf.attr_string(&unit, entry.path_name()) else { continue };
            let name = name.to_string_lossy().into_owned();
            let kept = recording.join("sources").join(name.trim_start_matches('/'));
            if let Some(parent) = kept.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(directory.join(&name), kept);
        }
    }
}

/// The sources of the children of a recorded run that recorded theirs,
/// and of their children, once the run ends.
pub fn keep_children(recording: &Path) {
    for child in std::fs::read_dir(recording.join("children")).into_iter().flatten().flatten() {
        let dir = child.path();
        keep_sources(&dir.join("program"), &dir);
        keep_children(&dir);
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
        .env_remove("TONDO_DUMP")
        .envs(std::env::var_os("TONDO_DUMP").map(|dir| ("TONDO_DUMP", dir)))
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
        Debugged::Replayed(dir) => {
            read_sources_of(dir);
            Some(dir.clone())
        }
        _ => None,
    };
    // A recording of the end of a run starts at a copy of the program.
    let held = replay.as_deref().map(held_copy).transpose()?.flatten();
    end_owned_on_signals();
    let mut copied = None;
    let (program, mut child) = match debugged {
        Debugged::Test(executable, node, environment, schedule) => {
            use std::os::unix::process::CommandExt;
            let plan = std::env::temp_dir().join(format!("tondo-debug-plan-{}", std::process::id()));
            std::fs::write(&plan, node.to_string()).map_err(|e| format!("cannot write {}: {e}", plan.display()))?;
            let mut child = std::process::Command::new(&executable)
                .env("TONDO_DEBUG", "1")
                .env("TONDO_LIVE_HOLD", "1")
                .env("TONDO_TEST_PLAN", &plan)
                .envs(environment)
                .envs(schedule.iter().flat_map(|order| order_of_tasks(order)))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            own(-(child.id() as i32));
            tested(child.stdout.take(), child.stderr.take(), node);
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Started(executable, arguments) => {
            use std::os::unix::process::CommandExt;
            // Its own process group, so that Ctrl-C pauses it instead of
            // ending it; and no input, which the debugger reads.
            let child = std::process::Command::new(&executable)
                .args(&arguments)
                .env("TONDO_DEBUG", "1")
                .env("TONDO_LIVE_HOLD", "1")
                .stdin(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            own(-(child.id() as i32));
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Replayed(dir) if held.is_some() => {
            let first = resumed(held.map_or(0, |(_, pid)| pid), 0, false, None)?;
            let program = observe::started(&dir.join("program"), first.pid())?;
            copied = Some(first);
            (program, None)
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
            own(-(child.id() as i32));
            if let Some(node) = test {
                tested(child.stdout.take(), child.stderr.take(), node);
            }
            (observe::started(&executable, child.id())?, Some(child))
        }
        Debugged::Running(name) => (crate::standing::connect(&name, None)?, None),
        Debugged::Remote(via, name) => (crate::standing::connect(&name, Some(&via))?, None),
    };
    let pid = program.pid;
    let (writer, said) = attach(program)?;
    let mut session = Session {
        said,
        writer,
        json,
        events: Vec::new(),
        ended: false,
        task: 0,
        checkpointed: Vec::new(),
        report: None,
    };
    // A replay goes back by running its recording again (`back`, `reverse`,
    // `why`): it keeps the commands that set how the program stops.
    let mut replaying = replay.map(|dir| Replaying {
        test: replayed_test(&dir),
        dir,
        root: PathBuf::new(),
        settings: Vec::new(),
        current: child.take().map(Running::Started).or(copied.take()),
        checkpoints: Vec::new(),
        piped: false,
        held,
    });
    let attached = session.answer()?;
    if let Some(error) = attached["error"].as_str() {
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
            disown(-(child.id() as i32));
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
    match held {
        Some((tick, _)) if !json => println!("the recording keeps the run from probe {tick} on, and stops there"),
        Some(_) => {}
        None if child.is_some() || replaying.is_some() => session.wait_event(),
        None => {}
    }
    // A replay keeps checkpoints as it runs, to go back from.
    if replaying.is_some() {
        let _ = ask(&mut session, CHECKPOINTING);
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
                let called = match replaying.as_mut() {
                    Some(replaying) => replaying.call(&mut ask, &place, rest, &bindings),
                    None => crate::live::call(pid, &mut ask, &place, rest, &bindings),
                };
                match called {
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
            // The instructions around where a frame is.
            "disassemble" => {
                let index = rest.parse::<usize>().unwrap_or(0);
                let found = ask(&mut session, "pcs").and_then(|frames| {
                    let pc = frames["pcs"][index].as_u64().filter(|&pc| pc != 0);
                    let pc = pc.ok_or("that frame's code is not known")?;
                    let list = crate::disassembly::around(&mut |c| ask(&mut session, c), pc, -8, 24)?;
                    let list: Vec<Value> = list.iter().map(crate::disassembly::data).collect();
                    Ok(serde_json::json!({ "disassembly": list, "pc": pc }))
                });
                match found {
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
                    stop["stopped"]["causes"] = causes_here(&mut |c| ask(&mut session, c), Path::new(""), &at);
                    Ok(stop)
                };
                match ask_why() {
                    Ok(stop) => session.show(&stop),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // Over the whole run: a replay runs it again from the start.
            "timeline" | "ran" if replaying.is_some() => {
                let replaying = replaying.as_mut().expect("a replay");
                let surveyed = ask(&mut session, "tick").and_then(|t| {
                    let anchor =
                        if word == "timeline" { anchor(&mut |c| ask(&mut session, c), rest)? } else { String::new() };
                    replaying.survey(word, rest, t["tick"].as_u64().unwrap_or(0), &anchor)
                });
                match surveyed {
                    Ok(answer) => session.show(&answer),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            "checkpoints" if rest.trim().is_empty() && replaying.is_some() => {
                let replaying = replaying.as_mut().expect("a replay");
                replaying.gather(&mut session);
                let ticks: Vec<u64> = replaying.checkpoints.iter().map(|&(tick, _)| tick).collect();
                match session.json {
                    true => session.show(&serde_json::json!({ "checkpoints": ticks })),
                    false => println!("{ticks:?}"),
                }
            }
            "seek" if replaying.is_some() => {
                let replaying = replaying.as_mut().expect("a replay");
                let sought = match rest.trim().parse::<u64>() {
                    Ok(to) => replaying.seek(&mut session, to, format!("probe {to}")),
                    Err(_) => Err("seek takes a probe's tick, as timeline and ran give them".to_string()),
                };
                match sought {
                    Ok(stop) => session.show(&stop),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // The line that made a condition hold, over the whole run.
            "first" | "last" => {
                let found = match replaying.as_mut() {
                    Some(replaying) => replaying
                        .when(&mut |c| ask(&mut session, c), word, rest.trim())
                        .and_then(|(to, why)| replaying.seek(&mut session, to, why)),
                    None => Err(format!("{word} searches a replay: `tondo run --record` records a run")),
                };
                match found {
                    Ok(stop) => session.show(&stop),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // What changed between two moments of a replay.
            "diff" => {
                let found = match replaying.as_mut() {
                    Some(replaying) => {
                        let probes: Vec<Option<u64>> = rest.split_whitespace().map(|p| p.parse().ok()).collect();
                        match probes[..] {
                            [Some(from)] => replaying.diff(&mut |c| ask(&mut session, c), from, None),
                            [Some(from), Some(to)] => replaying.diff(&mut |c| ask(&mut session, c), from, Some(to)),
                            _ => Err("diff takes a probe, or two, as tick, timeline and ran give them".into()),
                        }
                    }
                    None => Err("diff compares moments of a replay: `tondo run --record` records a run".into()),
                };
                match found {
                    Ok(answer) => session.show(&answer),
                    Err(message) => session.show(&serde_json::json!({ "error": message })),
                }
            }
            // Where a value came from, over the whole run.
            "origins" => {
                let found = match replaying.as_mut() {
                    Some(replaying) => {
                        let task = session.task;
                        replaying.origins(&mut |c| ask(&mut session, c), task, rest)
                    }
                    None => Err("origins reads a replay: `tondo run --record` records a run".to_string()),
                };
                match found {
                    Ok(answer) => session.show(&answer),
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
            // Who waits for whom, from what each task waits for.
            "blocked" => {
                session.send("tasks")?;
                let answer = session.answer()?;
                session.show(&serde_json::json!({ "blocked": crate::observe::blocked(&answer) }));
            }
            "break" | "delete" | "continue" | "step" | "next" | "finish" | "pause" | "locals" | "print" | "stack"
            | "why" | "tick" | "timeline" | "ran" | "tasks" | "cancel" | "restart" | "select" | "children" | "task"
            | "checkpoints" | "catch" | "set" | "context" | "watch" | "steps" | "record" | "back" | "forward"
            | "reverse" | "until" | "snapshot" | "freeze" | "thaw" | "retainers" | "memory" | "follow" => {
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
                    // A stop said before this answer is one the program went
                    // on from, as a breakpoint set on a program that runs;
                    // one that ran already goes on from none.
                    if answer["already"] != true {
                        session.events.retain(|event| event.get("stopped").is_none());
                    }
                    session.wait_event();
                    // A replay keeps those checkpoints it kept as it ran, and
                    // one where it stops.
                    if let Some(replaying) = replaying.as_mut() {
                        replaying.gather(&mut session);
                        if !session.ended {
                            replaying.keep(&mut |command| ask(&mut session, command));
                        }
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
    // it started. A replay's checkpoints end with it, those it just said
    // it kept too.
    if let Some(replaying) = replaying.as_mut() {
        while let Ok(value) = session.said.try_recv() {
            session.checkpointed(&value);
        }
        replaying.gather(&mut session);
    }
    let _ = session.writer.shutdown(std::net::Shutdown::Both);
    drop(session);
    // Its process ends too at `quit`, or goes on to its end.
    if let Some(replaying) = replaying {
        for (_, pid) in replaying.checkpoints {
            Running::Forked(pid, None).end();
        }
        match replaying.current {
            Some(Running::Started(started)) => child = Some(started),
            Some(Running::Forked(pid, _)) => {
                if quit {
                    Running::Forked(pid, None).end();
                }
                while observe::alive(pid) {
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
    disown(-(child.id() as i32));
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

/// What a cancelled request answers, as the protocol of editors names it.
pub(crate) const CANCELLED: &str = "cancelled";

/// The process that searches a replay now, which `cancel_search` ends, and
/// whether the search was cancelled.
static SEARCH: std::sync::Mutex<(Option<u32>, bool)> = std::sync::Mutex::new((None, false));

/// A search begins, not cancelled.
pub(crate) fn search_begins() {
    *SEARCH.lock().unwrap() = (None, false);
}

/// Ends the search that runs: its process, and the ones it would start.
pub(crate) fn cancel_search() {
    let mut search = SEARCH.lock().unwrap();
    search.1 = true;
    if let Some(pid) = search.0.take() {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

pub(crate) fn search_cancelled() -> bool {
    SEARCH.lock().unwrap().1
}

/// A replay forked from a checkpoint, which shows what the program writes
/// from probe `shown` on: to pipes the caller reads when `piped`, or here;
/// nowhere when nothing shows, but for why it ended, in `report`.
pub(crate) fn resumed(
    checkpoint: u32,
    shown: u64,
    piped: bool,
    report: Option<std::fs::File>,
) -> Result<Running, String> {
    use std::io::BufRead;
    // A checkpoint just forked may not listen yet.
    let path = observe::directory().join(format!("{checkpoint}.checkpoint"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match std::os::unix::net::UnixStream::connect(&path) {
            Ok(stream) => break stream,
            Err(_) if std::time::Instant::now() < deadline && observe::alive(checkpoint) => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(format!("the checkpoint {checkpoint} is gone: {e}")),
        }
    };
    // What the replay writes goes where this one's does, to pipes the
    // caller reads, or nowhere when nothing shows.
    use std::os::fd::AsRawFd;
    let cannot = |e: std::io::Error| e.to_string();
    let (fds, output, _ends) = match (shown, piped) {
        (u64::MAX, _) => {
            let null = std::fs::OpenOptions::new().write(true).open("/dev/null").map_err(cannot)?;
            let errors = report.unwrap_or(null.try_clone().map_err(cannot)?);
            (vec![null.as_raw_fd(), errors.as_raw_fd()], None, (Some((null, errors)), None))
        }
        (_, true) => {
            let ((out, out_end), (error, error_end)) =
                (std::io::pipe().map_err(cannot)?, std::io::pipe().map_err(cannot)?);
            let fds = vec![out_end.as_raw_fd(), error_end.as_raw_fd()];
            (fds, Some((out, error)), (None, Some((out_end, error_end))))
        }
        (_, false) => (vec![1, 2], None, (None, None)),
    };
    given(&stream, &format!("resume {shown} {}\n", std::process::id()), &fds).map_err(cannot)?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let pid: u32 = line.trim().parse().map_err(|_| format!("the checkpoint {checkpoint} did not fork"))?;
    own(pid as i32);
    Ok(Running::Forked(pid, output))
}

/// Where a recording of the last of a run goes on from: the oldest of the
/// copies of the program it held, which are the recording's start. None
/// when it has its start.
pub(crate) fn held_copy(dir: &Path) -> Result<Option<Checkpoint>, String> {
    let Ok(held) = std::fs::read_to_string(dir.join("held")) else { return Ok(None) };
    let copies: Vec<Checkpoint> = held
        .lines()
        .filter_map(|line| {
            let (tick, pid) = line.split_once(' ')?;
            Some((tick.parse().ok()?, pid.parse().ok()?))
        })
        .collect();
    if copies.first().is_some_and(|&(_, pid)| pid == 0) {
        return Ok(None);
    }
    let alive = copies.into_iter().find(|&(_, pid)| observe::alive(pid));
    alive.map(Some).ok_or(format!(
        "the copies of the program that held the end of {} are gone: they last an hour unused, on the machine it ran on, and end with a run that went well",
        dir.display()
    ))
}

/// What a recording of the end of a run has once the run ended.
pub enum Kept {
    /// Copies of the program, which go on from where the recording starts.
    Copies,
    /// All of the run, which was shorter than what it keeps.
    Whole,
    /// Nothing: the run went well, and there is nothing to look at.
    Nothing,
}

/// Ends a recording of the end of a run, when the run ended: the copies of
/// one that went well end, as its calls go.
pub fn end_held(dir: &Path, well: bool) -> Kept {
    let Ok(held) = std::fs::read_to_string(dir.join("held")) else { return Kept::Whole };
    let whole = held.starts_with("0 0\n");
    if !well {
        return if whole { Kept::Whole } else { Kept::Copies };
    }
    let pids = held.lines().filter_map(|line| line.split_once(' ')?.1.parse::<u32>().ok());
    pids.filter(|&pid| pid != 0).for_each(|pid| Running::Forked(pid, None).end());
    if whole {
        return Kept::Whole;
    }
    let _ = std::fs::write(dir.join("held"), "");
    for file in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if file.file_name().to_string_lossy().starts_with("calls") {
            let _ = std::fs::remove_file(file.path());
        }
    }
    Kept::Nothing
}

/// A replay under the debugger: its recording, the commands that set how
/// the program stops, the process the debugger talks to, and checkpoints:
/// copies of a replay where it stopped, by the probe they are at.
pub(crate) struct Replaying {
    dir: PathBuf,
    /// Where the program's files are, which its stops name.
    root: PathBuf,
    settings: Vec<String>,
    current: Option<Running>,
    checkpoints: Vec<Checkpoint>,
    /// What a replay started from the start writes goes to a pipe the
    /// caller reads, as the editor's adapter does, not to this process's.
    piped: bool,
    /// The test a recording of `tondo test --record` is of, whose frames
    /// say how it ended.
    pub(crate) test: Option<u32>,
    /// The copy of the program a recording of the end of a run starts at.
    held: Option<Checkpoint>,
}

/// A process of a replay: one the debugger started, or one a checkpoint
/// forked, which only ends when it is ended.
pub(crate) enum Running {
    Started(std::process::Child),
    /// One a checkpoint forked, which only ends when it is ended, and the
    /// pipes of what it writes, when the caller reads them.
    Forked(u32, Option<Output>),
}

/// The pipes a replay's standard output and error go to.
pub(crate) type Output = (std::io::PipeReader, std::io::PipeReader);

impl Running {
    pub(crate) fn pid(&self) -> u32 {
        match self {
            Running::Started(child) => child.id(),
            Running::Forked(pid, _) => *pid,
        }
    }

    pub(crate) fn end(self) {
        let mut search = SEARCH.lock().unwrap();
        search.0 = search.0.filter(|&pid| pid != self.pid());
        drop(search);
        let pid = self.pid() as i32;
        match self {
            Running::Started(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Ended at once, it leaves its sockets behind.
            Running::Forked(pid, _) => {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                let _ = std::fs::remove_file(observe::directory().join(pid.to_string()));
                let _ = std::fs::remove_file(observe::directory().join(format!("{pid}.checkpoint")));
            }
        }
        disown(pid);
        disown(-pid);
    }
}

/// The checkpoints a replay keeps; past them, the oldest go.
const CHECKPOINTS: usize = 32;

/// How many lines `origins` looks at the causes of, and how far back.
const ORIGINS: usize = 24;
const ORIGINS_DEPTH: usize = 8;

/// A checkpoint: the probe it is at, and its process.
pub(crate) type Checkpoint = (u64, u32);

/// A replay that runs keeps a checkpoint once a second.
pub(crate) const CHECKPOINTING: &str = "checkpoints 1000";

impl Replaying {
    /// The recording run again, attached and stopped: from its start, or
    /// from a checkpoint, with the debugger's settings; what it writes
    /// shows from probe `shown` on.
    fn start(&self, from: Option<(u64, u32)>, shown: u64, json: bool) -> Result<(Running, Session), String> {
        use std::os::unix::process::CommandExt;
        if search_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let from = from.or(self.held);
        let (executable, mut command) = replayed(&self.dir)?;
        // One that only looks writes nothing, but for why it ended, if it
        // ends before the program does.
        let report =
            (shown == u64::MAX).then(|| std::env::temp_dir().join(format!("tondo-replay-{}", std::process::id())));
        let report_file = || report.as_ref().and_then(|path| std::fs::File::create(path).ok());
        let output = || match (shown, self.piped || self.test.is_some()) {
            (u64::MAX, _) => std::process::Stdio::null(),
            (_, true) => std::process::Stdio::piped(),
            (_, false) => std::process::Stdio::inherit(),
        };
        let errors = || report_file().map_or_else(output, std::process::Stdio::from);
        let mut running = match from {
            None => Running::Started(
                command
                    .env("TONDO_RT_SHOWN_FROM", shown.to_string())
                    .stdin(std::process::Stdio::null())
                    .stdout(output())
                    .stderr(errors())
                    .process_group(0)
                    .spawn()
                    .inspect(|child| own(-(child.id() as i32)))
                    .map_err(|e| format!("cannot start {}: {e}", executable.display()))?,
            ),
            Some((_, checkpoint)) => resumed(checkpoint, shown, self.piped || self.test.is_some(), report_file())?,
        };
        match (&mut running, self.test, self.piped) {
            (Running::Started(child), Some(node), false) => tested(child.stdout.take(), child.stderr.take(), node),
            (Running::Forked(_, output), Some(node), false) => {
                if let Some((out, error)) = output.take() {
                    tested(Some(out), Some(error), node);
                }
            }
            _ => {}
        }
        let pid = running.pid();
        // A search that cancels ends it, and the session stays where it was.
        let mut search = SEARCH.lock().unwrap();
        search.0 = Some(pid);
        if search.1 {
            drop(search);
            running.end();
            return Err(CANCELLED.to_string());
        }
        drop(search);
        let (writer, said) = match observe::started(&executable, pid).and_then(attach) {
            Ok(attached) => attached,
            Err(message) => {
                running.end();
                return Err(message);
            }
        };
        let mut session =
            Session { said, writer, json, events: Vec::new(), ended: false, task: 0, checkpointed: Vec::new(), report };
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

    /// A call of the program's code in a replay: in a copy of the process
    /// that ends with it, so that what the code changes does not change the
    /// run, and what it says goes nowhere.
    pub(crate) fn call(
        &mut self,
        ask: &mut dyn FnMut(&str) -> Result<Value, String>,
        place: &str,
        expression: &str,
        bindings: &[(String, String)],
    ) -> Result<String, String> {
        let answer = ask("checkpoint")?;
        let (Some(tick), Some(pid)) = (answer["checkpoint"]["tick"].as_u64(), answer["checkpoint"]["pid"].as_u64())
        else {
            return Err(answer["error"].as_str().unwrap_or("the replay took no copy").to_string());
        };
        self.adopt(tick, pid as u32);
        // One of this moment may have been there before.
        let kept = self.checkpoints.iter().find(|&&(at, _)| at == tick).copied();
        let (running, mut copy) = self.start(kept, u64::MAX, true)?;
        let mut asked = |command: &str| self::ask(&mut copy, command);
        let called = crate::live::call(running.pid(), &mut asked, place, expression, bindings);
        running.end();
        called
    }

    /// A checkpoint where the program stopped, kept for going back.
    pub(crate) fn keep(&mut self, ask: &mut dyn FnMut(&str) -> Result<Value, String>) {
        let Ok(answer) = ask("checkpoint") else { return };
        let (Some(tick), Some(pid)) = (answer["checkpoint"]["tick"].as_u64(), answer["checkpoint"]["pid"].as_u64())
        else {
            return;
        };
        self.adopt(tick, pid as u32);
    }

    /// The checkpoints a replay that ended kept as it ran: those it said
    /// before it ended, read to the end of what it said.
    fn gather_ended(&mut self, session: &mut Session) {
        while let Ok(value) = session.said.recv_timeout(Duration::from_secs(2)) {
            session.checkpointed(&value);
        }
        self.gather(session);
    }

    /// The checkpoints a replay said it kept as it ran.
    fn gather(&mut self, session: &mut Session) {
        for (tick, pid) in std::mem::take(&mut session.checkpointed) {
            self.adopt(tick, pid);
        }
    }

    /// A checkpoint at probe `tick`, kept unless one is there already; the
    /// oldest go past `CHECKPOINTS`.
    pub(crate) fn adopt(&mut self, tick: u64, pid: u32) {
        // One kept there already is the same.
        if self.checkpoints.iter().any(|&(t, _)| t == tick) {
            Running::Forked(pid, None).end();
            return;
        }
        own(pid as i32);
        self.checkpoints.push((tick, pid));
        self.checkpoints.sort_unstable();
        while self.checkpoints.len() > CHECKPOINTS {
            let (_, oldest) = self.checkpoints.remove(0);
            Running::Forked(oldest, None).end();
        }
    }

    /// A replay started at the nearest checkpoint before probe `tick`, or
    /// at it with `at`, or from the start: one that is gone, as one that
    /// went unused too long, goes from the list.
    fn start_before(
        &mut self,
        tick: u64,
        at: bool,
        shown: u64,
        json: bool,
    ) -> Result<(Option<Checkpoint>, Running, Session), String> {
        for base in self.bases(tick, at) {
            match self.start(base, shown, json) {
                Ok((running, session)) => return Ok((base, running, session)),
                Err(_) if base.is_some() => self.checkpoints.retain(|&c| Some(c) != base),
                Err(message) => return Err(message),
            }
        }
        Err("the replay cannot start".into())
    }

    /// Where a replay may start to reach probe `tick`: the checkpoints
    /// before it, the nearest first, then the start. A breakpoint that
    /// counts its hits counts them from the start.
    fn bases(&self, tick: u64, at: bool) -> Vec<Option<Checkpoint>> {
        let counted = self.settings.iter().any(|s| s.contains(" hits "));
        let mut bases: Vec<Option<Checkpoint>> = match counted {
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
    /// breakpoints would stop, or the changes of the expression in the
    /// frame the debugger looks at; another runs to the probe that gives,
    /// and takes over.
    fn go_back(&mut self, session: &mut Session, word: &str, rest: &str) -> Result<Value, String> {
        let tick = ask(session, "tick")?["tick"].as_u64().unwrap_or(0);
        let task = session.task;
        let anchor = if word == "why" { anchor(&mut |c| ask(session, c), rest)? } else { String::new() };
        let (to, why, _) = self.find(word, rest, tick, task, &anchor)?;
        let (running, mut next, mut stop) = self.reach(to, why, session.json)?;
        if word == "why" {
            let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
            stop["stopped"]["causes"] = self.causes(&mut |c| ask(&mut next, c), &at, to);
        }
        if let Some(old) = self.current.replace(running) {
            old.end();
        }
        // The one the debugger talks to keeps checkpoints as it runs.
        let _ = ask(&mut next, CHECKPOINTING);
        std::mem::swap(session, &mut next);
        self.keep(&mut |command| ask(session, command));
        Ok(stop)
    }

    /// The probe `back`, `reverse` or `why` goes to from probe `tick`,
    /// where `task` stopped, and why: what a replay from the nearest
    /// checkpoint before, or an earlier one, finds on its way there; `why`
    /// follows its expression in the frame `anchor` gives.
    pub(crate) fn find(
        &mut self,
        word: &str,
        rest: &str,
        tick: u64,
        task: u64,
        anchor: &str,
    ) -> Result<(u64, String, Value), String> {
        let mut failed = String::new();
        for base in self.bases(tick, false) {
            let (running, mut scan) = match self.start(base, u64::MAX, true) {
                Ok(started) => started,
                Err(_) if base.is_some() => {
                    self.checkpoints.retain(|&c| Some(c) != base);
                    continue;
                }
                Err(message) => return Err(message),
            };
            let scanned = self.scan(&mut scan, word, rest, (tick, task), anchor);
            running.end();
            self.gather_ended(&mut scan);
            match scanned {
                Ok(target) => return Ok(target),
                Err(message) => failed = message,
            }
        }
        Err(failed)
    }

    /// A replay stopped at probe `to`, from the nearest checkpoint, and the
    /// stop, which says `why`.
    pub(crate) fn reach(&mut self, to: u64, why: String, json: bool) -> Result<(Running, Session, Value), String> {
        let (base, running, mut next) = self.start_before(to, true, to, json)?;
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
    pub(crate) fn for_editor(dir: PathBuf, root: PathBuf) -> Replaying {
        let test = replayed_test(&dir);
        let held = held_copy(&dir).ok().flatten();
        Replaying { dir, root, settings: Vec::new(), current: None, checkpoints: Vec::new(), piped: true, test, held }
    }

    /// The probes the checkpoints are at.
    pub(crate) fn kept_at(&self) -> Vec<u64> {
        self.checkpoints.iter().map(|&(tick, _)| tick).collect()
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
            Running::Forked(pid, None).end();
        }
        if let Some(current) = self.current.take() {
            current.end();
        }
    }

    /// `timeline <expression>` or `ran <file>:<line> [<expression>]` in a
    /// replay: what a replay from the start to probe `tick` finds, the
    /// changes of the expression in the frame `anchor` gives, or the runs
    /// of the line.
    pub(crate) fn survey(&mut self, word: &str, rest: &str, tick: u64, anchor: &str) -> Result<Value, String> {
        let (running, mut scan) = self.start(None, u64::MAX, true)?;
        let surveyed = (|| {
            match word {
                "timeline" => ask(&mut scan, &format!("track {anchor} {rest}"))?,
                _ => ask(&mut scan, &format!("count {rest}"))?,
            };
            ask(&mut scan, &format!("scan {tick}"))?;
            scan.next_event()?;
            let tracked = ask(&mut scan, "tracked")?;
            Ok(match word {
                "timeline" => {
                    let changes = tracked["tracked"][0]["changes"].as_array().cloned().unwrap_or_default();
                    let changes: Vec<Value> = changes
                        .into_iter()
                        .map(|mut change| {
                            if change["at"].is_null() {
                                change["at"] = "the start".into();
                            }
                            change
                        })
                        .collect();
                    serde_json::json!({ "timeline": changes })
                }
                _ => serde_json::json!({ "ran": tracked["ran"], "times": tracked["times"], "at": tracked["at"] }),
            })
        })();
        running.end();
        self.gather_ended(&mut scan);
        surveyed
    }

    /// Where each binding the line at probe `to` reads got the value it has
    /// there, found by a replay from a checkpoint before that follows them;
    /// `session` is stopped there, and says which bindings it has.
    pub(crate) fn causes(&mut self, here: Asking, at: &str, to: u64) -> Value {
        let Some(text) = line_text(&self.root, at) else { return Value::Array(Vec::new()) };
        let Ok(locals) = here("locals") else { return Value::Array(Vec::new()) };
        let bound: Vec<&str> =
            locals["locals"].as_array().into_iter().flatten().filter_map(|l| l["name"].as_str()).collect();
        let mut read: Vec<String> = read_by(&text).into_iter().filter(|n| bound.contains(&n.as_str())).collect();
        let mut seen = std::collections::HashSet::new();
        read.retain(|name| seen.insert(name.clone()));
        // A line that receives, or waits for a task's result, has a cause in
        // another task too: the line that sent it, or that the task ended on.
        let receives = ["receive", ".wait(", "select"].iter().any(|word| text.contains(word));
        if read.is_empty() && !receives {
            return Value::Array(Vec::new());
        }
        let Ok(anchors) = read.iter().map(|name| anchor(here, name)).collect::<Result<Vec<_>, _>>() else {
            return Value::Array(Vec::new());
        };
        // From the nearest checkpoint before the line; a binding whose last
        // change came before that checkpoint, from the start.
        let nearest = self.bases(to, false)[0];
        let mut causes = Vec::new();
        for base in std::iter::once(nearest).chain(nearest.map(|_| None)) {
            let Ok((running, mut scan)) = self.start(base, u64::MAX, true) else { continue };
            let found = (|| -> Result<Value, String> {
                ask(&mut scan, "track")?;
                for (name, anchor) in read.iter().zip(&anchors) {
                    ask(&mut scan, &format!("track {anchor} {name}"))?;
                }
                ask(&mut scan, &format!("scan {to}"))?;
                scan.next_event()?;
                // On to the end of the line, for what it receives.
                if receives {
                    ask(&mut scan, "next")?;
                    if let Some(tracked) = scan.next_event()?.get("tracked") {
                        return Ok(tracked.clone());
                    }
                }
                ask(&mut scan, "tracked")
            })();
            running.end();
            self.gather_ended(&mut scan);
            let found = found.unwrap_or_default();
            // The last change of each before the line, by the line before it.
            causes = found["tracked"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|followed| {
                    let changes = followed["changes"].as_array()?;
                    let change = changes.iter().rev().find(|c| c["tick"].as_u64().is_some_and(|t| t < to))?;
                    Some(serde_json::json!({
                        "name": followed["expression"], "value": change["value"], "at": change["at"], "tick": change["tick"],
                    }))
                })
                .collect();
            // What the line received: where it was sent from. A line that
            // waited for it started again once it came.
            let received = found["received"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|r| r["at"] == at && r["tick"].as_u64().is_some_and(|t| t <= to));
            if let Some(from) = received.map(|r| &r["from"]) {
                causes.push(serde_json::json!({
                    "name": "what it received", "received": true, "at": from["at"], "tick": from["tick"], "task": from["task"],
                }));
            }
            if causes.len() == read.len() + usize::from(receives) {
                break;
            }
        }
        Value::Array(causes)
    }

    /// `first <condition>` or `last <condition>` in a replay: the probe of
    /// the line that made the condition hold, the first or the last time
    /// over the whole run, as a replay from the start to the end finds it,
    /// and why.
    pub(crate) fn when(&mut self, here: Asking, which: &str, condition: &str) -> Result<(u64, String), String> {
        let anchor = anchor(here, condition)?;
        let (running, mut scan) = self.start(None, u64::MAX, true)?;
        let found = (|| -> Result<Value, String> {
            ask(&mut scan, &format!("track {anchor} {condition}"))?;
            Ok(scanned_to_end(&mut scan)?["tracked"][0].clone())
        })();
        running.end();
        self.gather_ended(&mut scan);
        let followed = found?;
        let held = |change: &&Value| change["value"] == "true" && change["tick"].is_u64();
        let changes = |key: &str| followed[key].as_array().cloned().unwrap_or_default();
        let change = match which {
            "first" => changes("firsts").iter().find(held).cloned(),
            _ => changes("changes").iter().rev().find(held).cloned(),
        };
        let change = change.ok_or(format!("`{condition}` holds at no line of the run"))?;
        let tick = change["tick"].as_u64().unwrap_or(0);
        Ok((tick, format!("the {which} time `{condition}` holds, after this line")))
    }

    /// `diff <from> [<to>]` in a replay: what changed between the program at
    /// probe `from` and at `to`, or where it is: the bindings of each task's
    /// frames, the tasks that started or ended, and the objects by where
    /// they were made. The program stays where it is.
    pub(crate) fn diff(&mut self, here: Asking, from: u64, to: Option<u64>) -> Result<Value, String> {
        let now = here("tick")?["tick"].as_u64().unwrap_or(0);
        let at = |tick: u64, this: &mut Self| -> Result<Value, String> {
            let (running, mut there, _) = this.reach(tick, String::new(), true)?;
            let found = moment(&mut |c| ask(&mut there, c));
            running.end();
            this.gather_ended(&mut there);
            found
        };
        let before = at(from, self)?;
        let (to, after) = match to.filter(|&to| to != now) {
            Some(to) => (to, at(to, self)?),
            None => (now, moment(here)?),
        };
        Ok(serde_json::json!({ "diff": changes(&before, &after, from, to) }))
    }

    /// `origins [<expression>]` in a replay: the line that gave the
    /// expression its value, or the line the program is at, with where each
    /// binding it reads got its own, and so on back, each line's causes
    /// found by a replay that stops there; the program stays where it is.
    pub(crate) fn origins(&mut self, here: Asking, task: u64, rest: &str) -> Result<Value, String> {
        let tick = here("tick")?["tick"].as_u64().unwrap_or(0);
        let (to, why, at, causes) = match rest.trim() {
            "" => {
                let at = here("snapshot")?["snapshot"]["frames"][0]["at"].as_str().unwrap_or_default().to_string();
                let causes = self.causes(here, &at, tick);
                (tick, "the program is here".to_string(), at, causes)
            }
            expression => {
                let anchor = anchor(here, expression)?;
                let (to, why, _) = self.find("why", expression, tick, task, &anchor)?;
                let (running, mut there, stop) = self.reach(to, why.clone(), true)?;
                let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
                let causes = self.causes(&mut |c| ask(&mut there, c), &at, to);
                running.end();
                self.gather_ended(&mut there);
                (to, why, at, causes)
            }
        };
        let (mut seen, mut left) = (std::collections::HashSet::from([to]), ORIGINS);
        let causes = self.origins_of(&causes, &mut seen, &mut left, 1);
        Ok(serde_json::json!({ "origins": { "why": why, "at": at, "tick": to, "causes": causes } }))
    }

    /// Each cause with the causes of the line that gave it, while `left`
    /// lines may be looked at and the chain is not too deep; a line looked
    /// at once is not again.
    fn origins_of(
        &mut self,
        causes: &Value,
        seen: &mut std::collections::HashSet<u64>,
        left: &mut usize,
        depth: usize,
    ) -> Vec<Value> {
        let mut out = Vec::new();
        for cause in causes.as_array().into_iter().flatten() {
            let mut node = cause.clone();
            let tick = cause["tick"].as_u64();
            if tick.is_some_and(|tick| seen.contains(&tick)) {
                node["above"] = true.into();
            } else if let Some(tick) = tick
                && depth < ORIGINS_DEPTH
                && *left > 0
                && seen.insert(tick)
            {
                *left -= 1;
                if let Ok((running, mut there, stop)) = self.reach(tick, String::new(), true) {
                    let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
                    let further = self.causes(&mut |c| ask(&mut there, c), &at, tick);
                    running.end();
                    self.gather_ended(&mut there);
                    node["causes"] = Value::Array(self.origins_of(&further, seen, left, depth + 1));
                }
            }
            out.push(node);
        }
        out
    }

    /// `seek <tick>` in a replay: on to that probe, or back to it from the
    /// nearest checkpoint.
    fn seek(&mut self, session: &mut Session, to: u64, why: String) -> Result<Value, String> {
        let tick = ask(session, "tick")?["tick"].as_u64().unwrap_or(0);
        if to > tick {
            ask(session, &format!("seek {to}"))?;
            let mut stop = session.next_event()?;
            if stop.get("stopped").is_some() {
                stop["stopped"]["why"] = why.into();
            }
            self.gather(session);
            self.keep(&mut |command| ask(session, command));
            return Ok(stop);
        }
        let (running, mut next, stop) = self.reach(to, why, session.json)?;
        self.take_over(running);
        let _ = ask(&mut next, CHECKPOINTING);
        std::mem::swap(session, &mut next);
        self.keep(&mut |command| ask(session, command));
        Ok(stop)
    }

    /// What a replay that started before probe `tick` finds on its way
    /// there, for `word`: the probe to go back to, and why.
    fn scan(
        &self,
        scan: &mut Session,
        word: &str,
        rest: &str,
        (tick, task): (u64, u64),
        anchor: &str,
    ) -> Result<(u64, String, Value), String> {
        if word == "why" {
            ask(scan, &format!("track {anchor} {rest}"))?;
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
            // The line before the last change of its value, which gave it;
            // one that came before this replay started is earlier.
            _ => {
                let tracked = ask(scan, "tracked")?;
                let last = tracked["tracked"][0]["changes"].as_array().and_then(|c| c.last()).cloned();
                let last = last.ok_or(format!("`{rest}` has no value here"))?;
                let to = last["tick"].as_u64().ok_or(format!("`{rest}` had this value before"))?;
                let value = last["value"].as_str().unwrap_or_default();
                Ok((to, format!("`{rest}` became {value} on this line"), Value::Null))
            }
        }
    }
}

/// Where each binding a line reads got its value, as the history says,
/// for the line `at` the debugger shows, of a program whose files are in
/// `root`: the causes of what it gives.
pub(crate) fn causes_here(ask: &mut dyn FnMut(&str) -> Result<Value, String>, root: &Path, at: &str) -> Value {
    let Some(text) = line_text(root, at) else { return Value::Array(Vec::new()) };
    let Ok(locals) = ask("locals") else { return Value::Array(Vec::new()) };
    let bound: Vec<&str> =
        locals["locals"].as_array().into_iter().flatten().filter_map(|l| l["name"].as_str()).collect();
    let mut read: Vec<String> = read_by(&text).into_iter().filter(|n| bound.contains(&n.as_str())).collect();
    read.dedup();
    if read.is_empty() {
        return Value::Array(Vec::new());
    }
    ask(&format!("causes {}", read.join(" "))).map_or(Value::Array(Vec::new()), |a| a["causes"].clone())
}

/// A cause of a line, in words: a binding's value and the line it came
/// from, or what the line received and the line of the task that sent it.
fn cause_text(cause: &Value) -> String {
    let at = cause["at"].as_str().unwrap_or_default();
    match cause["received"] == true {
        true => format!("what it received came from {at}, in task {}", cause["task"]),
        false => format!(
            "`{}` was {}, from {at}",
            cause["name"].as_str().unwrap_or_default(),
            cause["value"].as_str().unwrap_or_default()
        ),
    }
}

/// The program as it is where it stopped, to compare with another moment:
/// its tasks, each frame of each with its bindings, keyed by the task, the
/// function and how far from the bottom of the stack it is, and its
/// objects by where they were made.
fn moment(here: Asking) -> Result<Value, String> {
    let snapshot = here("snapshot")?;
    let tasks = snapshot["snapshot"]["tasks"].as_array().cloned().unwrap_or_default();
    let mut frames = serde_json::Map::new();
    for task in tasks.iter().filter_map(|t| t["task"].as_u64()) {
        let stack = here(&format!("stack {task}"))?;
        let places = stack["stack"].as_array().cloned().unwrap_or_default();
        for (index, place) in places.iter().enumerate() {
            let function = place.as_str().and_then(|p| p.split_once(" in ")).map_or("", |(_, f)| f);
            let key = format!("task {task}, {function} ({} from the bottom)", places.len() - 1 - index);
            if here(&format!("select {task} {index}")).is_ok()
                && let Ok(locals) = here("locals")
            {
                let bindings: serde_json::Map<String, Value> = locals["locals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| Some((l["name"].as_str()?.to_string(), l["value"].clone())))
                    .collect();
                frames.insert(key, Value::Object(bindings));
            }
        }
    }
    if let Some(task) = snapshot["snapshot"]["task"].as_u64() {
        let _ = here(&format!("select {task} 0"));
    }
    let memory: serde_json::Map<String, Value> = here("memory")
        .ok()
        .and_then(|m| m["memory"]["places"].as_array().cloned())
        .into_iter()
        .flatten()
        .fold(serde_json::Map::new(), |mut places, place| {
            let at = place["at"].as_str().unwrap_or_default().to_string();
            let (objects, bytes) = (place["objects"].as_i64().unwrap_or(0), place["bytes"].as_i64().unwrap_or(0));
            let sum = places.entry(at).or_insert(serde_json::json!([0, 0]));
            *sum = serde_json::json!([sum[0].as_i64().unwrap_or(0) + objects, sum[1].as_i64().unwrap_or(0) + bytes]);
            places
        });
    let tasks: Vec<Value> = tasks.iter().map(|t| serde_json::json!({ "task": t["task"], "at": t["at"] })).collect();
    Ok(serde_json::json!({ "tasks": tasks, "frames": frames, "memory": memory }))
}

/// What changed from one moment to another: the bindings that took
/// another value, came or went, frame by frame; the tasks that started or
/// ended; and the objects by place, as many more or fewer.
fn changes(before: &Value, after: &Value, from: u64, to: u64) -> Value {
    let empty = serde_json::Map::new();
    let (old, new) = (before["frames"].as_object().unwrap_or(&empty), after["frames"].as_object().unwrap_or(&empty));
    let mut bindings = Vec::new();
    for (frame, now) in new {
        let was = old.get(frame).and_then(Value::as_object);
        for (name, value) in now.as_object().unwrap_or(&empty) {
            let earlier = was.and_then(|w| w.get(name));
            if earlier != Some(value) {
                bindings.push(serde_json::json!({ "frame": frame, "name": name, "before": earlier, "after": value }));
            }
        }
    }
    // Those that went, with their frames or not.
    for (frame, was) in old {
        let now = new.get(frame).and_then(Value::as_object);
        for (name, value) in was.as_object().unwrap_or(&empty) {
            if now.is_none_or(|now| !now.contains_key(name)) {
                bindings.push(serde_json::json!({ "frame": frame, "name": name, "before": value, "after": null }));
            }
        }
    }
    let ids = |moment: &Value| -> Vec<Value> {
        moment["tasks"].as_array().into_iter().flatten().map(|t| t["task"].clone()).collect()
    };
    let (old_tasks, new_tasks) = (ids(before), ids(after));
    let started: Vec<&Value> =
        after["tasks"].as_array().into_iter().flatten().filter(|t| !old_tasks.contains(&t["task"])).collect();
    let ended: Vec<&Value> =
        before["tasks"].as_array().into_iter().flatten().filter(|t| !new_tasks.contains(&t["task"])).collect();
    let (old_memory, new_memory) =
        (before["memory"].as_object().unwrap_or(&empty), after["memory"].as_object().unwrap_or(&empty));
    let mut memory = Vec::new();
    for at in old_memory.keys().chain(new_memory.keys().filter(|k| !old_memory.contains_key(*k))) {
        let count = |m: &serde_json::Map<String, Value>, i: usize| m.get(at).and_then(|v| v[i].as_i64()).unwrap_or(0);
        let (objects, bytes) =
            (count(new_memory, 0) - count(old_memory, 0), count(new_memory, 1) - count(old_memory, 1));
        if objects != 0 || bytes != 0 {
            memory.push(serde_json::json!({ "at": at, "objects": objects, "bytes": bytes }));
        }
    }
    serde_json::json!({ "from": from, "to": to, "bindings": bindings, "started": started, "ended": ended, "memory": memory })
}

/// Runs a replay that looks to the end, where the program says what it
/// followed; or to the panic that stops it there, as a debugger's does, which
/// is what a recording of a failure ends with.
fn scanned_to_end(scan: &mut Session) -> Result<Value, String> {
    ask(scan, &format!("scan {}", u64::MAX - 1))?;
    let event = scan.next_event()?;
    match event.get("tracked") {
        Some(followed) => Ok(followed.clone()),
        None => ask(scan, "tracked"),
    }
}

/// Where each task went in a run: its lines, each a probe and a site.
type Trail = std::collections::BTreeMap<u64, Vec<(u64, u64)>>;

/// The lines each task of a recorded run ran, by a replay that looks.
fn trail_of(dir: &Path) -> Result<Trail, String> {
    let replaying = Replaying::for_editor(dir.to_path_buf(), PathBuf::new());
    let file = std::env::temp_dir().join(format!("tondo-trail-{}", std::process::id()));
    let (running, mut scan) = replaying.start(None, u64::MAX, true)?;
    let done = (|| -> Result<(), String> {
        ask(&mut scan, &format!("trail {}", file.display()))?;
        scanned_to_end(&mut scan).map(|_| ())
    })();
    running.end();
    done?;
    let bytes = std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let _ = std::fs::remove_file(&file);
    if bytes.is_empty() {
        return Err(format!("the replay of {} kept none of its lines in {}", dir.display(), file.display()));
    }
    let word = |at: &[u8]| u64::from_le_bytes(at.try_into().expect("8 bytes"));
    let mut trail = Trail::new();
    for line in bytes.as_chunks::<24>().0 {
        trail.entry(word(&line[..8])).or_default().push((word(&line[8..16]), word(&line[16..])));
    }
    Ok(trail)
}

/// The program at a probe of a recorded run: where it is, and its moment.
fn look_at(dir: &Path, tick: u64) -> Result<(String, Value), String> {
    let replaying = Replaying::for_editor(dir.to_path_buf(), PathBuf::new());
    let (running, mut there) = replaying.start(None, u64::MAX, true)?;
    let found = (|| -> Result<(String, Value), String> {
        ask(&mut there, &format!("seek {tick}"))?;
        let stop = there.next_event()?;
        let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
        Ok((at, moment(&mut |c| ask(&mut there, c))?))
    })();
    running.end();
    found
}

/// Where the lines of a task first differ between two runs: the task, how
/// many lines they share, of all the tasks the one that parts the soonest.
/// Each task is compared by itself, as two runs order their tasks in their
/// own way, and read another clock.
fn first_parting(first: &Trail, second: &Trail) -> Option<(u64, usize)> {
    let empty = Vec::new();
    let tasks = first.keys().chain(second.keys()).collect::<std::collections::BTreeSet<_>>();
    let parted = tasks.into_iter().filter_map(|&task| {
        let (a, b) = (first.get(&task).unwrap_or(&empty), second.get(&task).unwrap_or(&empty));
        let shared = a.iter().zip(b).take_while(|(x, y)| x.1 == y.1).count();
        (shared < a.len().max(b.len())).then(|| {
            let when = b.get(shared).or(a.get(shared)).map_or(u64::MAX, |line| line.0);
            (when, task, shared)
        })
    });
    parted.min().map(|(_, task, shared)| (task, shared))
}

/// The origins of `name` at a probe of a recorded run.
fn origins_at(dir: &Path, tick: u64, task: u64, name: &str) -> Option<Value> {
    let mut replaying = Replaying::for_editor(dir.to_path_buf(), PathBuf::new());
    let (running, mut there) = replaying.start(None, u64::MAX, true).ok()?;
    ask(&mut there, &format!("seek {tick}")).ok()?;
    there.next_event().ok()?;
    let found = replaying.origins(&mut |c| ask(&mut there, c), task, name);
    running.end();
    Some(found.ok()?["origins"].clone())
}

/// How many of what a line reads a comparison follows back.
const FOLLOWED: usize = 3;

/// Every cause of a tree of origins.
fn flatten_causes(causes: &Value, out: &mut Vec<Value>) {
    for cause in causes.as_array().into_iter().flatten() {
        out.push(cause.clone());
        flatten_causes(&cause["causes"], out);
    }
}

/// Of a tree of causes, the branches that lead to a value the other run got
/// differently at the same line: what made the runs differ, without what is
/// the same in both.
fn differing(others: &[Value], causes: &Value) -> Vec<Value> {
    let differs = |cause: &Value| {
        let mut there = others.iter().filter(|o| o["name"] == cause["name"] && o["at"] == cause["at"]).peekable();
        there.peek().is_some() && !there.any(|other| other["value"] == cause["value"])
    };
    let branch = |cause: &Value| {
        let below = differing(others, &cause["causes"]);
        (differs(cause) || !below.is_empty()).then(|| {
            let mut kept = cause.clone();
            kept["causes"] = Value::Array(below);
            kept
        })
    };
    causes.as_array().into_iter().flatten().filter_map(branch).collect()
}

/// Where two recorded runs of a program part, as one that passed and one
/// that failed: the first line a task of one runs that the same task of the
/// other does not, what differs in the program just before, and where that
/// came from.
pub fn compared(first: &Path, second: &Path) -> Result<Value, String> {
    let read = |dir: &Path| {
        std::fs::read(dir.join("program")).map_err(|e| format!("{} is not a recording: {e}", dir.display()))
    };
    if read(first)? != read(second)? {
        return Err(format!(
            "{} and {} are runs of different builds, whose lines do not compare: record both with the same one",
            first.display(),
            second.display()
        ));
    }
    read_sources_of(second);
    let (a, b) = (trail_of(first)?, trail_of(second)?);
    let Some((task, shared)) = first_parting(&a, &b) else {
        let lines = |trail: &Trail| trail.values().map(Vec::len).sum::<usize>();
        return Ok(serde_json::json!({ "parted": false, "lines": [lines(&a), lines(&b)] }));
    };
    // The line each goes on to, or that its task ended there.
    let next = |dir: &Path, trail: &Trail| -> Result<Value, String> {
        match trail.get(&task).and_then(|lines| lines.get(shared)) {
            Some(&(tick, _)) => {
                let (at, _) = look_at(dir, tick)?;
                Ok(serde_json::json!({ "at": at, "tick": tick }))
            }
            None => Ok(serde_json::json!({ "at": null, "tick": null })),
        }
    };
    let (next_a, next_b) = (next(first, &a)?, next(second, &b)?);
    // The last line both ran, and what differs there.
    let last = |trail: &Trail| shared.checked_sub(1).and_then(|at| trail.get(&task)?.get(at)).map(|line| line.0);
    let (mut after, mut differs): (_, Vec<Value>) = (None, Vec::new());
    let mut origins = Vec::new();
    if let (Some(tick_a), Some(tick_b)) = (last(&a), last(&b)) {
        let ((at, there_a), (_, there_b)) = (look_at(first, tick_a)?, look_at(second, tick_b)?);
        let changed = changes(&there_a, &there_b, tick_a, tick_b);
        let own = format!("task {task}, ");
        differs = changed["bindings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|binding| binding["frame"].as_str().is_some_and(|frame| frame.starts_with(&own)))
            .cloned()
            .collect();
        // What the line reads decided where each run went: anything else
        // that differs, as a clock, did not.
        let read = line_text(Path::new(""), &at).map_or(Vec::new(), |text| read_by(&text));
        for binding in &mut differs {
            binding["read"] = (binding["name"].as_str().is_some_and(|name| read.iter().any(|r| r == name))).into();
        }
        after = Some(at);
        let decided = differs.iter().filter(|binding| binding["read"] == true);
        for name in decided.filter_map(|binding| binding["name"].as_str()).take(FOLLOWED) {
            let (Some(there_a), Some(there_b)) =
                (origins_at(first, tick_a, task, name), origins_at(second, tick_b, task, name))
            else {
                continue;
            };
            let mut others = Vec::new();
            flatten_causes(&there_a["causes"], &mut others);
            origins.push(serde_json::json!({
                "name": name, "why": there_b["why"], "at": there_b["at"], "causes": differing(&others, &there_b["causes"]),
            }));
        }
    }
    Ok(serde_json::json!({
        "parted": true, "task": task, "after": after, "first": next_a, "second": next_b, "bindings": differs,
        "origins": origins,
    }))
}

/// What a failed run's end can tell without a debugger, for as long as the
/// copies that hold it last and after: where it ended, what every frame held
/// there, and where what its last line reads came from. Written to
/// `report.json` and `report.txt` of the recording.
pub fn write_report(dir: &Path) -> Result<PathBuf, String> {
    read_sources_of(dir);
    let trail = trail_of(dir)?;
    let last = trail.iter().filter_map(|(&task, lines)| lines.last().map(|line| (line.0, task))).max();
    let (tick, task) = last.ok_or("the end of the run has no lines of the program")?;
    let (at, there) = look_at(dir, tick)?;
    let read = line_text(Path::new(""), &at).map_or(Vec::new(), |text| read_by(&text));
    let origins: Vec<Value> = read
        .iter()
        .take(FOLLOWED)
        .filter_map(|name| {
            let found = origins_at(dir, tick, task, name)?;
            Some(serde_json::json!({ "name": name, "why": found["why"], "at": found["at"], "causes": found["causes"] }))
        })
        .collect();
    let report =
        serde_json::json!({ "task": task, "tick": tick, "at": at, "frames": there["frames"], "origins": origins });
    let mut text = format!("the run ended in task {task}, at {at} (probe {tick})\n");
    for (frame, bindings) in report["frames"].as_object().into_iter().flatten() {
        text.push_str(&format!("{frame}:\n"));
        for (name, value) in bindings.as_object().into_iter().flatten() {
            text.push_str(&format!("  {name} = {}\n", value.as_str().unwrap_or_default()));
        }
    }
    for origin in &origins {
        let (name, why) = (origin["name"].as_str().unwrap_or_default(), origin["why"].as_str().unwrap_or_default());
        text.push_str(&format!("where `{name}` came from: {why}, at {}\n", origin["at"].as_str().unwrap_or_default()));
        text.push_str(&origins_text(origin["causes"].as_array().map_or(&[][..], Vec::as_slice), "  "));
    }
    let cannot = |e: std::io::Error| format!("cannot write the report in {}: {e}", dir.display());
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&report).unwrap_or_default()).map_err(cannot)?;
    std::fs::write(dir.join("report.txt"), text).map_err(cannot)?;
    Ok(dir.join("report.txt"))
}

/// `compared` in words, naming the runs.
pub fn compared_text(found: &Value, first: &Path, second: &Path) -> String {
    if found["parted"] != true {
        return "the two runs went through the same lines\n".to_string();
    }
    let mut text = String::new();
    let mut line = |words: String| {
        text.push_str(&words);
        text.push('\n');
    };
    let task = &found["task"];
    line(match found["after"].as_str() {
        Some(at) => format!("the runs part in task {task}, after {at}:"),
        None => format!("the runs part at the first line of task {task}:"),
    });
    let went = |name: &Path, next: &Value| match next["at"].as_str() {
        Some(at) => format!("  {} goes on at {at} (probe {})", name.display(), next["tick"]),
        None => format!("  {} ends the task there", name.display()),
    };
    line(went(first, &found["first"]));
    line(went(second, &found["second"]));
    let bindings = found["bindings"].as_array().map_or(&[][..], Vec::as_slice);
    let shown = |value: &Value| value.as_str().unwrap_or("nothing").to_string();
    let mut list = |title: &str, read: bool| {
        let chosen: Vec<&Value> = bindings.iter().filter(|binding| (binding["read"] == true) == read).collect();
        if !chosen.is_empty() {
            line(title.to_string());
        }
        for binding in chosen {
            let (frame, name) =
                (binding["frame"].as_str().unwrap_or_default(), binding["name"].as_str().unwrap_or_default());
            line(format!("  {name}: {} → {}   ({frame})", shown(&binding["before"]), shown(&binding["after"])));
        }
    };
    list("what that line reads differs, from the first to the second:", true);
    list("what also differs there, which that line does not read:", false);
    for origin in found["origins"].as_array().into_iter().flatten() {
        let (name, why) = (origin["name"].as_str().unwrap_or_default(), origin["why"].as_str().unwrap_or_default());
        let at = origin["at"].as_str().unwrap_or_default();
        line(format!("where the `{name}` of the second came from, by what differs: {why}, at {at}"));
        let causes = origin["causes"].as_array().map_or(&[][..], Vec::as_slice);
        if !causes.is_empty() {
            line(origins_text(causes, "  ").trim_end().to_string());
        }
    }
    if let Some(tick) = found["second"]["tick"].as_u64() {
        line(format!(
            "`tondo debug --replay {}` and `seek {tick}` go there, where `why` follows a value",
            second.display()
        ));
    }
    text
}

/// `tondo debug --compare <first> <second>`.
pub fn compare(first: &Path, second: &Path, json: bool) -> Result<(), String> {
    let found = compared(first, second)?;
    match json {
        true => crate::cli::print_json("debug-compare", true, found),
        false => print!("{}", compared_text(&found, first, second)),
    }
    Ok(())
}

/// The causes `origins` found, as a tree, each with the line that gave it.
fn origins_text(causes: &[Value], indent: &str) -> String {
    let mut text = String::new();
    for (index, cause) in causes.iter().enumerate() {
        let last = index + 1 == causes.len();
        let (branch, next) = if last { ("└ ", "  ") } else { ("├ ", "│ ") };
        let at = cause["at"].as_str().unwrap_or_default();
        let above = if cause["above"] == true { ", as above" } else { "" };
        text.push_str(&format!("{indent}{branch}{}{}{above}\n", cause_text(cause), tick_of(cause)));
        if cause["above"] == true {
            continue;
        }
        let deeper = format!("{indent}{next}");
        if let Some(line) = line_text(Path::new(""), at) {
            text.push_str(&format!("{deeper}  {}\n", line.trim()));
        }
        text.push_str(&origins_text(cause["causes"].as_array().map_or(&[][..], Vec::as_slice), &deeper));
    }
    text
}

/// A replay's probe of a line the history lists, to `seek`; none outside
/// a replay, where it is zero.
fn tick_of(line: &Value) -> String {
    match line["tick"].as_u64() {
        Some(tick) if tick > 0 => format!(" (probe {tick})"),
        _ => String::new(),
    }
}

/// The frame that has the binding `expression` reads, the one the
/// debugger looks at or one its code can read, as a scan of another replay
/// of the run finds it.
fn anchor(here: Asking, expression: &str) -> Result<String, String> {
    Ok(here(&format!("anchor {expression}"))?["anchor"].as_str().unwrap_or_default().to_string())
}

/// What asks the program the debugger looks at, and gives its answer.
pub(crate) type Asking<'a> = &'a mut dyn FnMut(&str) -> Result<Value, String>;

/// The text of the line of `file:line in function`, a file in `root`.
fn line_text(root: &Path, at: &str) -> Option<String> {
    let (place, _) = at.split_once(" in ")?;
    let (file, line) = place.rsplit_once(':')?;
    let text = std::fs::read_to_string(kept_source(file).unwrap_or_else(|| root.join(file))).ok()?;
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
pub(crate) fn ask(session: &mut Session, command: &str) -> Result<Value, String> {
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
    /// The checkpoints a replay kept as it ran, by probe and process.
    checkpointed: Vec<(u64, u32)>,
    /// Where a replay that only looks says why it ended.
    report: Option<PathBuf>,
}

impl Session {
    fn send(&mut self, command: &str) -> Result<(), String> {
        let sent = writeln!(self.writer, "{command}");
        sent.map_err(|_| self.ended_error())
    }

    /// The next stop or end, which it does not show.
    /// The program ended with nothing said: why, of a replay that only
    /// looks, whose crash shows nowhere else.
    fn ended_error(&self) -> String {
        let said = self.report.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
        match said.filter(|said| !said.trim().is_empty()) {
            Some(said) => {
                let lines: Vec<&str> = said.lines().rev().take(20).collect();
                let tail: Vec<&str> = lines.into_iter().rev().collect();
                format!("the replay that searches the run ended early, which is a bug of tondo:\n{}", tail.join("\n"))
            }
            None => "the program ended".to_string(),
        }
    }

    fn next_event(&mut self) -> Result<Value, String> {
        if !self.events.is_empty() {
            return Ok(self.events.remove(0));
        }
        loop {
            let value = self.said.recv().map_err(|_| self.ended_error())?;
            if value.get("stopped").is_some() || value.get("exited").is_some() {
                return Ok(value);
            }
            self.checkpointed(&value);
        }
    }

    /// A checkpoint a replay kept as it ran, which it says as it comes.
    fn checkpointed(&mut self, value: &Value) -> bool {
        let checkpoint = &value["checkpointed"];
        match (checkpoint["tick"].as_u64(), checkpoint["pid"].as_u64()) {
            (Some(tick), Some(pid)) => {
                self.checkpointed.push((tick, pid as u32));
                true
            }
            _ => false,
        }
    }

    /// The answer to the last command; events that come first wait.
    fn answer(&mut self) -> Result<Value, String> {
        loop {
            let value = self.said.recv().map_err(|_| self.ended_error())?;
            if value.get("stopped").is_some() || value.get("exited").is_some() {
                self.events.push(value);
                continue;
            }
            if self.checkpointed(&value) {
                continue;
            }
            // A logpoint's message, a capture or a watch that ended, as it
            // comes.
            if value.get("logged").is_some()
                || value.get("unwatched").is_some()
                || value.get("captured").is_some()
                || value.get("child").is_some()
            {
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
                        || value.get("child").is_some()
                        || value.get("unwatched").is_some()
                        || value.get("captured").is_some() =>
                {
                    self.show(&value)
                }
                Ok(value) if self.checkpointed(&value) => {}
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
                let back = match cause["back"].as_u64() {
                    Some(back) => format!(", {back} line{} back", if back == 1 { "" } else { "s" }),
                    None => tick_of(cause),
                };
                println!("  {}{back}", cause_text(cause));
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
        } else if let Some(diff) = value.get("diff") {
            println!("from probe {} to probe {}:", diff["from"], diff["to"]);
            let mut frame = "";
            for change in diff["bindings"].as_array().into_iter().flatten() {
                if change["frame"].as_str() != Some(frame) {
                    frame = change["frame"].as_str().unwrap_or_default();
                    println!("{frame}:");
                }
                let shown = |v: &Value| v.as_str().map_or("nothing".to_string(), String::from);
                println!(
                    "  {}: {} → {}",
                    change["name"].as_str().unwrap_or_default(),
                    shown(&change["before"]),
                    shown(&change["after"])
                );
            }
            for task in diff["started"].as_array().into_iter().flatten() {
                println!("task {} started, now at {}", task["task"], task["at"].as_str().unwrap_or_default());
            }
            for task in diff["ended"].as_array().into_iter().flatten() {
                println!("task {} ended, last at {}", task["task"], task["at"].as_str().unwrap_or_default());
            }
            for place in diff["memory"].as_array().into_iter().flatten() {
                println!(
                    "memory made at {}: {:+} objects, {:+} bytes",
                    place["at"].as_str().unwrap_or_default(),
                    place["objects"].as_i64().unwrap_or(0),
                    place["bytes"].as_i64().unwrap_or(0)
                );
            }
        } else if let Some(origins) = value.get("origins") {
            println!(
                "{}, at {}{}",
                origins["why"].as_str().unwrap_or_default(),
                origins["at"].as_str().unwrap_or_default(),
                tick_of(origins)
            );
            if let Some(source) = source_line(origins["at"].as_str().unwrap_or_default()) {
                println!("{source}");
            }
            print!("{}", origins_text(origins["causes"].as_array().map_or(&[][..], Vec::as_slice), ""));
        } else if let Some(timeline) = value["timeline"].as_array() {
            if timeline.is_empty() {
                println!("no line of the history gave it a value");
            }
            for change in timeline {
                let back = change["back"].as_u64().map_or(String::new(), |back| format!(", {back} lines back"));
                println!(
                    "{}  at {}{back}{}",
                    change["value"].as_str().unwrap_or_default(),
                    change["at"].as_str().unwrap_or_default(),
                    tick_of(change)
                );
            }
        } else if let Some(runs) = value["ran"].as_array() {
            println!("{} ran {} times in the history", value["at"].as_str().unwrap_or_default(), value["times"]);
            for run in runs {
                let shown = run["value"].as_str().map_or(String::new(), |v| format!(": {v}"));
                println!("  task {}{}{shown}", run["task"], tick_of(run));
            }
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
        } else if value["blocked"].is_object() {
            println!("{}", crate::observe::blocked_text(&value["blocked"]));
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
        } else if let Some(pid) = value["child"]["pid"].as_u64() {
            let program = value["child"]["program"].as_str().unwrap_or_default();
            match value["child"]["recording"].as_str() {
                Some(recording) => println!(
                    "the program started {program}, process {pid}, which recorded its run: \
                     `tondo debug --replay {recording}` replays it"
                ),
                None => println!(
                    "the program started {program}, process {pid}: `tondo debug {pid}` debugs it, which waits 5 s for it"
                ),
            }
        } else if let Some(list) = value["disassembly"].as_array() {
            print!("{}", crate::disassembly::text(list, value["pc"].as_u64().unwrap_or(0)));
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
    let text = std::fs::read_to_string(kept_source(file).unwrap_or_else(|| PathBuf::from(file))).ok()?;
    let source = text.lines().nth(line.checked_sub(1)?)?;
    Some(format!("{line:>5} | {source}"))
}

/// What runs a test in an order a failure named: its choices, as `at:0.2.1`,
/// or a seed. The points of change are those of `tondo test`.
pub(crate) fn order_of_tasks(order: &str) -> [(&'static str, String); 2] {
    let chosen = match order.strip_prefix("at:") {
        Some(choices) => ("TONDO_RT_ORDER", choices.to_string()),
        None => ("TONDO_RT_SCHEDULE", order.to_string()),
    };
    [("TONDO_RACE", "1".to_string()), chosen]
}
