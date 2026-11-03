//! `tondo dap` (toolchain 7.6): the Debug Adapter Protocol on standard
//! input and output, for an editor. Each request becomes a command of the
//! debugger of debug builds; what the program says when it stops or ends
//! goes to the editor as an event when it comes.

use crate::observe;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// The path of the handle of a frame's contexts, before a context's name.
const CONTEXT: &str = "@context ";

/// The path of the scope of a function this one is written in, by its
/// name: a test's suite, a closure's maker.
const OUTER: &str = "@outer ";

/// The key of the function breakpoints among the breakpoints the adapter
/// keeps by file, and the start of a watch's, before its data id.
const FUNCTIONS: &str = "@functions";
/// The key of the breakpoints at events, which the exception filters set.
const EVENTS: &str = "@events";
const DATA: &str = "@data ";

/// The editor's side: messages framed with `Content-Length`, numbered.
#[derive(Clone)]
struct Editor {
    out: Arc<Mutex<(std::io::Stdout, u64)>>,
}

impl Editor {
    fn send(&self, mut message: Value) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.1 += 1;
        message["seq"] = json!(out.1);
        let text = message.to_string();
        let _ = write!(out.0, "Content-Length: {}\r\n\r\n{text}", text.len());
        let _ = out.0.flush();
    }

    fn event(&self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }));
    }

    /// A request to the editor, whose response the adapter does not wait for.
    fn request(&self, command: &str, arguments: Value) {
        self.send(json!({ "type": "request", "command": command, "arguments": arguments }));
    }

    fn respond(&self, request: &Value, body: Result<Value, String>) {
        let mut response = json!({
            "type": "response", "request_seq": request["seq"], "command": request["command"], "success": body.is_ok(),
        });
        match body {
            Ok(body) => response["body"] = body,
            Err(message) => response["message"] = json!(message),
        }
        self.send(response);
    }
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// The program being debugged: the connection, the answers to commands,
/// and the program it started.
struct Debuggee {
    pid: u32,
    writer: UnixStream,
    answers: mpsc::Receiver<Value>,
    child: Option<std::process::Child>,
    /// Where relative paths of the program's sources are.
    root: PathBuf,
    /// What each `variablesReference` names, until the program goes on: a
    /// task, a frame of its stack, and a path in it, empty for the frame's
    /// bindings.
    handles: Vec<(u64, u64, String)>,
    /// How many times the handles were forgotten: a table's next page is
    /// of the stop its first was.
    forgotten: u64,
    /// The calls of the line a task stopped at that `stepIn` can go into,
    /// by their `targetId`.
    targets: Vec<String>,
    /// Where what the program says goes, for a process that takes over.
    pumped: Pumped,
    /// A replay, which goes back by running its recording again; whether
    /// it kept a checkpoint where it stopped last; and the stop to tell
    /// the editor after the answer to the request that went back.
    replaying: Option<crate::debug::Replaying>,
    kept: bool,
    after: Option<Value>,
}

/// A frame's id: its task, and its place in the task's stack.
/// The frames after a task's stack, where it was spawned, from this index.
const SPAWNED: u64 = 9_000;

fn frame_id(task: u64, index: u64) -> u64 {
    task * 10_000 + index
}

fn frame_of(id: u64) -> (u64, u64) {
    (id / 10_000, id % 10_000)
}

impl Debuggee {
    /// Forgets the references, as the program goes on.
    fn forget(&mut self) {
        self.handles.clear();
        self.forgotten += 1;
        self.kept = false;
    }

    /// A replay's process stopped where the debugger went back to takes
    /// over: the debugger talks to it, the old one ends, and the editor
    /// hears the stop after the answer.
    fn take_over(&mut self, mut running: crate::debug::Running, session: crate::debug::Session, stop: &Value) {
        // Its output, to the editor's console: the end of each stream counts
        // toward the program's.
        let tested = self.replaying.as_ref().and_then(|r| r.test);
        let streams = match &mut running {
            crate::debug::Running::Started(child) => (
                child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
                child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            ),
            crate::debug::Running::Forked(_, output) => match output.take() {
                Some((out, error)) => {
                    (Some(Box::new(out) as Box<dyn Read + Send>), Some(Box::new(error) as Box<dyn Read + Send>))
                }
                None => (None, None),
            },
        };
        let opened = usize::from(streams.0.is_some()) + usize::from(streams.1.is_some());
        self.pumped.streams.fetch_add(opened, std::sync::atomic::Ordering::SeqCst);
        forward(streams, &self.pumped.events, &self.pumped.drained, tested);
        let (answer, answers) = mpsc::channel();
        let pumped = self.pumped.clone();
        let said = session.said;
        std::thread::spawn(move || pumped.pump(said.into_iter(), answer));
        self.writer = session.writer;
        self.answers = answers;
        // It keeps checkpoints as it runs, to go back from.
        let _ = self.ask(crate::debug::CHECKPOINTING);
        self.pid = running.pid();
        // The first process is the adapter's; those after, the replay's.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(replaying) = self.replaying.as_mut() {
            replaying.take_over(running);
        }
        self.forget();
        let stop = &stop["stopped"];
        self.after = Some(json!({ "reason": "step", "threadId": stop["task"], "allThreadsStopped": true,
            "description": stop["why"], "text": stop["why"] }));
    }

    /// On to probe `to` of the replay, or back to it from a checkpoint; the
    /// editor hears the stop, which says `why` when it goes back.
    fn seek(&mut self, to: u64, why: String) -> Result<(), String> {
        self.keep();
        let tick = self.ask("tick")?["tick"].as_u64().unwrap_or(0);
        if to > tick {
            self.forget();
            return self.ask(&format!("seek {to}")).map(drop);
        }
        self.adopt();
        let replaying = self.replaying.as_mut().expect("a replay");
        let (running, session, stop) = replaying.reach(to, why, true)?;
        self.take_over(running, session, &stop);
        Ok(())
    }

    /// The checkpoints the replay kept as it ran, kept for going back.
    fn adopt(&mut self) {
        let kept = std::mem::take(&mut *self.pumped.checkpointed.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(replaying) = self.replaying.as_mut() {
            for (tick, pid) in kept {
                replaying.adopt(tick, pid);
            }
        }
    }

    /// A checkpoint where a replay stopped, once.
    fn keep(&mut self) {
        self.adopt();
        if self.kept || self.replaying.is_none() {
            return;
        }
        self.kept = true;
        let mut replaying = self.replaying.take();
        if let Some(replaying) = replaying.as_mut() {
            replaying.keep(&mut |command| self.ask(command));
        }
        self.replaying = replaying;
    }

    /// A reference to what a path names in a frame, for `variables`.
    fn handle(&mut self, task: u64, frame: u64, path: String) -> u64 {
        self.handles.push((task, frame, path));
        self.handles.len() as u64
    }

    fn ask(&mut self, command: &str) -> Result<Value, String> {
        let answer = self.answer(command)?;
        if let Some(error) = answer["error"].as_str() {
            return Err(error.to_string());
        }
        // How the program stops, which a replay run again takes too.
        let word = command.split(' ').next().unwrap_or_default();
        if let Some(replaying) = self.replaying.as_mut()
            && matches!(word, "break" | "delete" | "catch" | "steps")
        {
            replaying.set(command);
        }
        Ok(answer)
    }

    /// The program's answer as it is, with its `error`: a reload reads
    /// why the program refused it.
    fn answer(&mut self, command: &str) -> Result<Value, String> {
        writeln!(self.writer, "{command}").map_err(|_| "the program ended".to_string())?;
        self.answers.recv().map_err(|_| "the program ended".to_string())
    }
}

/// What the debugger builds, for debugging: a program, live or not, and a
/// project's tests, with the identity of each node of their tree; none
/// when it does not compile.
pub struct Builds<'a> {
    pub program: BuildProgram<'a>,
    pub tests: BuildTests<'a>,
}

pub type BuildProgram<'a> = &'a dyn Fn(&Path, bool) -> Result<Option<PathBuf>, String>;
pub type BuildTests<'a> = &'a dyn Fn(&Path) -> Result<Option<(PathBuf, Vec<String>)>, String>;

