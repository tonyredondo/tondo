//! Breakpoints in a running build without probes (toolchain 7.5). Such a
//! program has no debugger to attach to, so this answers in its place, to
//! `tondo debug`, the tools and the editor alike: each breakpoint is a logpoint
//! on a connection of its own, which stops only its task and brings a dump of
//! the whole program as it was there, and what the debugger is asked comes from
//! that dump. The program goes on all the while.

use crate::observe::{self, Program};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::Duration;

/// For how long, in seconds, a breakpoint holds its task: a service does not
/// stay stopped because someone went away.
const HOLD: u64 = 300;
/// As many times as a breakpoint is reached.
const ALWAYS: u64 = 1 << 40;

/// A connection to the program named, on this machine or by a command.
fn reach(name: &str, via: Option<&str>) -> Result<Program, String> {
    match via {
        Some(via) => observe::via(via, name),
        None => observe::connect(name),
    }
}

/// A program to debug: the program itself in a build with probes; otherwise
/// one end of a conversation with what answers for it.
pub(crate) fn connect(name: &str, via: Option<&str>) -> Result<Program, String> {
    let mut program = reach(name, via)?;
    if program.ask("info")?["probes"] != false {
        return Ok(program);
    }
    let pid = program.pid;
    let (here, there) = UnixStream::pair().map_err(|e| e.to_string())?;
    let (said, heard) = mpsc::channel();
    let reader = BufReader::new(there.try_clone().map_err(|e| e.to_string())?);
    let asked = said.clone();
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            if asked.send(Said::Asked(line)).is_err() {
                return;
            }
        }
        let _ = asked.send(Said::Left);
    });
    let writer = program.writer.try_clone().map_err(|e| e.to_string())?;
    let _ = program.reader.get_ref().set_read_timeout(None);
    let ended = said.clone();
    std::thread::spawn(move || {
        let _ = program.reader.read_line(&mut String::new());
        let _ = ended.send(Said::Ended);
    });
    let mut standing = Standing {
        name: name.to_string(),
        via: via.map(String::from),
        out: there,
        said,
        points: HashMap::new(),
        _watching: Point { writer, text: None, deleted: false },
        held: None,
        seen: None,
        selected: (0, 0),
        waiting: false,
        untold: None,
    };
    std::thread::spawn(move || standing.serve(heard));
    let reader = BufReader::new(here.try_clone().map_err(|e| e.to_string())?);
    Ok(Program { pid, reader, writer: here })
}

enum Said {
    /// A line of who debugs.
    Asked(String),
    /// A line of the program, of a breakpoint's connection.
    Reached(Value),
    Ended,
    Left,
}

/// A breakpoint: its connection, and the message that makes it a logpoint.
struct Point {
    writer: UnixStream,
    text: Option<String>,
    /// Deleted while it holds a task: it goes when the task does.
    deleted: bool,
}

impl Drop for Point {
    /// Closing it takes the logpoint out, and lets its task go.
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

struct Standing {
    name: String,
    via: Option<String>,
    out: UnixStream,
    said: mpsc::Sender<Said>,
    points: HashMap<u64, Point>,
    /// The connection that says when the program ends.
    _watching: Point,
    /// The breakpoint that holds a task.
    held: Option<u64>,
    /// The tasks as the last stop saw them.
    seen: Option<Vec<Value>>,
    selected: (u64, usize),
    /// Who debugs waits for a stop: until it says `continue`, as it would
    /// with a debugger, a task that stops waits to be told.
    waiting: bool,
    /// The task that stopped before that, with the dump it brought.
    untold: Option<Value>,
}

impl Standing {
    fn serve(&mut self, heard: mpsc::Receiver<Said>) {
        while let Ok(said) = heard.recv() {
            match said {
                Said::Asked(line) => {
                    let line = line.trim();
                    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
                    let answer = self.answer(word, rest.trim());
                    self.tell(&answer);
                    match (word, self.untold.take()) {
                        ("pause", untold) => {
                            self.untold = untold;
                            self.stopped("pause");
                        }
                        ("continue", Some(hit)) => self.show(&hit),
                        (_, untold) => self.untold = untold,
                    }
                }
                Said::Reached(value) => self.reached(value),
                Said::Ended => self.tell(&json!({ "exited": true })),
                Said::Left => return,
            }
        }
    }

