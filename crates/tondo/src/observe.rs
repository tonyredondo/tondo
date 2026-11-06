//! The tools that look at running Tondo programs (toolchain 7.5): `tondo
//! ps` and `tondo counters`, through the socket each program serves in
//! `$TMPDIR/tondo-<uid>`, named after its process.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) fn directory() -> PathBuf {
    std::env::temp_dir().join(format!("tondo-{}", unsafe { libc::geteuid() }))
}

/// A connection to a running program.
pub(crate) struct Program {
    pub(crate) pid: u32,
    pub(crate) reader: BufReader<UnixStream>,
    pub(crate) writer: UnixStream,
}

impl Program {
    fn connect(pid: u32) -> std::io::Result<Program> {
        let stream = UnixStream::connect(directory().join(pid.to_string()))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Program { pid, reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    /// The program's answer to `request`, or why there is none.
    pub(crate) fn ask(&mut self, request: &str) -> Result<Value, String> {
        let ended = |_| format!("the program {} ended", self.pid);
        writeln!(self.writer, "{request}").map_err(ended)?;
        let mut line = String::new();
        if self.reader.read_line(&mut line).map_err(ended)? == 0 {
            return Err(format!("the program {} ended", self.pid));
        }
        serde_json::from_str(&line).map_err(|e| format!("the program {} answered `{}`: {e}", self.pid, line.trim()))
    }
}

/// The programs running now, each with its `info`. A socket whose
/// program is gone is removed.
/// Whether a process runs. One that ended and nobody collected still
/// answers a signal, as an orphan does in a container with no init, which
/// never collects it.
pub(crate) fn alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return false;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    !stat.rsplit_once(") ").is_some_and(|(_, state)| state.starts_with('Z'))
}

fn running() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(directory()) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let info = Program::connect(pid).map_err(|e| e.to_string()).and_then(|mut p| p.ask("info"));
        match info {
            Ok(info) => found.push(info),
            Err(_) if !alive(pid) => {
                let _ = std::fs::remove_file(entry.path());
            }
            Err(_) => {}
        }
    }
    found.sort_by_key(|info| info["pid"].as_u64());
    found
}

pub(crate) fn program_name(info: &Value) -> &str {
    let program = info["program"].as_str().unwrap_or_default();
    program.rsplit('/').next().unwrap_or(program)
}

/// `tondo ps [--json]`.
pub fn ps(json: bool) -> Result<(), String> {
    // Those of the machine `--via` reaches, as its `tondo ps` lists them.
    if let Some(command) = VIA.get() {
        let words: Vec<&str> = command.split_whitespace().collect();
        let (program, arguments) = words.split_first().ok_or("--via takes the command that reaches the programs")?;
        let status = std::process::Command::new(program)
            .args(arguments)
            .args(["tondo", "ps"])
            .args(json.then_some("--json"))
            .status()
            .map_err(|e| format!("cannot run `{command}`: {e}"))?;
        return if status.success() { Ok(()) } else { Err(format!("`{command} tondo ps` failed")) };
    }
    let programs = running();
    if json {
        crate::cli::print_json("ps", true, json!({ "programs": programs }));
        return Ok(());
    }
    if programs.is_empty() {
        println!("no Tondo program started with TONDO_DIAGNOSTICS=1 is running");
        return Ok(());
    }
    println!("{:<9} {:>9}  PROGRAM", "PID", "UPTIME");
    for info in &programs {
        let mut line = info["program"].as_str().unwrap_or_default().to_string();
        for argument in info["arguments"].as_array().into_iter().flatten() {
            line.push(' ');
            line.push_str(argument.as_str().unwrap_or_default());
        }
        let uptime = duration(info["uptime_ms"].as_f64().unwrap_or(0.0) / 1000.0);
        println!("{:<9} {uptime:>9}  {line}", info["pid"]);
    }
    Ok(())
}

/// The program a command names: its process ID, or its file's name when
/// one running program has it.
fn target(name: &str) -> Result<u32, String> {
    if let Ok(pid) = name.parse::<u32>() {
        return Ok(pid);
    }
    let matching: Vec<u64> =
        running().iter().filter(|info| program_name(info) == name).filter_map(|info| info["pid"].as_u64()).collect();
    match matching.as_slice() {
        [pid] => Ok(*pid as u32),
        [] => Err(format!(
            "no running Tondo program is named `{name}`; `tondo ps` lists those started with TONDO_DIAGNOSTICS=1"
        )),
        pids => {
            let pids: Vec<String> = pids.iter().map(u64::to_string).collect();
            Err(format!("{} programs are named `{name}`: give a process ID, one of {}", pids.len(), pids.join(", ")))
        }
    }
}

/// The command that reaches the programs the tools look at, on another
/// machine or in a container (`--via`), when it is not this one.
static VIA: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// From now on, the programs are where `command` reaches, through
/// `tondo relay` there.
pub fn reach_via(command: String) {
    let _ = VIA.set(command);
}

pub(crate) fn connect(name: &str) -> Result<Program, String> {
    connect_via(name, VIA.get().map(String::as_str))
}

/// A connection to the program `name`, here or where `via` reaches.
pub(crate) fn connect_via(name: &str, via: Option<&str>) -> Result<Program, String> {
    if let Some(command) = via {
        return self::via(command, name);
    }
    let pid = target(name)?;
    Program::connect(pid).map_err(|_| {
        format!(
            "no Tondo program with process ID {pid} answers: a program serves the tools when it starts with TONDO_DIAGNOSTICS=1, and `tondo ps` lists those that do"
        )
    })
}

/// A connection to a program on another machine, or in a container,
/// through a command that runs `tondo relay <name>` there, as `ssh host` or
/// `docker exec -i box`: what the relay reads and writes is the program's
/// connection, after a first line with its process ID.
pub(crate) fn via(command: &str, name: &str) -> Result<Program, String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some((program, arguments)) = words.split_first() else {
        return Err("--via takes the command that reaches the program's machine, as `ssh host`".into());
    };
    let mut child = std::process::Command::new(program)
        .args(arguments)
        .args(["tondo", "relay", name])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{command}`: {e}"))?;
    let (mut input, output) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"));
    let mut output = BufReader::new(output);
    let mut first = String::new();
    let _ = output.read_line(&mut first);
    let Some(pid) = serde_json::from_str::<Value>(&first).ok().and_then(|v| v["relay"].as_u64()) else {
        let _ = child.wait();
        return Err(format!("`{command} tondo relay {name}` did not reach the program: it says what failed above"));
    };
    // One end of a pair of sockets is the connection; the other goes to
    // and from the relay.
    let (ours, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
    let mut to_relay = theirs.try_clone().map_err(|e| e.to_string())?;
    let mut from_relay = theirs;
    std::thread::spawn(move || passed(&mut to_relay, &mut input));
    std::thread::spawn(move || {
        passed(&mut output, &mut from_relay);
        let _ = from_relay.shutdown(std::net::Shutdown::Both);
        let _ = child.wait();
    });
    Ok(Program { pid: pid as u32, reader: BufReader::new(ours.try_clone().map_err(|e| e.to_string())?), writer: ours })
}

/// `tondo relay <pid|name>`: joins its standard input and output to a
/// running program's connection, for a debugger on another machine that
/// reaches this one with `--via`; its first line is the program's process
/// ID.
pub fn relay(name: &str) -> Result<(), String> {
    let program = connect(name)?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", json!({ "relay": program.pid })).map_err(|e| e.to_string())?;
    stdout.flush().map_err(|e| e.to_string())?;
    let Program { reader, mut writer, .. } = program;
    let mut reader = reader.into_inner();
    let _ = reader.set_read_timeout(None);
    std::thread::spawn(move || {
        passed(&mut std::io::stdin(), &mut writer);
        let _ = writer.shutdown(std::net::Shutdown::Write);
    });
    passed(&mut reader, &mut std::io::stdout());
    Ok(())
}

/// Passes what `from` says to `to` until it ends, each piece at once: a
/// debugger waits for every answer.
fn passed(from: &mut impl std::io::Read, to: &mut impl Write) {
    let mut buffer = [0u8; 8192];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buffer[..read]).and_then(|_| to.flush()).is_err() {
                    return;
                }
            }
        }
    }
}

/// `tondo counters <pid|name> [--interval <seconds>] [--count <n>]
/// [--json]`: every interval, the counters and the rate of each total
/// since the last time.
pub fn counters(name: &str, interval: Duration, count: Option<u64>, json: bool) -> Result<(), String> {
    let mut program = connect(name)?;
    let info = program.ask("info")?;
    let mut last = program.ask("counters")?;
    let terminal = std::io::stdout().is_terminal() && !json;
    let mut shown = 0;
    while count.is_none_or(|count| shown < count) {
        std::thread::sleep(interval);
        let now = match program.ask("counters") {
            Ok(now) => now,
            Err(ended) if shown > 0 => {
                eprintln!("tondo: {ended}");
                return Ok(());
            }
            Err(ended) => return Err(ended),
        };
        let rows = rows(&last, &now);
        if json {
            println!("{}", json!({ "pid": program.pid, "uptime_ms": now["uptime_ms"], "counters": rows }));
        } else {
            let mut out = String::new();
            if terminal {
                out.push_str("\x1b[H\x1b[2J");
            } else if shown > 0 {
                out.push('\n');
            }
            let uptime = duration(now["uptime_ms"].as_f64().unwrap_or(0.0) / 1000.0);
            out.push_str(&format!("{} (pid {}), up {uptime}\n", program_name(&info), program.pid));
            out.push_str(&table(&rows));
            print!("{out}");
        }
        let _ = std::io::stdout().flush();
        last = now;
        shown += 1;
    }
    Ok(())
}

/// Each counter with its value, and each total with its rate per second
/// since `last`.
fn rows(last: &Value, now: &Value) -> Vec<Value> {
    let seconds = (now["uptime_ms"].as_f64().unwrap_or(0.0) - last["uptime_ms"].as_f64().unwrap_or(0.0)) / 1000.0;
    let before = |name: &Value| {
        last["counters"].as_array().into_iter().flatten().find(|c| c["name"] == *name).and_then(|c| c["value"].as_i64())
    };
    let mut rows = Vec::new();
    for counter in now["counters"].as_array().into_iter().flatten() {
        let value = counter["value"].as_i64().unwrap_or(0);
        let mut row = json!({ "name": counter["name"], "unit": counter["unit"], "value": value });
        if counter["kind"] == "counter" {
            let rate = before(&counter["name"]).map_or(0.0, |b| (value - b) as f64 / seconds.max(0.001));
            row["rate"] = json!((rate * 10.0).round() / 10.0);
        }
        rows.push(row);
    }
    rows
}

/// The rows grouped by the first part of their names: a level as it is, a
/// total as its rate and, after it, the total.
fn table(rows: &[Value]) -> String {
    let mut out = String::new();
    let mut group = "";
    for row in rows {
        let name = row["name"].as_str().unwrap_or_default();
        let (first, label) = name.split_once('.').unwrap_or(("", name));
        if first != group {
            out.push_str(&format!("\n{first}\n"));
            group = first;
        }
        let unit = row["unit"].as_str().unwrap_or_default();
        let value = row["value"].as_i64().unwrap_or(0);
        let line = match row["rate"].as_f64() {
            Some(rate) => format!("{:>16}   total {}", rate_text(rate, unit), amount(value as f64, unit)),
            None => format!("{:>16}", amount(value as f64, unit)),
        };
        out.push_str(&format!("  {label:<24}{line}\n"));
    }
    out
}

fn amount(value: f64, unit: &str) -> String {
    match unit {
        "bytes" => bytes(value),
        "us" => duration(value / 1_000_000.0),
        _ => format!("{value}"),
    }
}

/// A rate: of bytes per second, of time as the share of each second it
/// took (more than 100% on several cores), or of events per second.
fn rate_text(rate: f64, unit: &str) -> String {
    match unit {
        "bytes" => format!("{}/s", bytes(rate)),
        "us" => format!("{:.1}%", rate / 10_000.0),
        _ => format!("{rate:.1}/s"),
    }
}

fn bytes(value: f64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = value;
    let mut unit = 0;
    while value.abs() >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{value:.0} B") } else { format!("{value:.1} {}", units[unit]) }
}

fn duration(seconds: f64) -> String {
    match seconds {
        s if s < 0.001 => format!("{:.0} us", s * 1_000_000.0),
        s if s < 1.0 => format!("{:.1} ms", s * 1000.0),
        s if s < 60.0 => format!("{s:.1} s"),
        s if s < 3600.0 => format!("{}m {:02}s", (s / 60.0) as u64, s as u64 % 60),
        s => format!("{}h {:02}m", (s / 3600.0) as u64, (s as u64 % 3600) / 60),
    }
}

// ---------------------------------------------------------------------------
// Traces
// ---------------------------------------------------------------------------

/// The program `tondo trace` traces: one that runs, by process ID or name,
/// or an executable it starts with its arguments.
pub enum Traced {
    Running(String),
    Started(PathBuf, Vec<std::ffi::OsString>),
}

static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// What a trace gathered.
#[derive(Default)]
pub(crate) struct Recording {
    clock: u64,
    events: Vec<Value>,
    pub(crate) samples: Vec<Value>,
    pub(crate) frames: serde_json::Map<String, Value>,
    /// The sets of labels the samples name, by number.
    pub(crate) labels: serde_json::Map<String, Value>,
    pub(crate) dropped: u64,
    pub(crate) lost: u64,
}

/// What `record` got: the program's `info` and process, what the trace
/// recorded, how a program it started ended when not well and not by the
/// trace, and the answer to what it asked as it stopped, or the memory a
/// program that exited said it had.
pub(crate) struct Recorded {
    pub(crate) info: Value,
    pub(crate) pid: u32,
    pub(crate) recording: Recording,
    pub(crate) end: Option<String>,
    pub(crate) asked: Option<Value>,
    pub(crate) started: std::time::Instant,
}

/// Records what a program does until it ends, `duration` passes or
/// Ctrl-C, which ends a program it started too. Before it stops a program
/// that runs, it asks it `ask`, as `memory` or `dump`, on a connection of
/// its own. A program it starts counts its memory only when `counting`.
pub(crate) fn record(
    traced: Traced,
    duration: Option<Duration>,
    ask: Option<&str>,
    counting: bool,
) -> Result<Recorded, String> {
    let (mut program, mut child) = match traced {
        Traced::Running(name) => (connect(&name)?, None),
        Traced::Started(executable, arguments) => {
            use std::os::unix::process::CommandExt;
            // Its own process group, so that Ctrl-C ends the trace first.
            let child = std::process::Command::new(&executable)
                .args(&arguments)
                .env("TONDO_TRACE", if counting { "1" } else { "cpu" })
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
            let program = started(&executable, child.id())?;
            (program, Some(child))
        }
    };
    let info = program.ask("info")?;
    let reply = program.ask("trace")?;
    if let Some(error) = reply["error"].as_str() {
        return Err(format!("the program {} cannot be traced: {error}", program.pid));
    }
    unsafe { libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t) };
    let mut recording = Recording { clock: reply["clock"].as_u64().unwrap_or(0), ..Recording::default() };
    let _ = program.reader.get_ref().set_read_timeout(Some(Duration::from_millis(200)));
    let started_at = std::time::Instant::now();
    let mut stopping = false;
    let mut asked = None;
    let mut line = String::new();
    loop {
        let stop = INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
            || duration.is_some_and(|duration| started_at.elapsed() >= duration);
        if stop && !stopping {
            stopping = true;
            if let Some(request) = ask {
                asked = connect(&program.pid.to_string()).ok().and_then(|mut other| other.ask(request).ok());
            }
            let _ = writeln!(program.writer, "stop");
        }
        match program.reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let Ok(chunk) = serde_json::from_str::<Value>(&line) else { break };
                line.clear();
                if chunk.get("end").is_some() {
                    // A program that exits says what memory it had.
                    if !chunk["memory"].is_null() && asked.is_none() {
                        if let Some(Value::Object(frames)) = chunk.get("frames") {
                            recording.frames.extend(frames.clone());
                        }
                        asked = Some(json!({ "memory": chunk["memory"], "frames": {} }));
                    }
                    break;
                }
                recording.add(chunk);
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    let mut end = None;
    if let Some(child) = child.as_mut() {
        // A trace stopped early ends the program it started at once, even
        // one that computes without waiting, which `SIGINT` would cancel
        // only at its next wait; otherwise how the program ended is told,
        // unless it ended well.
        if stopping {
            let _ = child.kill();
        }
        if let Ok(status) = child.wait()
            && !status.success()
            && !stopping
        {
            end = Some(ended(status, "the program"));
        }
    }
    let pid = program.pid;
    Ok(Recorded { info, pid, recording, end, asked, started: started_at })
}

