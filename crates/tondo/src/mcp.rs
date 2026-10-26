//! `tondo mcp` (toolchain 7.9): the toolchain for agents, as a Model
//! Context Protocol server over standard input and output, a message a
//! line. Its tools do what the commands of `tondo` and the language server
//! do, with structured results; the guide, the skill and the standard
//! modules' APIs are its resources. It speaks both eras of the protocol:
//! 2026-07-28's, whose requests each name their version, and the
//! `initialize` handshake of 2025-11-25 and before.

use crate::diag::Severity;
use crate::driver::{self, Analysis};
use crate::lsp::Language;
use crate::source::{SourceFile, SourceMap, Span};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MODERN: &str = "2026-07-28";
const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// What the tools, resources and instructions of this `tondo` stay valid
/// for: as long as the binary does not change.
const TTL_MS: u64 = 3_600_000;

/// The most of a command's output or error stream a result carries.
const LIMIT: usize = 256 * 1024;

const INSTRUCTIONS: &str = "Tondo's toolchain. Tondo is a new language: read the guide before writing it \
(the `guide` tool, or the tondo://guide resources), and `api` for what a module offers. After each edit, \
`check` the program: each diagnostic says how to repair it, and `fix` applies the compiler's safe fixes. \
`run` and `test` build and run it; `debug_start` debugs a build. A program that keeps running is a \
`run_start` session, whose code `reload` changes after an edit without starting it again. Paths are relative to \
the server's directory; lines and columns count from 1, columns in characters.";

pub fn serve() -> Result<(), String> {
    // A session whose debugger ended fails to write, instead of ending the
    // server.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    let mut server = Server::default();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => server.handle(&message),
            Err(e) => Some(failure(Value::Null, -32700, &format!("not JSON: {e}"), Value::Null)),
        };
        if let Some(reply) = reply {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
                break;
            }
        }
    }
    for (_, session) in server.sessions.drain() {
        session.stop();
    }
    for (_, mut run) in server.runs.drain() {
        run.stop();
    }
    Ok(())
}