    fn tell(&mut self, value: &Value) {
        let _ = writeln!(self.out, "{value}");
    }

    /// A breakpoint was reached, or let its task go when its time ended.
    fn reached(&mut self, value: Value) {
        if let Some(error) = value["error"].as_str() {
            let told = format!("the step did not go: {error}");
            self.tell(&json!({ "logged": told }));
            return;
        }
        if let Some(id) = value["went"].as_u64() {
            if self.held == Some(id) {
                self.held = None;
                self.untold = None;
                self.points.retain(|point, kept| *point != id || !kept.deleted);
                let told = format!("the task went on: a breakpoint holds it for {HOLD} seconds at most");
                self.tell(&json!({ "logged": told }));
            }
            return;
        }
        let Some(id) = value["hit"].as_u64() else { return };
        let Some(point) = self.points.get_mut(&id) else { return };
        if point.deleted && value["step"] != true {
            let _ = writeln!(point.writer, "go {id}");
            return;
        }
        if let Some(text) = &point.text {
            let logged = observe::logged(&value, Some(text));
            self.tell(&json!({ "logged": logged }));
            return;
        }
        self.held = Some(id);
        match self.waiting {
            true => self.show(&value),
            false => self.untold = Some(value),
        }
    }

    /// The stop a breakpoint brought.
    fn show(&mut self, hit: &Value) {
        self.waiting = false;
        self.seen = Some(observe::seen_tasks(hit["whole"].clone()));
        self.selected = (hit["task"].as_u64().unwrap_or(1), 0);
        self.stopped(if hit["step"] == true { "step" } else { "breakpoint" });
    }

    /// Tells where the selected task stands.
    fn stopped(&mut self, why: &str) {
        let (task, _) = self.selected;
        let at = self.task(task).map_or(json!(""), |task| task["stack"][0].clone());
        self.tell(&json!({ "stopped": { "why": why, "task": task, "at": at, "alone": true } }));
    }

    fn task(&self, id: u64) -> Option<&Value> {
        self.seen.as_ref()?.iter().find(|task| task["id"] == id)
    }

    /// The bindings of the selected frame, or why there are none.
    fn frame(&self) -> Result<&Value, String> {
        let (task, frame) = self.selected;
        let task = self.task(task).ok_or("the program runs: a breakpoint, or `pause`, shows it")?;
        observe::frame_of_task(task, frame).ok_or("this frame has no bindings".to_string())
    }

    fn answer(&mut self, word: &str, rest: &str) -> Value {
        let result = match word {
            "debug" => Ok(json!({ "debugging": true, "lines": 0 })),
            "sources" | "requests" => reach(&self.name, self.via.as_deref()).and_then(|mut program| program.ask(word)),
            "break" => self.set(rest),
            "delete" => self.delete(rest),
            "continue" => {
                self.seen = None;
                self.waiting = true;
                // A stop nobody has seen yet is the one it goes on to.
                if self.untold.is_none()
                    && let Some(id) = self.held.take()
                    && let Some(point) = self.points.get_mut(&id)
                {
                    let _ = writeln!(point.writer, "go {id}");
                    self.points.retain(|point, kept| *point != id || !kept.deleted);
                }
                Ok(json!({ "running": true }))
            }
            "next" => self.stepped("next"),
            "step" => self.stepped("in"),
            "finish" => self.stepped("out"),
            "pause" => self.picture(),
            "select" => self.select(rest),
            "task" => self.select(&format!("{rest} 0")),
            "tasks" => Ok(self.tasks()),
            "stack" => self.stack(rest),
            "locals" => self.frame().map(|frame| json!({ "locals": listed(&frame["bindings"], "") })),
            "children" => self.children(rest),
            "print" => self.print(rest),
            "context" => match self.task(self.selected.0) {
                Some(task) => Ok(json!({ "contexts": task["contexts"] })),
                None => Err("the program runs: a breakpoint, or `pause`, shows it".to_string()),
            },
            "snapshot" => self.snapshot(),
            "files" => Ok(json!({ "files": [] })),
            "lines" => Ok(json!({ "lines": [] })),
            _ => Err(format!(
                "`{word}` needs a build with probes: here a breakpoint shows where a task stopped, `next` and \
                 `finish` go on line by line, and the task and the rest of the program go on with `continue`"
            )),
        };
        result.unwrap_or_else(|error| json!({ "error": error }))
    }