/// `tondo trace`: records until the program ends, `duration` passes or
/// Ctrl-C, then writes the timeline and the profile.
pub fn trace(traced: Traced, duration: Option<Duration>, output: Option<PathBuf>, as_json: bool) -> Result<(), String> {
    let Recorded { info, pid, recording, end, .. } = record(traced, duration, None, true)?;
    let base = output
        .map(|path| path.with_extension(""))
        .unwrap_or_else(|| PathBuf::from(format!("{}-{}", program_name(&info), pid)));
    let timeline = base.with_extension("json");
    let profile = base.with_extension("folded");
    let write = |path: &PathBuf, text: String| {
        std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    };
    let names = Names::new(&recording.frames, info["program"].as_str().unwrap_or_default());
    write(&timeline, recording.timeline(&info, pid, &names))?;
    write(&profile, recording.profile(&names))?;
    let mut summary = format!(
        "{} events and {} samples: {} (open it in https://ui.perfetto.dev) and {} (a CPU profile for flamegraph.pl, inferno or speedscope)",
        recording.events.len(),
        recording.samples.len(),
        timeline.display(),
        profile.display()
    );
    if recording.dropped + recording.lost > 0 {
        summary.push_str(&format!(
            "; {} events and {} samples were lost, too many to keep",
            recording.dropped, recording.lost
        ));
    }
    if as_json {
        let fields = json!({
            "timeline": timeline,
            "profile": profile,
            "events": recording.events.len(),
            "samples": recording.samples.len(),
            "lost": { "events": recording.dropped, "samples": recording.lost },
            "ended": end,
        });
        crate::cli::print_json("trace", true, fields);
        return Ok(());
    }
    if let Some(end) = end {
        summary.push_str(&format!("; {end}"));
    }
    eprintln!("tondo: {summary}");
    Ok(())
}

/// How a process ended, as a report says it: `who` ended by a signal, or
/// exited with a status.
pub(crate) fn ended(status: std::process::ExitStatus, who: &str) -> String {
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        let name = match signal {
            4 => "SIGILL",
            6 => "SIGABRT",
            7 if cfg!(target_os = "linux") => "SIGBUS",
            10 if cfg!(target_os = "macos") => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            11 => "SIGSEGV",
            15 => "SIGTERM",
            _ => return format!("{who} ended by signal {signal}"),
        };
        return format!("{who} ended by signal {signal} ({name})");
    }
    format!("{who} exited with status {}", status.code().unwrap_or(-1))
}

/// Connects to a program this trace started, once it listens.
pub(crate) fn started(executable: &std::path::Path, pid: u32) -> Result<Program, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(program) = Program::connect(pid) {
            return Ok(program);
        }
        if !alive(pid) {
            return Err(format!("{} ended before it could be traced", executable.display()));
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("{} does not answer: it is not a Tondo program", executable.display()));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl Recording {
    fn add(&mut self, chunk: Value) {
        let Value::Object(mut chunk) = chunk else { return };
        if let Some(Value::Array(events)) = chunk.remove("events") {
            self.events.extend(events);
        }
        if let Some(Value::Array(samples)) = chunk.remove("samples") {
            self.samples.extend(samples);
        }
        if let Some(Value::Object(frames)) = chunk.remove("frames") {
            self.frames.extend(frames);
        }
        if let Some(Value::Object(labels)) = chunk.remove("labels") {
            self.labels.extend(labels);
        }
        self.dropped += chunk.get("dropped").and_then(Value::as_u64).unwrap_or(0);
        self.lost += chunk.get("lost").and_then(Value::as_u64).unwrap_or(0);
    }

    /// Microseconds since the trace started, as Trace Event Format times.
    fn micros(&self, nanos: &Value) -> f64 {
        round(nanos.as_u64().unwrap_or(0).saturating_sub(self.clock) as f64 / 1000.0)
    }

    /// The timeline in Trace Event Format: each thread a track, with a
    /// slice for each run of a task and for each long call of C, and the
    /// collector's pauses on a track of their own.
    fn timeline(&self, info: &Value, pid: u32, names: &Names) -> String {
        let mut out =
            vec![json!({ "ph": "M", "name": "process_name", "pid": pid, "args": { "name": program_name(info) } })];
        out.push(json!({ "ph": "M", "name": "thread_name", "pid": pid, "tid": 0, "args": { "name": "collector" } }));
        // What each task runs, from its spawn.
        let mut labels: std::collections::HashMap<u64, String> = std::collections::HashMap::from([(1, "main".into())]);
        // What each task runs: from its spawn, or for one spawned before
        // the trace, the outermost function where it waits.
        for event in &self.events {
            let task = event[3].as_u64().unwrap_or(0);
            let stack = match event[0].as_str() {
                Some("spawn") => &event[5],
                Some("stop") if event[4] == "waits" && !labels.contains_key(&task) => &event[5],
                _ => continue,
            };
            // At each address the outermost function, the one its code was
            // inlined in.
            let outermost =
                stack.as_array().into_iter().flatten().rev().filter_map(Value::as_str).find_map(|address| {
                    let frame = self.frames.get(address)?;
                    let calls = frame["inlined"].as_array().and_then(|calls| calls.last());
                    calls.and_then(|call| call["function"].as_str()).or_else(|| frame["function"].as_str())
                });
            if let Some(function) = outermost {
                labels.insert(task, function.strip_prefix("spawn ").unwrap_or(function).to_string());
            }
        }
        let label = |task: u64| labels.get(&task).cloned().unwrap_or_else(|| format!("task {task}"));
        let mut threads = std::collections::BTreeSet::new();
        let mut running: std::collections::HashMap<u64, (u64, f64)> = std::collections::HashMap::new();
        for event in &self.events {
            let (ts, tid) = (self.micros(&event[1]), event[2].as_u64().unwrap_or(0));
            threads.insert(tid);
            let task = event[3].as_u64().unwrap_or(0);
            match event[0].as_str().unwrap_or_default() {
                "run" => {
                    running.insert(tid, (task, ts));
                }
                "stop" => {
                    let Some((_, start)) = running.remove(&tid) else { continue };
                    let then = match event[4].as_str().unwrap_or_default() {
                        "done" => "finished".to_string(),
                        "yields" => "let others run".to_string(),
                        _ => format!("waits at {}", names.place(&event[5])),
                    };
                    out.push(json!({ "ph": "X", "name": label(task), "pid": pid, "tid": tid, "ts": start, "dur": round(ts - start),
                        "args": { "task": task, "then": then } }));
                }
                "spawn" => {
                    let child = task;
                    out.push(
                        json!({ "ph": "i", "s": "t", "name": format!("spawns {}", label(child)), "pid": pid, "tid": tid,
                        "ts": ts, "args": { "task": child, "at": names.place(&event[6]) } }),
                    );
                }
                "pause" => {
                    let took = event[3].as_f64().unwrap_or(0.0) / 1000.0;
                    out.push(json!({ "ph": "X", "name": "collector pause", "pid": pid, "tid": 0, "ts": ts - took, "dur": took }));
                }
                "call" => {
                    let took = event[4].as_f64().unwrap_or(0.0) / 1000.0;
                    out.push(
                        json!({ "ph": "X", "name": "call of C", "pid": pid, "tid": tid, "ts": ts - took, "dur": took,
                        "args": { "task": task, "at": names.place(&event[5]) } }),
                    );
                }
                "panic" => {
                    out.push(json!({ "ph": "i", "s": "g", "name": "panic", "pid": pid, "tid": tid, "ts": ts,
                        "args": { "task": task, "message": event[4] } }));
                }
                _ => {}
            }
        }
        for sample in &self.samples {
            threads.insert(sample[1].as_u64().unwrap_or(0));
        }
        // A task that runs when the trace ends, or whose stop its thread
        // had not recorded as the program ended, runs to the end.
        let times = self.events.iter().map(|event| &event[1]).chain(self.samples.iter().map(|sample| &sample[0]));
        let end = times.map(|at| self.micros(at)).fold(0.0, f64::max);
        for (tid, (task, start)) in running {
            out.push(
                json!({ "ph": "X", "name": label(task), "pid": pid, "tid": tid, "ts": start, "dur": round(end - start),
                "args": { "task": task, "then": "ran as the trace ended" } }),
            );
        }
        for tid in threads {
            let name = if tid == 1 { "main thread".to_string() } else { format!("thread {tid}") };
            out.push(json!({ "ph": "M", "name": "thread_name", "pid": pid, "tid": tid, "args": { "name": name } }));
        }
        json!({ "traceEvents": out, "displayTimeUnit": "ms" }).to_string()
    }

    /// The samples as folded stacks: the frames from the outermost, joined
    /// by `;`, and how many samples had them.
    fn profile(&self, names: &Names) -> String {
        let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for sample in &self.samples {
            let frames: Vec<String> = sample[2]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .flat_map(|a| names.functions(a.as_str().unwrap_or_default()).into_iter().rev())
                .collect();
            *counts.entry(frames.join(";")).or_default() += 1;
        }
        counts.iter().map(|(stack, count)| format!("{stack} {count}\n")).collect()
    }
}

/// The names of the addresses of a trace: the program's functions as the
/// runtime describes them, and others from the symbols of the file that
/// holds them.
pub(crate) struct Names<'a> {
    frames: &'a serde_json::Map<String, Value>,
    program: String,
    symbols: std::cell::RefCell<std::collections::HashMap<String, Vec<Function>>>,
}

impl<'a> Names<'a> {
    pub(crate) fn new(frames: &'a serde_json::Map<String, Value>, program: &str) -> Names<'a> {
        Names { frames, program: program.to_string(), symbols: Default::default() }
    }

    /// A frame's function.
    pub(crate) fn function(&self, address: &str) -> String {
        let frame = &self.frames[address];
        if let Some(function) = frame["function"].as_str() {
            return function.to_string();
        }
        let image = frame["image"].as_str().filter(|i| !i.is_empty()).unwrap_or(&self.program).to_string();
        let offset = frame["offset"].as_u64().unwrap_or(0);
        match self.symbol(&image, offset) {
            Some(symbol) => symbol,
            None => format!("{}+{offset:#x}", image.rsplit('/').next().unwrap_or(&image)),
        }
    }

    /// A frame's function, and those its code was inlined into, from the
    /// innermost out.
    pub(crate) fn functions(&self, address: &str) -> Vec<String> {
        let inlined = self.frames[address]["inlined"].as_array().into_iter().flatten();
        let outer = inlined.filter_map(|frame| frame["function"].as_str().map(String::from));
        std::iter::once(self.function(address)).chain(outer).collect()
    }

    /// Where the innermost frame of the program in a stack is: its function,
    /// file and line.
    fn place(&self, stack: &Value) -> String {
        let addresses = stack.as_array().into_iter().flatten().filter_map(Value::as_str);
        for address in addresses {
            let frame = &self.frames[address];
            if let (Some(function), Some(file)) = (frame["function"].as_str(), frame["file"].as_str()) {
                return format!("{file}:{} in {function}", frame["line"]);
            }
        }
        "an unknown place".into()
    }

    /// The function of a file whose code holds `offset`, from where the
    /// file is loaded.
    fn symbol(&self, image: &str, offset: u64) -> Option<String> {
        let mut cache = self.symbols.borrow_mut();
        let symbols = cache.entry(image.to_string()).or_insert_with(|| functions(image));
        let at = symbols.partition_point(|(address, ..)| *address <= offset).checked_sub(1)?;
        let (address, size, name) = &symbols[at];
        (*size == 0 || offset < address + size).then(|| name.clone())
    }
}

/// A function a file defines: its offset from where the file is loaded,
/// its size and its name.
type Function = (u64, u64, String);

fn functions(path: &str) -> Vec<Function> {
    use object::{Object, ObjectSegment, ObjectSymbol};
    let Ok(data) = std::fs::read(path) else { return Vec::new() };
    let Ok(file) = object::File::parse(data.as_slice()) else { return Vec::new() };
    // Offsets count from where the file's header is loaded: the segment
    // that maps it, at 0x1_0000_0000 in an executable for macOS.
    let base = file
        .segments()
        .find(|s| s.file_range().0 == 0 && s.file_range().1 > 0)
        .map_or(file.relative_address_base(), |s| s.address());
    // Mach-O writes every symbol with an underscore before its name.
    let macho = file.format() == object::BinaryFormat::MachO;
    let name = |raw: &str| demangled(if macho { raw.strip_prefix('_').unwrap_or(raw) } else { raw });
    let mut out: Vec<Function> = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|s| s.kind() == object::SymbolKind::Text && s.address() >= base)
        .filter_map(|s| Some((s.address() - base, s.size(), name(s.name().ok()?))))
        .collect();
    out.sort();
    out.dedup_by_key(|(address, ..)| *address);
    out
}

/// Microseconds to the nanosecond.
fn round(micros: f64) -> f64 {
    (micros * 1000.0).round() / 1000.0
}

/// A symbol as Rust or C would write it: without the hash Rust adds.
fn demangled(symbol: &str) -> String {
    format!("{:#}", rustc_demangle::demangle(symbol))
}

// ---------------------------------------------------------------------------
// Dumps
// ---------------------------------------------------------------------------

/// `tondo dump <pid|name>`: asks a running program for a dump and writes
/// it, with every frame named, so that it reads anywhere.
pub fn dump(name: &str, output: Option<PathBuf>, as_json: bool) -> Result<(), String> {
    let mut program = connect(name)?;
    let _ = program.reader.get_ref().set_read_timeout(Some(Duration::from_secs(60)));
    let mut dump = program.ask("dump")?;
    if dump.get("dump").is_none() {
        return Err(format!("the program {} gave no dump: {dump}", program.pid));
    }
    name_frames(&mut dump);
    let path =
        output.unwrap_or_else(|| PathBuf::from(format!("{}-{}.dump.json", program_name(&dump["info"]), program.pid)));
    let text = serde_json::to_string_pretty(&dump).unwrap_or_default();
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    if as_json {
        crate::cli::print_json("dump", true, json!({ "file": path, "summary": summary(&dump) }));
        return Ok(());
    }
    print!("{}", summary(&dump));
    eprintln!("tondo: the dump is in {}; `tondo dump read {}` shows it", path.display(), path.display());
    Ok(())
}