fn failure(id: Value, code: i64, message: &str, data: Value) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if !data.is_null() {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

#[derive(Default)]
struct Server {
    sessions: HashMap<String, Session>,
    runs: HashMap<String, Run>,
    started: u32,
}

impl Server {
    /// The reply to a message: none to a notification or a response.
    fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let Some(method) = message["method"].as_str() else {
            return (message.get("result").is_none() && message.get("error").is_none())
                .then(|| failure(id, -32600, "a request names its method", Value::Null));
        };
        let params = &message["params"];
        let modern = match params["_meta"]["io.modelcontextprotocol/protocolVersion"].as_str() {
            None => method == "server/discover",
            Some(MODERN) => true,
            Some(version) if LEGACY.contains(&version) => false,
            Some(version) => {
                let data = json!({ "supported": supported(), "requested": version });
                return Some(failure(id, -32022, "Unsupported protocol version", data));
            }
        };
        let capabilities = json!({ "tools": {}, "resources": {} });
        let info = json!({ "name": "tondo", "version": driver::VERSION });
        let result = match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = LEGACY.iter().find(|v| **v == asked).unwrap_or(&LEGACY[0]);
                Ok(json!({ "protocolVersion": version, "capabilities": capabilities, "serverInfo": info,
                    "instructions": INSTRUCTIONS }))
            }
            "ping" => Ok(json!({})),
            "server/discover" => Ok(json!({ "supportedVersions": supported(), "capabilities": capabilities,
                "_meta": { "io.modelcontextprotocol/serverInfo": info }, "instructions": INSTRUCTIONS })),
            "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(Tool::listed).collect::<Vec<_>>() })),
            "tools/call" => self.call(params),
            "resources/list" => Ok(json!({ "resources": resources() })),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "resources/read" => {
                let uri = params["uri"].as_str().unwrap_or_default();
                match resource(uri) {
                    Some((mime, text)) => Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] })),
                    None => {
                        let code = if modern { -32602 } else { -32002 };
                        let message = format!("no resource `{uri}`: resources/list names them");
                        return Some(failure(id, code, &message, json!({ "uri": uri })));
                    }
                }
            }
            _ => Err((-32601, format!("`{method}` is not a method of tondo's MCP server"))),
        };
        Some(match result {
            Ok(mut result) => {
                if modern {
                    result["resultType"] = json!("complete");
                    if matches!(method, "server/discover" | "tools/list" | "resources/list" | "resources/read") {
                        result["ttlMs"] = json!(TTL_MS);
                        result["cacheScope"] = json!("public");
                    }
                }
                json!({ "jsonrpc": "2.0", "id": id, "result": result })
            }
            Err((code, message)) => failure(id, code, &message, Value::Null),
        })
    }

    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default();
        if !TOOLS.iter().any(|tool| tool.name == name) {
            return Err((-32602, format!("unknown tool `{name}`: tools/list names them")));
        }
        let empty = json!({});
        let args = Args(params.get("arguments").filter(|a| a.is_object()).unwrap_or(&empty));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tool(name, &args)));
        let reply = match outcome {
            Ok(Ok(reply)) => reply,
            Ok(Err(message)) => Reply::error(message),
            Err(_) => Reply::error(format!("tondo failed inside `{name}`; its error output says where")),
        };
        Ok(json!({
            "content": [{ "type": "text", "text": reply.text }],
            "structuredContent": reply.structured,
            "isError": reply.error,
        }))
    }

    fn tool(&mut self, name: &str, args: &Args) -> Result<Reply, String> {
        match name {
            "check" => {
                let analysis = analyze(args)?;
                let diagnostics = diagnostics(&analysis);
                let text = report(&analysis, &diagnostics);
                Ok(Reply::new(text, json!({ "compiles": analysis.program.is_some(), "diagnostics": diagnostics })))
            }
            "fix" => fix(args),
            "graph" => {
                use crate::graph::View;
                let view = match args.text("view").as_deref().unwrap_or("modules") {
                    "modules" => View::Modules,
                    "packages" => View::Packages,
                    "files" => View::Files,
                    other => return Err(format!("no view `{other}`: modules, packages or files")),
                };
                let mut graph = crate::graph::graph(&args.path())?;
                if let Some(module) = args.text("module").or(graph.focus.clone())
                    && view != View::Packages
                {
                    graph.around(&module, args.flag("reverse"))?;
                }
                Ok(Reply::new(graph.text(view), graph.json(view == View::Packages)))
            }
            "definition" | "references" | "hover" | "complete" | "rename" | "symbols" | "implementations" | "calls" => {
                language(name, args)
            }
            "explain" => {
                let code = args.need("code")?;
                let explained = crate::explain::explain(&code)
                    .ok_or(format!("there is no code `{code}`: E, W and P and four digits"))?;
                Ok(Reply::new(crate::explain::text(&explained), explained))
            }
            "new_project" => {
                let path = PathBuf::from(args.need("path")?);
                let init = args.flag("init");
                let name = match args.text("name") {
                    Some(name) => name,
                    None => {
                        let whole = if init { path.canonicalize().unwrap_or(path.clone()) } else { path.clone() };
                        whole.file_name().unwrap_or_default().to_string_lossy().replace('-', "_")
                    }
                };
                let written = crate::manage::create(&path, &name, !init)?;
                let text = format!("made the project `{name}` in {}: `run` runs it, `test` tests it", path.display());
                Ok(Reply::new(text, json!({ "project": path, "name": name, "files": written })))
            }
            "debug_start" => self.debug_start(args),
            "debug" => self.debug(args),
            "run_start" => self.run_start(args),
            "run_output" | "run_input" | "run_stop" => {
                let id = args.need("session")?;
                let run = self.runs.get_mut(&id).ok_or_else(|| format!("no session `{id}`: `run_start` starts one"))?;
                match name {
                    "run_input" => {
                        let text = args.text("text").unwrap_or_default();
                        let stdin = run.child.stdin.as_mut().ok_or("the session's input is closed")?;
                        stdin
                            .write_all(text.as_bytes())
                            .and_then(|()| stdin.flush())
                            .map_err(|_| "the program ended")?;
                        if args.flag("close") {
                            drop(run.child.stdin.take());
                        }
                        let wait = Duration::from_secs_f64(args.number("wait").unwrap_or(0.5).clamp(0.0, 600.0));
                        Ok(run.heard(&id, wait))
                    }
                    "run_stop" => {
                        let mut run = self.runs.remove(&id).expect("a session");
                        run.stop();
                        Ok(run.heard(&id, Duration::ZERO))
                    }
                    _ => {
                        let wait = Duration::from_secs_f64(args.number("wait").unwrap_or(1.0).clamp(0.0, 600.0));
                        Ok(run.heard(&id, wait))
                    }
                }
            }
            "reload" => {
                let program = match (args.text("session"), args.text("program")) {
                    (Some(id), _) => {
                        let run =
                            self.runs.get(&id).ok_or_else(|| format!("no session `{id}`: `run_start` starts one"))?;
                        if run.watch {
                            return Err(format!("{id} watches its files and reloads itself"));
                        }
                        run.child.id().to_string()
                    }
                    (None, Some(program)) => program,
                    (None, None) => return Err("give `session`, or `program`: a running live build".into()),
                };
                match crate::live::reload_named(&program) {
                    Ok(reloaded) => Ok(Reply::new(reloaded.summary(), reloaded.json())),
                    Err(refused) => Err(refused.message()),
                }
            }
            "debug_stop" => {
                let id = args.need("session")?;
                let session = self.sessions.remove(&id).ok_or_else(|| unknown(&id))?;
                let output = session.stop();
                Ok(Reply::new(format!("session {id} ended\n{output}"), json!({ "session": id, "output": output })))
            }
            _ => command(name, args),
        }
    }

    fn run_start(&mut self, args: &Args) -> Result<Reply, String> {
        use std::os::unix::process::CommandExt;
        let mut argv = vec!["run".to_string(), args.path().display().to_string()];
        argv.push(if args.flag("watch") { "--watch" } else { "--live" }.into());
        let program_args = args.list("args");
        if !program_args.is_empty() {
            argv.push("--".into());
            argv.extend(program_args);
        }
        let mut child = Command::new(tondo()?)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot run tondo {}: {e}", argv.join(" ")))?;
        let gather = |stream: Box<dyn Read + Send>| {
            let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let shared = buffer.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut chunk = [0u8; 8192];
                while let Ok(count) = stream.read(&mut chunk) {
                    if count == 0 {
                        break;
                    }
                    shared.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..count]);
                }
            });
            buffer
        };
        let stdout = gather(Box::new(child.stdout.take().expect("piped")));
        let stderr = gather(Box::new(child.stderr.take().expect("piped")));
        if let Some(text) = args.text("stdin") {
            let stdin = child.stdin.as_mut().expect("piped");
            let _ = stdin.write_all(text.as_bytes()).and_then(|()| stdin.flush());
        }
        self.started += 1;
        let id = format!("run-{}", self.started);
        let mut run = Run { child, stdout, stderr, read: (0, 0), watch: args.flag("watch"), status: None };
        let wait = Duration::from_secs_f64(args.number("wait").unwrap_or(2.0).clamp(0.0, 600.0));
        let reply = run.heard(&id, wait);
        self.runs.insert(id, run);
        Ok(reply)
    }

    fn debug_start(&mut self, args: &Args) -> Result<Reply, String> {
        let target = args.need("target")?;
        let mut argv = vec!["debug".to_string(), "--json".into()];
        if args.flag("live") {
            argv.push("--live".into());
        }
        argv.push(target.clone());
        let program_args = args.list("args");
        if !program_args.is_empty() {
            argv.push("--".into());
            argv.extend(program_args);
        }
        let mut child = Command::new(tondo()?)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start `tondo debug`: {e}"))?;
        let (send, lines) = mpsc::channel();
        let stdout = child.stdout.take().expect("piped");
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = drain(child.stderr.take().expect("piped"));
        let mut session = Session { child, lines, errors: Some(errors), running: false, ended: false };
        // A program it starts waits before main; one it attaches to answers.
        let started = Path::new(&target).exists();
        let heard = session.hear(
            |value| {
                value.get("stopped").is_some()
                    || value.get("error").is_some()
                    || (!started && value.get("debugging").is_some())
            },
            Duration::from_secs(600),
        );
        let failed = heard.said.iter().find_map(|value| value["error"].as_str().map(String::from));
        if session.ended || failed.is_some() {
            let mut message = failed.unwrap_or_default();
            message.push_str(&session.stop());
            return Err(match message.trim().is_empty() {
                true => format!("`tondo debug {target}` ended before debugging it"),
                false => message.trim().to_string(),
            });
        }
        self.started += 1;
        let id = format!("debug-{}", self.started);
        let reply = heard.reply(&id, &session);
        self.sessions.insert(id, session);
        Ok(reply)
    }

    fn debug(&mut self, args: &Args) -> Result<Reply, String> {
        let id = args.need("session")?;
        let command = args.need("command")?.trim().to_string();
        let wait = Duration::from_secs_f64(args.number("wait").unwrap_or(10.0).clamp(0.0, 3600.0));
        let session = self.sessions.get_mut(&id).ok_or_else(|| unknown(&id))?;
        if session.ended {
            self.sessions.remove(&id).map(Session::stop);
            return Err(format!("the program of {id} ended: `debug_start` debugs it again"));
        }
        let word = command.split_whitespace().next().unwrap_or_default();
        let event = |value: &Value| value.get("stopped").is_some() || value.get("exited").is_some();
        let heard = match (session.running, word) {
            (_, "help" | "h") => {
                return Ok(Reply::new(crate::debug::HELP.to_string(), json!({ "help": crate::debug::HELP })));
            }
            (_, "quit" | "q" | "exit") => {
                let output = self.sessions.remove(&id).map(Session::stop).unwrap_or_default();
                return Ok(Reply::new(
                    format!("session {id} ended\n{output}"),
                    json!({ "session": id, "output": output }),
                ));
            }
            // The debugger waits for the program to stop, and pauses it on
            // an interrupt, as on Ctrl-C.
            (true, "pause") => {
                unsafe { libc::kill(session.child.id() as i32, libc::SIGINT) };
                session.hear(event, wait)
            }
            (true, "wait") => session.hear(event, wait),
            (true, _) => return Err("the program runs: `pause` stops it, and `wait` waits for it to stop".into()),
            (false, "wait") => {
                return Err("the program is stopped: `continue`, `step`, `next` or `finish` runs it".into());
            }
            (false, _) => {
                let stdin = session.child.stdin.as_mut().expect("piped");
                if writeln!(stdin, "{command}").and_then(|()| stdin.flush()).is_err() {
                    session.ended = true;
                    return Err(format!("the program of {id} ended: `debug_start` debugs it again"));
                }
                let mut heard = session.hear(|value| !event(value), wait.max(Duration::from_secs(10)));
                if heard.said.last().is_some_and(|answer| answer["running"] == true) {
                    let more = session.hear(event, wait);
                    heard.said.extend(more.said);
                    heard.output.push_str(&more.output);
                    heard.met = more.met;
                } else {
                    heard.met = true;
                }
                heard
            }
        };
        session.running = !heard.met && !session.ended;
        Ok(heard.reply(&id, session))
    }
}

fn supported() -> Vec<&'static str> {
    std::iter::once(MODERN).chain(LEGACY).collect()
}

fn unknown(id: &str) -> String {
    format!("no debugging session `{id}`: it ended, or `debug_start` never gave it")
}

/// The path to this `tondo`, which the tools run as their commands.
fn tondo() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot find tondo itself: {e}"))
}

/// A tool's result: text for the model, and the same as data.
struct Reply {
    text: String,
    structured: Value,
    error: bool,
}

impl Reply {
    fn new(text: String, structured: Value) -> Reply {
        Reply { text, structured, error: false }
    }

    fn error(message: String) -> Reply {
        Reply { structured: json!({ "error": message }), text: message, error: true }
    }
}