    /// `break <file>:<line> [if <condition>] [log <message>]`.
    fn set(&mut self, rest: &str) -> Result<Value, String> {
        let (rest, text) = rest.split_once(" log ").map_or((rest, None), |(rest, text)| (rest, Some(text.to_string())));
        let (rest, hits) = rest.split_once(" hits ").unwrap_or((rest, ""));
        let (place, condition) = rest.split_once(" if ").unwrap_or((rest, ""));
        let (place, request) = place.split_once(" request ").unwrap_or((place, ""));
        let Some((_, line)) = place.rsplit_once(':').filter(|(file, _)| !file.contains(' ')) else {
            return Err("a build without probes stops at a line: `break <file>:<line>`".into());
        };
        let mut program = reach(&self.name, self.via.as_deref())?;
        let stops = match text {
            Some(_) => "\t".to_string(),
            None => format!("-\t{HOLD}"),
        };
        let answer = program.ask(&format!("logpoint {ALWAYS} {place}\t{condition}\t{stops}\t{request}\t{hits}"))?;
        if let Some(error) = answer["error"].as_str() {
            return Err(error.to_string());
        }
        let id = answer["logpoint"].as_u64().unwrap_or(0);
        let Program { writer, reader, .. } = program;
        let _ = reader.get_ref().set_read_timeout(None);
        let said = self.said.clone();
        std::thread::spawn(move || {
            let values = reader.lines().map_while(|line| serde_json::from_str::<Value>(&line.ok()?).ok());
            for value in values {
                if said.send(Said::Reached(value)).is_err() {
                    return;
                }
            }
        });
        self.points.insert(id, Point { writer, text, deleted: false });
        let line = line.parse::<u64>().unwrap_or(0);
        Ok(json!({ "breakpoint": { "id": id, "at": place, "line": line } }))
    }

    fn delete(&mut self, rest: &str) -> Result<Value, String> {
        let id = rest.parse::<u64>().map_err(|_| "delete takes a breakpoint's number")?;
        // A task that stands stays until it is told to go on: changing the
        // breakpoints of a file deletes them all.
        if self.held == Some(id) {
            self.points.get_mut(&id).ok_or(format!("no breakpoint {id}"))?.deleted = true;
        } else {
            self.points.remove(&id).ok_or(format!("no breakpoint {id}"))?;
        }
        Ok(json!({ "deleted": id }))
    }

    /// The task that stands goes on: to a line of its function or of those
    /// that called it, of those alone, or of any of the program's.
    fn stepped(&mut self, how: &str) -> Result<Value, String> {
        let id = self.held.ok_or("no task stands here: `step` goes on from a breakpoint")?;
        let point = self.points.get_mut(&id).ok_or("its breakpoint is gone")?;
        writeln!(point.writer, "step {id} {how}").map_err(|e| e.to_string())?;
        self.held = None;
        self.seen = None;
        self.waiting = true;
        Ok(json!({ "running": true }))
    }

    /// `pause` does not stop a build without probes: what it shows is a
    /// picture of it.
    fn picture(&mut self) -> Result<Value, String> {
        let mut program = reach(&self.name, self.via.as_deref())?;
        let _ = program.reader.get_ref().set_read_timeout(Some(Duration::from_secs(60)));
        let tasks = observe::seen_tasks(program.ask("dump")?);
        let first = tasks.iter().find(|task| task["at"].is_string()).or(tasks.first());
        self.selected = (first.and_then(|task| task["id"].as_u64()).unwrap_or(1), 0);
        self.seen = Some(tasks);
        self.tell(&json!({ "logged": "a picture of the program as it was: it goes on" }));
        Ok(json!({ "pausing": true }))
    }