/// `tondo dump plugin <lldb|gdb>`: writes the debugger's `tondo` command,
/// which has the program write a dump and shows it with `dump read`, or
/// answer `locals` and `print` in a frame, and says how the debugger loads
/// it.
pub fn plugin(debugger: &str, json: bool) -> Result<(), String> {
    let (script, load) = match debugger {
        "lldb" => (include_str!("plugins/tondo_lldb.py"), "command script import"),
        "gdb" => (include_str!("plugins/tondo_gdb.py"), "source"),
        other => return Err(format!("dump plugin is for lldb or gdb, not `{other}`")),
    };
    let tondo = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = crate::driver::cache_dir().join("plugins");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot write to {}: {e}", dir.display()))?;
    let path = dir.join(format!("tondo_{debugger}.py"));
    let script = script.replace("@TONDO@", &tondo.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
    std::fs::write(&path, script).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let command = format!("{load} {}", path.display());
    match json {
        true => crate::cli::print_json("dump-plugin", true, json!({ "file": path, "load": command })),
        false => println!("{command}"),
    }
    Ok(())
}

/// What `tondo dump read` shows of a dump.
pub enum View {
    Summary,
    Tasks,
    Task(u64),
    Heap,
    /// Every object of a debug build, by type, or one and what holds it.
    Objects,
    Object(String),
    /// The bindings of a frame: of a task, the first with some by default.
    Locals(Option<u64>, usize),
    /// A binding or a part of one, as `user.name` or `items[2]`.
    Print(String, Option<u64>, usize),
    /// What the dump says went wrong: who waits for whom, and where the
    /// memory is.
    Why,
    /// The tasks by the stack they share.
    Stacks,
    /// The requests in flight, the oldest first.
    Requests,
    /// What the program did last.
    Events,
    /// Where its files came from, and whether those at hand are the same.
    Sources,
    /// Where a text is: in values, stacks, contexts and the memory's sites.
    Find(String),
}

/// The frame of a dump's task that a view names.
fn dump_frame(tasks: &[Value], task: Option<u64>, frame: usize) -> Result<&Value, String> {
    let has_frames = |t: &&Value| t["frames"].as_array().is_some_and(|frames| !frames.is_empty());
    let chosen = match task {
        Some(id) => tasks.iter().find(|t| t["id"] == id).ok_or(format!("the dump has no task {id}"))?,
        None => tasks.iter().find(has_frames).ok_or(NO_BINDINGS)?,
    };
    let frames = chosen["frames"].as_array().filter(|frames| !frames.is_empty()).ok_or(NO_BINDINGS)?;
    let shown =
        |frames: &[Value]| frames.iter().map(|f| f["at"].as_str().unwrap_or_default().to_string()).collect::<Vec<_>>();
    frames.get(frame).ok_or(format!("that task has bindings in these frames: {}", shown(frames).join(", ")))
}

const NO_BINDINGS: &str = "this dump has no bindings of that task: a task must have reached a line of the program";

/// What a path names in a frame of a dump: a binding, then its parts, each
/// by the name `.field`, `[2]` or `["key"]`.
pub fn dumped_value<'a>(frame: &'a Value, path: &str) -> Option<&'a Value> {
    let end = path.find(['.', '[']).unwrap_or(path.len());
    let (name, mut rest) = path.split_at(end);
    let bindings = frame["bindings"].as_array()?;
    let mut value = bindings.iter().find(|binding| binding["name"] == name)?;
    while !rest.is_empty() {
        let parts = value["parts"].as_array()?;
        let part = parts.iter().find(|part| part["name"].as_str().is_some_and(|name| rest.starts_with(name)))?;
        rest = &rest[part["name"].as_str()?.len()..];
        value = part;
    }
    Some(value)
}

/// What a dump's frames in `file` held in a binding named `name`: each
/// different value, with its task and line, for an editor to show.
pub fn dumped_bindings(dump: &Value, file: &str, name: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for task in tasks(dump) {
        for frame in task["frames"].as_array().into_iter().flatten() {
            let at = frame["at"].as_str().unwrap_or_default();
            if !at.starts_with(&format!("{file}:")) {
                continue;
            }
            let bindings = frame["bindings"].as_array().into_iter().flatten();
            for binding in bindings.filter(|binding| binding["name"] == name) {
                let value = binding["value"].as_str().unwrap_or_default();
                let line = format!("`{value}` (task {}, {at})", task["id"]);
                if !found.contains(&line) {
                    found.push(line);
                }
            }
        }
    }
    found
}

/// The JSON of a dump's file, compressed or not.
fn raw(path: &std::path::Path, bytes: &[u8]) -> Result<Value, String> {
    let mut text = Vec::new();
    let bytes = match bytes.starts_with(&[0x1f, 0x8b]) {
        true => {
            let mut decoder = flate2::read::GzDecoder::new(bytes);
            std::io::Read::read_to_end(&mut decoder, &mut text)
                .map_err(|e| format!("{} is not a dump: {e}", path.display()))?;
            &text[..]
        }
        false => bytes,
    };
    serde_json::from_slice(bytes).map_err(|e| format!("{} is not a dump: {e}", path.display()))
}

/// A dump from its file, compressed or not, with every frame named.
pub fn load(path: &std::path::Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut dump = match bytes.starts_with(b"\x7fELF") || bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        true => crate::cores::read(path, None)?,
        false => raw(path, &bytes)?,
    };
    if dump.get("dump").is_none() {
        return Err(format!("{} is not a dump of a Tondo program", path.display()));
    }
    name_frames(&mut dump);
    Ok(dump)
}

/// The dumps in a directory, the oldest first; those that do not read are
/// left out.
fn dumps_in(dir: &std::path::Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let is_dump = |path: &PathBuf| {
        let name = path.to_string_lossy();
        name.ends_with(".dump.json") || name.ends_with(".dump.json.gz")
    };
    let files = entries.flatten().map(|entry| entry.path()).filter(is_dump);
    let mut dumps: Vec<(PathBuf, Value)> = files.filter_map(|path| Some((path.clone(), load(&path).ok()?))).collect();
    dumps.sort_by(|a, b| a.1["time_ms"].as_f64().unwrap_or(0.0).total_cmp(&b.1["time_ms"].as_f64().unwrap_or(0.0)));
    Ok(dumps)
}

/// What says two dumps are the same failure: why the program stopped, by
/// its code and not its message, which holds values; and the functions it
/// was in, without their lines, which move from a build to the next.
fn signature(dump: &Value) -> String {
    let tasks = tasks(dump);
    let running = tasks.iter().find(|t| t["state"] == "runs" && !tondo_frames(t).is_empty());
    let task = running.or(tasks.first());
    let functions = task.map_or(Vec::new(), tondo_frames);
    let functions: Vec<&str> = functions.iter().filter_map(|f| f.split_once(" in ").map(|(_, f)| f)).take(5).collect();
    let message = dump["message"].as_str().unwrap_or_default();
    let code = message.split(':').next().filter(|code| code.starts_with("panic[")).unwrap_or_default();
    let parts = [dump["reason"].as_str().unwrap_or_default(), code, &functions.join("\n")];
    crate::driver::fingerprint(&parts.map(str::as_bytes))[..12].to_string()
}

/// The commit of the program a dump is of, if it says.
fn commit_of(dump: &Value) -> Value {
    roots_of(dump).roots.first().filter(|root| root.prefix.is_empty()).map_or(Value::Null, |root| json!(root.commit))
}

/// What `dump test` found to call: where the test goes, and what it says.
struct Reproduction {
    file: PathBuf,
    name: String,
    text: String,
}

/// A test that calls the function of `frame` of the task that failed with
/// what the dump says it had; the innermost it can, when no frame is asked.
fn reproduction(dump: &Value, frame: Option<u64>) -> Result<Reproduction, String> {
    let tasks = tasks(dump);
    let running = tasks.iter().find(|t| t["state"] == "runs" && !tondo_frames(t).is_empty());
    let task = running.or(tasks.first()).ok_or("the dump has no tasks")?;
    let values = dump["values"].as_array().map_or(&[][..], Vec::as_slice);
    let of_task: Vec<&Value> = values.iter().filter(|v| v["task"] == task["id"]).collect();
    let mut frames: Vec<u64> = of_task.iter().filter_map(|v| v["frame"].as_u64()).collect();
    frames.dedup();
    let mut why = String::new();
    for &at in frames.iter().filter(|&&at| frame.is_none_or(|frame| frame == at)) {
        let bound: Vec<&&Value> = of_task.iter().filter(|v| v["frame"] == at).collect();
        let place = bound[0]["at"].as_str().unwrap_or_default();
        let Some((file, function)) = place.split_once(" in ") else { continue };
        let file = file.rsplit_once(':').map_or(file, |(file, _)| file);
        if function == "main" || function.starts_with("spawn ") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            why = format!("{file} is not here: run this where the program's sources are");
            continue;
        };
        let Some(start) = text.find(&format!("fn {function}(")).or_else(|| text.find(&format!("fn {function}[")))
        else {
            why = format!("`{function}` is not in {file}: it changed since this build");
            continue;
        };
        let after = start + "fn ".len() + function.len();
        let Some(list) = crate::refactor::list_at(&text, after) else { continue };
        let names: Vec<&str> = list.items.iter().map(|&(a, b)| crate::refactor::parameter_name(&text[a..b])).collect();
        let value = |name: &str| bound.iter().find(|v| v["name"] == name).and_then(|v| v["value"].as_str());
        if let Some(missing) = names.iter().find(|name| value(name).is_none_or(|value| value.contains('?'))) {
            why = format!("the dump has no value of `{missing}` in `{function}`: a build with --debug keeps them all");
            continue;
        }
        let rest = text[list.close + 1..].lines().next().unwrap_or_default();
        let question = if rest.contains('!') { "?" } else { "" };
        let gives = rest.trim_start().starts_with(':') && !rest.trim_start().starts_with(": !");
        let mut body = String::new();
        let mut arguments = Vec::new();
        for name in &names {
            let shown = value(name).unwrap_or_default();
            let bound = if *name == "self" { "receiver" } else { name };
            body.push_str(&format!("    let {bound} = {shown}\n"));
            if *name != "self" {
                arguments.push(name.to_string());
            }
        }
        let called = match (names.first(), function.split_once('.')) {
            (Some(&"self"), Some((_, method))) => format!("receiver.{method}({})", arguments.join(", ")),
            _ => format!("{function}({})", arguments.join(", ")),
        };
        body.push_str(&match gives {
            true => format!("    _ = {called}{question}\n"),
            false => format!("    {called}{question}\n"),
        });
        let path = std::path::Path::new(file);
        let stem = path.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned());
        let tests = path.with_file_name(format!("{stem}_test.to"));
        let existing = std::fs::read_to_string(&tests).unwrap_or_default();
        let base: String = function.split('.').map(|part| part[..1].to_uppercase() + &part[1..]).collect();
        let mut name = format!("failedIn{base}");
        let mut number = 1;
        while existing.contains(&format!("test {name} ")) {
            number += 1;
            name = format!("failedIn{base}{number}");
        }
        let message = dump["message"].as_str().unwrap_or_default().lines().next().unwrap_or_default().to_string();
        let test = format!("// From a dump: {message}\ntest {name} {{\n{body}}}\n");
        return Ok(Reproduction { file: tests, name, text: test });
    }
    Err(match why.is_empty() {
        true => "the dump has no function with values to call".to_string(),
        false => why,
    })
}

/// `tondo dump test <file> [--frame <n>]`: writes the test of a failure in a
/// dump, next to the function, and says whether it fails as the dump did.
pub fn dump_test(path: &std::path::Path, frame: Option<u64>, json: bool) -> Result<(), String> {
    let dump = load(path)?;
    let found = reproduction(&dump, frame)?;
    let before = std::fs::read_to_string(&found.file).ok();
    let gap = match before.as_deref() {
        None | Some("") => "",
        Some(text) if text.ends_with('\n') => "\n",
        Some(_) => "\n\n",
    };
    let written = format!("{}{gap}{}", before.as_deref().unwrap_or_default(), found.text);
    std::fs::write(&found.file, &written).map_err(|e| format!("cannot write {}: {e}", found.file.display()))?;
    // A value may not write back as code: what cannot compile does not stay.
    let tondo = std::env::current_exe().map_err(|e| e.to_string())?;
    let ran = std::process::Command::new(&tondo)
        .args(["test", "--filter", &found.name, "--json"])
        .current_dir(project_of(&found.file))
        .output()
        .map_err(|e| e.to_string())?;
    let report: Value = serde_json::from_slice(&ran.stdout).unwrap_or(Value::Null);
    if report["compiled"] == false || report["summary"]["executed"].as_u64().unwrap_or(0) == 0 {
        match before {
            Some(text) => std::fs::write(&found.file, text),
            None => std::fs::remove_file(&found.file),
        }
        .map_err(|e| e.to_string())?;
        let said = String::from_utf8_lossy(&ran.stderr);
        let said = said.lines().find(|line| line.starts_with("error")).unwrap_or("it does not compile");
        return Err(format!("a value does not write back as code: {said}\nthe test was:\n{}", found.text));
    }
    let reproduces = !ran.status.success();
    let shown = found.file.display();
    match json {
        true => crate::cli::print_json(
            "dump-test",
            true,
            json!({ "file": found.file, "test": found.name, "reproduces": reproduces }),
        ),
        false if reproduces => println!("{shown}: `{}` fails as the dump did", found.name),
        false => println!("{shown}: `{}` passes: the failure takes more than these values", found.name),
    }
    Ok(())
}

/// The project a file is in, or where the file is.
fn project_of(file: &std::path::Path) -> PathBuf {
    let directory = file.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let directory = directory.canonicalize().unwrap_or_else(|_| directory.to_path_buf());
    directory.ancestors().find(|dir| dir.join("tondo.toml").exists()).unwrap_or(&directory).to_path_buf()
}

/// `tondo dump redact <file> <output>`: the dump without what the program
/// held, to attach where anyone reads it: where it was and how it went
/// stay; values, contexts, requests, texts and the words of a panic go.
pub fn redact(path: &std::path::Path, output: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut dump = raw(path, &bytes)?;
    if dump.get("dump").is_none() {
        return Err(format!("{} is not a dump of a Tondo program", path.display()));
    }
    // Of what it says, only the code of a panic is not the program's.
    let code = dump["message"].as_str().unwrap_or_default().split(':').next().filter(|code| code.starts_with("panic["));
    dump["message"] = json!(code.unwrap_or_default());
    dump["info"]["arguments"] = json!([]);
    dump["values"] = json!([]);
    dump["values_off"] = json!(true);
    if let Some(object) = dump.as_object_mut() {
        object.remove("requests");
        object.remove("events");
    }
    for task in dump["tasks"].as_array_mut().into_iter().flatten() {
        task["contexts"] = json!([]);
        if task["wait"].is_object() {
            task["wait"]["for"] = json!("a value");
        }
    }
    for object in dump["objects"]["list"].as_array_mut().into_iter().flatten() {
        if let Some(fields) = object.as_object_mut() {
            fields.remove("text");
        }
    }
    let text = serde_json::to_string(&dump).unwrap_or_default();
    std::fs::write(output, text).map_err(|e| format!("cannot write {}: {e}", output.display()))
}

/// `tondo dump group <dir>`: the dumps of a directory by the failure they
/// are, the most frequent first.
pub fn group(dir: &std::path::Path, since: Option<&str>, json: bool) -> Result<(), String> {
    let mut groups = groups_in(dir)?;
    // What came with a change: failures first seen after that commit, as
    // the repository here says.
    if let Some(since) = since {
        groups.retain(|group| group["first_commit"].as_str().is_some_and(|first| after(first, since)));
    }
    if json {
        crate::cli::print_json("dump-group", true, json!({ "groups": groups }));
        return Ok(());
    }
    if groups.is_empty() {
        println!("{} has no dumps", dir.display());
    }
    for group in &groups {
        let text = |key: &str| group[key].as_str().unwrap_or_default();
        println!("{} x {} {}", group["count"], text("signature"), text("reason"));
        println!("  {}", text("message"));
        println!("  at {}", group["at"].as_str().unwrap_or("nowhere known"));
        if let Some(first) = group["first_commit"].as_str() {
            let short = |commit: &str| commit[..commit.len().min(10)].to_string();
            match group["last_commit"].as_str().filter(|last| *last != first) {
                Some(last) => println!("  first seen at {}, and still at {}", short(first), short(last)),
                None => println!("  seen at {}", short(first)),
            }
        }
        println!("  the last: {}", text("file"));
    }
    Ok(())
}

/// The failures in a directory of dumps, the most frequent first.
fn groups_in(dir: &std::path::Path) -> Result<Vec<Value>, String> {
    let mut groups: Vec<Value> = Vec::new();
    for (path, dump) in dumps_in(dir)? {
        let id = signature(&dump);
        let time = &dump["time_ms"];
        match groups.iter_mut().find(|g| g["signature"] == id) {
            Some(group) => {
                group["count"] = json!(group["count"].as_u64().unwrap_or(0) + 1);
                group["last_ms"] = time.clone();
                group["times"].as_array_mut().expect("a list").push(time.clone());
                group["file"] = json!(path);
                group["last_commit"] = commit_of(&dump);
                let known = group["lock_orders"].as_array_mut().expect("a list");
                for order in dump["lock_order"].as_array().into_iter().flatten() {
                    if !known.contains(order) {
                        known.push(order.clone());
                    }
                }
            }
            None => {
                let tasks = tasks(&dump);
                let at = tasks.iter().find(|t| t["state"] == "runs" && t["at"].is_string()).or(tasks.first());
                groups.push(json!({
                    "signature": id, "count": 1, "reason": dump["reason"], "file": path,
                    "message": dump["message"].as_str().unwrap_or_default().lines().next(),
                    "at": at.map(|t| &t["at"]), "first_ms": time, "last_ms": time,
                    "first_commit": commit_of(&dump), "last_commit": commit_of(&dump),
                    "stack": at.map_or(Vec::new(), tondo_frames),
                    "times": [time],
                    "lock_orders": dump["lock_order"].as_array().cloned().unwrap_or_default(),
                }));
            }
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group["count"].as_u64()));
    Ok(groups)
}