/// A tool's arguments.
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn text(&self, key: &str) -> Option<String> {
        match &self.0[key] {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    fn need(&self, key: &str) -> Result<String, String> {
        self.text(key).ok_or_else(|| format!("give `{key}`: the tool's input schema says what it is"))
    }

    fn flag(&self, key: &str) -> bool {
        self.0[key].as_bool().unwrap_or(false)
    }

    fn list(&self, key: &str) -> Vec<String> {
        match &self.0[key] {
            Value::Array(items) => items.iter().filter_map(|item| Args(item).text_of()).collect(),
            Value::String(text) => vec![text.clone()],
            _ => Vec::new(),
        }
    }

    fn text_of(&self) -> Option<String> {
        match self.0 {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    fn number(&self, key: &str) -> Option<f64> {
        match &self.0[key] {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        }
    }

    /// A line or column, which counts from 1.
    fn count(&self, key: &str) -> Result<u64, String> {
        match self.number(key) {
            Some(n) if n >= 1.0 => Ok(n as u64),
            _ => Err(format!("give `{key}` counting from 1")),
        }
    }

    /// The program: a .to file or a project directory, this one by default.
    fn path(&self) -> PathBuf {
        PathBuf::from(self.text("path").unwrap_or_else(|| ".".into()))
    }
}

fn analyze(args: &Args) -> Result<Analysis, String> {
    match args.flag("library") {
        true => driver::analyze_library(&args.path()),
        false => driver::check(&args.path()),
    }
}

/// Where a span is: its file and its first and last line and column.
fn place(sources: &SourceMap, span: Span) -> Value {
    let file = sources.file_of(span.start);
    let (line, column) = file.line_col(span.start);
    let (end_line, end_column) = file.line_col(span.end.max(span.start));
    json!({ "file": shown(file), "line": line, "column": column, "end_line": end_line, "end_column": end_column })
}

fn shown(file: &SourceFile) -> String {
    match file.logical.starts_with('<') {
        true => file.logical.clone(),
        false => file.path.display().to_string(),
    }
}

/// The diagnostics of an analysis, with the ids `tondo check --json` gives
/// them.
fn diagnostics(analysis: &Analysis) -> Vec<Value> {
    let rendered: Vec<Value> =
        serde_json::from_str(&analysis.diags.render_json(&analysis.sources, &analysis.source_id)).unwrap_or_default();
    let sources = &analysis.sources;
    analysis
        .diags
        .list
        .iter()
        .zip(rendered)
        .map(|(diagnostic, rendered)| {
            let mut item = json!({
                "id": rendered["id"],
                "severity": if diagnostic.severity == Severity::Error { "error" } else { "warning" },
                "code": rendered["code"],
                "message": diagnostic.message,
            });
            if let Some(span) = diagnostic.span {
                item["at"] = place(sources, span);
            }
            let fixes = diagnostic.fixes.iter().map(|fix| {
                let edits = fix.edits.iter().map(|(span, replacement)| {
                    let mut edit = place(sources, *span);
                    edit["replacement"] = json!(replacement);
                    edit
                });
                json!({ "title": fix.title, "safe": fix.safe, "edits": edits.collect::<Vec<_>>() })
            });
            item["fixes"] = json!(fixes.collect::<Vec<_>>());
            item
        })
        .collect()
}

/// The diagnostics as `tondo check` prints them, and the fixes each offers
/// by the id `fix` takes.
fn report(analysis: &Analysis, diagnostics: &[Value]) -> String {
    let mut text = analysis.diags.render(&analysis.sources);
    for diagnostic in diagnostics {
        for (index, fix) in diagnostic["fixes"].as_array().into_iter().flatten().enumerate() {
            let kind = if fix["safe"] == true { "safe" } else { "needs a decision" };
            let title = fix["title"].as_str().unwrap_or_default();
            text.push_str(&format!("fix {{\"id\": {}, \"fix\": {index}}}: {title} ({kind})\n", diagnostic["id"]));
        }
    }
    match (text.is_empty(), analysis.program.is_some()) {
        (true, true) => "no diagnostics: it compiles".into(),
        (true, false) => "it does not compile".into(),
        (false, _) => text,
    }
}

/// Applies the fixes the arguments choose, or every safe one, to the
/// program's own files, and checks it again.
fn fix(args: &Args) -> Result<Reply, String> {
    let analysis = analyze(args)?;
    let ids: Vec<Value> = diagnostics(&analysis).into_iter().map(|d| d["id"].clone()).collect();
    let list = &analysis.diags.list;
    let chosen: Vec<(usize, usize)> = match args.0["apply"].as_array() {
        Some(apply) => apply
            .iter()
            .map(|choice| {
                let id = choice["id"].as_str().unwrap_or_default();
                let index = ids
                    .iter()
                    .position(|known| known == id)
                    .ok_or_else(|| format!("no diagnostic has the id `{id}` now: `check` gives the current ones"))?;
                let fix = choice["fix"].as_u64().unwrap_or(0) as usize;
                match fix < list[index].fixes.len() {
                    true => Ok((index, fix)),
                    false => Err(format!("diagnostic {id} has {} fixes, counted from 0", list[index].fixes.len())),
                }
            })
            .collect::<Result<_, String>>()?,
        None => {
            list.iter().enumerate().filter_map(|(i, d)| d.fixes.iter().position(|f| f.safe).map(|f| (i, f))).collect()
        }
    };
    let root = args.path().canonicalize().map_err(|e| format!("cannot read {}: {e}", args.path().display()))?;
    let sources = &analysis.sources;
    let own = |file: &SourceFile| file.path.canonicalize().is_ok_and(|path| path.starts_with(&root));
    let (mut applied, mut skipped) = (Vec::new(), Vec::new());
    let mut edits: Vec<(Span, &str)> = Vec::new();
    for (index, fix) in chosen {
        let fix = &list[index].fixes[fix];
        let entry = json!({ "id": ids[index], "title": fix.title });
        let clashes = |span: &Span| {
            edits.iter().any(|(e, _)| e.start == span.start || (e.start < span.end && span.start < e.end))
        };
        if fix.edits.iter().any(|(span, _)| !own(sources.file_of(span.start))) {
            skipped.push(json!({ "fix": entry, "why": "it edits a file outside the program" }));
        } else if fix.edits.iter().any(|(span, _)| clashes(span)) {
            skipped.push(json!({ "fix": entry, "why": "it overlaps another fix: `fix` again applies it" }));
        } else {
            edits.extend(fix.edits.iter().map(|(span, text)| (*span, text.as_str())));
            applied.push(entry);
        }
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut changed: Vec<(u32, String)> = Vec::new();
    for (span, replacement) in edits {
        let file = sources.file_of(span.start);
        if changed.last().is_none_or(|(base, _)| *base != file.base) {
            changed.push((file.base, file.text.clone()));
        }
        let text = &mut changed.last_mut().expect("pushed").1;
        text.replace_range(
            (span.start - file.base) as usize..(span.end.max(span.start) - file.base) as usize,
            replacement,
        );
    }
    for (base, text) in &changed {
        let file = sources.file_of(*base);
        std::fs::write(&file.path, text).map_err(|e| format!("cannot write {}: {e}", file.path.display()))?;
    }
    let after = analyze(args)?;
    let diagnostics = diagnostics(&after);
    let mut text = format!("fixes applied: {}", applied.len());
    for (entry, why) in skipped.iter().map(|s| (&s["fix"], &s["why"])) {
        text.push_str(&format!("; skipped {}: {}", entry["title"], why.as_str().unwrap_or_default()));
    }
    text.push_str(&format!("\n{}", report(&after, &diagnostics)));
    let structured = json!({ "applied": applied, "skipped": skipped, "compiles": after.program.is_some(),
        "diagnostics": diagnostics });
    Ok(Reply::new(text, structured))
}

/// `tondo fix`: the safe repairs of a program applied to its files, as the
/// tool `fix` applies them: what it says, and its data.
pub fn fix_files(path: &Path, library: bool) -> Result<(String, Value), String> {
    let arguments = json!({ "path": path, "library": library });
    let reply = fix(&Args(&arguments))?;
    Ok((reply.text, reply.structured))
}

/// The language server's answer at a place of a file.
fn language(name: &str, args: &Args) -> Result<Reply, String> {
    let file = args.need("file")?;
    let path = Path::new(&file).canonicalize().map_err(|e| format!("cannot read {file}: {e}"))?;
    let document = json!({ "uri": crate::lsp::file_uri(&path) });
    let mut server = Language::default();
    if name == "symbols" {
        let mut symbols = server.ask("textDocument/documentSymbol", &json!({ "textDocument": document }))?;
        if let Some(query) = args.text("query") {
            symbols = server.ask("workspace/symbol", &json!({ "query": query }))?;
        }
        let symbols: Vec<Value> = symbols.as_array().into_iter().flatten().map(symbol).collect();
        let mut text = String::new();
        for symbol in &symbols {
            outline(symbol, 0, &mut text);
        }
        if text.is_empty() {
            text = "no declarations found".into();
        }
        return Ok(Reply::new(text, json!({ "symbols": symbols })));
    }
    let position = json!({ "line": args.count("line")? - 1, "character": args.count("column")? - 1 });
    let at = json!({ "textDocument": document, "position": position });
    let mut lines = Lines::default();
    match name {
        "calls" => {
            let prepared = server.ask("textDocument/prepareCallHierarchy", &at)?;
            let Some(item) = prepared.get(0).cloned() else {
                return Ok(Reply::new(
                    "no function is declared at that line and column".into(),
                    json!({ "calls": [] }),
                ));
            };
            let incoming = args.text("direction").as_deref() != Some("outgoing");
            let method = if incoming { "callHierarchy/incomingCalls" } else { "callHierarchy/outgoingCalls" };
            let answer = server.ask(method, &json!({ "item": item }))?;
            let key = if incoming { "from" } else { "to" };
            let calls: Vec<Value> = answer
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|call| {
                    let other = &call[key];
                    let place = lines.place(&json!({ "uri": other["uri"], "range": other["selectionRange"] }))?;
                    let times = call["fromRanges"].as_array().map_or(0, Vec::len);
                    Some(json!({ "function": other["name"], "place": place, "times": times }))
                })
                .collect();
            let said = if incoming { "is called by" } else { "calls" };
            let mut text = format!("{} {said}:\n", item["name"].as_str().unwrap_or_default());
            for call in &calls {
                let place = &call["place"];
                text.push_str(&format!(
                    "  {} at {}:{} ({} times)\n",
                    call["function"].as_str().unwrap_or_default(),
                    place["file"].as_str().unwrap_or_default(),
                    place["line"],
                    call["times"]
                ));
            }
            if calls.is_empty() {
                text.push_str("  nothing\n");
            }
            Ok(Reply::new(text, json!({ "calls": calls })))
        }
        "definition" | "references" | "implementations" => {
            let method = match name {
                "implementations" => "textDocument/implementation".to_string(),
                _ => format!("textDocument/{name}"),
            };
            let params =
                json!({ "textDocument": document, "position": position, "context": { "includeDeclaration": true } });
            let answer = server.ask(&method, &params)?;
            let locations = match answer {
                Value::Array(list) => list,
                Value::Null => Vec::new(),
                one => vec![one],
            };
            let places: Vec<Value> = locations.iter().filter_map(|l| lines.place(l)).collect();
            let text = match places.is_empty() {
                true => format!("no {name} found there: is a name at that line and column?"),
                false => places
                    .iter()
                    .map(|p| {
                        format!(
                            "{}:{}:{}: {}\n",
                            p["file"].as_str().unwrap_or_default(),
                            p["line"],
                            p["column"],
                            p["text"].as_str().unwrap_or_default()
                        )
                    })
                    .collect(),
            };
            Ok(Reply::new(text, json!({ "places": places })))
        }
        "hover" => {
            let answer = server.ask("textDocument/hover", &at)?;
            let text = answer["contents"]["value"].as_str().unwrap_or("nothing is known there").to_string();
            Ok(Reply::new(text.clone(), json!({ "text": text })))
        }
        "complete" => {
            let answer = server.ask("textDocument/completion", &at)?;
            let items: Vec<Value> = answer["items"]
                .as_array()
                .into_iter()
                .flatten()
                .take(200)
                .map(|item| json!({ "label": item["label"], "detail": item["detail"] }))
                .collect();
            let mut text: String = items
                .iter()
                .map(|i| {
                    format!(
                        "{} {}\n",
                        i["label"].as_str().unwrap_or_default(),
                        i["detail"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            if text.is_empty() {
                text = "nothing to complete there".into();
            }
            Ok(Reply::new(text, json!({ "items": items })))
        }
        _ => {
            let new_name = args.need("new_name")?;
            let mut params = at;
            params["newName"] = json!(new_name);
            let answer = server.ask("textDocument/rename", &params)?;
            let Some(changes) = answer["changes"].as_object() else {
                return Err("no declaration of the program is named there".into());
            };
            let mut changed = Vec::new();
            for (uri, edits) in changes {
                let path = crate::lsp::uri_path(uri).ok_or_else(|| format!("cannot rename in {uri}"))?;
                let mut text =
                    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let mut spans: Vec<(usize, usize, &str)> = edits
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|edit| {
                        let start = offset(&text, &edit["range"]["start"])?;
                        Some((start, offset(&text, &edit["range"]["end"])?, edit["newText"].as_str()?))
                    })
                    .collect();
                spans.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
                for (start, end, replacement) in &spans {
                    text.replace_range(start..end, replacement);
                }
                std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                changed.push(json!({ "file": relative(&path), "edits": spans.len() }));
            }
            let text = changed
                .iter()
                .map(|c| format!("{}: {} edits\n", c["file"].as_str().unwrap_or_default(), c["edits"]))
                .collect();
            Ok(Reply::new(text, json!({ "changed": changed })))
        }
    }
}

/// The lines of the files places are in, read once.
#[derive(Default)]
struct Lines(HashMap<PathBuf, String>);

impl Lines {
    /// An LSP location as the tools give places: from 1, with its line.
    fn place(&mut self, location: &Value) -> Option<Value> {
        let path = crate::lsp::uri_path(location["uri"].as_str()?)?;
        let range = &location["range"];
        let line = range["start"]["line"].as_u64()?;
        let text = self.0.entry(path.clone()).or_insert_with(|| std::fs::read_to_string(&path).unwrap_or_default());
        let source = text.lines().nth(line as usize).unwrap_or_default().trim();
        Some(json!({
            "file": relative(&path),
            "line": line + 1,
            "column": range["start"]["character"].as_u64()? + 1,
            "end_line": range["end"]["line"].as_u64()? + 1,
            "end_column": range["end"]["character"].as_u64()? + 1,
            "text": source,
        }))
    }
}

/// The byte offset of an LSP position whose column counts characters.
fn offset(text: &str, position: &Value) -> Option<usize> {
    let line = position["line"].as_u64()? as usize;
    let column = position["character"].as_u64()? as usize;
    let start = match line {
        0 => 0,
        _ => text.match_indices('\n').nth(line - 1)?.0 + 1,
    };
    let rest = &text[start..];
    let rest = &rest[..rest.find('\n').unwrap_or(rest.len())];
    Some(start + rest.char_indices().nth(column).map_or(rest.len(), |(i, _)| i))
}

/// A path as given relative to the server's directory, when it is inside.
fn relative(path: &Path) -> String {
    let here = std::env::current_dir().ok().and_then(|dir| dir.canonicalize().ok());
    match here.as_deref().and_then(|here| path.strip_prefix(here).ok()) {
        Some(inside) => inside.display().to_string(),
        None => path.display().to_string(),
    }
}

/// A document or workspace symbol, as `symbols` gives it.
fn symbol(value: &Value) -> Value {
    let kind = match value["kind"].as_u64().unwrap_or(0) {
        12 => "function",
        6 => "method",
        23 => "type",
        10 => "enum",
        11 => "trait",
        14 => "const",
        8 => "field",
        22 => "variant",
        _ => "binding",
    };
    let (path, start) = match value.get("location") {
        Some(location) => {
            (crate::lsp::uri_path(location["uri"].as_str().unwrap_or_default()), &location["range"]["start"])
        }
        None => (None, &value["selectionRange"]["start"]),
    };
    let mut item = json!({
        "name": value["name"],
        "kind": kind,
        "line": start["line"].as_u64().unwrap_or(0) + 1,
        "column": start["character"].as_u64().unwrap_or(0) + 1,
    });
    if let Some(path) = path {
        item["file"] = json!(relative(&path));
    }
    if let Some(children) = value["children"].as_array().filter(|c| !c.is_empty()) {
        item["children"] = json!(children.iter().map(symbol).collect::<Vec<_>>());
    }
    item
}

fn outline(symbol: &Value, depth: usize, text: &mut String) {
    let file = symbol["file"].as_str().map(|f| format!("{f}:")).unwrap_or_default();
    text.push_str(&format!(
        "{:indent$}{} {} {file}{}:{}\n",
        "",
        symbol["kind"].as_str().unwrap_or_default(),
        symbol["name"].as_str().unwrap_or_default(),
        symbol["line"],
        symbol["column"],
        indent = depth * 2
    ));
    for child in symbol["children"].as_array().into_iter().flatten() {
        outline(child, depth + 1, text);
    }
}

/// A tool that is a command of `tondo`: its arguments, and its output as
/// the result.
fn command(name: &str, args: &Args) -> Result<Reply, String> {
    let mut argv: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| argv.extend(items.iter().map(|s| s.to_string()));
    let option = |argv: &mut Vec<String>, flag: &str, key: &str| {
        if let Some(value) = args.text(key) {
            argv.extend([flag.to_string(), value]);
        }
    };
    let switch = |argv: &mut Vec<String>, flag: &str, key: &str| {
        if args.flag(key) {
            argv.push(flag.to_string());
        }
    };
    let building = |argv: &mut Vec<String>| {
        switch(argv, "--static", "static");
        switch(argv, "--debug", "debug");
        option(argv, "--target", "target");
        for library in args.list("link") {
            argv.extend(["--link".to_string(), library]);
        }
    };
    let path = args.path().display().to_string();
    let mut timeout = None;
    // The tests' report, besides what `tondo test` prints for people, and
    // their coverage.
    let report = std::env::temp_dir().join(format!("tondo-mcp-{}-report.json", std::process::id()));
    let coverage = std::env::temp_dir().join(format!("tondo-mcp-{}-coverage.json", std::process::id()));
    match name {
        "format" => {
            push(&["fmt", "--json"]);
            switch(&mut argv, "--check", "check");
            let files = args.list("files");
            if files.is_empty() {
                return Err("give `files`: the .to files to format".into());
            }
            argv.extend(files);
        }
        "compact" => push(&["fmt", "--compact", &args.need("file")?]),
        "expand" => push(&["expand", &path, "--json"]),
        "api" => {
            push(&["api"]);
            argv.extend(args.text("module"));
            switch(&mut argv, "--private", "private");
            switch(&mut argv, "--list", "list");
            option(&mut argv, "--project", "project");
        }
        "guide" => {
            push(&["guide"]);
            argv.extend(args.text("topic"));
            switch(&mut argv, "--list", "list");
            switch(&mut argv, "--skill", "skill");
        }
        "build" => {
            push(&["build", "--json"]);
            switch(&mut argv, "--library", "library");
            argv.push(path);
            option(&mut argv, "--output", "output");
            switch(&mut argv, "--strip", "strip");
            building(&mut argv);
        }
        "run" => {
            push(&["run", &path]);
            building(&mut argv);
            let program_args = args.list("args");
            if !program_args.is_empty() {
                argv.push("--".into());
                argv.extend(program_args);
            }
            timeout = Some(args.number("timeout").unwrap_or(60.0).clamp(0.1, 86_400.0));
        }
        "test" => {
            push(&["test", &path]);
            match args.flag("list") {
                // The list is its own report.
                true => push(&["--list", "--test-format", "json"]),
                false => push(&["--report", &format!("json={}", report.display())]),
            }
            if args.flag("coverage") {
                push(&["--coverage", "--report", &format!("coverage={}", coverage.display())]);
            }
            option(&mut argv, "--filter", "filter");
            option(&mut argv, "--glob", "glob");
            option(&mut argv, "--exact", "exact");
            switch(&mut argv, "--library", "library");
            switch(&mut argv, "--update-snapshots", "update_snapshots");
            building(&mut argv);
            argv.extend(args.list("flags"));
        }
        "doc_test" => push(&["doc-test", &args.need("file")?]),
        "bind" => {
            push(&["bind", "--json"]);
            argv.extend(args.list("headers"));
            option(&mut argv, "--package", "package");
            for library in args.list("libraries") {
                argv.extend(["--library".to_string(), library]);
            }
            option(&mut argv, "--project", "project");
            option(&mut argv, "--output", "output");
            for dir in args.list("include") {
                argv.extend(["-I".to_string(), dir]);
            }
            for define in args.list("define") {
                argv.extend(["-D".to_string(), define]);
            }
        }
        "update" => {
            push(&["update", "--json"]);
            argv.extend(args.text("project"));
            argv.extend(args.list("dependencies"));
        }
        "target_pack" => {
            push(&["target", "pack", "--json"]);
            argv.extend(args.text("dir"));
        }
        "ps" => push(&["ps", "--json"]),
        "add_dependency" => {
            push(&["add", &args.need("alias")?, "--json"]);
            for key in ["path", "git", "tag", "branch", "rev", "url", "sha256", "project"] {
                option(&mut argv, &format!("--{key}"), key);
            }
            switch(&mut argv, "--dev", "dev");
        }
        "remove_dependency" => {
            push(&["remove", &args.need("alias")?, "--json"]);
            option(&mut argv, "--project", "project");
        }
        "counters" => {
            let count = args.number("count").unwrap_or(1.0).max(1.0).to_string();
            push(&["counters", &args.need("program")?, "--json", "--count", &count]);
            option(&mut argv, "--interval", "interval");
        }
        "trace" => {
            push(&["trace", "--json"]);
            argv.extend(args.text("program"));
            option(&mut argv, "--duration", "duration");
            option(&mut argv, "--output", "output");
            let started = args.list("command");
            if !started.is_empty() {
                argv.push("--".into());
                argv.extend(started);
            }
        }
        "dump" => {
            push(&["dump", &args.need("program")?, "--json"]);
            option(&mut argv, "--output", "output");
        }
        "dump_read" => {
            push(&["dump", "read", &args.need("file")?]);
            match args.text("view").as_deref().unwrap_or("summary") {
                "summary" => {}
                "task" => push(&["task", &args.need("task")?]),
                "object" => push(&["object", &args.need("address")?]),
                view @ ("tasks" | "heap" | "objects") => push(&[view]),
                other => return Err(format!("no view `{other}`: summary, tasks, task, heap, objects or object")),
            }
            push(&["--json"]);
        }
        _ => unreachable!("every tool has its command"),
    }
    let stdin = args.text("stdin").filter(|_| name == "run");
    let ran = execute(&argv, stdin, timeout.map(Duration::from_secs_f64))?;
    // What the command gives as JSON is the result's data.
    let json = !matches!(name, "compact" | "api" | "guide" | "run" | "test" | "doc_test")
        || (name == "test" && args.flag("list"));
    // The result of a program, of its tests or of a check is what it
    // reports, not a failure of the tool.
    let reported = matches!(name, "run" | "test" | "doc_test") || (name == "format" && args.flag("check"));
    let mut structured = json!({ "exit_code": ran.code, "stdout": ran.stdout, "stderr": ran.stderr });
    if ran.timed_out {
        structured["timed_out"] = json!(true);
    }
    if name == "test"
        && let Some(parsed) = std::fs::read(&report).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        structured["report"] = parsed;
        let _ = std::fs::remove_file(&report);
    }
    if name == "test"
        && let Some(parsed) =
            std::fs::read(&coverage).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        structured["coverage"] = parsed;
        let _ = std::fs::remove_file(&coverage);
    }
    if json {
        let parsed = serde_json::from_str::<Value>(&ran.stdout).ok().or_else(|| {
            ran.stdout.lines().map(serde_json::from_str::<Value>).collect::<Result<Vec<_>, _>>().ok().map(Value::from)
        });
        if let Some(parsed) = parsed.filter(|_| !ran.stdout.trim().is_empty()) {
            structured["report"] = parsed;
            structured.as_object_mut().expect("object").remove("stdout");
        }
    }
    let mut text = match (ran.stdout.trim().is_empty(), ran.stderr.trim().is_empty()) {
        (false, false) => format!("stdout:\n{}\nstderr:\n{}\n", ran.stdout.trim_end(), ran.stderr.trim_end()),
        (false, true) => format!("{}\n", ran.stdout.trim_end()),
        (true, false) => format!("{}\n", ran.stderr.trim_end()),
        (true, true) => "done, printing nothing\n".into(),
    };
    match (ran.timed_out, ran.code) {
        (true, _) => {
            text.push_str(&format!("timed out after {} seconds, and was ended\n", timeout.unwrap_or_default()))
        }
        (false, Some(0)) if name != "run" => {}
        (false, Some(code)) => text.push_str(&format!("exit code {code}\n")),
        (false, None) => text.push_str("ended by a signal\n"),
    }
    let error = !reported && (ran.timed_out || ran.code != Some(0));
    Ok(Reply { text, structured, error })
}

struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// Runs `tondo` with arguments, in a process group of its own that a
/// timeout ends whole.
fn execute(argv: &[String], stdin: Option<String>, timeout: Option<Duration>) -> Result<Ran, String> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(tondo()?)
        .args(argv)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot run tondo {}: {e}", argv.join(" ")))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || pipe.write_all(input.as_bytes()));
    }
    let stdout = drain(child.stdout.take().expect("piped"));
    let stderr = drain(child.stderr.take().expect("piped"));
    let start = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break (status, false);
        }
        if timeout.is_some_and(|limit| start.elapsed() >= limit) {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            break (child.wait().map_err(|e| e.to_string())?, true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let text = |handle: std::thread::JoinHandle<Vec<u8>>| bounded(handle.join().unwrap_or_default());
    Ok(Ran { code: status.code(), stdout: text(stdout), stderr: text(stderr), timed_out })
}

/// Reads a stream to its end on a thread of its own.
fn drain(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        bytes
    })
}

fn bounded(bytes: Vec<u8>) -> String {
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= LIMIT {
        return text.into_owned();
    }
    let cut = (0..=LIMIT).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}\n… {} more bytes", &text[..cut], text.len() - cut)
}