/// What a frame of the runner inside a test build shows in the editor's
/// console (testing 7): `\x1e<nonce> <sequence>` alone marks the output so
/// far; a frame, its kind, node and the length of the payload that follows,
/// which says how the test debugged, `tested`, ended, or what it logged.
pub(crate) fn test_frame(header: &str, stream: &mut impl BufRead, tested: Option<u32>) -> Option<String> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let [_, _, kind, node, length] = words[..] else { return None };
    let mut payload = vec![0; length.parse().ok()?];
    stream.read_exact(&mut payload).ok()?;
    let payload = String::from_utf8_lossy(&payload);
    let payload: Vec<&str> = payload.split('\0').filter(|part| !part.is_empty()).collect();
    let this = node.parse().ok() == tested;
    match kind {
        "error" | "panic" if this => Some(format!("the test failed: {}\n", payload.join(": ").trim_end())),
        "end" if this => Some("the test passed\n".into()),
        "log" => Some(format!("{}\n", payload.join(" "))),
        _ => None,
    }
}

/// Serves an editor until it disconnects.
pub fn serve(builds: Builds) -> Result<(), String> {
    let editor = Editor { out: Arc::new(Mutex::new((std::io::stdout(), 0))) };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut debuggee: Option<Debuggee> = None;
    let mut dumped: Option<Dumped> = None;
    let mut breakpoints: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    while let Some(request) = read_message(&mut input) {
        // The editor's answers to the adapter's own requests.
        if request["type"] == "response" {
            continue;
        }
        let arguments = &request["arguments"];
        let command = request["command"].as_str().unwrap_or_default();
        // A dump: tasks to look at, which do not run.
        if let Some(dumped) = &dumped
            && !matches!(command, "initialize" | "launch" | "disconnect" | "terminate")
        {
            let result = dumped.answer(command, arguments);
            editor.respond(&request, result);
            if command == "configurationDone" {
                let first = dumped.tasks.first().map_or(json!(1), |t| t["id"].clone());
                let text = dumped.summary.lines().next().unwrap_or_default();
                editor.event(
                    "stopped",
                    json!({ "reason": "entry", "description": text, "threadId": first, "allThreadsStopped": true }),
                );
            }
            continue;
        }
        let result: Result<Value, String> = match command {
            "initialize" => Ok(json!({
                "supportsConfigurationDoneRequest": true,
                "supportsConditionalBreakpoints": true,
                "supportsHitConditionalBreakpoints": true,
                "supportsLogPoints": true,
                "supportsTerminateRequest": true,
                "supportsEvaluateForHovers": true,
                "supportsRestartFrame": true,
                "supportsExceptionInfoRequest": true,
                "supportsSetVariable": true,
                "supportsSetExpression": true,
                "supportsCompletionsRequest": true,
                "supportsStepInTargetsRequest": true,
                "supportsStepBack": true,
                "supportsGotoTargetsRequest": true,
                "completionTriggerCharacters": ["."],
                "supportsFunctionBreakpoints": true,
                "supportsDataBreakpoints": true,
                "exceptionBreakpointFilters": [
                    { "filter": "panic", "label": "Panics", "description": "Stop where a task panics and no recover stops it", "default": true,
                      "supportsCondition": true, "conditionDescription": "Codes or names of panics, as `P0001 checked-overflow`" },
                    { "filter": "error", "label": "Errors", "description": "Stop where an error starts: a fail, or a ? on what the standard library failed with", "default": false,
                      "supportsCondition": true, "conditionDescription": "Types of errors, as `ConfigError json.DecodeError`" },
                    { "filter": "spawn", "label": "Task Starts", "description": "Stop a task at its first line", "default": false },
                    { "filter": "send", "label": "Channel Sends", "description": "Stop a task as it sends on a channel", "default": false },
                    { "filter": "receive", "label": "Channel Receives", "description": "Stop a task as it receives from a channel", "default": false },
                    { "filter": "wait", "label": "Long Waits", "description": "Stop a task that waited long for a lock, a channel or a task", "default": false,
                      "supportsCondition": true, "conditionDescription": "Milliseconds, 100 by default" }
                ],
            })),
            "disconnect" | "terminate" if dumped.is_some() => Ok(json!({})),
            "launch" if arguments["dump"].is_string() => {
                let path = PathBuf::from(arguments["dump"].as_str().unwrap_or_default());
                let root = arguments["cwd"]
                    .as_str()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                match observe::opened(&root.join(&path)) {
                    Ok((summary, tasks, heap)) => {
                        editor.event("output", json!({ "category": "console", "output": summary.clone() }));
                        dumped = Some(Dumped { summary, tasks, heap, root });
                        editor.respond(&request, Ok(json!({})));
                        editor.event("initialized", json!({}));
                        continue;
                    }
                    Err(message) => Err(message),
                }
            }
            "launch" | "attach" => match start(command, arguments, &builds, &editor) {
                Ok(started) => {
                    debuggee = Some(started);
                    editor.respond(&request, Ok(json!({})));
                    editor.event("initialized", json!({}));
                    continue;
                }
                Err(message) => Err(message),
            },
            _ => match debuggee.as_mut() {
                None => Err("launch or attach first".to_string()),
                Some(program) => handle(program, command, arguments, &mut breakpoints),
            },
        };
        let ending = matches!(command, "disconnect" | "terminate");
        editor.respond(&request, result);
        // Where a replay went back to, after the answer.
        if let Some(stop) = debuggee.as_mut().and_then(|program| program.after.take()) {
            editor.event("stopped", stop);
        }
        if ending {
            if let Some(mut program) = debuggee.take() {
                let _ = program.writer.shutdown(std::net::Shutdown::Both);
                if let Some(child) = program.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                program.adopt();
                if let Some(replaying) = program.replaying.as_mut() {
                    replaying.end();
                }
            }
            editor.event("terminated", json!({}));
            break;
        }
    }
    Ok(())
}

/// A dump the editor looks at (`launch` with `dump`): its tasks as
/// threads, their stacks, and what the memory held.
struct Dumped {
    summary: String,
    tasks: Vec<Value>,
    heap: Value,
    root: PathBuf,
}

impl Dumped {
    fn answer(&self, command: &str, arguments: &Value) -> Result<Value, String> {
        let task = |id: &Value| self.tasks.iter().find(|t| t["id"] == *id);
        match command {
            "setBreakpoints" => Ok(json!({ "breakpoints": [] })),
            "setExceptionBreakpoints" | "configurationDone" => Ok(json!({})),
            "threads" => {
                let threads: Vec<Value> = self
                    .tasks
                    .iter()
                    .map(|t| {
                        let name = format!(
                            "task {} ({}) {}",
                            t["id"],
                            t["runs"].as_str().unwrap_or_default(),
                            t["state"].as_str().unwrap_or_default()
                        );
                        json!({ "id": t["id"], "name": name })
                    })
                    .collect();
                Ok(json!({ "threads": threads }))
            }
            "stackTrace" => {
                let id = &arguments["threadId"];
                let lines = task(id).and_then(|t| t["stack"].as_array()).cloned().unwrap_or_default();
                let thread = id.as_u64().unwrap_or(0);
                let frames: Vec<Value> = lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let text = line.as_str().unwrap_or_default();
                        let mut frame =
                            json!({ "id": frame_id(thread, index as u64), "name": text, "line": 0, "column": 0 });
                        if let Some((place, function)) = text.split_once(" in ")
                            && let Some((file, line)) = place.rsplit_once(':')
                        {
                            frame["name"] = json!(function);
                            frame["line"] = json!(line.parse::<u64>().unwrap_or(0));
                            frame["column"] = json!(1);
                            frame["source"] =
                                json!({ "name": file, "path": self.root.join(file).display().to_string() });
                        }
                        frame
                    })
                    .collect();
                Ok(json!({ "stackFrames": frames, "totalFrames": frames.len() }))
            }
            "scopes" => {
                let (thread, _) = frame_of(arguments["frameId"].as_u64().unwrap_or(0));
                Ok(json!({ "scopes": [
                    { "name": "Task", "variablesReference": thread + 1, "expensive": false },
                    { "name": "Memory", "variablesReference": 1, "expensive": false },
                ] }))
            }
            // The memory by where it was made, or a task's state.
            "variables" => {
                let reference = arguments["variablesReference"].as_u64().unwrap_or(0);
                let variables: Vec<Value> = if reference == 1 {
                    let sites = self.heap["sites"].as_array().cloned().unwrap_or_default();
                    sites
                        .iter()
                        .take(100)
                        .map(|site| {
                            let value = format!("{} bytes in {} objects", site["bytes"], site["objects"]);
                            json!({ "name": format!("{} at {}", site["kind"].as_str().unwrap_or_default(), site["site"].as_str().unwrap_or_default()), "value": value, "variablesReference": 0 })
                        })
                        .collect()
                } else {
                    let Some(task) = task(&json!(reference - 1)) else { return Ok(json!({ "variables": [] })) };
                    let mut rows = vec![
                        ("state", task["state"].clone()),
                        ("runs", task["runs"].clone()),
                        ("cancelled", task["cancelled"].clone()),
                    ];
                    if let Some(spawned) = task["spawned"].as_array().and_then(|s| s.first()) {
                        rows.push(("spawned at", spawned.clone()));
                    }
                    for lock in task["locks"].as_array().into_iter().flatten() {
                        rows.push(("holds a lock taken at", lock.clone()));
                    }
                    rows.into_iter()
                        .map(|(name, value)| {
                            let value = value.as_str().map_or(value.to_string(), str::to_string);
                            json!({ "name": name, "value": value, "variablesReference": 0 })
                        })
                        .collect()
                };
                Ok(json!({ "variables": variables }))
            }
            _ => Err("a dump does not run: it shows the program as it was when it was dumped".into()),
        }
    }
}