/// Whether `commit` came after `since`, in the repository here.
fn after(commit: &str, since: &str) -> bool {
    let ancestor = |first: &str, second: &str| {
        let run = std::process::Command::new("git").args(["merge-base", "--is-ancestor", first, second]).output();
        run.is_ok_and(|run| run.status.success())
    };
    ancestor(since, commit) && !ancestor(commit, since)
}

/// `tondo dump issue <dir> <signature>`: a failure of a directory of dumps
/// as an issue to file, in Markdown.
pub fn issue(dir: &std::path::Path, wanted: &str) -> Result<(), String> {
    let groups = groups_in(dir)?;
    let matching = |group: &&Value| group["signature"].as_str().is_some_and(|id| id.starts_with(wanted));
    let group = groups.iter().find(matching).ok_or_else(|| {
        format!("{} has no failure {wanted}: `tondo dump group {}` lists them", dir.display(), dir.display())
    })?;
    let text = |key: &str| group[key].as_str().unwrap_or_default();
    println!("# {}\n", text("message"));
    println!("Seen {} times, at {}.", group["count"], group["at"].as_str().unwrap_or("nowhere known"));
    match (group["first_commit"].as_str(), group["last_commit"].as_str()) {
        (Some(first), Some(last)) if first != last => println!("First seen at {first}, and still at {last}."),
        (Some(first), _) => println!("Seen at {first}."),
        _ => {}
    }
    println!("\n```");
    for frame in group["stack"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("{frame}");
    }
    println!("```\n\nSignature `{}`. The last dump is {}.", text("signature"), text("file"));
    Ok(())
}

/// How many times each failure fell, over the time of all of them, as a
/// row of bars.
fn bars(times: &[f64], from: f64, to: f64) -> String {
    const BARS: usize = 24;
    let mut counts = [0u32; BARS];
    let width = ((to - from) / BARS as f64).max(1.0);
    for time in times {
        counts[(((time - from) / width) as usize).min(BARS - 1)] += 1;
    }
    let most = counts.iter().copied().max().unwrap_or(1).max(1);
    let rects: String = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(at, count)| {
            let height = (*count as f64 / most as f64 * 20.0).max(2.0);
            format!("<rect x=\"{}\" y=\"{:.1}\" width=\"4\" height=\"{height:.1}\"/>", at * 5, 20.0 - height)
        })
        .collect();
    format!("<svg width=\"120\" height=\"20\" fill=\"#c33\" role=\"img\">{rects}</svg>")
}

/// A page of the failures of a directory of dumps, the most frequent first,
/// each with how it went over time; those first seen after `since` when
/// there is one, and with links when it is `live`.
fn failures_page(dir: &std::path::Path, live: bool, since: Option<&str>) -> Result<String, String> {
    use crate::profile::escape;
    let mut groups = groups_in(dir)?;
    let times = |group: &Value| -> Vec<f64> {
        group["times"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect()
    };
    let all: Vec<f64> = groups.iter().flat_map(times).collect();
    let from = all.iter().copied().fold(f64::MAX, f64::min);
    let to = all.iter().copied().fold(0.0, f64::max);
    // The commits there are to choose from, before any is chosen.
    let mut commits: Vec<String> =
        groups.iter().filter_map(|group| group["first_commit"].as_str().map(str::to_string)).collect();
    commits.sort();
    commits.dedup();
    if let Some(since) = since {
        groups.retain(|group| group["first_commit"].as_str().is_some_and(|first| after(first, since)));
    }
    let refresh = if live { "<meta http-equiv=\"refresh\" content=\"5\">" } else { "" };
    let mut rows = String::new();
    for group in &groups {
        let text = |key: &str| escape(group[key].as_str().unwrap_or_default());
        let signature = text("signature");
        let short = |key: &str| text(key).chars().take(10).collect::<String>();
        let name = match live {
            true => format!("<a href=\"/failure/{signature}\">{}</a>", text("message")),
            false => text("message"),
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{name}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            group["count"],
            bars(&times(group), from, to),
            text("at"),
            short("first_commit"),
            short("last_commit"),
        ));
    }
    let filters: String = match live && !commits.is_empty() {
        true => {
            let links: Vec<String> = commits
                .iter()
                .map(|commit| {
                    let short: String = escape(commit).chars().take(10).collect();
                    format!("<a href=\"/?since={}\">{short}</a>", escape(commit))
                })
                .collect();
            format!("<p>New after: <a href=\"/\">any</a> {}</p>", links.join(" "))
        }
        false => String::new(),
    };
    let shown = escape(&dir.display().to_string());
    Ok(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{refresh}<title>Failures of {shown}</title>\
         <style>body{{font:14px system-ui;margin:2em}}table{{border-collapse:collapse}}\
         td,th{{border-bottom:1px solid #ccc;padding:.4em .8em;text-align:left}}</style></head><body>\
         <h1>Failures of {shown}</h1><p>{} failures in {} dumps.</p>{filters}<table><tr><th>Times</th><th>Over time</th>\
         <th>Failure</th><th>Where</th><th>First seen</th><th>Last seen</th></tr>\n{rows}</table></body></html>\n",
        groups.len(),
        groups.iter().filter_map(|group| group["count"].as_u64()).sum::<u64>(),
    ))
}

/// `tondo dump serve <dir>`: the failures of a directory as a page on this
/// machine, up to date as dumps come; or written once, with `--output`.
pub fn serve(dir: &std::path::Path, port: u16, output: Option<&std::path::Path>) -> Result<(), String> {
    use std::io::Read;
    if let Some(output) = output {
        let page = failures_page(dir, false, None)?;
        return std::fs::write(output, page).map_err(|e| format!("cannot write {}: {e}", output.display()));
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot listen: {e}"))?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    println!("http://{address}/");
    let _ = std::io::stdout().flush();
    for stream in listener.incoming().flatten() {
        let mut stream = stream;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        // All of the head: what is left unread would reset the connection.
        let (mut request, mut part) = (Vec::new(), [0; 2048]);
        while !request.windows(4).any(|end| end == b"\r\n\r\n") && request.len() < 1 << 16 {
            match stream.read(&mut part) {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&part[..read]),
            }
        }
        let request = String::from_utf8_lossy(&request);
        let path = request.split_whitespace().nth(1).unwrap_or("/");
        // A failure is named by its signature: no path of the machine comes
        // from the request.
        let page = match path.strip_prefix("/failure/") {
            Some(wanted) => groups_in(dir).ok().and_then(|groups| {
                let group = groups.into_iter().find(|group| group["signature"] == wanted)?;
                let file = PathBuf::from(group["file"].as_str()?);
                Some(report_html(&load(&file).ok()?))
            }),
            None if path == "/" => failures_page(dir, true, None).ok(),
            // Only a commit: what `git` is given is not free text.
            None => match path.strip_prefix("/?since=").filter(|c| c.chars().all(|c| c.is_ascii_hexdigit())) {
                Some(since) => failures_page(dir, true, Some(since)).ok(),
                None => None,
            },
        };
        let (status, body) = match page {
            Some(body) => ("200 OK", body),
            None => ("404 Not Found", "nothing here".to_string()),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes()));
    }
    Ok(())
}

/// `tondo dump report <file>`: all a dump says, as one page to read or to
/// attach to an issue. A place is a link an editor follows.
pub fn report(path: &std::path::Path, output: Option<&std::path::Path>) -> Result<(), String> {
    let html = report_html(&load(path)?);
    match output {
        Some(output) => std::fs::write(output, html).map_err(|e| format!("cannot write {}: {e}", output.display())),
        None => {
            print!("{html}");
            Ok(())
        }
    }
}

fn report_html(dump: &Value) -> String {
    use crate::profile::escape;
    let text = |value: &Value| escape(value.as_str().unwrap_or_default());
    // A place of the program as a link, `file:line in function` or `file:line`.
    let place = |at: &str| -> String {
        let spot = at.split(" in ").next().unwrap_or(at);
        match spot.rsplit_once(':').filter(|(_, line)| line.parse::<u64>().is_ok()) {
            Some((file, line)) => {
                format!("<a href=\"#\" data-file=\"{}\" data-line=\"{line}\">{}</a>", escape(file), escape(at))
            }
            None => escape(at),
        }
    };
    let tasks = tasks(dump);
    let mut out = String::from(REPORT_HEAD);
    let info = &dump["info"];
    out.push_str(&format!("<h1>{}</h1>\n", escape(program_name(info))));
    out.push_str(&format!("<pre>{}</pre>\n", escape(summary(dump).trim_end())));
    // What other parts of the page say, it does not say twice.
    let mut found = why(dump);
    found["reason"] = json!("");
    found["events"] = json!([]);
    found["requests"] = json!([]);
    found["memory"] = json!([]);
    out.push_str(&format!("<h2>What stands out</h2>\n<pre>{}</pre>\n", escape(&why_text(&found))));
    let groups = stacks(&tasks);
    out.push_str(&format!("<h2>Tasks ({})</h2>\n", tasks.len()));
    for group in &groups {
        let ids: Vec<String> = group["tasks"].as_array().into_iter().flatten().map(Value::to_string).collect();
        let waited = group["waited_ms"].as_f64().filter(|ms| *ms > 0.0);
        let waited = waited.map_or(String::new(), |ms| format!(", the longest for {}", duration(ms / 1000.0)));
        let shown = ids.iter().take(20).cloned().collect::<Vec<_>>().join(", ");
        let frames: Vec<String> =
            group["stack"].as_array().into_iter().flatten().filter_map(Value::as_str).map(&place).collect();
        let state = group["state"].as_str().unwrap_or_default();
        let who = match ids.len() {
            1 => format!("1 task {}", escape(state)),
            count => format!("{count} tasks {}", escape(&plural(state))),
        };
        out.push_str(&format!(
            "<details open><summary>{who} <span class=\"dim\">({shown}){waited}</span></summary><ol>{}</ol></details>\n",
            frames.iter().map(|f| format!("<li>{f}</li>")).collect::<String>()
        ));
    }
    let requests = requests(dump);
    if !requests.is_empty() {
        out.push_str("<h2>Requests in flight</h2>\n<table><tr><th>Request</th><th>For</th><th>Task</th></tr>\n");
        for request in &requests {
            out.push_str(&format!(
                "<tr><td>{} {}</td><td>{}</td><td>{}</td></tr>\n",
                text(&request["method"]),
                text(&request["path"]),
                duration(request["ms"].as_f64().unwrap_or(0.0) / 1000.0),
                request["task"]
            ));
        }
        out.push_str("</table>\n");
    }
    let heap = heap(dump);
    let sites = heap["sites"].as_array().map_or(&[][..], Vec::as_slice);
    out.push_str("<h2>Live memory, sampled</h2>\n");
    if sites.is_empty() {
        out.push_str("<p class=\"dim\">No allocation was sampled.</p>\n");
    } else {
        out.push_str("<table><tr><th>Held</th><th>Kind</th><th>Made at</th></tr>\n");
    }
    for site in sites.iter().take(30) {
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            bytes(site["bytes"].as_f64().unwrap_or(0.0)),
            text(&site["kind"]),
            place(site["site"].as_str().unwrap_or_default())
        ));
    }
    if !sites.is_empty() {
        out.push_str("</table>\n");
    }
    let events = events(dump);
    if !events.is_empty() {
        out.push_str(&format!("<h2>What came last</h2>\n<pre>{}</pre>\n", escape(&events_text(&events))));
    }
    let where_from = sources(dump);
    if where_from["repositories"].as_array().is_some_and(|list| !list.is_empty()) {
        out.push_str(&format!("<h2>Sources</h2>\n<pre>{}</pre>\n", escape(&sources_text(&where_from))));
    }
    out.push_str("</body></html>\n");
    out
}

/// The start of a report: it follows the editor's colors in a webview, and
/// the system's light or dark elsewhere.
const REPORT_HEAD: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Tondo dump</title><style>\
:root{--fg:#1f2328;--bg:#fff;--muted:#59636e;--line:#d0d7de;--link:#0969da}\
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--bg:#0d1117;--muted:#9198a1;--line:#3d444d;--link:#4493f8}}\
body{font:13px/1.5 var(--vscode-font-family,system-ui,sans-serif);margin:1.5rem;max-width:72rem;\
color:var(--vscode-foreground,var(--fg));background:var(--vscode-editor-background,var(--bg))}\
h1{font-size:1.25rem;margin:0 0 .5rem}h2{font-size:1rem;margin:1.5rem 0 .5rem}\
pre,ol{font:12px/1.5 var(--vscode-editor-font-family,ui-monospace,monospace)}\
pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere}ol{margin:.25rem 0 0;padding-left:2rem}\
table{border-collapse:collapse;width:100%}th{text-align:left;font-weight:600}\
td,th{padding:.2rem .6rem .2rem 0;border-bottom:1px solid var(--vscode-panel-border,var(--line));\
vertical-align:top;overflow-wrap:anywhere}\
details{margin:.5rem 0}summary{cursor:pointer}a{color:var(--vscode-textLink-foreground,var(--link))}\
.dim{color:var(--vscode-descriptionForeground,var(--muted))}</style></head><body>\n";

/// `tondo dump trend <dir>`: what grew over the dumps of a program that
/// are in a directory, as the schedule of `TONDO_DUMP_WHEN` leaves them.
pub fn trend(dir: &std::path::Path, json: bool) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let is_dump = |path: &PathBuf| {
        path.to_string_lossy().ends_with(".dump.json") || path.to_string_lossy().ends_with(".dump.json.gz")
    };
    let files = entries.flatten().map(|entry| entry.path()).filter(is_dump);
    let mut dumps: Vec<Value> = files.filter_map(|path| load(&path).ok()).collect();
    dumps.sort_by(|a, b| a["time_ms"].as_f64().unwrap_or(0.0).total_cmp(&b["time_ms"].as_f64().unwrap_or(0.0)));
    // The dumps of the program that dumped last.
    let last = dumps.last().map(|dump| dump["info"]["pid"].clone()).unwrap_or_default();
    dumps.retain(|dump| dump["info"]["pid"] == last);
    if dumps.len() < 3 {
        return Err(format!("a trend takes 3 dumps of a program, and {} has {}", dir.display(), dumps.len()));
    }
    let found = trends(&dumps);
    if json {
        crate::cli::print_json("dump-trend", true, found);
    } else {
        println!("{}", trend_text(&found).trim_end());
    }
    Ok(())
}

/// How far a series goes up, and whether it does so most of the time:
/// samples of the memory go up and down a little as they come.
fn rising(series: &[f64]) -> bool {
    let steps = series.windows(2).filter(|pair| pair[1] >= pair[0]).count();
    series[series.len() - 1] > series[0] && steps * 5 >= (series.len() - 1) * 4
}

