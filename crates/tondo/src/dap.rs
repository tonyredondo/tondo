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
}

impl Debuggee {
    fn ask(&mut self, command: &str) -> Result<Value, String> {
        writeln!(self.writer, "{command}").map_err(|_| "the program ended".to_string())?;
        let answer = self.answers.recv().map_err(|_| "the program ended".to_string())?;
        match answer["error"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(answer),
        }
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
fn test_frame(header: &str, stream: &mut impl BufRead, tested: Option<u32>) -> Option<String> {
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
    let mut breakpoints: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    while let Some(request) = read_message(&mut input) {
        let arguments = &request["arguments"];
        let command = request["command"].as_str().unwrap_or_default();
        let result: Result<Value, String> = match command {
            "initialize" => Ok(json!({
                "supportsConfigurationDoneRequest": true,
                "supportsConditionalBreakpoints": true,
                "supportsTerminateRequest": true,
                "supportsEvaluateForHovers": true,
                "supportsRestartFrame": true,
            })),
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
        let arguments: Vec<String> =
            arguments["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
        let mut command = std::process::Command::new(&executable);
        if let Some(plan) = &plan {
            command.env("TONDO_TEST_PLAN", plan);
        }
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
                let reason = match stop["why"].as_str().unwrap_or_default() {
                    "start" => "entry",
                    "step" => "step",
                    "pause" => "pause",
                    _ => "breakpoint",
                };
                events
                    .event("stopped", json!({ "reason": reason, "threadId": stop["task"], "allThreadsStopped": true }));
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
    let debuggee = Debuggee { pid, writer, answers, child, root };
    let attached = debuggee.answers.recv().map_err(|_| "the program ended".to_string())?;
    if let Some(error) = attached["error"].as_str() {
        return Err(format!("{error}: a program is debugged in a build with --debug"));
    }
    Ok(debuggee)
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
            for wanted in arguments["breakpoints"].as_array().into_iter().flatten() {
                let line = wanted["line"].as_u64().unwrap_or(0);
                let condition = wanted["condition"].as_str().map_or(String::new(), |c| format!(" if {c}"));
                match program.ask(&format!("break {path}:{line}{condition}")) {
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
        "configurationDone" | "continue" => program.ask("continue").map(|_| json!({ "allThreadsContinued": true })),
        "next" => program.ask("next").map(|_| json!({})),
        "stepIn" => program.ask("step").map(|_| json!({})),
        "stepOut" => program.ask("finish").map(|_| json!({})),
        "pause" => program.ask("pause").map(|_| json!({})),
        // The frame where the task stopped, the only one that restarts.
        "restartFrame" => match arguments["frameId"].as_u64() {
            Some(0) | None => program.ask("restart").map(|_| json!({})),
            Some(_) => Err("only the frame where the task stopped restarts".into()),
        },
        "threads" => {
            let tasks = program.ask("tasks")?;
            let mut threads: Vec<Value> = tasks["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| json!({ "id": t["task"], "name": format!("task {}", t["task"]) }))
                .collect();
            if threads.is_empty() {
                threads.push(json!({ "id": 1, "name": "task 1" }));
            }
            Ok(json!({ "threads": threads }))
        }
        "stackTrace" => {
            let stack = program.ask("stack")?;
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
                        "id": index, "name": function, "line": line.parse::<u64>().ok()?, "column": 1,
                        "source": { "name": file, "path": path.display().to_string() },
                    }))
                })
                .collect();
            Ok(json!({ "stackFrames": frames, "totalFrames": frames.len() }))
        }
        // The debugger shows the bindings of the frame where the task
        // stopped.
        "scopes" => Ok(json!({ "scopes": [{ "name": "Locals", "variablesReference": 1, "expensive": false }] })),
        "variables" => {
            let locals = program.ask("locals")?;
            let variables: Vec<Value> = locals["locals"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| json!({ "name": l["name"], "value": l["value"], "type": l["type"], "variablesReference": 0 }))
                .collect();
            Ok(json!({ "variables": variables }))
        }
        "evaluate" => {
            let name = arguments["expression"].as_str().unwrap_or_default().trim();
            let value = program.ask(&format!("print {name}"))?;
            Ok(json!({ "result": value["value"], "type": value["type"], "variablesReference": 0 }))
        }
        // Tondo's own: a live build's code as its files are now.
        "reload" => {
            let pid = program.pid;
            match crate::live::reload(pid, &mut |request| program.ask(request)) {
                Ok(reloaded) => Ok(reloaded.json()),
                Err(refused) => Err(refused.message()),
            }
        }
        "disconnect" | "terminate" => Ok(json!({})),
        other => Err(format!("tondo dap does not do `{other}`")),
    }
}