    /// `select <task> <frame>`, or a frame of the task selected.
    fn select(&mut self, rest: &str) -> Result<Value, String> {
        let numbers: Vec<u64> = rest.split_whitespace().filter_map(|word| word.parse().ok()).collect();
        self.selected = match numbers[..] {
            [task, frame] => (task, frame as usize),
            [frame] => (self.selected.0, frame as usize),
            _ => return Err("select takes a task and a frame".into()),
        };
        Ok(json!({ "selected": { "task": self.selected.0, "frame": self.selected.1 } }))
    }

    fn tasks(&self) -> Value {
        let held = self.held.map(|_| self.selected.0);
        let mut tasks: Vec<Value> = self
            .seen
            .iter()
            .flatten()
            .map(|task| {
                let why = if task["id"].as_u64() == held { json!("breakpoint") } else { task["state"].clone() };
                json!({ "task": task["id"], "at": task["at"], "why": why, "request": task["request"] })
            })
            .collect();
        // The one stopped first, as the debugger's tools look for it.
        tasks.sort_by_key(|task| task["why"] != "breakpoint");
        json!({ "tasks": tasks })
    }

    fn stack(&self, rest: &str) -> Result<Value, String> {
        let id = rest.parse().unwrap_or(self.selected.0);
        let task = self.task(id).ok_or(format!("task {id} is not stopped"))?;
        Ok(json!({ "stack": task["stack"], "spawned": task["spawned"] }))
    }

    /// `children <path> [<start> <count>]`.
    fn children(&self, rest: &str) -> Result<Value, String> {
        let mut words = rest.split_whitespace();
        let path = words.next().unwrap_or_default();
        let value = observe::dumped_value(self.frame()?, path).ok_or(format!("nothing is called `{path}` here"))?;
        let start = words.next().and_then(|word| word.parse().ok()).unwrap_or(0);
        let count = words.next().and_then(|word| word.parse().ok()).unwrap_or(usize::MAX);
        let parts = value["parts"].as_array().map_or(&[][..], Vec::as_slice);
        let page = &parts[parts.len().min(start)..parts.len().min(start.saturating_add(count))];
        Ok(json!({ "children": listed(&json!(page), path) }))
    }

    fn print(&self, path: &str) -> Result<Value, String> {
        let value = observe::dumped_value(self.frame()?, path)
            .ok_or(format!("the stop has no `{path}` here: it holds the bindings and their parts"))?;
        let mut shown = listed(&json!([value]), "").remove(0);
        shown["name"] = json!(path);
        shown["path"] = json!(path);
        Ok(shown)
    }

    fn snapshot(&self) -> Result<Value, String> {
        let (id, _) = self.selected;
        let task = self.task(id).ok_or("the program runs: a breakpoint, or `pause`, shows it")?;
        let frames: Vec<Value> = (0..task["stack"].as_array().map_or(0, Vec::len))
            .map(|index| {
                let bindings = observe::frame_of_task(task, index).map_or(json!([]), |frame| frame["bindings"].clone());
                json!({ "at": task["stack"][index], "locals": listed(&bindings, "") })
            })
            .collect();
        let tasks = self.tasks()["tasks"].clone();
        let why = if self.held.is_some() { "breakpoint" } else { "pause" };
        let answer = json!({ "task": id, "why": why, "frames": frames, "spawned": task["spawned"], "tasks": tasks });
        Ok(json!({ "snapshot": answer }))
    }
}

/// Bindings or parts as the debugger gives them, with the path of each under
/// `path`.
fn listed(values: &Value, path: &str) -> Vec<Value> {
    let values = values.as_array().map_or(&[][..], Vec::as_slice);
    let shown = |value: &Value| {
        let name = value["name"].as_str().unwrap_or_default();
        let parts = value["parts"].as_array().map_or(0, Vec::len);
        let indexed = value["parts"][0]["name"].as_str().is_some_and(|name| name.starts_with('['));
        json!({
            "name": name, "path": format!("{path}{name}"), "value": value["value"], "type": value["type"],
            "children": parts, "indexed": indexed,
        })
    };
    values.iter().map(shown).collect()
}