fn trends(dumps: &[Value]) -> Value {
    let (first, last) = (&dumps[0], &dumps[dumps.len() - 1]);
    let seconds = (last["time_ms"].as_f64().unwrap_or(0.0) - first["time_ms"].as_f64().unwrap_or(0.0)) / 1000.0;
    let per_minute = |grew: f64| if seconds > 0.0 { grew / seconds * 60.0 } else { 0.0 };
    // Every place of the memory, by what it held in each dump.
    let mut sites: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for (index, dump) in dumps.iter().enumerate() {
        for site in heap(dump)["sites"].as_array().into_iter().flatten() {
            let place = format!(
                "{} from {}",
                site["kind"].as_str().unwrap_or_default(),
                site["site"].as_str().unwrap_or_default()
            );
            sites.entry(place).or_insert_with(|| vec![0.0; dumps.len()])[index] +=
                site["bytes"].as_f64().unwrap_or(0.0);
        }
    }
    let mut growing: Vec<(String, Vec<f64>)> = sites.into_iter().filter(|(_, series)| rising(series)).collect();
    growing.sort_by(|a, b| {
        let grew = |series: &[f64]| series[series.len() - 1] - series[0];
        grew(&b.1).total_cmp(&grew(&a.1))
    });
    let memory: Vec<Value> = growing
        .iter()
        .map(|(place, series)| {
            let grew = series[series.len() - 1] - series[0];
            json!({ "place": place, "bytes": grew, "per_minute": per_minute(grew), "series": series })
        })
        .collect();
    // The counters that say how the program goes.
    let counter = |dump: &Value, name: &str| {
        dump["counters"].as_array().into_iter().flatten().find(|c| c["name"] == name).and_then(|c| c["value"].as_f64())
    };
    let level = |name: &str| {
        let series: Vec<f64> = dumps.iter().map(|dump| counter(dump, name).unwrap_or(0.0)).collect();
        let grew = series[series.len() - 1] - series[0];
        json!({ "first": series[0], "last": series[series.len() - 1], "per_minute": per_minute(grew), "rising": rising(&series) })
    };
    let tasks: Vec<f64> = dumps.iter().map(|dump| dump["tasks"].as_array().map_or(0.0, |t| t.len() as f64)).collect();
    // The tasks by where they are: a leak of them is a place that fills up.
    let mut places: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for (index, dump) in dumps.iter().enumerate() {
        for group in stacks(&self::tasks(dump)) {
            let frames: Vec<&str> = group["stack"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let place = frames
                .first()
                .map_or(String::new(), |at| format!("{} {at}", group["state"].as_str().unwrap_or_default()));
            places.entry(place).or_insert_with(|| vec![0.0; dumps.len()])[index] +=
                group["count"].as_f64().unwrap_or(0.0);
        }
    }
    let mut piling: Vec<(String, Vec<f64>)> =
        places.into_iter().filter(|(place, series)| !place.is_empty() && rising(series)).collect();
    piling.sort_by(|a, b| {
        let grew = |series: &[f64]| series[series.len() - 1] - series[0];
        grew(&b.1).total_cmp(&grew(&a.1))
    });
    let piling: Vec<Value> = piling
        .iter()
        .map(|(place, series)| json!({ "place": place, "first": series[0] as u64, "last": series[series.len() - 1] as u64 }))
        .collect();
    let task_count = |count: f64| count as u64;
    json!({
        "program": program_name(&last["info"]), "pid": last["info"]["pid"], "dumps": dumps.len(), "seconds": seconds,
        "held": level("memory.held"), "resident": level("os.resident"), "memory": memory,
        "tasks": {
            "first": task_count(tasks[0]), "last": task_count(tasks[tasks.len() - 1]), "rising": rising(&tasks),
            "piling": piling,
        },
    })
}

fn trend_text(found: &Value) -> String {
    let (dumps, seconds) = (&found["dumps"], found["seconds"].as_f64().unwrap_or(0.0));
    let mut out = format!(
        "{} (pid {}), {dumps} dumps over {}\n",
        found["program"].as_str().unwrap_or_default(),
        found["pid"],
        duration(seconds)
    );
    for (key, title) in [("held", "memory held"), ("resident", "resident memory")] {
        let level = &found[key];
        let number = |name: &str| level[name].as_f64().unwrap_or(0.0);
        let goes = if level["rising"] == true {
            format!(", up {} a minute", bytes(number("per_minute")))
        } else {
            String::new()
        };
        out.push_str(&format!("{title}: {} to {}{goes}\n", bytes(number("first")), bytes(number("last"))));
    }
    let tasks = &found["tasks"];
    let goes = if tasks["rising"] == true { ", and rising" } else { "" };
    out.push_str(&format!("tasks: {} to {}{goes}\n", tasks["first"], tasks["last"]));
    for place in tasks["piling"].as_array().into_iter().flatten().take(10) {
        let at = place["place"].as_str().unwrap_or_default();
        out.push_str(&format!("  {} to {} {at}\n", place["first"], place["last"]));
    }
    let places = found["memory"].as_array().map_or(&[][..], Vec::as_slice);
    if !places.is_empty() {
        out.push_str("what keeps growing:\n");
    }
    for place in places.iter().take(10) {
        out.push_str(&format!(
            "  +{} ({} a minute) {}\n",
            bytes(place["bytes"].as_f64().unwrap_or(0.0)),
            bytes(place["per_minute"].as_f64().unwrap_or(0.0)),
            place["place"].as_str().unwrap_or_default()
        ));
    }
    out
}

/// `tondo dump read <file> [tasks | task <id> | heap] [--json]`.
pub fn read(path: &std::path::Path, view: View, json: bool) -> Result<(), String> {
    let dump = load(path)?;
    let tasks = tasks(&dump);
    // With `--json`, the view under its name.
    let shown = |name: &str, value: Value| {
        crate::cli::print_json("dump-read", true, json!({ name: value }));
        Ok(())
    };
    let out = match view {
        View::Summary if json => return shown("summary", Value::String(summary(&dump))),
        View::Summary => summary(&dump),
        View::Tasks if json => return shown("tasks", Value::Array(tasks)),
        View::Tasks => tasks.iter().map(|task| task_text(task, false)).collect::<Vec<_>>().join("\n"),
        View::Task(id) => {
            let task = tasks.iter().find(|t| t["id"] == id).ok_or_else(|| {
                format!("the dump has no task {id}; `tondo dump read {} tasks` lists them", path.display())
            })?;
            if json {
                return shown("task", task.clone());
            }
            task_text(task, true)
        }
        View::Why if json => return shown("why", why(&dump)),
        View::Why => why_text(&why(&dump)),
        View::Stacks if json => return shown("stacks", Value::Array(stacks(&tasks))),
        View::Stacks => stacks_text(&stacks(&tasks)),
        View::Requests if json => return shown("requests", Value::Array(requests(&dump))),
        View::Requests => requests_text(&requests(&dump)),
        View::Sources if json => return shown("sources", sources(&dump)),
        View::Sources => sources_text(&sources(&dump)),
        View::Events if json => return shown("events", Value::Array(events(&dump))),
        View::Events => events_text(&events(&dump)),
        View::Find(text) => {
            let found = find(&dump, &tasks, &text);
            if json {
                return shown("found", Value::Array(found));
            }
            match found.is_empty() {
                true => format!("`{text}` is nowhere in this dump"),
                false => found.iter().filter_map(|f| f["text"].as_str()).collect::<Vec<_>>().join("\n"),
            }
        }
        View::Locals(..) | View::Print(..) if dump["values_off"] == true => {
            return Err("this dump has no values: the program ran with TONDO_DUMP_VALUES=off".into());
        }
        View::Locals(task, frame) => {
            let frame = dump_frame(&tasks, task, frame)?;
            if json {
                return shown("locals", frame.clone());
            }
            let binding = |b: &Value| {
                let text = |key: &str| b[key].as_str().unwrap_or_default().to_string();
                format!("{}: {} = {}", text("name"), text("type"), text("value"))
            };
            let bindings = frame["bindings"].as_array().map_or(&[][..], Vec::as_slice);
            let lines: Vec<String> = bindings.iter().map(binding).collect();
            format!("{}\n{}", frame["at"].as_str().unwrap_or_default(), lines.join("\n"))
        }
        View::Print(path_of, task, frame) => {
            let frame = dump_frame(&tasks, task, frame)?;
            let value = dumped_value(frame, &path_of)
                .ok_or(format!("no binding or part `{path_of}` in {}", frame["at"].as_str().unwrap_or_default()))?;
            if json {
                return shown("value", value.clone());
            }
            value["value"].as_str().unwrap_or_default().to_string()
        }
        View::Heap if json => return shown("heap", heap(&dump)),
        View::Heap => heap_text(&heap(&dump)),
        View::Objects | View::Object(_) if dump.get("objects").is_none() => {
            return Err("this dump has no objects: a dump of a build with --debug has every object".into());
        }
        View::Objects if json => return shown("objects", objects_by_type(&dump)),
        View::Objects => objects_text(&objects_by_type(&dump)),
        View::Object(address) => {
            let object = object(&dump, &address).ok_or_else(|| {
                format!(
                    "the dump has no object at {address}; `tondo dump read {} objects` lists them by type",
                    path.display()
                )
            })?;
            if json {
                return shown("object", object);
            }
            object_text(&object)
        }
    };
    println!("{}", out.trim_end());
    Ok(())
}

/// `tondo dump diff <first> <second>`: what changed from one dump of a
/// program to a later one: its memory by where it was made, its counters,
/// its tasks, and the bindings of the frames both have.
pub fn diff(first: &std::path::Path, second: &std::path::Path, json: bool) -> Result<(), String> {
    let (a, b) = (load(first)?, load(second)?);
    let found = dumps_differ(&a, &b);
    if json {
        crate::cli::print_json("dump-diff", true, found);
    } else {
        println!("{}", diff_text(&found).trim_end());
    }
    Ok(())
}

fn dumps_differ(a: &Value, b: &Value) -> Value {
    // The counters that moved.
    let before = |name: &Value| a["counters"].as_array()?.iter().find(|c| c["name"] == *name)?["value"].as_f64();
    let counters: Vec<Value> = b["counters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|counter| {
            let (old, new) = (before(&counter["name"])?, counter["value"].as_f64()?);
            (old != new)
                .then(|| json!({ "name": counter["name"], "before": old, "after": new, "unit": counter["unit"] }))
        })
        .collect();
    // The memory by the place that made it, what grew the most first.
    let places = |dump: &Value| -> std::collections::HashMap<String, f64> {
        let heap = heap(dump);
        let sites = heap["sites"].as_array().cloned().unwrap_or_default();
        let key = |site: &Value| {
            format!("{} at {}", site["kind"].as_str().unwrap_or_default(), site["site"].as_str().unwrap_or_default())
        };
        sites.iter().map(|site| (key(site), site["bytes"].as_f64().unwrap_or(0.0))).collect()
    };
    let (was, now) = (places(a), places(b));
    let mut memory: Vec<(String, f64)> = was
        .keys()
        .chain(now.keys())
        .map(|place| (place.clone(), now.get(place).copied().unwrap_or(0.0) - was.get(place).copied().unwrap_or(0.0)))
        .filter(|(_, grew)| *grew != 0.0)
        .collect();
    memory.sort_by(|x, y| y.1.abs().total_cmp(&x.1.abs()).then(x.0.cmp(&y.0)));
    memory.dedup_by(|x, y| x.0 == y.0);
    let memory: Vec<Value> = memory.iter().map(|(place, grew)| json!({ "place": place, "bytes": grew })).collect();
    // The tasks: those that came, those that went, those that moved.
    let (old, new) = (tasks(a), tasks(b));
    let find = |list: &[Value], id: &Value| list.iter().find(|t| t["id"] == *id).cloned();
    let brief = |t: &Value| json!({ "id": t["id"], "runs": t["runs"], "state": t["state"], "at": t["at"] });
    let started: Vec<Value> = new.iter().filter(|t| find(&old, &t["id"]).is_none()).map(brief).collect();
    let ended: Vec<Value> = old.iter().filter(|t| find(&new, &t["id"]).is_none()).map(brief).collect();
    let mut moved = Vec::new();
    let mut bindings = Vec::new();
    for task in &new {
        let Some(earlier) = find(&old, &task["id"]) else { continue };
        if task["at"] != earlier["at"] || task["state"] != earlier["state"] {
            moved.push(json!({
                "id": task["id"], "from": earlier["at"], "to": task["at"], "was": earlier["state"], "is": task["state"],
            }));
        }
        for frame in task["frames"].as_array().into_iter().flatten() {
            let then = earlier["frames"].as_array().into_iter().flatten().find(|f| f["at"] == frame["at"]);
            let Some(then) = then else { continue };
            for binding in frame["bindings"].as_array().into_iter().flatten() {
                let same = then["bindings"].as_array().into_iter().flatten().find(|b| b["name"] == binding["name"]);
                if let Some(same) = same.filter(|same| same["value"] != binding["value"]) {
                    bindings.push(json!({
                        "task": task["id"], "at": frame["at"], "name": binding["name"], "before": same["value"],
                        "after": binding["value"],
                    }));
                }
            }
        }
    }
    let seconds = (b["time_ms"].as_f64().unwrap_or(0.0) - a["time_ms"].as_f64().unwrap_or(0.0)) / 1000.0;
    json!({
        "seconds": seconds, "memory": memory, "counters": counters, "started": started, "ended": ended,
        "moved": moved, "bindings": bindings,
    })
}

fn diff_text(found: &Value) -> String {
    let list = |key: &str| found[key].as_array().map_or(&[][..], Vec::as_slice);
    let mut out = format!("{} later:\n", duration(found["seconds"].as_f64().unwrap_or(0.0)));
    let signed = |n: f64| format!("{}{}", if n < 0.0 { "-" } else { "+" }, bytes(n.abs()));
    if !list("memory").is_empty() {
        out.push_str("memory, by where it was made:\n");
    }
    for place in list("memory").iter().take(20) {
        out.push_str(&format!(
            "  {} {}\n",
            signed(place["bytes"].as_f64().unwrap_or(0.0)),
            place["place"].as_str().unwrap_or_default()
        ));
    }
    for (key, title) in [("started", "tasks that started:"), ("ended", "tasks that ended:")] {
        if !list(key).is_empty() {
            out.push_str(&format!("{title}\n"));
        }
        for task in list(key) {
            let (runs, at) =
                (task["runs"].as_str().unwrap_or_default(), task["at"].as_str().unwrap_or("nowhere known"));
            out.push_str(&format!("  task {} ({runs}) at {at}\n", task["id"]));
        }
    }
    if !list("moved").is_empty() {
        out.push_str("tasks that moved:\n");
    }
    for task in list("moved") {
        let at = |key: &str| task[key].as_str().unwrap_or("nowhere known").to_string();
        out.push_str(&format!("  task {}: {} {} -> {} {}\n", task["id"], at("was"), at("from"), at("is"), at("to")));
    }
    if !list("bindings").is_empty() {
        out.push_str("bindings that changed:\n");
    }
    for binding in list("bindings") {
        let text = |key: &str| binding[key].as_str().unwrap_or_default().to_string();
        out.push_str(&format!("  {} in {}: {} -> {}\n", text("name"), text("at"), text("before"), text("after")));
    }
    if !list("counters").is_empty() {
        out.push_str("counters:\n");
    }
    for counter in list("counters") {
        let number = |key: &str| counter[key].as_f64().unwrap_or(0.0);
        out.push_str(&format!(
            "  {}: {} -> {}\n",
            counter["name"].as_str().unwrap_or_default(),
            number("before"),
            number("after")
        ));
    }
    out
}

/// A dump as a debugger shows it after the fact (toolchain 7.6): its
/// summary, each task with its stack in words, and the memory by site.
pub fn opened(path: &std::path::Path) -> Result<(String, Vec<Value>, Value, crate::roots::Roots), String> {
    let dump = load(path)?;
    let summary = format!("{}\n{}\n", why_text(&why(&dump)), summary(&dump).trim_end());
    Ok((summary, tasks(&dump), heap(&dump), roots_of(&dump)))
}

/// The tasks of a dump that came over a connection.
pub(crate) fn seen_tasks(mut dump: Value) -> Vec<Value> {
    name_frames(&mut dump);
    tasks(&dump)
}

/// What a dump holds of the frame at `index` of a task's stack: of its frames
/// with the same place, the one in the same order.
pub(crate) fn frame_of_task(task: &Value, index: usize) -> Option<&Value> {
    let stack = task["stack"].as_array()?;
    let place = stack.get(index)?;
    let before = stack[..index].iter().filter(|line| *line == place).count();
    task["frames"].as_array()?.iter().filter(|frame| frame["at"] == *place).nth(before)
}

/// Names each frame the program did not, from the symbols of its files.
fn name_frames(dump: &mut Value) {
    let program = dump["info"]["program"].as_str().unwrap_or_default().to_string();
    let Some(frames) = dump.get_mut("frames").and_then(Value::as_object_mut) else { return };
    let original = frames.clone();
    let names = Names::new(&original, &program);
    for (address, frame) in frames.iter_mut() {
        if frame.get("function").is_none() && frame.get("name").is_none() {
            frame["name"] = json!(names.function(address));
        }
    }
}