/// Where what a program says goes: events to the editor, the stop at the
/// start only when it asked for it, and the end once the program's output
/// was read; answers to the command waiting for them.
#[derive(Clone)]
struct Pumped {
    events: Editor,
    entry: bool,
    drained: Arc<(Mutex<usize>, std::sync::Condvar)>,
    /// The program's output streams, as replays that take over add theirs.
    streams: Arc<std::sync::atomic::AtomicUsize>,
    /// The checkpoints a replay kept as it ran, by probe and process.
    checkpointed: Arc<Mutex<Vec<(u64, u32)>>>,
}

impl Pumped {
    fn pump(self, values: impl Iterator<Item = Value>, answer: mpsc::Sender<Value>) {
        let Pumped { events, entry, drained: read_out, streams, checkpointed } = self;
        for value in values {
            if let Some(stop) = value.get("stopped") {
                if stop["why"] == "start" && !entry {
                    continue;
                }
                let why = stop["why"].as_str().unwrap_or_default();
                let reason = match why {
                    "start" => "entry",
                    "step" | "history" => "step",
                    "goto" => "goto",
                    "pause" => "pause",
                    _ if why.starts_with("panic[") || why.starts_with("error: ") => "exception",
                    _ if why.starts_with("watch ") => "data breakpoint",
                    _ => "breakpoint",
                };
                let mut body = json!({ "reason": reason, "threadId": stop["task"], "allThreadsStopped": true });
                if reason == "exception" || reason == "data breakpoint" {
                    body["text"] = json!(why);
                    body["description"] = json!(why);
                }
                events.event("stopped", body);
            } else if let Some(logged) = value["logged"].as_str() {
                events.event("output", json!({ "category": "console", "output": format!("{logged}\n") }));
            } else if let Some(captured) = value.get("captured") {
                // A capture, in the console: where, and each binding.
                let locals: Vec<String> = captured["locals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| {
                        format!(
                            "{} = {}",
                            l["name"].as_str().unwrap_or_default(),
                            l["value"].as_str().unwrap_or_default()
                        )
                    })
                    .collect();
                let at = captured["at"].as_str().unwrap_or_default();
                let output = format!("captured at {at}: {}\n", locals.join(", "));
                events.event("output", json!({ "category": "console", "output": output, "data": captured }));
            } else if let (Some(tick), Some(pid)) =
                (value["checkpointed"]["tick"].as_u64(), value["checkpointed"]["pid"].as_u64())
            {
                checkpointed.lock().unwrap_or_else(|e| e.into_inner()).push((tick, pid as u32));
            } else if let Some(pid) = value["child"]["pid"].as_u64() {
                // A child it follows, which the editor debugs in a session
                // of its own; a debug build of Tondo waits for it.
                let program = value["child"]["program"].as_str().unwrap_or_default();
                let name = format!(
                    "{} ({pid})",
                    Path::new(program).file_name().map_or(program.into(), |n| n.to_string_lossy())
                );
                let configuration = json!({ "type": "tondo", "request": "attach", "name": name, "processId": pid });
                // Once it listens, which a program that is not Tondo's never
                // does.
                let editor = events.clone();
                std::thread::spawn(move || {
                    let socket = observe::directory().join(pid.to_string());
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !socket.exists() {
                        if std::time::Instant::now() > deadline {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    editor.request("startDebugging", json!({ "request": "attach", "configuration": configuration }));
                });
            } else if let Some(id) = value.get("unwatched") {
                let why = value["why"].as_str().unwrap_or_default();
                let breakpoint = json!({ "id": id, "verified": false, "message": format!("the watch ended: {why}") });
                events.event("breakpoint", json!({ "reason": "changed", "breakpoint": breakpoint }));
            } else if value.get("exited").is_some() {
                let (count, ended) = &*read_out;
                let count = count.lock().unwrap_or_else(|e| e.into_inner());
                let wait = std::time::Duration::from_secs(2);
                let opened = streams.load(std::sync::atomic::Ordering::SeqCst);
                drop(ended.wait_timeout_while(count, wait, |count| *count < opened));
                events.event("terminated", json!({}));
            } else if answer.send(value).is_err() {
                break;
            }
        }
    }
}

/// The program's output, to the editor's console as it comes; each stream
/// counts in `drained` once read to its end.
#[allow(clippy::type_complexity)]
fn forward(
    (stdout, stderr): (Option<Box<dyn Read + Send>>, Option<Box<dyn Read + Send>>),
    editor: &Editor,
    drained: &Arc<(Mutex<usize>, std::sync::Condvar)>,
    tested: Option<u32>,
) {
    for (stream, category) in [(stdout, "stdout"), (stderr, "stderr")] {
        let Some(stream) = stream else { continue };
        let editor = editor.clone();
        let drained = drained.clone();
        std::thread::spawn(move || {
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            while matches!(stream.read_line(&mut line), Ok(n) if n > 0) {
                let shown = match line.strip_prefix('\x1e') {
                    // A test's frame, from the runner inside a test build.
                    Some(frame) => test_frame(frame, &mut stream, tested),
                    None => Some(std::mem::take(&mut line)),
                };
                if let Some(output) = shown {
                    editor.event("output", json!({ "category": category, "output": output }));
                }
                line.clear();
            }
            let (count, ended) = &*drained;
            *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            ended.notify_all();
        });
    }
}

/// Starts the program the editor names, or attaches to a running one.
fn start(command: &str, arguments: &Value, builds: &Builds, editor: &Editor) -> Result<Debuggee, String> {
    let root =
        arguments["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let replay = arguments["replay"].as_str().filter(|_| command == "launch").map(|dir| root.join(dir));
    let (program, child, tested) = if let Some(dir) = &replay {
        // A recorded run, again (`tondo debug --replay`).
        use std::os::unix::process::CommandExt;
        let (executable, mut command) = crate::debug::replayed(dir)?;
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
        (observe::started(&executable, child.id())?, Some(child), crate::debug::replayed_test(dir))
    } else if command == "launch" {
        let program = arguments["program"]
            .as_str()
            .ok_or("`launch` takes a `program`: a .to file, a project or an executable")?;
        let path = root.join(program);
        // A test of the project: its tests' executable, which runs the plan
        // it is given, that test alone.
        let mut plan = None;
        let mut tested = None;
        let mut environment = Vec::new();
        let executable = if let Some(test) = arguments["test"].as_str() {
            let (executable, ids) = (builds.tests)(&path)?
                .ok_or_else(|| format!("the tests of {program} do not compile: `tondo test {program}` shows why"))?;
            let Some(node) = ids.iter().position(|id| id == test) else {
                return Err(format!("{program} has no test `{test}`: `tondo test --list {program}` lists them"));
            };
            let file = std::env::temp_dir().join(format!("tondo-debug-plan-{}", std::process::id()));
            std::fs::write(&file, node.to_string()).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            plan = Some(file);
            tested = Some(node as u32);
            // The project's snapshots and the inputs of its fuzz store.
            environment = crate::runner::test_environment(&path)?;
            executable
        } else if path.is_file() && path.extension().is_none_or(|e| e != "to") {
            path
        } else {
            // A live build's code changes with the custom request `reload`.
            let live = arguments["live"].as_bool().unwrap_or(false);
            (builds.program)(&path, live)?
                .ok_or_else(|| format!("{program} does not compile: `tondo check {program}` shows why"))?
        };
        use std::os::unix::process::CommandExt;
        // The variables the editor gives the program.
        let variables: Vec<(String, String)> = arguments["env"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, value)| (name.clone(), value.as_str().map_or(value.to_string(), str::to_string)))
            .collect();
        let schedule = match arguments["schedule"].as_str() {
            Some(seed) if u64::from_str_radix(seed, 16).is_ok() => Some(seed.to_string()),
            Some(seed) => {
                return Err(format!("`schedule` takes the seed in hex of an order of a test's tasks, not `{seed}`"));
            }
            None => None,
        };
        let arguments: Vec<String> =
            arguments["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
        let mut command = std::process::Command::new(&executable);
        if let Some(plan) = &plan {
            command.env("TONDO_TEST_PLAN", plan);
            command.envs(environment);
            // The order of its tasks a failure of `--schedules` named.
            if let Some(seed) = schedule {
                command.env("TONDO_RT_SCHEDULE", seed);
            }
        }
        command.envs(variables);
        let child = command
            .args(&arguments)
            .current_dir(&root)
            .env("TONDO_DEBUG", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
        (observe::started(&executable, child.id())?, Some(child), tested)
    } else {
        let name = match (&arguments["processId"], &arguments["program"]) {
            (Value::Number(pid), _) => pid.to_string(),
            (_, Value::String(name)) => name.clone(),
            _ => return Err("`attach` takes a `processId` or a program's name".into()),
        };
        // On another machine, or in a container, by the command that
        // reaches it, as `ssh host`.
        match arguments["via"].as_str() {
            Some(via) => (observe::via(via, &name)?, None, None),
            None => (observe::connect(&name)?, None, None),
        }
    };
    let pid = program.pid;
    let observe::Program { mut writer, reader, .. } = program;
    let _ = reader.get_ref().set_read_timeout(None);
    let (answer, answers) = mpsc::channel();
    let (events, editor_events) = (editor.clone(), editor.clone());
    // The program waits before `main` until `configurationDone`, which the
    // editor shows only when it asked to stop there.
    let entry = arguments["stopOnEntry"] == true;
    // The streams of the program's output read to their end, which the
    // editor gets before it hears that the program ended.
    let drained = Arc::new((Mutex::new(0), std::sync::Condvar::new()));
    let streams = if child.is_some() { 2 } else { 0 };
    let read_out = drained.clone();
    // What the program says: events go to the editor as they come; answers
    // to the command waiting for them.
    let checkpointed = Arc::new(Mutex::new(Vec::new()));
    let streams = Arc::new(std::sync::atomic::AtomicUsize::new(streams));
    let pumped = Pumped { events, entry, drained: read_out, streams, checkpointed };
    let values = reader.lines().map_while(|line| serde_json::from_str::<Value>(&line.ok()?).ok());
    let pumping = pumped.clone();
    std::thread::spawn(move || pumping.pump(values, answer));
    let mut child = child;
    if let Some(child) = child.as_mut() {
        let streams = (
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        );
        forward(streams, &editor_events, &drained, tested);
    }
    writeln!(writer, "debug").map_err(|e| e.to_string())?;
    if let Some(dir) = &replay {
        crate::debug::read_sources_of(dir);
    }
    let replaying = replay.map(|dir| crate::debug::Replaying::for_editor(dir, root.clone()));
    let (handles, targets, kept, after) = (Vec::new(), Vec::new(), false, None);
    let mut debuggee =
        Debuggee { pid, writer, answers, child, root, handles, forgotten: 0, targets, pumped, replaying, kept, after };
    let attached = debuggee.answers.recv().map_err(|_| "the program ended".to_string())?;
    if let Some(error) = attached["error"].as_str() {
        return Err(format!("{error}: a program is debugged in a build with --debug"));
    }
    // A replay keeps checkpoints as it runs, to go back from.
    if debuggee.replaying.is_some() {
        debuggee.ask(crate::debug::CHECKPOINTING)?;
    }
    // Steps go into the standard library's code too.
    if arguments["justMyCode"] == false {
        debuggee.ask("steps all")?;
    }
    // The children the program starts, each debugged in a session of its
    // own.
    if arguments["followChildren"] == true {
        debuggee.ask("follow")?;
    }
    // A history of the lines run, which Step Back goes through.
    match &arguments["record"] {
        Value::Bool(true) => drop(debuggee.ask("record")?),
        Value::Number(lines) => drop(debuggee.ask(&format!("record {lines}"))?),
        _ => {}
    }
    Ok(debuggee)
}

/// The calls a line of source makes, as `step into` names them: each name
/// before a `(`, with its receiver or module, once, in the code and the
/// interpolations of texts, not in comments.
fn calls(line: &str) -> Vec<String> {
    const WORDS: [&str; 14] =
        ["if", "for", "while", "match", "fn", "return", "fail", "not", "and", "or", "in", "with", "within", "spawn"];
    // The line with the texts blanked out, their interpolations kept.
    let (mut code, mut quote, mut nested, mut escaped) = (Vec::new(), None, 0, false);
    for c in line.chars() {
        let kept = match (quote, c) {
            (None, '/') if code.last() == Some(&'/') => {
                code.pop();
                break;
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                ' '
            }
            (None, _) => c,
            (Some(_), _) if escaped => {
                escaped = false;
                ' '
            }
            (Some(_), '\\') if nested == 0 => {
                escaped = true;
                ' '
            }
            (Some(open), _) if nested == 0 && c == open => {
                quote = None;
                ' '
            }
            (Some('"'), '{') => {
                nested += 1;
                if nested == 1 { ' ' } else { c }
            }
            (Some(_), '}') if nested > 0 => {
                nested -= 1;
                if nested == 0 { ' ' } else { c }
            }
            (Some(_), _) if nested > 0 => c,
            (Some(_), _) => ' ',
        };
        code.push(kept);
    }
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < code.len() {
        let c = code[at];
        let word_char = |c: char| c.is_alphanumeric() || c == '_';
        // A name that goes on a chain, as `map` in `items.map`, starts with
        // it; one after `].` or `).` starts there.
        let chained = at > 0 && (word_char(code[at - 1]) || (code[at - 1] == '.' && at > 1 && word_char(code[at - 2])));
        if !(c.is_alphabetic() || c == '_') || chained {
            at += 1;
            continue;
        }
        let start = at;
        while at < code.len() && (word_char(code[at]) || code[at] == '.') {
            at += 1;
        }
        let word: String = code[start..at].iter().collect();
        // Type arguments between the name and its arguments.
        let mut next = at;
        if code.get(next) == Some(&'[') {
            let mut depth = 0;
            while next < code.len() {
                depth += match code[next] {
                    '[' => 1,
                    ']' => -1,
                    _ => 0,
                };
                next += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if code.get(next) == Some(&'(') && !WORDS.contains(&word.as_str()) && !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

/// The value whose part the console writes after a dot that ends `text`:
/// the path or expression before it, as `user.address` or `items[i]`.
fn owner(text: &str) -> Option<&str> {
    let before = text.strip_suffix('.')?;
    let mut depth = 0;
    let mut start = before.len();
    for (at, c) in before.char_indices().rev() {
        match c {
            ']' | ')' => depth += 1,
            '[' | '(' if depth > 0 => depth -= 1,
            _ if depth > 0 || c.is_alphanumeric() || c == '_' || c == '.' => {}
            _ => break,
        }
        start = at;
    }
    Some(&before[start..]).filter(|owner| !owner.is_empty())
}

/// The path of a variable the editor names in the variables of `path`: a
/// binding in a frame's scope, or a part of the value `path` names.
fn part_path(path: &str, name: &str) -> String {
    match path.is_empty() || path.starts_with(OUTER) {
        true => name.to_string(),
        false if name.starts_with('[') => format!("{path}{name}"),
        false => format!("{path}.{name}"),
    }
}

/// A breakpoint's condition, count of hits and message, as `break` takes
/// them.
fn options(wanted: &Value) -> String {
    let condition = wanted["condition"].as_str().map_or(String::new(), |c| format!(" if {c}"));
    let hits = wanted["hitCondition"].as_str().map_or(String::new(), |h| format!(" hits {h}"));
    let log = wanted["logMessage"].as_str().map_or(String::new(), |m| format!(" log {m}"));
    format!("{condition}{hits}{log}")
}

/// A breakpoint's answer once its condition is compiled, in a live build,
/// or why it could not be.
fn compiled(program: &mut Debuggee, mut answer: Value) -> Result<Value, String> {
    let pid = program.pid;
    crate::live::compile_condition(pid, &mut |request| program.answer(request), &mut answer);
    match answer["error"].as_str() {
        Some(error) => Err(error.to_string()),
        None => Ok(answer),
    }
}

/// Gives a variable with parts the reference that opens them, and an
/// array's or a set's the count of its items, which the editor asks for in
/// pages.
fn opened(program: &mut Debuggee, task: u64, index: u64, path: &str, answer: &Value, variable: &mut Value) {
    let parts = answer["children"].as_u64().unwrap_or(0);
    // A view opens to its value as it is, `raw()`, even without parts.
    if answer["viewed"] == true {
        variable["variablesReference"] = json!(program.handle(task, index, path.to_string()));
        return;
    }
    if parts == 0 {
        return;
    }
    variable["variablesReference"] = json!(program.handle(task, index, path.to_string()));
    if answer["indexed"] == true {
        variable["indexedVariables"] = json!(parts);
    }
}

fn handle(
    program: &mut Debuggee,
    command: &str,
    arguments: &Value,
    breakpoints: &mut std::collections::HashMap<String, Vec<u64>>,
) -> Result<Value, String> {
    match command {
        "setBreakpoints" => {
            let path = arguments["source"]["path"].as_str().unwrap_or_default().to_string();
            for id in breakpoints.remove(&path).unwrap_or_default() {
                program.ask(&format!("delete {id}"))?;
            }
            let mut set = Vec::new();
            let mut ids = Vec::new();
            // A file of another system has no code in this program
            // (language 6.8).
            let host = crate::project::System::host();
            let elsewhere = crate::project::systems_of(Path::new(&path)).filter(|systems| !systems.contains(&host));
            for wanted in arguments["breakpoints"].as_array().into_iter().flatten() {
                let line = wanted["line"].as_u64().unwrap_or(0);
                if let Some(systems) = elsewhere {
                    let named: Vec<&str> = systems.iter().map(|s| s.shown()).collect();
                    let message =
                        format!("this file is for {}; the program runs on {}", named.join(" and "), host.shown());
                    set.push(json!({ "verified": false, "line": line, "message": message }));
                    continue;
                }
                let answer = program.ask(&format!("break {path}:{line}{}", options(wanted)));
                match answer.and_then(|answer| compiled(program, answer)) {
                    Ok(answer) => {
                        ids.push(answer["breakpoint"]["id"].as_u64().unwrap_or(0));
                        set.push(json!({ "verified": true, "line": answer["breakpoint"]["line"] }));
                    }
                    Err(message) => set.push(json!({ "verified": false, "line": line, "message": message })),
                }
            }
            breakpoints.insert(path, ids);
            Ok(json!({ "breakpoints": set }))
        }
        // Where each function of a name starts.
        "setFunctionBreakpoints" => {
            for id in breakpoints.remove(FUNCTIONS).unwrap_or_default() {
                program.ask(&format!("delete {id}"))?;
            }
            let (mut set, mut ids) = (Vec::new(), Vec::new());
            for wanted in arguments["breakpoints"].as_array().into_iter().flatten() {
                let name = wanted["name"].as_str().unwrap_or_default().trim();
                let answer = program.ask(&format!("break fn {name}{}", options(wanted)));
                match answer.and_then(|answer| compiled(program, answer)) {
                    Ok(answer) => {
                        let at = &answer["breakpoint"];
                        ids.push(at["id"].as_u64().unwrap_or(0));
                        let file = at["file"].as_str().unwrap_or_default();
                        let source = json!({ "name": file, "path": crate::debug::source_for_editor(&program.root, file).display().to_string() });
                        set.push(json!({ "verified": true, "line": at["line"], "source": source }));
                    }
                    Err(message) => set.push(json!({ "verified": false, "message": message })),
                }
            }
            breakpoints.insert(FUNCTIONS.to_string(), ids);
            Ok(json!({ "breakpoints": set }))
        }
        // A binding, or a part of one, that a watch can look at: what names
        // it in its frame.
        "dataBreakpointInfo" => {
            let name = arguments["name"].as_str().unwrap_or_default();
            let found = match arguments["variablesReference"].as_u64() {
                Some(reference) => (reference as usize)
                    .checked_sub(1)
                    .and_then(|r| program.handles.get(r).cloned())
                    .filter(|(.., path)| !path.starts_with(CONTEXT))
                    .map(|(task, index, path)| (task, index, part_path(&path, name))),
                None => {
                    arguments["frameId"].as_u64().map(frame_of).map(|(task, index)| (task, index, name.to_string()))
                }
            };
            Ok(match found {
                Some((task, index, path)) => json!({
                    "dataId": format!("{task} {index} {path}"), "description": path,
                    "accessTypes": ["write"], "canPersist": false,
                }),
                None => json!({ "dataId": null, "description": "a context's field is not watched" }),
            })
        }
        // Watches: a new one looks at its frame from now on, until it
        // returns; one no longer wanted goes.
        "setDataBreakpoints" => {
            let asked: Vec<&Value> = arguments["breakpoints"].as_array().into_iter().flatten().collect();
            let wanted: Vec<String> = asked.iter().filter_map(|b| b["dataId"].as_str().map(String::from)).collect();
            let known: Vec<String> = breakpoints.keys().filter(|k| k.starts_with(DATA)).cloned().collect();
            for key in known.into_iter().filter(|key| !wanted.iter().any(|w| key[DATA.len()..] == *w)) {
                for id in breakpoints.remove(&key).unwrap_or_default() {
                    program.ask(&format!("delete {id}"))?;
                }
            }
            let mut set = Vec::new();
            for (data, options) in wanted.into_iter().zip(asked.iter().map(|b| self::options(b))) {
                let key = format!("{DATA}{data}");
                if let Some(&id) = breakpoints.get(&key).and_then(|ids| ids.first()) {
                    set.push(json!({ "verified": true, "id": id }));
                    continue;
                }
                let mut parts = data.splitn(3, ' ');
                let (task, index, path) = (parts.next(), parts.next(), parts.next().unwrap_or_default());
                let selected = program.ask(&format!("select {} {}", task.unwrap_or("1"), index.unwrap_or("0")));
                match selected.and_then(|_| program.ask(&format!("watch {path}{options}"))) {
                    Ok(answer) => {
                        let id = answer["watch"]["id"].as_u64().unwrap_or(0);
                        breakpoints.insert(key, vec![id]);
                        set.push(json!({ "verified": true, "id": id }));
                    }
                    Err(message) => set.push(json!({ "verified": false, "message": message })),
                }
            }
            Ok(json!({ "breakpoints": set }))
        }
        "configurationDone" | "continue" => {
            program.forget();
            program.ask("continue").map(|_| json!({ "allThreadsContinued": true }))
        }
        "next" | "stepIn" | "stepOut" => {
            program.forget();
            let target = arguments["targetId"].as_u64().and_then(|id| program.targets.get(id as usize));
            let asked = match (command, target) {
                ("next", _) => "next".to_string(),
                ("stepIn", Some(target)) => format!("step into {target}"),
                ("stepIn", None) => "step".to_string(),
                _ => "finish".to_string(),
            };
            program.ask(&asked).map(|_| json!({}))
        }
        // A live build's frame goes on at another line of its function:
        // each line is a target, which the move checks.
        "gotoTargets" => {
            let line = arguments["line"].as_u64().unwrap_or(0);
            Ok(json!({ "targets": [{ "id": line, "label": format!("line {line}"), "line": line }] }))
        }
        "goto" => {
            let line = arguments["targetId"].as_u64().unwrap_or(0);
            let task = arguments["threadId"].as_u64().unwrap_or(1);
            let stack = program.ask(&format!("stack {task}"))?;
            let place = stack["stack"][0].as_str().unwrap_or_default().to_string();
            let pid = program.pid;
            let hex = crate::live::goto(pid, &mut |request| program.answer(request), &place, line)?;
            program.forget();
            program.ask(&format!("goto {line} {hex}")).map(|_| json!({}))
        }
        // Back in the history `record` keeps: a line, or to the last line
        // at a breakpoint.
        // A replay goes back by running its recording again, to the whole
        // program as it was.
        "stepBack" | "reverseContinue" if program.replaying.is_some() => {
            program.keep();
            let word = if command == "stepBack" { "back" } else { "reverse" };
            let task = arguments["threadId"].as_u64().unwrap_or(1);
            let tick = program.ask("tick")?["tick"].as_u64().unwrap_or(0);
            program.adopt();
            let replaying = program.replaying.as_mut().expect("a replay");
            let (to, why, _) = replaying.find(word, "1", tick, task, "")?;
            let (running, session, stop) = replaying.reach(to, why, true)?;
            program.take_over(running, session, &stop);
            Ok(json!({}))
        }
        // Each value an expression took over the run, with the line that
        // gave it: in a replay, as a replay from the start finds it, with
        // the probe of each, to `seek`; in the history otherwise.
        "timeline" => {
            let expression = arguments["expression"].as_str().unwrap_or_default().trim().to_string();
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(0));
            program.ask(&format!("select {task} {index}"))?;
            if program.replaying.is_some() {
                let tick = program.ask("tick")?["tick"].as_u64().unwrap_or(0);
                let anchor =
                    program.ask(&format!("anchor {expression}"))?["anchor"].as_str().unwrap_or_default().to_string();
                program.adopt();
                let replaying = program.replaying.as_mut().expect("a replay");
                let found = replaying.survey("timeline", &expression, tick, &anchor)?;
                return Ok(json!({ "changes": found["timeline"], "tick": tick, "checkpoints": replaying.kept_at() }));
            }
            let found = program.ask(&format!("timeline {expression}"))?;
            Ok(json!({ "changes": found["timeline"], "tick": 0, "checkpoints": [] }))
        }
        // On to a probe of a replay, or back to it from a checkpoint.
        "seek" if program.replaying.is_some() => {
            let to = arguments["tick"].as_u64().ok_or("seek takes a probe")?;
            program.seek(to, format!("probe {to}")).map(|()| json!({}))
        }
        // The line after which a condition held, the first or the last time
        // in the whole run of a replay.
        "when" if program.replaying.is_some() => {
            let condition = arguments["condition"].as_str().unwrap_or_default().trim().to_string();
            let which = if arguments["which"] == "last" { "last" } else { "first" };
            let (task, index) = match arguments["frameId"].as_u64() {
                Some(id) => frame_of(id),
                None => (program.ask("snapshot")?["snapshot"]["task"].as_u64().unwrap_or(1), 0),
            };
            program.ask(&format!("select {task} {index}"))?;
            program.keep();
            let mut replaying = program.replaying.take().expect("a replay");
            let found = replaying.when(&mut |c| program.ask(c), which, &condition);
            program.replaying = Some(replaying);
            let (to, why) = found?;
            program.seek(to, why.clone()).map(|()| json!({ "why": why, "tick": to }))
        }
        // What changed between two probes of a replay, or one and now.
        "diff" if program.replaying.is_some() => {
            let from = arguments["from"].as_u64().ok_or("diff takes the probe to compare with, `from`")?;
            let mut replaying = program.replaying.take().expect("a replay");
            let found = replaying.diff(&mut |c| program.ask(c), from, arguments["to"].as_u64());
            program.replaying = Some(replaying);
            program.adopt();
            found
        }
        // The tasks: where each is, and what each waits for and on whom.
        "tasks" => program.ask("tasks"),
        // What each line of the call the frame shows changed the last time
        // it ran, as the history has it, for the editor to show beside it.
        "effects" => {
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(0));
            program.ask(&format!("select {task} {index}"))?;
            let answer = match program.ask("effects") {
                Ok(answer) if index == 0 => answer,
                _ => return Ok(json!({ "lines": [] })),
            };
            let lines: Vec<Value> = answer["effects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|effect| {
                    let changed: Vec<String> = effect["changed"]
                        .as_array()?
                        .iter()
                        .map(|c| {
                            format!(
                                "{} = {}",
                                c["name"].as_str().unwrap_or_default(),
                                c["value"].as_str().unwrap_or_default()
                            )
                        })
                        .collect();
                    let path =
                        crate::debug::source_for_editor(&program.root, effect["file"].as_str()?).display().to_string();
                    (!changed.is_empty())
                        .then(|| json!({ "path": path, "line": effect["line"], "text": changed.join(", ") }))
                })
                .collect();
            Ok(json!({ "lines": lines }))
        }
        // Where a value came from in a replay, back over the run.
        "origins" => {
            let expression = arguments["expression"].as_str().unwrap_or_default().trim().to_string();
            let (task, index) = match arguments["frameId"].as_u64() {
                Some(id) => frame_of(id),
                None => (program.ask("snapshot")?["snapshot"]["task"].as_u64().unwrap_or(1), 0),
            };
            program.ask(&format!("select {task} {index}"))?;
            program.keep();
            let Some(mut replaying) = program.replaying.take() else {
                return Err("origins reads a replay: `tondo run --record` records a run".into());
            };
            let found = replaying.origins(&mut |c| program.ask(c), task, &expression);
            program.replaying = Some(replaying);
            program.adopt();
            found
        }
        // The line that gave an expression its value, and where what that
        // line reads got its own: the program as it was there in a replay,
        // the history otherwise.
        "why" => {
            let expression = arguments["expression"].as_str().unwrap_or_default().trim().to_string();
            let (task, index) = match arguments["frameId"].as_u64() {
                Some(id) => frame_of(id),
                None => (program.ask("snapshot")?["snapshot"]["task"].as_u64().unwrap_or(1), 0),
            };
            program.ask(&format!("select {task} {index}"))?;
            if program.replaying.is_some() {
                program.keep();
                let tick = program.ask("tick")?["tick"].as_u64().unwrap_or(0);
                let anchor =
                    program.ask(&format!("anchor {expression}"))?["anchor"].as_str().unwrap_or_default().to_string();
                program.adopt();
                let replaying = program.replaying.as_mut().expect("a replay");
                let (to, why, _) = replaying.find("why", &expression, tick, task, &anchor)?;
                let (running, mut session, stop) = replaying.reach(to, why, true)?;
                let at = stop["stopped"]["at"].as_str().unwrap_or_default().to_string();
                let causes = replaying.causes(&mut |c| crate::debug::ask(&mut session, c), &at, to);
                program.take_over(running, session, &stop);
                return Ok(json!({ "why": stop["stopped"]["why"], "at": at, "causes": causes }));
            }
            program.forget();
            let value = program.ask(&format!("print {expression}"))?["value"].as_str().unwrap_or_default().to_string();
            program.ask(&format!("why {expression}"))?;
            let stack = program.ask(&format!("stack {task}"))?;
            let at = stack["stack"][0].as_str().unwrap_or_default().to_string();
            let root = program.root.clone();
            let causes = crate::debug::causes_here(&mut |command| program.ask(command), &root, &at);
            Ok(json!({ "why": format!("`{expression}` became {value} on this line"), "at": at, "causes": causes }))
        }
        "stepBack" | "reverseContinue" => {
            program.forget();
            let asked = if command == "stepBack" { "back" } else { "reverse" };
            program.ask(asked).map(|_| json!({}))
        }
        // The calls of the line a frame is at, which `stepIn` goes into.
        "stepInTargets" => {
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(10_000));
            let stack = program.ask(&format!("stack {task}"))?;
            let place = stack["stack"].get(index as usize).and_then(Value::as_str).unwrap_or_default().to_string();
            let line = place.split_once(" in ").and_then(|(at, _)| at.rsplit_once(':')).and_then(|(file, line)| {
                let text = std::fs::read_to_string(crate::debug::source_for_editor(&program.root, file)).ok()?;
                text.lines().nth(line.parse::<usize>().ok()?.checked_sub(1)?).map(String::from)
            });
            program.targets = line.as_deref().map(calls).unwrap_or_default();
            let targets: Vec<Value> =
                program.targets.iter().enumerate().map(|(id, label)| json!({ "id": id, "label": label })).collect();
            Ok(json!({ "targets": targets }))
        }
        "pause" => program.ask("pause").map(|_| json!({})),
        "setExceptionBreakpoints" => {
            // Each filter, with the codes or types its condition names.
            let plain = arguments["filters"].as_array().into_iter().flatten().map(|f| (f.as_str(), None));
            let options = arguments["filterOptions"].as_array().into_iter().flatten();
            let options = options.map(|o| (o["filterId"].as_str(), o["condition"].as_str()));
            let mut caught = Vec::new();
            // The events, breakpoints of their own, set again each time.
            for id in breakpoints.remove(EVENTS).unwrap_or_default() {
                program.ask(&format!("delete {id}"))?;
            }
            let mut events = Vec::new();
            for (filter, condition) in plain.chain(options) {
                let event = match (filter, condition) {
                    (Some(filter @ ("spawn" | "send" | "receive")), _) => filter.to_string(),
                    (Some("wait"), condition) => {
                        format!("wait {}", condition.filter(|c| !c.trim().is_empty()).unwrap_or("100").trim())
                    }
                    _ => String::new(),
                };
                if !event.is_empty() {
                    let set = program.ask(&format!("break on {event}"))?;
                    events.extend(set["breakpoint"]["id"].as_u64());
                    continue;
                }
                let Some(filter @ ("panic" | "error")) = filter else { continue };
                let names = condition.unwrap_or_default().replace(',', " ");
                caught.push(format!("{filter} {names}").trim_end().to_string());
            }
            breakpoints.insert(EVENTS.to_string(), events);
            let caught = if caught.is_empty() { "none".to_string() } else { caught.join(" ") };
            program.ask(&format!("catch {caught}")).map(|_| json!({}))
        }
        // Why the task stopped at a panic or an error: its message, with
        // where.
        "exceptionInfo" => {
            let task = arguments["threadId"].as_u64().unwrap_or(1);
            let tasks = program.ask("tasks")?;
            let held = tasks["tasks"].as_array().into_iter().flatten().find(|t| t["task"] == task);
            let why = held.and_then(|t| t["why"].as_str()).unwrap_or_default();
            let id = match why {
                _ if why.starts_with("panic[") => "panic",
                _ if why.starts_with("error: ") => "error",
                _ => return Err(format!("task {task} did not stop at a panic or an error")),
            };
            Ok(json!({ "exceptionId": id, "description": why, "breakMode": "always", "details": { "message": why } }))
        }
        // The frame where the task stopped, the only one that restarts.
        "restartFrame" => match arguments["frameId"].as_u64().map(frame_of) {
            Some((_, 0)) | None => {
                program.forget();
                program.ask("restart").map(|_| json!({}))
            }
            Some(_) => Err("only the frame where the task stopped restarts".into()),
        },
        "threads" => {
            let tasks = program.ask("tasks")?;
            let mut threads: Vec<Value> = tasks["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| match t["why"].as_str() {
                    Some(why @ ("waits" | "ready")) => {
                        json!({ "id": t["task"], "name": format!("task {} ({why})", t["task"]) })
                    }
                    _ => json!({ "id": t["task"], "name": format!("task {}", t["task"]) }),
                })
                .collect();
            if threads.is_empty() {
                threads.push(json!({ "id": 1, "name": "task 1" }));
            }
            Ok(json!({ "threads": threads }))
        }
        "stackTrace" => {
            // A replay keeps a checkpoint where it stopped, which the
            // editor asks the stack of.
            program.keep();
            let task = arguments["threadId"].as_u64().unwrap_or(1);
            let stack = program.ask(&format!("stack {task}"))?;
            let frames: Vec<Value> = stack["stack"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(index, frame)| {
                    let (place, function) = frame.as_str()?.split_once(" in ")?;
                    let (file, line) = place.rsplit_once(':')?;
                    let path = crate::debug::source_for_editor(&program.root, file);
                    Some(json!({
                        "id": frame_id(task, index as u64), "name": function, "line": line.parse::<u64>().ok()?,
                        "column": 1, "source": { "name": file, "path": path.display().to_string() },
                    }))
                })
                .collect();
            // Where the task was spawned, and its spawner, after a label:
            // places to look at, without bindings.
            let mut frames = frames;
            let spawned = stack["spawned"].as_array().cloned().unwrap_or_default();
            if !spawned.is_empty() {
                let label = json!({ "id": frame_id(task, SPAWNED), "name": "spawned at", "line": 0, "column": 0,
                    "presentationHint": "label" });
                frames.push(label);
            }
            for (index, place) in spawned.iter().enumerate() {
                let Some((at, function)) = place.as_str().and_then(|p| p.split_once(" in ")) else { continue };
                let Some((file, line)) = at.rsplit_once(':') else { continue };
                let path = crate::debug::source_for_editor(&program.root, file);
                frames.push(json!({
                    "id": frame_id(task, SPAWNED + 1 + index as u64), "name": function,
                    "line": line.parse::<u64>().unwrap_or(0), "column": 1, "presentationHint": "subtle",
                    "source": { "name": file, "path": path.display().to_string() },
                }));
            }
            Ok(json!({ "stackFrames": frames, "totalFrames": frames.len() }))
        }
        "scopes" => {
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(10_000));
            // A place where a task was spawned has no bindings to show.
            if index >= SPAWNED {
                return Ok(json!({ "scopes": [] }));
            }
            let locals = program.handle(task, index, String::new());
            let mut scopes = vec![json!({ "name": "Locals", "variablesReference": locals, "expensive": false })];
            // The bindings of the functions this one is written in, as a
            // test's suites, a scope for each.
            program.ask(&format!("select {task} {index}"))?;
            let mut outers: Vec<String> = Vec::new();
            for local in program.ask("locals").ok().iter().flat_map(|l| l["locals"].as_array().cloned()).flatten() {
                if let Some(from) = local["from"].as_str().filter(|from| !outers.iter().any(|o| o == from)) {
                    outers.push(from.to_string());
                }
            }
            for outer in outers {
                let reference = program.handle(task, index, format!("{OUTER}{outer}"));
                scopes.push(json!({ "name": outer, "variablesReference": reference, "expensive": false }));
            }
            // The task's contexts (language 11.16).
            let context = program.handle(task, index, CONTEXT.to_string());
            scopes.push(json!({ "name": "Context", "variablesReference": context, "expensive": false }));
            Ok(json!({ "scopes": scopes }))
        }
        // A frame's bindings, or the parts of one or of a part, each with a
        // reference to its own parts when it has some.
        "variables" => {
            let reference = arguments["variablesReference"].as_u64().unwrap_or(0) as usize;
            let Some((task, index, path)) = reference.checked_sub(1).and_then(|r| program.handles.get(r).cloned())
            else {
                return Ok(json!({ "variables": [] }));
            };
            program.ask(&format!("select {task} {index}"))?;
            let mut variables = Vec::new();
            if let Some(name) = path.strip_prefix(CONTEXT) {
                // The contexts, and the fields of one, with the `with` that
                // set each.
                let contexts = program.ask("context")?;
                for context in contexts["contexts"].as_array().into_iter().flatten() {
                    let named = context["name"].as_str().unwrap_or_default();
                    if name.is_empty() {
                        let reference = program.handle(task, index, format!("{CONTEXT}{named}"));
                        variables.push(json!({ "name": named, "value": "", "variablesReference": reference }));
                    } else if name == named {
                        for field in context["fields"].as_array().into_iter().flatten() {
                            let value = match field["set"].as_str() {
                                Some(at) => format!("{}  (with at {at})", field["value"].as_str().unwrap_or_default()),
                                None => field["value"].as_str().unwrap_or_default().to_string(),
                            };
                            variables.push(json!({ "name": field["name"], "value": value, "variablesReference": 0 }));
                        }
                    }
                }
            } else if path.is_empty() || path.starts_with(OUTER) {
                let outer = path.strip_prefix(OUTER);
                let locals = program.ask("locals")?;
                // What a function returned in the step that stopped here.
                if let (Some(returned), None) = (locals.get("returned"), outer) {
                    let name = format!("({} returned)", returned["function"].as_str().unwrap_or_default());
                    variables.push(json!({ "name": name, "value": returned["value"], "variablesReference": 0 }));
                }
                for local in locals["locals"].as_array().into_iter().flatten() {
                    if local["from"].as_str() != outer {
                        continue;
                    }
                    let name = local["name"].as_str().unwrap_or_default().to_string();
                    let mut variable = json!({
                        "name": name, "value": local["value"], "type": local["type"], "evaluateName": name,
                        "variablesReference": 0,
                    });
                    opened(program, task, index, &name, local, &mut variable);
                    variables.push(variable);
                }
            } else if arguments["filter"] != "named" {
                // All the parts, or a page of an array's.
                let page = match (arguments["start"].as_u64(), arguments["count"].as_u64()) {
                    (Some(start), Some(count)) if count > 0 => format!(" {start} {count}"),
                    _ => String::new(),
                };
                let parts = program.ask(&format!("children {path}{page}"))?;
                for part in parts["children"].as_array().into_iter().flatten() {
                    let name = part["name"].as_str().unwrap_or_default();
                    let shown = name.strip_prefix('.').unwrap_or(name);
                    let path = part["path"].as_str().unwrap_or_default().to_string();
                    let mut variable =
                        json!({ "name": shown, "value": part["value"], "evaluateName": path, "variablesReference": 0 });
                    opened(program, task, index, &path, part, &mut variable);
                    variables.push(variable);
                }
            }
            Ok(json!({ "variables": variables }))
        }
        // The editor's table of a value's parts, as an array or a map of
        // records: a row for each part of a page, a column for each field.
        "table" => {
            if arguments["stop"].as_u64().is_some_and(|stop| stop != program.forgotten) {
                return Err("the program went on: show the table again where it stops".into());
            }
            let reference = arguments["variablesReference"].as_u64().unwrap_or(0) as usize;
            let Some((task, index, path)) = reference.checked_sub(1).and_then(|r| program.handles.get(r).cloned())
            else {
                return Err("only a value with parts, as an array or a map, shows as a table".into());
            };
            if path.is_empty() || path.starts_with(CONTEXT) || path.starts_with(OUTER) {
                return Err("only a value with parts, as an array or a map, shows as a table".into());
            }
            program.ask(&format!("select {task} {index}"))?;
            let start = arguments["start"].as_u64().unwrap_or(0);
            let count = arguments["count"].as_u64().unwrap_or(500);
            let parts = program.ask(&format!("children {path} {start} {count}"))?;
            let mut columns: Vec<String> = Vec::new();
            let mut rows = Vec::new();
            for part in parts["children"].as_array().into_iter().flatten() {
                let mut cells = serde_json::Map::new();
                // A part with fields gives a cell to each; an array, or a
                // value without parts, one.
                let fields = match part["children"].as_u64().unwrap_or(0) > 0 && part["indexed"] != true {
                    true => program.ask(&format!("children {}", part["path"].as_str().unwrap_or_default()))?,
                    false => json!({ "children": [{ "name": "value", "value": part["value"] }] }),
                };
                for field in fields["children"].as_array().into_iter().flatten() {
                    let name = field["name"].as_str().unwrap_or_default();
                    let name = name.strip_prefix('.').unwrap_or(name).to_string();
                    if !columns.contains(&name) {
                        columns.push(name.clone());
                    }
                    cells.insert(name, field["value"].clone());
                }
                rows.push(json!({ "name": part["name"], "cells": cells }));
            }
            let stop = program.forgotten;
            Ok(json!({ "columns": columns, "rows": rows, "start": start, "total": parts["total"], "stop": stop }))
        }
        // A binding of a frame where its task stopped, or a part of one,
        // changed to what an expression gives.
        "setVariable" => {
            let reference = arguments["variablesReference"].as_u64().unwrap_or(0) as usize;
            let Some((task, index, path)) = reference.checked_sub(1).and_then(|r| program.handles.get(r).cloned())
            else {
                return Err("no such variables".into());
            };
            if path.starts_with(CONTEXT) {
                return Err("a context's field changes with `with`".into());
            }
            program.ask(&format!("select {task} {index}"))?;
            let target = part_path(&path, arguments["name"].as_str().unwrap_or_default());
            let value = arguments["value"].as_str().unwrap_or_default();
            let set = program.ask(&format!("set {target} = {value}"))?;
            Ok(json!({ "value": set["value"], "type": set["type"], "variablesReference": 0 }))
        }
        "setExpression" => {
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(10_000));
            program.ask(&format!("select {task} {index}"))?;
            let target = arguments["expression"].as_str().unwrap_or_default();
            let value = arguments["value"].as_str().unwrap_or_default();
            let set = program.ask(&format!("set {target} = {value}"))?;
            Ok(json!({ "value": set["value"], "type": set["type"], "variablesReference": 0 }))
        }
        // What the console can write next: the bindings of the frame, the
        // fields of a value after its dot and the methods expressions call,
        // or the words of expressions.
        "completions" => {
            let text = arguments["text"].as_str().unwrap_or_default();
            let column = arguments["column"].as_u64().map_or(usize::MAX, |c| c.saturating_sub(1) as usize);
            let before: String = text.chars().take(column).collect();
            let start = before.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
            let word = &before[start..];
            let (task, index) = match arguments["frameId"].as_u64() {
                Some(id) => frame_of(id),
                None => (1, 0),
            };
            program.ask(&format!("select {task} {index}"))?;
            let mut found: Vec<(String, &str)> = Vec::new();
            match owner(&before[..start]) {
                Some(owner) => {
                    let value = program.ask(&format!("print {owner}")).unwrap_or_default();
                    if let Some(path) = value["path"].as_str() {
                        let parts = program.ask(&format!("children {path} 0 1000")).unwrap_or_default();
                        for part in parts["children"].as_array().into_iter().flatten() {
                            if let Some(field) = part["name"].as_str().and_then(|n| n.strip_prefix('.')) {
                                found.push((field.to_string(), "field"));
                            }
                        }
                    }
                    let methods = ["length", "isEmpty", "isSome", "isNone", "isOk", "isErr", "contains", "startsWith"];
                    found.extend(methods.iter().chain(&["endsWith"]).map(|m| (format!("{m}()"), "method")));
                }
                None => {
                    let locals = program.ask("locals").unwrap_or_default();
                    for local in locals["locals"].as_array().into_iter().flatten() {
                        found.push((local["name"].as_str().unwrap_or_default().to_string(), "variable"));
                    }
                    found.extend(["and", "or", "not", "true", "false", "none"].map(|k| (k.to_string(), "keyword")));
                }
            }
            let length = word.chars().count();
            let first = before[..start].chars().count() + 1;
            let targets: Vec<Value> = found
                .into_iter()
                .filter(|(label, _)| label.starts_with(word))
                .map(|(label, kind)| json!({ "label": label, "type": kind, "start": first, "length": length }))
                .collect();
            Ok(json!({ "targets": targets }))
        }
        // What an expression gives in the frame chosen; a binding or a part
        // of one opens by its path.
        "evaluate" => {
            let path = arguments["expression"].as_str().unwrap_or_default().trim().to_string();
            let (task, index) = match arguments["frameId"].as_u64() {
                Some(id) => frame_of(id),
                None => {
                    let tasks = program.ask("tasks")?;
                    let stopped = tasks["tasks"].as_array().into_iter().flatten().find(|t| t["why"] != "held");
                    (stopped.and_then(|t| t["task"].as_u64()).unwrap_or(1), 0)
                }
            };
            program.ask(&format!("select {task} {index}"))?;
            // In the console, the debugger's own commands that only look.
            let word = path.split_whitespace().next().unwrap_or_default();
            let looking = ["memory", "retainers", "snapshot", "tasks", "freeze", "thaw", "record", "context"];
            if arguments["context"] == "repl" && looking.contains(&word) {
                let answer = program.ask(&path)?;
                let shown = serde_json::to_string_pretty(&answer).unwrap_or_default();
                return Ok(json!({ "result": shown, "variablesReference": 0 }));
            }
            let value = match program.ask(&format!("print {path}")) {
                Ok(value) => value,
                // In the console, an expression that calls the program's
                // code runs, in a live build.
                Err(read) if arguments["context"] == "repl" && path.contains('(') => {
                    let stack = program.ask(&format!("stack {task}"))?;
                    let place = stack["stack"][index as usize].as_str().unwrap_or_default().to_string();
                    let locals = program.ask("locals")?;
                    let bindings: Vec<(String, String)> = locals["locals"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|l| {
                            (
                                l["name"].as_str().unwrap_or_default().into(),
                                l["type"].as_str().unwrap_or_default().into(),
                            )
                        })
                        .collect();
                    let pid = program.pid;
                    let called =
                        crate::live::call(pid, &mut |request| program.answer(request), &place, &path, &bindings);
                    let value = called.map_err(|call| format!("{read}; as a call: {call}"))?;
                    return Ok(json!({ "result": value, "variablesReference": 0 }));
                }
                Err(read) => return Err(read),
            };
            let mut result = json!({ "result": value["value"], "type": value["type"], "variablesReference": 0 });
            if let Some(path) = value["path"].as_str() {
                opened(program, task, index, path, &value, &mut result);
            }
            Ok(result)
        }
        // Tondo's own: a live build's code as its files are now.
        "reload" => {
            let pid = program.pid;
            match crate::live::reload(pid, &mut |request| program.answer(request)) {
                Ok(reloaded) => Ok(reloaded.json()),
                Err(crate::live::Refused::Failed(message)) => Err(message),
                // Not done, which the editor shows apart from a failure.
                Err(refused) => Ok(refused.json()),
            }
        }
        "disconnect" | "terminate" => Ok(json!({})),
        other => Err(format!("tondo dap does not do `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_calls_of_a_line() {
        let line = r#"    let total = plus(twice(1), twice[Int](2)) + [1, 2].map((x) { x * 3 }).length() // keep(1)"#;
        assert_eq!(super::calls(line), ["plus", "twice", "map", "length"]);
        let line = r#"    console.println("{sum(items)} \"f(x)\" {json.encode(p)}")"#;
        assert_eq!(super::calls(line), ["console.println", "sum", "json.encode"]);
        assert_eq!(super::calls("    if ready(x) and not done() {"), ["ready", "done"]);
    }
}
