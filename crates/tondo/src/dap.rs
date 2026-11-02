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
}

/// A frame's id: its task, and its place in the task's stack.
fn frame_id(task: u64, index: u64) -> u64 {
    task * 10_000 + index
}

fn frame_of(id: u64) -> (u64, u64) {
    (id / 10_000, id % 10_000)
}

impl Debuggee {
    /// A reference to what a path names in a frame, for `variables`.
    fn handle(&mut self, task: u64, frame: u64, path: String) -> u64 {
        self.handles.push((task, frame, path));
        self.handles.len() as u64
    }

    fn ask(&mut self, command: &str) -> Result<Value, String> {
        let answer = self.answer(command)?;
        match answer["error"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(answer),
        }
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
                "supportsFunctionBreakpoints": true,
                "supportsDataBreakpoints": true,
                "exceptionBreakpointFilters": [
                    { "filter": "panic", "label": "Panics", "description": "Stop where a task panics and no recover stops it", "default": true },
                    { "filter": "error", "label": "Errors", "description": "Stop where an error starts: a fail, or a ? on what the standard library failed with", "default": false }
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
        if ending {
            if let Some(mut program) = debuggee.take() {
                let _ = program.writer.shutdown(std::net::Shutdown::Both);
                if let Some(child) = program.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
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

/// Starts the program the editor names, or attaches to a running one.
fn start(command: &str, arguments: &Value, builds: &Builds, editor: &Editor) -> Result<Debuggee, String> {
    let root =
        arguments["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let (program, child, tested) = if command == "launch" {
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
        let arguments: Vec<String> =
            arguments["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
        let mut command = std::process::Command::new(&executable);
        if let Some(plan) = &plan {
            command.env("TONDO_TEST_PLAN", plan);
            command.envs(environment);
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
        (observe::connect(&name)?, None, None)
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
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(value) = serde_json::from_str::<Value>(&line.unwrap_or_default()) else { break };
            if let Some(stop) = value.get("stopped") {
                if stop["why"] == "start" && !entry {
                    continue;
                }
                let why = stop["why"].as_str().unwrap_or_default();
                let reason = match why {
                    "start" => "entry",
                    "step" => "step",
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
            } else if let Some(id) = value.get("unwatched") {
                let why = value["why"].as_str().unwrap_or_default();
                let breakpoint = json!({ "id": id, "verified": false, "message": format!("the watch ended: {why}") });
                events.event("breakpoint", json!({ "reason": "changed", "breakpoint": breakpoint }));
            } else if value.get("exited").is_some() {
                let (count, ended) = &*read_out;
                let count = count.lock().unwrap_or_else(|e| e.into_inner());
                let wait = std::time::Duration::from_secs(2);
                drop(ended.wait_timeout_while(count, wait, |count| *count < streams));
                events.event("terminated", json!({}));
            } else if answer.send(value).is_err() {
                break;
            }
        }
    });
    let mut child = child;
    if let Some(child) = child.as_mut() {
        // The program's output goes to the editor's console.
        for (stream, category) in [
            (child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stdout"),
            (child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stderr"),
        ] {
            let Some(stream) = stream else { continue };
            let editor = editor_events.clone();
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
    writeln!(writer, "debug").map_err(|e| e.to_string())?;
    let mut debuggee = Debuggee { pid, writer, answers, child, root, handles: Vec::new() };
    let attached = debuggee.answers.recv().map_err(|_| "the program ended".to_string())?;
    if let Some(error) = attached["error"].as_str() {
        return Err(format!("{error}: a program is debugged in a build with --debug"));
    }
    // Steps go into the standard library's code too.
    if arguments["justMyCode"] == false {
        debuggee.ask("steps all")?;
    }
    Ok(debuggee)
}

/// A breakpoint's condition, count of hits and message, as `break` takes
/// them.
fn options(wanted: &Value) -> String {
    let condition = wanted["condition"].as_str().map_or(String::new(), |c| format!(" if {c}"));
    let hits = wanted["hitCondition"].as_str().map_or(String::new(), |h| format!(" hits {h}"));
    let log = wanted["logMessage"].as_str().map_or(String::new(), |m| format!(" log {m}"));
    format!("{condition}{hits}{log}")
}

/// Gives a variable with parts the reference that opens them, and an
/// array's or a set's the count of its items, which the editor asks for in
/// pages.
fn opened(program: &mut Debuggee, task: u64, index: u64, path: &str, answer: &Value, variable: &mut Value) {
    let parts = answer["children"].as_u64().unwrap_or(0);
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
                match program.ask(&format!("break {path}:{line}{}", options(wanted))) {
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
                match program.ask(&format!("break fn {name}{}", options(wanted))) {
                    Ok(answer) => {
                        let at = &answer["breakpoint"];
                        ids.push(at["id"].as_u64().unwrap_or(0));
                        let file = at["file"].as_str().unwrap_or_default();
                        let source = json!({ "name": file, "path": program.root.join(file).display().to_string() });
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
                    .map(|(task, index, path)| match path.is_empty() || path.starts_with(OUTER) {
                        true => (task, index, name.to_string()),
                        false if name.starts_with('[') => (task, index, format!("{path}{name}")),
                        false => (task, index, format!("{path}.{name}")),
                    }),
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
            let wanted: Vec<String> = arguments["breakpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| b["dataId"].as_str().map(String::from))
                .collect();
            let known: Vec<String> = breakpoints.keys().filter(|k| k.starts_with(DATA)).cloned().collect();
            for key in known.into_iter().filter(|key| !wanted.iter().any(|w| key[DATA.len()..] == *w)) {
                for id in breakpoints.remove(&key).unwrap_or_default() {
                    program.ask(&format!("delete {id}"))?;
                }
            }
            let mut set = Vec::new();
            for data in wanted {
                let key = format!("{DATA}{data}");
                if let Some(&id) = breakpoints.get(&key).and_then(|ids| ids.first()) {
                    set.push(json!({ "verified": true, "id": id }));
                    continue;
                }
                let mut parts = data.splitn(3, ' ');
                let (task, index, path) = (parts.next(), parts.next(), parts.next().unwrap_or_default());
                let selected = program.ask(&format!("select {} {}", task.unwrap_or("1"), index.unwrap_or("0")));
                match selected.and_then(|_| program.ask(&format!("watch {path}"))) {
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
            program.handles.clear();
            program.ask("continue").map(|_| json!({ "allThreadsContinued": true }))
        }
        "next" | "stepIn" | "stepOut" => {
            program.handles.clear();
            let asked = match command {
                "next" => "next",
                "stepIn" => "step",
                _ => "finish",
            };
            program.ask(asked).map(|_| json!({}))
        }
        "pause" => program.ask("pause").map(|_| json!({})),
        "setExceptionBreakpoints" => {
            let filters: Vec<&str> = arguments["filters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|f| matches!(*f, "panic" | "error"))
                .collect();
            let caught = if filters.is_empty() { "none".to_string() } else { filters.join(" ") };
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
                program.handles.clear();
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
                    let path = program.root.join(file);
                    Some(json!({
                        "id": frame_id(task, index as u64), "name": function, "line": line.parse::<u64>().ok()?,
                        "column": 1, "source": { "name": file, "path": path.display().to_string() },
                    }))
                })
                .collect();
            Ok(json!({ "stackFrames": frames, "totalFrames": frames.len() }))
        }
        "scopes" => {
            let (task, index) = frame_of(arguments["frameId"].as_u64().unwrap_or(10_000));
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
        // A binding of a frame where its task stopped, changed.
        "setVariable" => {
            let reference = arguments["variablesReference"].as_u64().unwrap_or(0) as usize;
            let Some((task, index, path)) = reference.checked_sub(1).and_then(|r| program.handles.get(r).cloned())
            else {
                return Err("no such variables".into());
            };
            if !path.is_empty() {
                return Err("a whole binding changes, not a part of it".into());
            }
            program.ask(&format!("select {task} {index}"))?;
            let name = arguments["name"].as_str().unwrap_or_default();
            let value = arguments["value"].as_str().unwrap_or_default();
            let set = program.ask(&format!("set {name} {value}"))?;
            Ok(json!({ "value": set["value"], "type": set["type"], "variablesReference": 0 }))
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
            let value = program.ask(&format!("print {path}"))?;
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