/// A frame as a dump shows it: the program's as `file:line in function`,
/// others by name.
fn frame_text(dump: &Value, address: &Value) -> Option<String> {
    let frame = &dump["frames"][address.as_str()?];
    match (frame["function"].as_str(), frame["file"].as_str()) {
        (Some(function), Some(file)) => Some(format!("{file}:{} in {function}", frame["line"])),
        (Some(function), None) => Some(function.to_string()),
        _ => frame["name"].as_str().map(str::to_string),
    }
}

/// A frame as a dump shows it, and those of the calls its code was inlined
/// at, from the innermost out.
fn frame_texts(dump: &Value, address: &Value) -> Vec<String> {
    let inlined = address.as_str().map(|a| &dump["frames"][a]["inlined"]);
    let outer = inlined.and_then(Value::as_array).into_iter().flatten().filter_map(|frame| {
        Some(format!("{}:{} in {}", frame["file"].as_str()?, frame["line"], frame["function"].as_str()?))
    });
    frame_text(dump, address).into_iter().chain(outer).collect()
}

/// Whether a frame is the program's own, with a place in its source.
fn placed(dump: &Value, address: &Value) -> bool {
    address.as_str().is_some_and(|a| dump["frames"][a]["file"].is_string())
}

/// Whether a frame is compiled Tondo code, the program's or the standard
/// library's.
fn compiled(dump: &Value, address: &Value) -> bool {
    address.as_str().is_some_and(|a| dump["frames"][a]["function"].is_string())
}

/// The tasks with what each runs, where it is and its stack in words.
fn tasks(dump: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for task in dump["tasks"].as_array().into_iter().flatten() {
        let stack: Vec<&Value> = task["stack"].as_array().into_iter().flatten().collect();
        let runs = if task["main"] == true {
            "main".to_string()
        } else {
            let outermost = stack.iter().rev().find_map(|a| dump["frames"][a.as_str()?]["function"].as_str());
            outermost.map_or("a task".to_string(), |f| f.strip_prefix("spawn ").unwrap_or(f).to_string())
        };
        let at = stack.iter().find(|a| compiled(dump, a)).and_then(|a| frame_text(dump, a));
        let words = |list: &Value| -> Vec<String> {
            list.as_array().into_iter().flatten().flat_map(|a| frame_texts(dump, a)).collect()
        };
        // The bindings of each frame, as the dump showed them.
        let mut frames: Vec<Value> = Vec::new();
        for value in dump["values"].as_array().into_iter().flatten().filter(|v| v["task"] == task["id"]) {
            if frames.last().is_none_or(|last| last["frame"] != value["frame"]) {
                frames.push(json!({ "frame": value["frame"], "at": value["at"], "bindings": [] }));
            }
            if let Some(bindings) = frames.last_mut().and_then(|last| last["bindings"].as_array_mut()) {
                bindings.push(json!({
                    "name": value["name"], "type": value["type"], "value": value["value"], "parts": value["parts"],
                }));
            }
        }
        out.push(json!({
            "id": task["id"], "runs": runs, "state": task["state"], "at": at, "cancelled": task["cancelled"],
            "stack": words(&task["stack"]), "spawned": words(&task["spawned"]), "locks": task["locks"],
            "contexts": task.get("contexts").cloned().unwrap_or_else(|| json!([])),
            "frames": frames, "wait": task["wait"],
        }));
    }
    out
}

fn task_text(task: &Value, whole: bool) -> String {
    let state = task["state"].as_str().unwrap_or_default();
    let mut out = format!("task {} ({}) {state}", task["id"], task["runs"].as_str().unwrap_or_default());
    if let Some(at) = task["at"].as_str() {
        out.push_str(&format!(" at {at}"));
    }
    if task["cancelled"] == true {
        out.push_str(", cancelled");
    }
    out.push('\n');
    if let Some(wait) = task["wait"]["for"].as_str() {
        let waited = duration(task["wait"]["ms"].as_f64().unwrap_or(0.0) / 1000.0);
        out.push_str(&format!("  waited {waited} for {wait}\n"));
    }
    // A short view shows the program's frames; the whole one, every frame.
    let lines: Vec<&str> = task["stack"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let shown: Vec<&&str> = lines.iter().filter(|l| whole || l.contains(" in ")).collect();
    for line in shown {
        out.push_str(&format!("    {line}\n"));
    }
    if let Some(spawned) = task["spawned"].as_array().and_then(|s| s.iter().find_map(Value::as_str)) {
        out.push_str(&format!("  spawned at {spawned}\n"));
    }
    for lock in task["locks"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        out.push_str(&format!("  holds a lock it took at {lock}\n"));
    }
    // The fields of its contexts that do not have their initial values.
    for context in task["contexts"].as_array().into_iter().flatten() {
        let fields: Vec<String> = context["fields"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, value)| format!("{name} = {}", value.as_str().unwrap_or_default()))
            .collect();
        out.push_str(&format!("  {}: {}\n", context["name"].as_str().unwrap_or_default(), fields.join(", ")));
    }
    out
}

/// The table of where a dump's program came from.
pub(crate) fn roots_of(dump: &Value) -> crate::roots::Roots {
    crate::roots::Roots::parse(dump["sources"].as_str().unwrap_or_default())
}

/// Each file of a dump's program, where it is for a tool here, and where it
/// came from.
fn sources(dump: &Value) -> Value {
    use crate::roots::Found;
    let roots = roots_of(dump);
    let here = std::env::current_dir().unwrap_or_default();
    let file = |file: &String| {
        let state = match roots.find(&here, file) {
            Found::Same(_) => "same",
            Found::Changed(_) => "changed",
            Found::Elsewhere => "elsewhere",
            Found::Lost => "lost",
        };
        json!({ "file": file, "state": state, "link": roots.link(file, 1) })
    };
    let repositories: Vec<Value> = roots
        .roots
        .iter()
        .map(|root| json!({ "prefix": root.prefix, "repository": root.repository, "commit": root.commit }))
        .collect();
    json!({ "repositories": repositories, "files": roots.files.keys().map(file).collect::<Vec<_>>() })
}

fn sources_text(found: &Value) -> String {
    let mut out = String::new();
    let list = |key: &str| found[key].as_array().map_or(&[][..], Vec::as_slice);
    if list("repositories").is_empty() {
        return "this dump does not say where its files came from".to_string();
    }
    for root in list("repositories") {
        let (repository, commit) =
            (root["repository"].as_str().unwrap_or_default(), root["commit"].as_str().unwrap_or_default());
        match root["prefix"].as_str().unwrap_or_default() {
            "" => out.push_str(&format!("the program: {repository} at {commit}\n")),
            prefix => out.push_str(&format!("{}: {repository} at {commit}\n", prefix.trim_end_matches('/'))),
        }
    }
    for file in list("files") {
        let (name, link) = (file["file"].as_str().unwrap_or_default(), file["link"].as_str().unwrap_or_default());
        match file["state"].as_str().unwrap_or_default() {
            "same" => out.push_str(&format!("  {name}\n")),
            "changed" => out.push_str(&format!("  {name}: it changed since the build\n")),
            "elsewhere" => {
                out.push_str(&format!("  {name}: not as it was here; `tondo source {name} --from <dump>`, {link}\n"))
            }
            _ => out.push_str(&format!("  {name}: lost\n")),
        }
    }
    out.trim_end().to_string()
}

/// `tondo source <file> --from <dump|recording|executable>`: the file of a
/// build, as it was: here if it is the same, or from the commit it was
/// built from, which comes from its repository the first time.
pub fn source(file: &str, from: &std::path::Path, line: u64, json: bool) -> Result<(), String> {
    use crate::roots::{Found, Roots};
    let program = from.join("program");
    let roots = match (program.is_file(), load(from)) {
        (true, _) => Roots::of_executable(&program)?,
        (_, Ok(dump)) => roots_of(&dump),
        _ => Roots::of_executable(from)?,
    };
    let here = std::env::current_dir().unwrap_or_default();
    let found = roots.find(&here, file);
    let link = roots.link(file, line);
    let (path, state) = match found {
        Found::Same(path) => (path, "same"),
        Found::Changed(path) => (path, "changed"),
        Found::Elsewhere => {
            let fetched = roots.fetch(file).map_err(|e| match &link {
                Some(link) => format!("{e}; it is at {link}"),
                None => e,
            })?;
            (fetched, "fetched")
        }
        Found::Lost => return Err(format!("{} does not say where {file} came from", from.display())),
    };
    if json {
        let commit = roots.place(file).map(|(root, _)| root.commit.clone());
        crate::cli::print_json("source", true, json!({ "path": path, "state": state, "link": link, "commit": commit }));
        return Ok(());
    }
    println!("{}", path.display());
    if state == "changed" {
        eprintln!("tondo: {file} changed since the build, so its lines may not be those of the build");
    }
    Ok(())
}

/// What the program did last, the oldest first, with places in words.
fn events(dump: &Value) -> Vec<Value> {
    let mut list = dump["events"].as_array().cloned().unwrap_or_default();
    for event in &mut list {
        if let Some(at) = event["at"].as_array().and_then(|at| at.first()).and_then(|at| frame_text(dump, at)) {
            event["at"] = json!(at);
        }
    }
    list
}

fn events_text(list: &[Value]) -> String {
    if list.is_empty() {
        return "this dump has no events: a program keeps them with TONDO_DUMP in its environment".to_string();
    }
    let line = |event: &Value| {
        let ago = duration(event["ago_ms"].as_f64().unwrap_or(0.0) / 1000.0);
        let took = duration(event["took_ms"].as_f64().unwrap_or(0.0) / 1000.0);
        let (task, text) = (&event["task"], event["text"].as_str().unwrap_or_default());
        let what = match event["what"].as_str().unwrap_or_default() {
            "spawned" => {
                format!("task {task} spawned task {} at {}", event["other"], event["at"].as_str().unwrap_or("?"))
            }
            "ended" => format!("task {task} ended"),
            "panicked" => format!("task {task} panicked: {text}"),
            "request" => format!("came {text}"),
            "answered" => format!("answered {text} in {took}"),
            "waited" => format!("task {task} waited {took}"),
            "paused" => format!("the collector stopped the program for {took}"),
            other => other.to_string(),
        };
        format!("{ago:>9} ago  {what}")
    };
    list.iter().map(line).collect::<Vec<_>>().join("\n")
}

/// The requests in flight, the oldest first.
fn requests(dump: &Value) -> Vec<Value> {
    let mut list = dump["requests"].as_array().cloned().unwrap_or_default();
    list.sort_by_key(|request| std::cmp::Reverse(request["ms"].as_u64()));
    list
}

fn requests_text(list: &[Value]) -> String {
    let line = |request: &Value| {
        let (method, path) =
            (request["method"].as_str().unwrap_or_default(), request["path"].as_str().unwrap_or_default());
        let waited = duration(request["ms"].as_f64().unwrap_or(0.0) / 1000.0);
        let task = request["task"].as_u64().filter(|task| *task != 0);
        let answer = task.map_or(", no task has it yet".to_string(), |task| format!(", in task {task}"));
        format!("{method} {path} for {waited}{answer}")
    };
    match list.is_empty() {
        true => "no request in flight".to_string(),
        false => list.iter().map(line).collect::<Vec<_>>().join("\n"),
    }
}

/// A task's state, said of several.
fn plural(state: &str) -> String {
    match state {
        "waits" => "wait".to_string(),
        "runs" => "run".to_string(),
        other => format!("are {other}"),
    }
}

/// The frames of a task's stack that are code of Tondo.
fn tondo_frames(task: &Value) -> Vec<&str> {
    task["stack"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|l| l.contains(" in ")).collect()
}

/// The tasks by the stack they share, the most crowded first.
fn stacks(tasks: &[Value]) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<&str>, Vec<&Value>)> = Vec::new();
    for task in tasks {
        let (state, stack) = (task["state"].as_str().unwrap_or_default().to_string(), tondo_frames(task));
        match groups.iter_mut().find(|(s, frames, _)| *s == state && *frames == stack) {
            Some((.., members)) => members.push(task),
            None => groups.push((state, stack, vec![task])),
        }
    }
    groups.sort_by_key(|(.., members)| std::cmp::Reverse(members.len()));
    let group = |(state, stack, members): (String, Vec<&str>, Vec<&Value>)| {
        let longest = members.iter().filter_map(|t| t["wait"]["ms"].as_f64()).fold(0.0, f64::max);
        let ids: Vec<&Value> = members.iter().map(|t| &t["id"]).collect();
        json!({ "count": members.len(), "state": state, "tasks": ids, "stack": stack, "waited_ms": longest })
    };
    groups.into_iter().map(group).collect()
}

fn stacks_text(groups: &[Value]) -> String {
    let mut out = String::new();
    for group in groups {
        let ids: Vec<String> = group["tasks"].as_array().into_iter().flatten().map(Value::to_string).collect();
        let shown = ids.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
        let more = if ids.len() > 10 { ", ..." } else { "" };
        let state = group["state"].as_str().unwrap_or_default();
        let waited = group["waited_ms"].as_f64().filter(|ms| *ms > 0.0);
        let waited = waited.map_or(String::new(), |ms| format!(", the longest for {}", duration(ms / 1000.0)));
        let (tasks, state) = match ids.len() {
            1 => ("task", state.to_string()),
            _ => ("tasks", plural(state)),
        };
        out.push_str(&format!("{} {tasks} {state} ({shown}{more}){waited}\n", ids.len()));
        for frame in group["stack"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            out.push_str(&format!("    {frame}\n"));
        }
    }
    out.trim_end().to_string()
}

/// Tasks that others wait for, each with those.
type HeldUp = Vec<(u64, Vec<u64>)>;

