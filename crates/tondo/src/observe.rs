//! The tools that look at running Tondo programs (toolchain 7.5): `tondo
//! ps` and `tondo counters`, through the socket each program serves in
//! `$TMPDIR/tondo-<uid>`, named after its process.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

fn directory() -> PathBuf {
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
fn running() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(directory()) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let info = Program::connect(pid).map_err(|e| e.to_string()).and_then(|mut p| p.ask("info"));
        match info {
            Ok(info) => found.push(info),
            Err(_) if unsafe { libc::kill(pid as i32, 0) } != 0 => {
                let _ = std::fs::remove_file(entry.path());
            }
            Err(_) => {}
        }
    }
    found.sort_by_key(|info| info["pid"].as_u64());
    found
}

fn program_name(info: &Value) -> &str {
    let program = info["program"].as_str().unwrap_or_default();
    program.rsplit('/').next().unwrap_or(program)
}

/// `tondo ps [--json]`.
pub fn ps(json: bool) -> Result<(), String> {
    let programs = running();
    if json {
        println!("{}", Value::Array(programs));
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

pub(crate) fn connect(name: &str) -> Result<Program, String> {
    let pid = target(name)?;
    Program::connect(pid).map_err(|_| {
        format!(
            "no Tondo program with process ID {pid} answers: a program serves the tools when it starts with TONDO_DIAGNOSTICS=1, and `tondo ps` lists those that do"
        )
    })
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
struct Recording {
    clock: u64,
    events: Vec<Value>,
    samples: Vec<Value>,
    frames: serde_json::Map<String, Value>,
    dropped: u64,
    lost: u64,
}

/// `tondo trace`: records until the program ends, `duration` passes or
/// Ctrl-C, then writes the timeline and the profile.
pub fn trace(traced: Traced, duration: Option<Duration>, output: Option<PathBuf>) -> Result<(), String> {
    let (mut program, mut child) = match traced {
        Traced::Running(name) => (connect(&name)?, None),
        Traced::Started(executable, arguments) => {
            use std::os::unix::process::CommandExt;
            // Its own process group, so that Ctrl-C ends the trace first.
            let child = std::process::Command::new(&executable)
                .args(&arguments)
                .env("TONDO_TRACE", "1")
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
    let mut line = String::new();
    loop {
        let stop = INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
            || duration.is_some_and(|duration| started_at.elapsed() >= duration);
        if stop && !stopping {
            stopping = true;
            let _ = writeln!(program.writer, "stop");
        }
        match program.reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let Ok(chunk) = serde_json::from_str::<Value>(&line) else { break };
                line.clear();
                if chunk.get("end").is_some() {
                    break;
                }
                recording.add(chunk);
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    if let Some(child) = child.as_mut() {
        // A trace stopped early ends the program it started.
        if stopping {
            unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
        }
        let _ = child.wait();
    }
    let base = output
        .map(|path| path.with_extension(""))
        .unwrap_or_else(|| PathBuf::from(format!("{}-{}", program_name(&info), program.pid)));
    let timeline = base.with_extension("json");
    let profile = base.with_extension("folded");
    let write = |path: &PathBuf, text: String| {
        std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    };
    let names = Names::new(&recording.frames, info["program"].as_str().unwrap_or_default());
    write(&timeline, recording.timeline(&info, program.pid, &names))?;
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
    eprintln!("tondo: {summary}");
    Ok(())
}

/// Connects to a program this trace started, once it listens.
pub(crate) fn started(executable: &std::path::Path, pid: u32) -> Result<Program, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(program) = Program::connect(pid) {
            return Ok(program);
        }
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
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
            let outermost = stack
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .filter_map(Value::as_str)
                .find_map(|address| self.frames.get(address).and_then(|frame| frame["function"].as_str()));
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
                .map(|a| names.function(a.as_str().unwrap_or_default()))
                .collect();
            *counts.entry(frames.join(";")).or_default() += 1;
        }
        counts.iter().map(|(stack, count)| format!("{stack} {count}\n")).collect()
    }
}

/// The names of the addresses of a trace: the program's functions as the
/// runtime describes them, and others from the symbols of the file that
/// holds them.
struct Names<'a> {
    frames: &'a serde_json::Map<String, Value>,
    program: String,
    symbols: std::cell::RefCell<std::collections::HashMap<String, Vec<Function>>>,
}

impl<'a> Names<'a> {
    fn new(frames: &'a serde_json::Map<String, Value>, program: &str) -> Names<'a> {
        Names { frames, program: program.to_string(), symbols: Default::default() }
    }

    /// A frame's function.
    fn function(&self, address: &str) -> String {
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
    use object::{Object, ObjectSymbol};
    let Ok(data) = std::fs::read(path) else { return Vec::new() };
    let Ok(file) = object::File::parse(data.as_slice()) else { return Vec::new() };
    let base = file.relative_address_base();
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
pub fn dump(name: &str, output: Option<PathBuf>) -> Result<(), String> {
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
    print!("{}", summary(&dump));
    eprintln!("tondo: the dump is in {}; `tondo dump read {}` shows it", path.display(), path.display());
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
}

/// `tondo dump read <file> [tasks | task <id> | heap] [--json]`.
pub fn read(path: &std::path::Path, view: View, json: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut dump: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not a dump: {e}", path.display()))?;
    if dump.get("dump").is_none() {
        return Err(format!("{} is not a dump of a Tondo program", path.display()));
    }
    name_frames(&mut dump);
    let tasks = tasks(&dump);
    let out = match view {
        View::Summary if json => json!({ "summary": summary(&dump) }).to_string(),
        View::Summary => summary(&dump),
        View::Tasks if json => Value::Array(tasks).to_string(),
        View::Tasks => tasks.iter().map(|task| task_text(task, false)).collect::<Vec<_>>().join("\n"),
        View::Task(id) => {
            let task = tasks.iter().find(|t| t["id"] == id).ok_or_else(|| {
                format!("the dump has no task {id}; `tondo dump read {} tasks` lists them", path.display())
            })?;
            if json { task.to_string() } else { task_text(task, true) }
        }
        View::Heap if json => heap(&dump).to_string(),
        View::Heap => heap_text(&heap(&dump)),
        View::Objects | View::Object(_) if dump.get("objects").is_none() => {
            return Err("this dump has no objects: a dump of a build with --debug has every object".into());
        }
        View::Objects if json => objects_by_type(&dump).to_string(),
        View::Objects => objects_text(&objects_by_type(&dump)),
        View::Object(address) => {
            let object = object(&dump, &address).ok_or_else(|| {
                format!(
                    "the dump has no object at {address}; `tondo dump read {} objects` lists them by type",
                    path.display()
                )
            })?;
            if json { object.to_string() } else { object_text(&object) }
        }
    };
    println!("{}", out.trim_end());
    Ok(())
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
            list.as_array().into_iter().flatten().filter_map(|a| frame_text(dump, a)).collect()
        };
        out.push(json!({
            "id": task["id"], "runs": runs, "state": task["state"], "at": at, "cancelled": task["cancelled"],
            "stack": words(&task["stack"]), "spawned": words(&task["spawned"]), "locks": task["locks"],
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
        let own = stack.iter().position(|a| {
            placed(dump, a)
                && !dump["frames"][a.as_str().unwrap_or_default()]["file"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("<std>/")
        });
        let site = own
            .or(first)
            .and_then(|i| frame_text(dump, stack[i]))
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
    json!({ "bytes": total, "rate": rate, "dropped": dump["heap"]["dropped"], "sites": rows })
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
        reason => format!("at a {reason}"),
    };
    let mut out = format!("{} (pid {}), dumped {reason}, after {uptime}\n", program_name(info), info["pid"]);
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
    json!({ "types": rows })
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
