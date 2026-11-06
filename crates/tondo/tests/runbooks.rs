//! Every step of the runbooks of `tondo guide` runs, in order, against the
//! tools of `tondo mcp`: its files are written, its commands run, and each
//! call gives what the runbook says it gives.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

struct Agent {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Agent {
    fn start(dir: &Path) -> Agent {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("mcp")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Agent { child, input, output, next: 0 }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.next += 1;
        let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28" });
        let params = json!({ "name": tool, "arguments": arguments, "_meta": meta });
        let message = json!({ "jsonrpc": "2.0", "id": self.next, "method": "tools/call", "params": params });
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()["result"].clone()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The fenced blocks of a topic: their info string and their lines.
fn blocks(text: &str) -> Vec<(String, Vec<String>)> {
    let mut found = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.strip_prefix("```") else { continue };
        let code = lines.by_ref().take_while(|line| !line.starts_with("```")).map(str::to_string).collect();
        found.push((info.to_string(), code));
    }
    found
}

/// What stands for the things earlier steps made.
#[derive(Default)]
struct Known {
    session: String,
    pid: String,
    signature: String,
    /// What the session said when it started, which its output does not repeat.
    started: String,
}

fn first_dump(dir: &Path) -> String {
    let mut dumps: Vec<PathBuf> = std::fs::read_dir(dir.join("dumps"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    dumps.sort();
    dumps.first().map_or(String::new(), |dump| dump.display().to_string())
}

/// A call and what its result must say, against a program that may still
/// be on its way there: a session's output is asked again.
fn step(agent: &mut Agent, dir: &Path, topic: &str, call: &str, expected: &[&str], known: &mut Known) {
    let call =
        call.replace("<session>", &known.session).replace("<pid>", &known.pid).replace("<dump>", &first_dump(dir));
    let call = call.replace("<signature>", &known.signature);
    let (tool, arguments) = call.split_once(' ').unwrap_or((&call, "{}"));
    let arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|e| panic!("{topic}: {call}: {e}"));
    let until = Instant::now() + Duration::from_secs(120);
    let mut seen = if tool == "run_output" { known.started.clone() } else { String::new() };
    loop {
        let result = agent.call(tool, arguments.clone());
        assert_ne!(result["isError"], true, "{topic}: {call}: {result}");
        let structured = &result["structuredContent"];
        if matches!(tool, "run_start" | "debug_start") {
            known.session = structured["session"].as_str().unwrap().to_string();
            known.pid = structured["pid"].to_string();
            known.started = structured.to_string();
        }
        if let Some(signature) = structured["report"]["groups"][0]["signature"].as_str() {
            known.signature = signature.to_string();
        }
        seen.push_str(result["content"][0]["text"].as_str().unwrap_or_default());
        seen.push_str(&result["structuredContent"].to_string());
        if expected.iter().all(|text| seen.contains(text)) {
            return;
        }
        assert!(tool == "run_output" && Instant::now() < until, "{topic}: {call} gave no {expected:?}: {seen}");
    }
}

fn replay(topic: &str, text: &str) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(topic);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tondo = Path::new(env!("CARGO_BIN_EXE_tondo"));
    let path = format!("{}:{}", tondo.parent().unwrap().display(), std::env::var("PATH").unwrap_or_default());
    let mut agent = Agent::start(&dir);
    let mut known = Known::default();
    // A port out of what the system gives, of this runbook alone.
    let port = (21000 + (std::process::id() % 1000) * 10 + topic.len() as u32 % 10).to_string();
    for (info, lines) in blocks(text) {
        let lines: Vec<String> = lines.iter().map(|line| line.replace("<port>", &port)).collect();
        let named = info.split_whitespace().find_map(|word| word.strip_prefix("file="));
        let file = named.or(info.split_once(' ').map(|(_, rest)| rest.trim()));
        match info.split(' ').next().unwrap_or_default() {
            "tondo" => {
                let file = file.unwrap_or_else(|| panic!("{topic}: {info}"));
                std::fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
            }
            "file" => {
                let file = file.unwrap_or_else(|| panic!("{topic}: a file block names its file"));
                std::fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
            }
            "sh" => {
                let script = lines.join("\n");
                let ran =
                    Command::new("sh").args(["-c", &script]).current_dir(&dir).env("PATH", &path).output().unwrap();
                let said = String::from_utf8_lossy(&ran.stderr);
                assert!(!said.contains("not found") && !said.contains("error["), "{topic}: {script}: {said}");
            }
            "edit" => {
                let file = dir.join(file.unwrap_or_else(|| panic!("{topic}: an edit names its file")));
                let gone: Vec<&str> = lines.iter().filter_map(|line| line.strip_prefix('-')).collect();
                let new: Vec<&str> = lines.iter().filter_map(|line| line.strip_prefix('+')).collect();
                let text = std::fs::read_to_string(&file).unwrap();
                let old = gone.join("\n");
                assert!(text.contains(&old), "{topic}: {} has no `{old}`", file.display());
                std::fs::write(&file, text.replace(&old, &new.join("\n"))).unwrap();
            }
            "mcp" => {
                let mut at = 0;
                while at < lines.len() {
                    let expected: Vec<&str> =
                        lines[at + 1..].iter().map_while(|line| line.strip_prefix("=> ")).collect();
                    step(&mut agent, &dir, topic, &lines[at], &expected, &mut known);
                    at += 1 + expected.len();
                }
            }
            _ => {}
        }
    }
    if !known.session.is_empty() {
        agent.call("run_stop", json!({ "session": known.session }));
    }
}

#[test]
fn every_runbook_runs_as_it_says() {
    if cfg!(target_os = "windows") {
        return;
    }
    let topics: Vec<_> = tondo::guide::TOPICS.iter().filter(|(name, _)| name.starts_with("runbook-")).collect();
    assert!(topics.len() >= 4);
    std::thread::scope(|scope| {
        let runs: Vec<_> = topics.iter().map(|(name, text)| scope.spawn(move || replay(name, text))).collect();
        runs.into_iter().for_each(|run| run.join().unwrap());
    });
}

/// The index names every runbook, and the server's instructions name the index.
#[test]
fn the_runbooks_can_be_found() {
    let index = tondo::guide::topic("runbooks").unwrap();
    for (name, _) in tondo::guide::TOPICS.iter().filter(|(name, _)| name.starts_with("runbook-")) {
        assert!(index.contains(&format!("`{name}`")), "`tondo guide runbooks` does not name {name}");
    }
    assert!(include_str!("../src/mcp.rs").contains("`guide` topic `runbooks`"), "the server does not name them");
}