/// Who waits for whom among the tasks, each by the tasks that can end its
/// wait: the cycles, and for each task that holds others up, which.
fn waits(tasks: &[Value]) -> (Vec<Vec<u64>>, HeldUp) {
    let on = |task: &Value| -> Vec<u64> {
        task["wait"]["on"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect()
    };
    let edges: std::collections::BTreeMap<u64, Vec<u64>> =
        tasks.iter().filter_map(|t| Some((t["id"].as_u64()?, on(t)))).filter(|(_, on)| !on.is_empty()).collect();
    let mut cycles: Vec<Vec<u64>> = Vec::new();
    // Each path from a task, which ends in a cycle when it comes back.
    fn walk(at: u64, path: &mut Vec<u64>, edges: &std::collections::BTreeMap<u64, Vec<u64>>, out: &mut Vec<Vec<u64>>) {
        if let Some(start) = path.iter().position(|&t| t == at) {
            let mut cycle = path[start..].to_vec();
            let lowest = cycle.iter().enumerate().min_by_key(|(_, t)| **t).map_or(0, |(i, _)| i);
            cycle.rotate_left(lowest);
            if !out.contains(&cycle) {
                out.push(cycle);
            }
            return;
        }
        path.push(at);
        for &next in edges.get(&at).map_or(&[][..], Vec::as_slice) {
            walk(next, path, edges, out);
        }
        path.pop();
    }
    for &task in edges.keys() {
        walk(task, &mut Vec::new(), &edges, &mut cycles);
    }
    // Those the rest wait for, directly or not, by how many.
    let mut behind: std::collections::BTreeMap<u64, Vec<u64>> = Default::default();
    for &task in edges.keys() {
        let (mut seen, mut next) = (std::collections::BTreeSet::new(), edges[&task].clone());
        while let Some(on) = next.pop() {
            if seen.insert(on) {
                next.extend(edges.get(&on).into_iter().flatten());
            }
        }
        for on in seen.into_iter().filter(|on| !edges.contains_key(on)) {
            behind.entry(on).or_default().push(task);
        }
    }
    let mut holding: HeldUp = behind.into_iter().collect();
    holding.sort_by_key(|(_, held)| std::cmp::Reverse(held.len()));
    (cycles, holding)
}

/// Who holds whom up among some tasks: the cycles, the tasks the rest wait
/// for, and the longest wait.
fn wait_graph(tasks: &[Value]) -> Value {
    let (cycles, holding) = waits(tasks);
    let task = |id: u64| tasks.iter().find(|t| t["id"] == id);
    let brief = |id: u64| {
        let task = task(id);
        json!({ "task": id, "at": task.map(|t| &t["at"]), "for": task.map(|t| &t["wait"]["for"]) })
    };
    let cycles: Vec<Value> = cycles.iter().map(|c| Value::Array(c.iter().map(|&t| brief(t)).collect())).collect();
    let holding: Vec<Value> = holding
        .iter()
        .take(5)
        .map(|(id, held)| {
            let at = task(*id).map(|t| &t["at"]);
            let state = task(*id).map(|t| &t["state"]);
            json!({ "task": id, "at": at, "state": state, "holds_up": held })
        })
        .collect();
    let longest = tasks
        .iter()
        .filter(|t| t["wait"].is_object())
        .max_by(|a, b| a["wait"]["ms"].as_f64().unwrap_or(0.0).total_cmp(&b["wait"]["ms"].as_f64().unwrap_or(0.0)));
    let longest =
        longest.map(|t| json!({ "task": t["id"], "at": t["at"], "for": t["wait"]["for"], "ms": t["wait"]["ms"] }));
    json!({ "cycles": cycles, "holding": holding, "longest": longest })
}

/// What a dump says went wrong, as far as it can tell: the tasks that wait
/// for each other, those that hold others up, the longest wait, many tasks
/// in one place, and where the memory is.
fn why(dump: &Value) -> Value {
    let tasks = tasks(dump);
    let mut found = wait_graph(&tasks);
    let heap = heap(dump);
    found["reason"] = dump["reason"].clone();
    found["message"] = dump["message"].clone();
    found["signature"] = json!(signature(dump));
    found["lock_order"] = dump["lock_order"].clone();
    let events = events(dump);
    found["events"] = events[events.len().saturating_sub(10)..].to_vec().into();
    found["requests"] = requests(dump).into_iter().take(5).collect();
    found["in_flight"] = json!(dump["requests"].as_array().map_or(0, Vec::len));
    found["crowded"] = stacks(&tasks).into_iter().filter(|g| g["count"].as_u64() > Some(1)).take(5).collect();
    found["memory"] = heap["sites"].as_array().into_iter().flatten().take(3).cloned().collect();
    found["bytes"] = heap["bytes"].clone();
    found
}

/// `blocked` in a debugger: who waits for whom now, from what `tasks`
/// answers, or from a running program's dump.
pub(crate) fn blocked(answer: &Value) -> Value {
    let task = |t: &Value| {
        let wait = t.get("for").map_or(Value::Null, |wanted| json!({ "for": wanted, "on": t["on"], "ms": t["ms"] }));
        let state = if t["why"].as_str().is_some_and(|why| why.starts_with("wait")) { "waits" } else { "stopped" };
        json!({ "id": t["task"], "at": t["at"], "state": state, "wait": wait })
    };
    let tasks: Vec<Value> = answer["tasks"].as_array().into_iter().flatten().map(task).collect();
    let mut graph = wait_graph(&tasks);
    graph["waits"] = tasks.iter().filter(|t| t["wait"].is_object()).cloned().collect();
    graph
}

/// `blocked` as text: each wait, then what they add up to.
pub(crate) fn blocked_text(graph: &Value) -> String {
    let list = graph["waits"].as_array().map_or(&[][..], Vec::as_slice);
    if list.is_empty() {
        return "no task waits for another".to_string();
    }
    let lines = list.iter().map(|t| {
        let waited = duration(t["wait"]["ms"].as_f64().unwrap_or(0.0) / 1000.0);
        let (at, wanted) = (t["at"].as_str().unwrap_or("nowhere known"), t["wait"]["for"].as_str().unwrap_or_default());
        format!("task {} at {at} has waited {waited} for {wanted}", t["id"])
    });
    format!("{}\n{}", lines.collect::<Vec<_>>().join("\n"), why_text(graph)).trim_end().to_string()
}

/// What `tondo logpoint` is asked for.
pub struct Logpoint {
    pub name: String,
    pub place: String,
    pub text: Option<String>,
    /// What must hold for a time to count: `order.total > 100`.
    pub condition: Option<String>,
    /// Where each time leaves a dump of the whole program, as it was there.
    pub dump: Option<PathBuf>,
    /// For how long each time holds its task, in seconds.
    pub stop: Option<u64>,
    pub max: u64,
    pub seconds: Option<u64>,
}

/// `tondo logpoint <pid|name> <file:line> [<text>]`: what a line of a
/// running program holds each time it goes by, up to `max` times or for
/// `seconds`, without stopping it. `{user.id}` in the text reads a part.
pub fn logpoint(asked: &Logpoint, json: bool) -> Result<(), String> {
    let Logpoint { name, place, text, condition, dump, stop, max, seconds } = asked;
    let (text, max) = (text.as_deref(), *max);
    let mut program = connect(name)?;
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let dump = dump.as_ref().map_or(String::new(), |path| here.join(path).display().to_string());
    let held = stop.map_or(String::new(), |seconds| seconds.to_string());
    let request = format!("logpoint {max} {place}\t{}\t{dump}\t{held}", condition.as_deref().unwrap_or_default());
    let answer = program.ask(&request)?;
    if let Some(error) = answer["error"].as_str() {
        return Err(error.to_string());
    }
    if !json {
        eprintln!("tondo: a logpoint on {place}, in {} places: it ends after {max} times", answer["places"]);
    }
    let waiting = seconds.map(Duration::from_secs);
    let _ = program.reader.get_ref().set_read_timeout(waiting);
    let deadline = waiting.map(|waiting| std::time::Instant::now() + waiting);
    let mut hits = Vec::new();
    loop {
        let mut line = String::new();
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            break;
        }
        match program.reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(hit) = serde_json::from_str::<Value>(&line) else { continue };
        if hit.get("done").is_some() {
            break;
        }
        if hit.get("hit").is_none() {
            continue;
        }
        match json {
            true => hits.push(
                json!({ "task": hit["task"], "text": logged(&hit, text), "values": hit["values"], "dump": hit["dump"] }),
            ),
            false => {
                println!("{}", logged(&hit, text));
                if let Some(dump) = hit["dump"].as_str() {
                    eprintln!("tondo: a dump of the program as it was there is in {dump}");
                }
                for frame in hit["stack"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    println!("  {frame}");
                }
                if let Some(seconds) = hit["stopped"].as_u64() {
                    println!("task {} stands here for {seconds} seconds, the others go on: Enter lets it go", hit["task"]);
                    let _ = std::io::stdout().flush();
                    let mut enter = String::new();
                    let _ = std::io::stdin().read_line(&mut enter);
                    let _ = writeln!(program.writer, "go {}", hit["hit"]);
                }
            }
        }
    }
    if json {
        crate::cli::print_json("logpoint", true, json!({ "hits": hits }));
    }
    Ok(())
}