/// A program a session keeps running, live: what it printed, and how much
/// of it a tool read.
struct Run {
    child: Child,
    stdout: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    stderr: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    read: (usize, usize),
    watch: bool,
    status: Option<std::process::ExitStatus>,
}

impl Run {
    fn lengths(&self) -> (usize, usize) {
        let length = |buffer: &std::sync::Mutex<Vec<u8>>| buffer.lock().unwrap_or_else(|e| e.into_inner()).len();
        (length(&self.stdout), length(&self.stderr))
    }

    /// What the program printed since the last time, after waiting up to
    /// `wait` for something new, and whether it runs.
    fn heard(&mut self, id: &str, wait: Duration) -> Reply {
        let deadline = Instant::now() + wait;
        loop {
            if self.status.is_none() {
                self.status = self.child.try_wait().ok().flatten();
            }
            if self.lengths() != self.read || self.status.is_some() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // A last moment for what the program wrote as it ended.
        if self.status.is_some() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let take = |buffer: &std::sync::Mutex<Vec<u8>>, from: usize| {
            let bytes = buffer.lock().unwrap_or_else(|e| e.into_inner());
            (bounded(bytes[from.min(bytes.len())..].to_vec()), bytes.len())
        };
        let (stdout, out_end) = take(&self.stdout, self.read.0);
        let (stderr, err_end) = take(&self.stderr, self.read.1);
        self.read = (out_end, err_end);
        let code = self.status.and_then(|status| status.code());
        let mut text = match (stdout.is_empty(), stderr.is_empty()) {
            (false, false) => format!("stdout:\n{stdout}stderr:\n{stderr}"),
            (false, true) => stdout.clone(),
            (true, false) => stderr.clone(),
            (true, true) => String::new(),
        };
        match self.status {
            None => text.push_str(&format!("{id} runs\n")),
            Some(_) => {
                text.push_str(&format!("{id} ended, exit code {}\n", code.map_or("none".into(), |c| c.to_string())))
            }
        }
        let structured = json!({ "session": id, "pid": self.child.id(), "stdout": stdout, "stderr": stderr,
            "running": self.status.is_none(), "exit_code": code });
        Reply::new(text, structured)
    }

    fn stop(&mut self) {
        if self.status.is_none() {
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            self.status = self.child.wait().ok();
        }
    }
}

/// A debugging session: `tondo debug --json`, which prints the debugger's
/// answers and events as lines of JSON among the program's own output.
struct Session {
    child: Child,
    lines: mpsc::Receiver<String>,
    errors: Option<std::thread::JoinHandle<Vec<u8>>>,
    /// The program runs, and the debugger waits for it to stop.
    running: bool,
    ended: bool,
}

/// What a session said: the debugger's answers and events, and what the
/// program printed.
struct Heard {
    said: Vec<Value>,
    output: String,
    /// What was awaited came.
    met: bool,
}

impl Heard {
    fn reply(self, id: &str, session: &Session) -> Reply {
        let mut text: String = self.said.iter().map(|value| format!("{value}\n")).collect();
        if !self.output.is_empty() {
            text.push_str(&format!("the program printed:\n{}", self.output));
        }
        match (session.ended, session.running) {
            (true, _) => text.push_str("the program ended\n"),
            (false, true) => text.push_str("the program runs: `pause` stops it, and `wait` waits for it to stop\n"),
            _ => {}
        }
        let structured = json!({ "session": id, "said": self.said, "output": self.output,
            "running": session.running, "ended": session.ended });
        Reply::new(text, structured)
    }
}

impl Session {
    /// Collects what the session says until a value `until` accepts, the
    /// program ends, or `wait` passes.
    fn hear(&mut self, until: impl Fn(&Value) -> bool, wait: Duration) -> Heard {
        let deadline = Instant::now() + wait;
        let mut heard = Heard { said: Vec::new(), output: String::new(), met: false };
        loop {
            match self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => match serde_json::from_str::<Value>(&line).ok().filter(debugger) {
                    Some(value) => {
                        self.ended |= value.get("exited").is_some();
                        heard.met = until(&value);
                        heard.said.push(value);
                        if heard.met || self.ended {
                            self.running = false;
                            return heard;
                        }
                    }
                    None => {
                        heard.output.push_str(&line);
                        heard.output.push('\n');
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.running = !heard.met;
                    return heard;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.ended = true;
                    return heard;
                }
            }
        }
    }

