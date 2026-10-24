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
struct Program {
    pid: u32,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Program {
    fn connect(pid: u32) -> std::io::Result<Program> {
        let stream = UnixStream::connect(directory().join(pid.to_string()))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Program { pid, reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    /// The program's answer to `request`, or why there is none.
    fn ask(&mut self, request: &str) -> Result<Value, String> {
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
        println!("no Tondo program is running");
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
        [] => Err(format!("no running Tondo program is named `{name}`; `tondo ps` lists them")),
        pids => {
            let pids: Vec<String> = pids.iter().map(u64::to_string).collect();
            Err(format!("{} programs are named `{name}`: give a process ID, one of {}", pids.len(), pids.join(", ")))
        }
    }
}

fn connect(name: &str) -> Result<Program, String> {
    let pid = target(name)?;
    Program::connect(pid).map_err(|_| {
        format!(
            "no Tondo program with process ID {pid} answers: `tondo ps` lists those that do (a program started with TONDO_DIAGNOSTICS=0 does not)"
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