/// A hit of a logpoint as a line: the text with its parts in place, or
/// every binding.
pub(crate) fn logged(hit: &Value, text: Option<&str>) -> String {
    let values = hit["values"].as_array().map_or(&[][..], Vec::as_slice);
    let frame = json!({ "bindings": values });
    let at = values.first().and_then(|value| value["at"].as_str()).unwrap_or_default();
    let shown = |value: &Value| value["value"].as_str().unwrap_or_default().to_string();
    let Some(text) = text else {
        let all: Vec<String> =
            values.iter().map(|v| format!("{} = {}", v["name"].as_str().unwrap_or_default(), shown(v))).collect();
        return format!("task {} {at}: {}", hit["task"], all.join(", "));
    };
    let (mut out, mut rest) = (String::new(), text);
    while let Some((before, after)) = rest.split_once('{') {
        let Some((path, tail)) = after.split_once('}') else { break };
        out.push_str(before);
        out.push_str(&dumped_value(&frame, path).map_or("?".to_string(), shown));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// `tondo blocked <pid|name>`: who waits for whom in a running program.
pub fn blocked_in(name: &str, as_json: bool) -> Result<(), String> {
    let mut program = connect(name)?;
    let _ = program.reader.get_ref().set_read_timeout(Some(Duration::from_secs(60)));
    let mut dump = program.ask("dump")?;
    if dump.get("dump").is_none() {
        return Err(format!("the program {} gave no dump: {dump}", program.pid));
    }
    name_frames(&mut dump);
    let tasks = tasks(&dump);
    let mut graph = wait_graph(&tasks);
    graph["waits"] = tasks.iter().filter(|t| t["wait"].is_object()).cloned().collect();
    match as_json {
        true => crate::cli::print_json("blocked", true, graph),
        false => println!("{}", blocked_text(&graph)),
    }
    Ok(())
}

fn why_text(found: &Value) -> String {
    let mut out = String::new();
    // A program that went on has nothing to say of why it was dumped.
    if !matches!(found["reason"].as_str().unwrap_or_default(), "request" | "schedule" | "signal" | "logpoint" | "") {
        out.push_str(&format!("it stopped at a {}\n", found["reason"].as_str().unwrap_or_default()));
        for line in found["message"].as_str().unwrap_or_default().lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    let list = |key: &str| found[key].as_array().map_or(&[][..], Vec::as_slice);
    let text = |value: &Value| value.as_str().unwrap_or("nowhere known").to_string();
    for order in list("lock_order") {
        out.push_str(&format!("{}\n", text(order)));
    }
    for cycle in list("cycles") {
        out.push_str("these tasks wait for each other, and none can go on:\n");
        for member in cycle.as_array().into_iter().flatten() {
            let wants = text(&member["for"]);
            out.push_str(&format!("  task {} at {} waits for {wants}\n", member["task"], text(&member["at"])));
        }
    }
    for task in list("holding") {
        let held: Vec<String> = task["holds_up"].as_array().into_iter().flatten().map(Value::to_string).collect();
        let what = if held.len() == 1 { "task" } else { "tasks" };
        let state = text(&task["state"]);
        let ids = held.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
        let more = if held.len() > 10 { ", ..." } else { "" };
        let (id, count, at) = (&task["task"], held.len(), text(&task["at"]));
        out.push_str(&format!("task {id} holds up {count} {what} ({ids}{more}): it {state} at {at}\n"));
    }
    if found["longest"].is_object() {
        let longest = &found["longest"];
        let waited = duration(longest["ms"].as_f64().unwrap_or(0.0) / 1000.0);
        let (id, at, wants) = (&longest["task"], text(&longest["at"]), text(&longest["for"]));
        out.push_str(&format!("the longest wait: task {id}, {waited} at {at}, for {wants}\n"));
    }
    for group in list("crowded") {
        let at = group["stack"][0].as_str().unwrap_or("nowhere known");
        out.push_str(&format!("{} tasks {} at {at}\n", group["count"], plural(&text(&group["state"]))));
    }
    if !list("requests").is_empty() {
        out.push_str(&format!("{} requests in flight, the oldest:\n", found["in_flight"]));
        for line in requests_text(list("requests")).lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    let total = found["bytes"].as_f64().unwrap_or(0.0);
    for site in list("memory") {
        let held = site["bytes"].as_f64().unwrap_or(0.0);
        let share = if total > 0.0 { held / total * 100.0 } else { 0.0 };
        let (at, kind) = (text(&site["site"]), text(&site["kind"]));
        out.push_str(&format!("{} of the live memory ({share:.0} %) is {kind} from {at}\n", bytes(held)));
    }
    if !list("events").is_empty() {
        out.push_str("what came last:\n");
        for line in events_text(list("events")).lines() {
            out.push_str(&format!("{line}\n"));
        }
    }
    match out.is_empty() {
        true => "nothing stands out in this dump".to_string(),
        false => out.trim_end().to_string(),
    }
}

/// Where `text` is in a dump, without minding case: each place once, as
/// deep as it is, so `user.name` and not `user` as well.
fn find(dump: &Value, tasks: &[Value], text: &str) -> Vec<Value> {
    let wanted = text.to_lowercase();
    let has = |value: &str| value.to_lowercase().contains(&wanted);
    let mut out = Vec::new();
    let mut found = |place: Value, line: String| {
        let mut entry = place;
        entry["text"] = json!(line);
        out.push(entry);
    };
    if has(dump["message"].as_str().unwrap_or_default()) {
        found(json!({ "in": "message" }), format!("in the message: {}", dump["message"].as_str().unwrap_or_default()));
    }
    // The deepest parts of a value that hold the text.
    fn parts(value: &Value, path: &str, has: &dyn Fn(&str) -> bool, out: &mut Vec<(String, String)>) {
        let before = out.len();
        for part in value["parts"].as_array().into_iter().flatten() {
            let name = part["name"].as_str().unwrap_or_default();
            parts(part, &format!("{path}{name}"), has, out);
        }
        let text = value["value"].as_str().unwrap_or_default();
        if out.len() == before && has(text) {
            out.push((path.to_string(), text.to_string()));
        }
    }
    for task in tasks {
        let id = &task["id"];
        for frame in task["frames"].as_array().into_iter().flatten() {
            let at = frame["at"].as_str().unwrap_or_default();
            for binding in frame["bindings"].as_array().into_iter().flatten() {
                let mut places = Vec::new();
                parts(binding, binding["name"].as_str().unwrap_or_default(), &has, &mut places);
                for (path, value) in places {
                    let place = json!({ "in": "value", "task": id, "frame": frame["frame"], "at": at, "path": path, "value": value });
                    found(place, format!("task {id}, {at}: {path} = {value}"));
                }
            }
        }
        for context in task["contexts"].as_array().into_iter().flatten() {
            for (name, value) in context["fields"].as_object().into_iter().flatten() {
                let value = value.as_str().unwrap_or_default();
                if has(value) {
                    let context = context["name"].as_str().unwrap_or_default();
                    found(
                        json!({ "in": "context", "task": id, "path": format!("{context}.{name}"), "value": value }),
                        format!("task {id}, context {context}: {name} = {value}"),
                    );
                }
            }
        }
        for frame in task["stack"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|f| has(f)) {
            found(json!({ "in": "stack", "task": id, "at": frame }), format!("task {id} is in {frame}"));
        }
    }
    for request in dump["requests"].as_array().into_iter().flatten() {
        let path = request["path"].as_str().unwrap_or_default();
        if has(path) {
            found(
                json!({ "in": "request", "task": request["task"], "path": path }),
                format!("request: {}", requests_text(std::slice::from_ref(request))),
            );
        }
    }
    for object in dump["objects"]["list"].as_array().into_iter().flatten() {
        let value = object["text"].as_str().unwrap_or_default();
        if has(value) {
            let address = object["address"].as_str().unwrap_or_default();
            found(json!({ "in": "object", "address": address, "value": value }), format!("object {address}: {value}"));
        }
    }
    for site in heap(dump)["sites"].as_array().into_iter().flatten() {
        let at = site["site"].as_str().unwrap_or_default();
        if has(at) {
            found(
                json!({ "in": "memory", "at": at }),
                format!("memory: {} from {at}", bytes(site["bytes"].as_f64().unwrap_or(0.0))),
            );
        }
    }
    out
}

/// The live memory the samples stand for, by where it was allocated and
/// by kind: a sample of `size` bytes stands for `size / (1 - e^(-size /
/// rate))` of them, as in Go.
fn heap(dump: &Value) -> Value {
    let rate = dump["heap"]["rate"].as_f64().unwrap_or(524_288.0);
    let mut sites: std::collections::HashMap<(String, String), (f64, f64, u64)> = std::collections::HashMap::new();
    for sample in dump["heap"]["samples"].as_array().into_iter().flatten() {
        let size = sample["size"].as_f64().unwrap_or(0.0).max(1.0);
        let weight = size / (1.0 - (-size / rate).exp());
        let stack: Vec<&Value> = sample["stack"].as_array().into_iter().flatten().collect();
        // The kind from the runtime's function that Tondo code called; the
        // site where the program's own code was, past the standard
        // library's.
        let first = stack.iter().position(|a| placed(dump, a));
        // The type the program's statement assigned, as the compiler wrote
        // it, or else what the runtime's function makes.
        let typed = first.and_then(|i| dump["frames"][stack[i].as_str()?]["type"].as_str()).map(str::to_string);
        let made = || first.and_then(|i| i.checked_sub(1)).and_then(|i| frame_text(dump, stack[i])).map(|f| kind(&f));
        let kind = typed.or_else(made).unwrap_or_else(|| "other".into());
        // Code of the standard library inlined into the program's is in a
        // frame of the program's own.
        let own = stack.iter().filter(|a| placed(dump, a)).flat_map(|a| frame_texts(dump, a));
        let site = own
            .into_iter()
            .find(|text| !text.starts_with("<std>/"))
            .or_else(|| first.and_then(|i| frame_text(dump, stack[i])))
            .unwrap_or_else(|| "outside the program's code".into());
        let entry = sites.entry((site, kind)).or_default();
        entry.0 += weight;
        entry.1 += weight / size;
        entry.2 += 1;
    }
    let mut rows: Vec<Value> = sites
        .into_iter()
        .map(|((site, kind), (bytes, objects, samples))| {
            json!({ "site": site, "kind": kind, "bytes": bytes.round(), "objects": objects.round(), "samples": samples })
        })
        .collect();
    rows.sort_by(|a, b| b["bytes"].as_f64().partial_cmp(&a["bytes"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let total = rows.iter().filter_map(|r| r["bytes"].as_f64()).fold(0.0, |sum, bytes| sum + bytes);
    // The same by kind, whatever made it.
    let mut kinds: std::collections::BTreeMap<String, (f64, f64)> = Default::default();
    for row in &rows {
        let entry = kinds.entry(row["kind"].as_str().unwrap_or_default().to_string()).or_default();
        entry.0 += row["bytes"].as_f64().unwrap_or(0.0);
        entry.1 += row["objects"].as_f64().unwrap_or(0.0);
    }
    let mut kinds: Vec<(String, (f64, f64))> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    let kinds: Vec<Value> = kinds
        .into_iter()
        .map(|(kind, (bytes, objects))| json!({ "kind": kind, "bytes": bytes, "objects": objects }))
        .collect();
    json!({ "bytes": total, "rate": rate, "dropped": dump["heap"]["dropped"], "sites": rows, "kinds": kinds })
}

/// What a runtime function allocates, by its name.
fn kind(function: &str) -> String {
    let name = function.strip_prefix("tondo_rt_").unwrap_or(function);
    let kinds =
        [("str", "String"), ("text", "String"), ("array", "Array"), ("deque", "Deque"), ("map", "Map"), ("set", "Set")];
    let found = kinds.iter().find(|(prefix, _)| name.starts_with(prefix)).map(|(_, kind)| kind.to_string());
    found.unwrap_or_else(|| match function.starts_with("tondo_rt_") {
        true => name.to_string(),
        false => "other".to_string(),
    })
}

fn heap_text(heap: &Value) -> String {
    let rows: Vec<&Value> = heap["sites"].as_array().into_iter().flatten().collect();
    let width = rows.iter().filter_map(|r| r["site"].as_str()).map(str::len).max().unwrap_or(4).max(4);
    let mut out = format!(
        "live memory: about {}, from samples of one allocation every {} on average\n",
        bytes(heap["bytes"].as_f64().unwrap_or(0.0)),
        bytes(heap["rate"].as_f64().unwrap_or(0.0))
    );
    out.push_str(&format!("{:>12} {:>10}  {:<width$}  TYPE\n", "BYTES", "OBJECTS", "SITE"));
    for row in rows {
        out.push_str(&format!(
            "{:>12} {:>10}  {:<width$}  {}\n",
            bytes(row["bytes"].as_f64().unwrap_or(0.0)),
            row["objects"].as_f64().unwrap_or(0.0) as u64,
            row["site"].as_str().unwrap_or_default(),
            row["kind"].as_str().unwrap_or_default()
        ));
    }
    let kinds = heap["kinds"].as_array().map_or(&[][..], Vec::as_slice);
    if !kinds.is_empty() {
        out.push_str(&format!("\n{:>12} {:>10}  BY TYPE\n", "BYTES", "OBJECTS"));
    }
    for kind in kinds {
        out.push_str(&format!(
            "{:>12} {:>10}  {}\n",
            bytes(kind["bytes"].as_f64().unwrap_or(0.0)),
            kind["objects"].as_f64().unwrap_or(0.0) as u64,
            kind["kind"].as_str().unwrap_or_default()
        ));
    }
    if heap["dropped"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!("({} samples were not kept, too many alive at once)\n", heap["dropped"]));
    }
    out
}

/// A dump at a glance: why it was taken, memory, tasks by state and where
/// most wait, and where the live memory was allocated.
fn summary(dump: &Value) -> String {
    let info = &dump["info"];
    let uptime = duration(info["uptime_ms"].as_f64().unwrap_or(0.0) / 1000.0);
    let reason = match dump["reason"].as_str().unwrap_or_default() {
        "request" => "on request".to_string(),
        "schedule" => "on schedule".to_string(),
        "signal" => "on a signal".to_string(),
        reason => format!("at a {reason}"),
    };
    let mut out = format!("{} (pid {}), dumped {reason}, after {uptime}\n", program_name(info), info["pid"]);
    if let Some(built) = roots_of(dump).summary() {
        out.push_str(&format!("{built}\n"));
    }
    out.push_str(&format!("signature: {}\n", signature(dump)));
    let found = sources(dump);
    let others = found["files"].as_array().into_iter().flatten().filter(|file| file["state"] != "same").count();
    if others > 0 {
        out.push_str(&format!("{others} of its files are not here as it was built: `dump read <file> sources`\n"));
    }
    for line in dump["message"].as_str().unwrap_or_default().lines() {
        out.push_str(&format!("  {line}\n"));
    }
    let counter = |name: &str| {
        dump["counters"].as_array().into_iter().flatten().find(|c| c["name"] == name).and_then(|c| c["value"].as_f64())
    };
    out.push_str(&format!(
        "memory: {} held, {} objects, {} resident\n",
        bytes(counter("memory.held").unwrap_or(0.0)),
        counter("memory.objects").unwrap_or(0.0),
        bytes(counter("os.resident").unwrap_or(0.0))
    ));
    let tasks = tasks(dump);
    let mut states: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut places: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for task in &tasks {
        *states.entry(task["state"].as_str().unwrap_or_default().to_string()).or_default() += 1;
        if let Some(at) = task["at"].as_str() {
            *places.entry(format!("{} {at}", task["state"].as_str().unwrap_or_default())).or_default() += 1;
        }
    }
    let states: Vec<String> = states.iter().map(|(state, n)| format!("{n} {state}")).collect();
    out.push_str(&format!("tasks: {} ({})\n", tasks.len(), states.join(", ")));
    let mut places: Vec<(String, usize)> = places.into_iter().collect();
    places.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (place, count) in places.iter().take(5) {
        out.push_str(&format!("  {count} {place}\n"));
    }
    let heap = heap(dump);
    out.push_str(&format!("live memory, sampled: about {}\n", bytes(heap["bytes"].as_f64().unwrap_or(0.0))));
    for row in heap["sites"].as_array().into_iter().flatten().take(5) {
        out.push_str(&format!(
            "  {:>10}  {}  {}\n",
            bytes(row["bytes"].as_f64().unwrap_or(0.0)),
            row["site"].as_str().unwrap_or_default(),
            row["kind"].as_str().unwrap_or_default()
        ));
    }
    out
}

/// The type an object's call wrote, or the call's place.
fn object_type(dump: &Value, object: &Value) -> String {
    let site = object["site"][0].as_str().unwrap_or_default();
    let frame = &dump["frames"][site];
    match frame["type"].as_str() {
        Some(ty) => ty.to_string(),
        None => frame_text(dump, &object["site"][0]).map_or("unknown".into(), |at| format!("made at {at}")),
    }
}

/// Every object by type: how many and how many bytes, most first.
fn objects_by_type(dump: &Value) -> Value {
    let mut types: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    for object in dump["objects"]["list"].as_array().into_iter().flatten() {
        let entry = types.entry(object_type(dump, object)).or_default();
        entry.0 += 1;
        entry.1 += object["size"].as_u64().unwrap_or(0);
    }
    let mut rows: Vec<Value> =
        types.into_iter().map(|(ty, (count, bytes))| json!({ "type": ty, "objects": count, "bytes": bytes })).collect();
    rows.sort_by(|a, b| b["bytes"].as_u64().cmp(&a["bytes"].as_u64()));
    json!({ "types": rows, "largest": largest_objects(dump) })
}

/// What each object keeps alive, itself included: all that nothing else
/// reaches from a task's stack, by the dominators of the graph of objects
/// (Cooper, Harvey and Kennedy). Gives the objects that keep the most.
fn largest_objects(dump: &Value) -> Vec<Value> {
    let objects = &dump["objects"];
    let list = objects["list"].as_array().map_or(&[][..], Vec::as_slice);
    let pair = |e: &Value| Some((e[0].as_u64()? as usize, e[1].as_u64()? as usize));
    let count = list.len();
    // The stacks are one more node, from which the roots hang.
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); count + 1];
    let mut before: Vec<Vec<usize>> = vec![Vec::new(); count + 1];
    let mut link = |from: usize, to: usize| {
        if from <= count && to < count {
            after[from].push(to);
            before[to].push(from);
        }
    };
    objects["edges"].as_array().into_iter().flatten().filter_map(pair).for_each(|(from, to)| link(from, to));
    objects["roots"].as_array().into_iter().flatten().filter_map(pair).for_each(|(_, to)| link(count, to));
    // Each node after all it reaches, without recursion: chains are long.
    let (mut order, mut seen, mut stack) = (Vec::new(), vec![false; count + 1], vec![(count, 0)]);
    seen[count] = true;
    while let Some((node, next)) = stack.pop() {
        match after[node].get(next) {
            Some(&to) => {
                stack.push((node, next + 1));
                if !seen[to] {
                    seen[to] = true;
                    stack.push((to, 0));
                }
            }
            None => order.push(node),
        }
    }
    let mut position = vec![usize::MAX; count + 1];
    for (at, &node) in order.iter().enumerate() {
        position[node] = at;
    }
    let mut dominator = vec![usize::MAX; count + 1];
    dominator[count] = count;
    let meet = |dominator: &[usize], mut a: usize, mut b: usize| {
        while a != b {
            while position[a] < position[b] {
                a = dominator[a];
            }
            while position[b] < position[a] {
                b = dominator[b];
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &node in order.iter().rev().skip(1) {
            let known = before[node].iter().copied().filter(|&from| dominator[from] != usize::MAX);
            let found = known.reduce(|a, b| meet(&dominator, a, b));
            if let Some(found) = found.filter(|&found| dominator[node] != found) {
                dominator[node] = found;
                changed = true;
            }
        }
    }
    let mut kept: Vec<u64> = list.iter().map(|object| object["size"].as_u64().unwrap_or(0)).chain([0]).collect();
    for &node in &order {
        if node != count {
            kept[dominator[node]] += kept[node];
        }
    }
    let mut largest: Vec<usize> = order.iter().copied().filter(|&node| node != count).collect();
    largest.sort_by_key(|&node| std::cmp::Reverse(kept[node]));
    let shown = |&node: &usize| {
        let object = &list[node];
        json!({
            "address": object["address"], "type": object_type(dump, object), "size": object["size"],
            "retained": kept[node],
        })
    };
    largest.iter().take(10).map(shown).collect()
}

fn objects_text(by_type: &Value) -> String {
    let mut out = format!("{:>12} {:>10}  TYPE\n", "BYTES", "OBJECTS");
    for row in by_type["types"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "{:>12} {:>10}  {}\n",
            bytes(row["bytes"].as_f64().unwrap_or(0.0)),
            row["objects"].as_u64().unwrap_or(0),
            row["type"].as_str().unwrap_or_default()
        ));
    }
    let largest = by_type["largest"].as_array().map_or(&[][..], Vec::as_slice);
    if !largest.is_empty() {
        out.push_str(&format!("\n{:>12} {:>12}  WHAT KEEPS THE MOST ALIVE\n", "RETAINED", "ITS OWN"));
    }
    for object in largest {
        out.push_str(&format!(
            "{:>12} {:>12}  {} {}\n",
            bytes(object["retained"].as_f64().unwrap_or(0.0)),
            bytes(object["size"].as_f64().unwrap_or(0.0)),
            object["type"].as_str().unwrap_or_default(),
            object["address"].as_str().unwrap_or_default()
        ));
    }
    out.push_str("`object <address>` shows one and what holds it; `--json` lists them\n");
    out
}

/// One object: its type, size, where it was made and, for a string, its
/// text; the objects that point to it; and the shortest chain from a
/// task's stack that holds it.
fn object(dump: &Value, address: &str) -> Option<Value> {
    let objects = &dump["objects"];
    let list = objects["list"].as_array()?;
    let wanted = u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()?;
    let target = list.iter().position(|o| {
        u64::from_str_radix(o["address"].as_str().unwrap_or_default().trim_start_matches("0x"), 16).ok() == Some(wanted)
    })?;
    let pair = |e: &Value| Some((e[0].as_u64()? as usize, e[1].as_u64()? as usize));
    let edges: Vec<(usize, usize)> = objects["edges"].as_array().into_iter().flatten().filter_map(pair).collect();
    let roots: Vec<(usize, usize)> = objects["roots"].as_array().into_iter().flatten().filter_map(pair).collect();
    let describe = |index: usize| {
        let object = &list[index];
        format!("{} {}", object_type(dump, object), object["address"].as_str().unwrap_or_default())
    };
    let holders: Vec<String> = edges.iter().filter(|&&(_, to)| to == target).map(|&(from, _)| describe(from)).collect();
    // Breadth first from what the tasks' stacks point to.
    let mut parent: std::collections::HashMap<usize, Option<usize>> = std::collections::HashMap::new();
    let mut task_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    for &(task, object) in &roots {
        if parent.insert(object, None).is_none() {
            task_of.insert(object, task);
            queue.push_back(object);
        }
    }
    let mut next: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for &(from, to) in &edges {
        next.entry(from).or_default().push(to);
    }
    while let Some(at) = queue.pop_front() {
        if at == target {
            break;
        }
        for &to in next.get(&at).into_iter().flatten() {
            if let std::collections::hash_map::Entry::Vacant(entry) = parent.entry(to) {
                entry.insert(Some(at));
                queue.push_back(to);
            }
        }
    }
    let chain = parent.contains_key(&target).then(|| {
        let mut chain = vec![describe(target)];
        let mut at = target;
        while let Some(Some(up)) = parent.get(&at) {
            chain.push(describe(*up));
            at = *up;
        }
        chain.push(format!("task {}", task_of.get(&at).copied().unwrap_or(0)));
        chain.reverse();
        chain
    });
    let object = &list[target];
    Some(json!({
        "address": object["address"], "type": object_type(dump, object), "size": object["size"],
        "made": frame_text(dump, &object["site"][0]), "text": object["text"], "held_by": holders, "chain": chain,
    }))
}

fn object_text(object: &Value) -> String {
    let mut out = format!(
        "{} at {}, {} bytes, made at {}\n",
        object["type"].as_str().unwrap_or_default(),
        object["address"].as_str().unwrap_or_default(),
        object["size"],
        object["made"].as_str().unwrap_or("an unknown place")
    );
    if let Some(text) = object["text"].as_str() {
        out.push_str(&format!("  text: {text:?}\n"));
    }
    for holder in object["held_by"].as_array().into_iter().flatten() {
        out.push_str(&format!("  held by {}\n", holder.as_str().unwrap_or_default()));
    }
    match object["chain"].as_array() {
        Some(chain) => {
            let chain: Vec<&str> = chain.iter().filter_map(Value::as_str).collect();
            out.push_str(&format!("  kept alive by {}\n", chain.join(" -> ")));
        }
        None => out.push_str("  no task's stack reaches it: the runtime holds it, or it is garbage of a cycle\n"),
    }
    out
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_nobody_collected_has_ended() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        for _ in 0..500 {
            if !super::alive(child.id()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!super::alive(child.id()));
        child.wait().unwrap();
    }

    /// A function of an executable for macOS is found by its offset from
    /// where the executable's header is loaded, as the runtime gives it.
    #[test]
    fn names_functions_of_macho_executables() {
        let path = std::env::temp_dir().join(format!("tondo-symbols-{}", std::process::id()));
        std::fs::write(&path, crate::link::macho::example(false)).unwrap();
        let functions = super::functions(&path.display().to_string());
        let _ = std::fs::remove_file(&path);
        let at = |name: &str| functions.iter().find(|(_, _, n)| n == name).map(|(offset, ..)| *offset);
        let main = at("main").expect("main");
        assert!(main < 0x1_0000, "{main:#x}");
        assert_eq!(at("helper"), Some(main + 4));
    }
}