    /// Ends the session: `quit` ends the program it started. Gives what
    /// was left to read and what the debugger said on its error stream.
    fn stop(mut self) -> String {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = writeln!(stdin, "quit");
        }
        drop(self.child.stdin.take());
        let start = Instant::now();
        while self.child.try_wait().ok().flatten().is_none() {
            if start.elapsed() > Duration::from_secs(5) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut left: String = self.lines.try_iter().map(|line| format!("{line}\n")).collect();
        if let Some(errors) = self.errors.take() {
            left.push_str(&bounded(errors.join().unwrap_or_default()));
        }
        left
    }
}

/// A line the debugger wrote, rather than the program.
fn debugger(value: &Value) -> bool {
    const KEYS: [&str; 12] = [
        "debugging",
        "running",
        "pausing",
        "stopped",
        "exited",
        "breakpoint",
        "locals",
        "value",
        "stack",
        "tasks",
        "deleted",
        "error",
    ];
    value.as_object().is_some_and(|object| KEYS.iter().any(|key| object.contains_key(*key)))
}

fn resources() -> Vec<Value> {
    let mut list =
        vec![json!({ "uri": "tondo://skill", "name": "skill", "title": "Tondo, as an agent skill (SKILL.md)",
        "mimeType": "text/markdown" })];
    for (topic, _) in crate::guide::TOPICS {
        list.push(json!({ "uri": format!("tondo://guide/{topic}"), "name": format!("guide/{topic}"),
            "title": format!("The guide to Tondo: {topic}"), "mimeType": "text/markdown" }));
    }
    for module in crate::api::standard_modules() {
        list.push(json!({ "uri": format!("tondo://api/{module}"), "name": format!("api/{module}"),
            "title": format!("The API of {module}"), "mimeType": "text/plain" }));
    }
    list
}

fn resource(uri: &str) -> Option<(&'static str, String)> {
    if uri == "tondo://skill" {
        return Some(("text/markdown", crate::guide::skill()));
    }
    if let Some(topic) = uri.strip_prefix("tondo://guide/") {
        return crate::guide::topic(topic).map(|text| ("text/markdown", text.to_string()));
    }
    crate::api::standard(uri.strip_prefix("tondo://api/")?).map(|text| ("text/plain", text))
}

/// How a tool acts on the world, as its annotations say.
#[derive(Clone, Copy)]
enum Kind {
    /// Reads only.
    Reads,
    /// Writes files of the project, which it can do again.
    Edits,
    /// Runs programs, which can do anything.
    Runs,
}

struct Tool {
    name: &'static str,
    description: &'static str,
    kind: Kind,
    /// Each parameter: its name, its type and what it is. A type ending in
    /// `!` is required; `strings` is an array of strings.
    params: &'static [(&'static str, &'static str, &'static str)],
}

const PATH: (&str, &str, &str) =
    ("path", "string", "A .to file or a project directory; the server's directory by default.");
const LIBRARY: (&str, &str, &str) = ("library", "boolean", "As a library for C, without main (`--library`).");
const FILE: (&str, &str, &str) = ("file", "string!", "The .to file.");
const LINE: (&str, &str, &str) = ("line", "integer!", "The line, from 1.");
const COLUMN: (&str, &str, &str) = ("column", "integer!", "The column, from 1, in characters.");
const STATIC: (&str, &str, &str) = ("static", "boolean", "A static executable, with musl on Linux (`--static`).");
const DEBUG: (&str, &str, &str) = ("debug", "boolean", "A debug build, which `debug_start` can debug (`--debug`).");
const TARGET: (&str, &str, &str) =
    ("target", "string", "Build for another machine: x86_64-linux or aarch64-linux, static (`--target`).");
const LINK: (&str, &str, &str) = ("link", "strings", "Native libraries to link, by name (-l) or file (`--link`).");
const PROGRAM: (&str, &str, &str) =
    ("program", "string!", "A running program's process ID or name, as `ps` lists them.");

const TOOLS: &[Tool] = &[
    Tool {
        name: "check",
        description: "Check a program without building it: its diagnostics, each with an id and the fixes the compiler \
suggests. Check after every edit.",
        kind: Kind::Reads,
        params: &[PATH, LIBRARY],
    },
    Tool {
        name: "fix",
        description: "Apply the compiler's fixes to the program's own files, then check it again: every safe fix, or \
those `apply` chooses by the ids `check` gives (a fix that needs a decision applies only when chosen).",
        kind: Kind::Edits,
        params: &[
            PATH,
            LIBRARY,
            ("apply", "fixes", "The fixes to apply: a diagnostic's id and the index of its fix, 0 by default."),
        ],
    },
    Tool {
        name: "format",
        description: "Format .to files canonically, rewriting them; with `check`, only tell which are not formatted.",
        kind: Kind::Edits,
        params: &[("files", "strings!", "The .to files."), ("check", "boolean", "Change nothing (`--check`).")],
    },
    Tool {
        name: "compact",
        description: "A .to file in compact form, which says the same in fewer tokens; the file does not change.",
        kind: Kind::Reads,
        params: &[FILE],
    },
    Tool {
        name: "expand",
        description: "What each `derive` of a program expands to.",
        kind: Kind::Reads,
        params: &[PATH],
    },
    Tool {
        name: "api",
        description: "The declarations and documentation of a module: a standard one (std.console), one of the \
project, or a .to file; `list` names the modules.",
        kind: Kind::Reads,
        params: &[
            ("module", "string", "The module: std.<name>, prelude, a module of the project, or a .to file."),
            ("private", "boolean", "Its private declarations too."),
            ("list", "boolean", "Name the modules instead."),
            ("project", "string", "The project whose modules to show; the server's directory by default."),
        ],
    },
    Tool {
        name: "guide",
        description: "The guide to Tondo, a topic at a time: without a topic, the overview, which names the others. \
Read it before writing Tondo.",
        kind: Kind::Reads,
        params: &[
            ("topic", "string", "The topic, such as syntax, types, errors, tasks, testing or pitfalls."),
            ("list", "boolean", "Name the topics."),
            ("skill", "boolean", "The overview as an agent skill (SKILL.md)."),
        ],
    },
    Tool {
        name: "graph",
        description: "What a program is made of: its packages, its modules with their files, and what each module \
imports. Given a directory or file inside a project, its project, and the module it belongs to.",
        kind: Kind::Reads,
        params: &[
            ("path", "string", "A project, or a directory or .to file in one; the server's directory by default."),
            ("view", "string", "modules (the default), packages, or files."),
            ("module", "string", "Only what this module imports, directly or not."),
            ("reverse", "boolean", "With `module`, only what imports it, directly or not."),
        ],
    },
    Tool {
        name: "definition",
        description: "Where the name at a place is declared.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN],
    },
    Tool {
        name: "references",
        description: "Every place that names the declaration at a place, the declaration included.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN],
    },
    Tool {
        name: "hover",
        description: "The type and documentation of what is at a place.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN],
    },
    Tool {
        name: "complete",
        description: "What can be written at a place: names, members and keywords.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN],
    },
    Tool {
        name: "rename",
        description: "Rename the program's declaration at a place, and every reference to it, rewriting the files.",
        kind: Kind::Edits,
        params: &[FILE, LINE, COLUMN, ("new_name", "string!", "The new name.")],
    },
    Tool {
        name: "implementations",
        description: "The `impl`s of the trait or type at a place, or the methods that implement the trait's method there.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN],
    },
    Tool {
        name: "calls",
        description: "The functions that call the function at a place, or with `direction: outgoing`, those it calls.",
        kind: Kind::Reads,
        params: &[FILE, LINE, COLUMN, ("direction", "string", "incoming, the default, or outgoing.")],
    },
    Tool {
        name: "explain",
        description: "What the code of an error, a warning or a panic means (E1102, W1002, P0007), with the guide's examples of it.",
        kind: Kind::Reads,
        params: &[("code", "string!", "The code, as a diagnostic or a panic gives it.")],
    },
    Tool {
        name: "new_project",
        description: "Make a Tondo project, tondo.toml, src/main.to and a test, in a new directory, or with `init` in one that exists.",
        kind: Kind::Edits,
        params: &[
            ("path", "string!", "The project's directory."),
            ("name", "string", "The package's name: the directory's by default."),
            ("init", "boolean", "Make a project of a directory that exists."),
        ],
    },
    Tool {
        name: "add_dependency",
        description: "Add a dependency to a project's tondo.toml and fetch it: a directory, a git repository or an archive.",
        kind: Kind::Edits,
        params: &[
            ("alias", "string!", "The name the program imports it by."),
            ("path", "string", "A directory of the dependency."),
            ("git", "string", "A git repository's URL."),
            ("tag", "string", "With git: a tag."),
            ("branch", "string", "With git: a branch."),
            ("rev", "string", "With git: a commit."),
            ("url", "string", "An archive's URL."),
            ("sha256", "string", "With url: the archive's SHA-256."),
            ("dev", "boolean", "Only for the tests ([dev-dependencies])."),
            ("project", "string", "The project; the server's directory by default."),
        ],
    },
    Tool {
        name: "remove_dependency",
        description: "Take a dependency out of a project's tondo.toml.",
        kind: Kind::Edits,
        params: &[
            ("alias", "string!", "The name the program imports it by."),
            ("project", "string", "The project; the server's directory by default."),
        ],
    },
    Tool {
        name: "symbols",
        description: "The declarations of a file; with `query`, those of its whole project whose names contain it.",
        kind: Kind::Reads,
        params: &[FILE, ("query", "string", "Part of the names to find in the project.")],
    },
    Tool {
        name: "build",
        description: "Build a native executable; with `library`, a library for C: lib<name>.so (.dylib), lib<name>.a \
and the header <name>.h.",
        kind: Kind::Edits,
        params: &[
            PATH,
            ("output", "string", "The executable's path, or the library's directory."),
            LIBRARY,
            STATIC,
            ("strip", "boolean", "Without its symbol table (`--strip`)."),
            DEBUG,
            TARGET,
            LINK,
        ],
    },
    Tool {
        name: "run",
        description: "Build and run a program, with its arguments and standard input: its exit code and output. A \
program that runs past `timeout` is ended.",
        kind: Kind::Runs,
        params: &[
            PATH,
            ("args", "strings", "The program's arguments."),
            ("stdin", "string", "What the program reads on its standard input."),
            ("timeout", "number", "Seconds before it is ended; 60 by default."),
            STATIC,
            DEBUG,
            TARGET,
            LINK,
        ],
    },
    Tool {
        name: "test",
        description: "Run a project's tests: what failed and why, and the report of each test as data. `filter`, \
`glob` or `exact` selects them, `list` only lists them, and `flags` takes the other options of `tondo test`.",
        kind: Kind::Runs,
        params: &[
            ("path", "string", "The project directory; the server's directory by default."),
            ("filter", "string", "The tests whose ids contain this."),
            ("glob", "string", "The tests whose ids match this pattern."),
            ("exact", "string", "The test with this id."),
            ("list", "boolean", "List the tests instead of running them."),
            ("coverage", "boolean", "Also say which lines, functions and ways of branches of the project's code ran."),
            LIBRARY,
            ("update_snapshots", "boolean", "Record the snapshots instead of checking them."),
            STATIC,
            DEBUG,
            TARGET,
            ("flags", "strings", "Other options, as `tondo test` takes them: --seed, --timeout, --repeat..."),
        ],
    },
    Tool {
        name: "doc_test",
        description: "Check the Tondo examples of a Markdown document.",
        kind: Kind::Runs,
        params: &[("file", "string!", "The Markdown document.")],
    },
    Tool {
        name: "bind",
        description: "Write a module of extern declarations for C headers.",
        kind: Kind::Edits,
        params: &[
            ("headers", "strings", "The headers; those of the project's [native] section by default."),
            ("package", "string", "A pkg-config package for the headers and flags."),
            ("libraries", "strings", "The libraries the declarations link."),
            ("project", "string", "The project; the server's directory by default."),
            ("output", "string", "The .to file to write."),
            ("include", "strings", "Directories of headers (-I)."),
            ("define", "strings", "Macros, as name or name=value (-D)."),
        ],
    },
    Tool {
        name: "update",
        description: "Fetch a project's dependencies again and rewrite tondo.lock: all, or those named.",
        kind: Kind::Edits,
        params: &[
            ("project", "string", "The project; the server's directory by default."),
            ("dependencies", "strings", "The dependencies to update."),
        ],
    },
    Tool {
        name: "target_pack",
        description: "Write this machine's target pack, which a tondo elsewhere builds for it with when \
TONDO_TARGETS names the directory.",
        kind: Kind::Edits,
        params: &[("dir", "string", "The directory of packs.")],
    },
    Tool {
        name: "ps",
        description: "The running Tondo programs that can be observed: those started with TONDO_DIAGNOSTICS=1, and \
debug builds.",
        kind: Kind::Reads,
        params: &[],
    },
    Tool {
        name: "counters",
        description: "A running program's counters, the runtime's and its own (std.diagnostics).",
        kind: Kind::Reads,
        params: &[
            PROGRAM,
            ("count", "integer", "How many readings; 1 by default."),
            ("interval", "number", "Seconds between readings."),
        ],
    },
    Tool {
        name: "trace",
        description: "Record what a program's tasks do, for Perfetto, and a profile of where it spends its time: of \
a running program, or of `command`, which it starts.",
        kind: Kind::Edits,
        params: &[
            ("program", "string", "A running program's process ID or name."),
            ("command", "strings", "An executable to start and trace, and its arguments."),
            ("duration", "number", "Seconds to record."),
            ("output", "string", "The trace file to write."),
        ],
    },
    Tool {
        name: "dump",
        description: "Write a dump of a running program: its tasks and where they are, its counters, its live memory.",
        kind: Kind::Edits,
        params: &[PROGRAM, ("output", "string", "The dump file to write.")],
    },
    Tool {
        name: "dump_read",
        description: "Read a dump, as `dump` writes or a crash leaves with TONDO_DUMP=<dir>.",
        kind: Kind::Reads,
        params: &[
            ("file", "string!", "The dump."),
            ("view", "string", "summary (by default), tasks, task, heap, objects or object."),
            ("task", "integer", "The task the task view shows."),
            ("address", "string", "The object the object view shows."),
        ],
    },
    Tool {
        name: "debug_start",
        description: "Debug a program: a .to file or a project (built with --debug), a debug executable, or a running \
debug build's process ID or name. A program it starts waits before main for breakpoints. Gives the session that \
`debug` takes; a session lasts until `debug_stop`, or the program ends.",
        kind: Kind::Runs,
        params: &[
            ("target", "string!", "The program to debug."),
            ("args", "strings", "The arguments of a program it starts."),
            ("live", "boolean", "Build it live (`--live`), so that `reload` changes its code while it is debugged."),
        ],
    },
    Tool {
        name: "debug",
        description: "A command of a debugging session: break <file>:<line> [if <name> <op> <value>], delete <id>, \
continue, step, next, finish, pause, locals, print <name>, stack, tasks, reload (a program started with `live`: \
its code as its files are now, even while it is stopped), restart (live: the stopped function returns and is \
called again with the same arguments, from its start, as its code is now); and wait, which waits for a running \
program to stop. Gives the debugger's answers and events, and what the program printed.",
        kind: Kind::Runs,
        params: &[
            ("session", "string!", "The session `debug_start` gave."),
            ("command", "string!", "The command."),
            ("wait", "number", "Seconds to wait for a running program to stop; 10 by default."),
        ],
    },
    Tool {
        name: "debug_stop",
        description: "End a debugging session, and the program it started.",
        kind: Kind::Runs,
        params: &[("session", "string!", "The session `debug_start` gave.")],
    },
    Tool {
        name: "run_start",
        description: "Start a program and keep it running, as a live build: `reload` changes its code while it \
runs, and with `watch` it reloads itself when its files change, running again when a change needs it. Gives the \
session that run_output, run_input, run_stop and reload take; its output so far, and whether it runs.",
        kind: Kind::Runs,
        params: &[
            PATH,
            ("args", "strings", "The program's arguments."),
            ("stdin", "string", "What to write to its standard input first."),
            ("watch", "boolean", "Reload when its files change (`--watch`)."),
            ("wait", "number", "Seconds to wait for its first output; 2 by default."),
        ],
    },
    Tool {
        name: "run_output",
        description: "What a session's program printed since the last time, waiting up to `wait` seconds for \
something; and whether it still runs.",
        kind: Kind::Reads,
        params: &[
            ("session", "string!", "The session `run_start` gave."),
            ("wait", "number", "Seconds to wait for output; 1 by default."),
        ],
    },
    Tool {
        name: "run_input",
        description: "Write to a session's standard input, and give what the program printed then.",
        kind: Kind::Runs,
        params: &[
            ("session", "string!", "The session `run_start` gave."),
            ("text", "string!", "What to write; a line ends with \\n."),
            ("close", "boolean", "End the input after it."),
            ("wait", "number", "Seconds to wait for output; 0.5 by default."),
        ],
    },
    Tool {
        name: "run_stop",
        description: "End a session's program, and give what it printed last.",
        kind: Kind::Runs,
        params: &[("session", "string!", "The session `run_start` gave.")],
    },
    Tool {
        name: "reload",
        description: "Change a running live build's code to what its files say now, while it runs: a session's \
program, or one started with `tondo run --live` by its process ID or name. Says which functions changed and \
which it added; frames already running finish with the old code. A change it cannot make in place, as a type \
with other fields, says to start the program again; files that do not compile leave it as it was.",
        kind: Kind::Runs,
        params: &[
            ("session", "string", "The session `run_start` gave."),
            ("program", "string", "A running live build's process ID or name."),
        ],
    },
];

impl Tool {
    fn listed(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (name, kind, description) in self.params {
            let kind = match kind.strip_suffix('!') {
                Some(kind) => {
                    required.push(*name);
                    kind
                }
                None => kind,
            };
            let schema = match kind {
                "strings" => json!({ "type": "array", "items": { "type": "string" }, "description": description }),
                "fixes" => json!({ "type": "array", "description": description, "items": { "type": "object",
                    "properties": { "id": { "type": "string" }, "fix": { "type": "integer", "minimum": 0 } },
                    "required": ["id"] } }),
                _ => json!({ "type": kind, "description": description }),
            };
            properties.insert(name.to_string(), schema);
        }
        let annotations = match self.kind {
            Kind::Reads => json!({ "readOnlyHint": true, "openWorldHint": false }),
            Kind::Edits => json!({ "readOnlyHint": false, "destructiveHint": false }),
            Kind::Runs => json!({ "readOnlyHint": false }),
        };
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": { "type": "object", "properties": properties, "required": required },
            "annotations": annotations,
        })
    }
}
