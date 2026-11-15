//! How long each part of a build takes. With `TONDO_COMPILE_TIMES`,
//! internal, a line on stderr as each part ends, for the benchmarks of
//! bench/compile (docs/speed.md, A1). With `--timings` (toolchain 7.20),
//! kept and said after the build: each phase, its parts summed by name,
//! and the time each function took the code generator on its core. A part
//! that runs once per analysis, as `tondo check` makes one for each system
//! and one for the tests, counts as often.

use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A part of a build, timed until it is dropped.
pub struct Part {
    name: &'static str,
    started: Option<Instant>,
}

/// Starts timing `name`, when the switch is on or `--timings` asks.
pub fn part(name: &'static str) -> Part {
    let on = printing() || keeping();
    Part { name, started: on.then(Instant::now) }
}

fn printing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TONDO_COMPILE_TIMES").is_some())
}

impl Drop for Part {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        let took = started.elapsed();
        if printing() {
            eprintln!("tondo: {} took {:.2} ms", self.name, took.as_secs_f64() * 1000.0);
        }
        if keeping() {
            let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
            match kept.parts.iter_mut().find(|(name, _)| *name == self.name) {
                Some((_, sum)) => *sum += took,
                None => kept.parts.push((self.name, took)),
            }
        }
    }
}

/// What `--timings` keeps.
struct Kept {
    /// What was timed, `build` or `check`, and since when.
    what: &'static str,
    started: Option<Instant>,
    parts: Vec<(&'static str, Duration)>,
    /// Each function's time in the code generator, with its file.
    functions: Vec<(String, String, Duration)>,
    /// Whether a build came from the cache.
    cached: bool,
    /// Whether a command's JSON has said what was kept.
    said: bool,
}

static KEEPING: AtomicBool = AtomicBool::new(false);
impl Kept {
    const NONE: Kept =
        Kept { what: "build", started: None, parts: Vec::new(), functions: Vec::new(), cached: false, said: false };
}

static KEPT: Mutex<Kept> = Mutex::new(Kept::NONE);

/// Keeps the times of what this process builds, from now: `what` is the
/// build, or the check.
pub fn keep(what: &'static str) {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    *kept = Kept { what, started: Some(Instant::now()), ..Kept::NONE };
    KEEPING.store(true, Ordering::Relaxed);
}

/// Keeps no more, as a server does after the one check that asked.
pub fn stop() {
    KEEPING.store(false, Ordering::Relaxed);
}

pub fn keeping() -> bool {
    KEEPING.load(Ordering::Relaxed)
}

/// The time the code generator took on `function`, of `file`.
pub fn function(function: String, file: String, took: Duration) {
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).functions.push((function, file, took));
}

/// A build that came from the cache.
pub fn cached() {
    if keeping() {
        KEPT.lock().unwrap_or_else(|e| e.into_inner()).cached = true;
    }
}

/// The phases, each with the parts it is made of.
const PHASES: [(&str, &[&str]); 6] = [
    ("finding it in the cache", &["cache"]),
    ("reading and checking", &["parse", "meta, derive and templates", "std", "modules", "check", "mono"]),
    ("lowering", &["lower"]),
    ("passes", &["MIR passes"]),
    ("code generator", &["IR", "Cranelift", "object"]),
    ("link", &["link"]),
];

/// How many files and functions are named.
const NAMED: usize = 10;

/// What was kept, as data: the whole, each phase that took any time, and
/// the files and the functions that took the code generator longest.
pub fn json() -> Value {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    kept.said = true;
    let ms = |took: Duration| (took.as_secs_f64() * 100_000.0).round() / 100.0;
    let total = kept.started.map_or(Duration::ZERO, |started| started.elapsed());
    let phases: Vec<Value> = PHASES
        .iter()
        .filter_map(|(phase, parts)| {
            let took: Duration =
                kept.parts.iter().filter(|(name, _)| parts.contains(name)).map(|(_, took)| *took).sum();
            (!took.is_zero()).then(|| json!({ "phase": phase, "ms": ms(took) }))
        })
        .collect();
    let mut files: Vec<(String, Duration, usize)> = Vec::new();
    for (_, file, took) in &kept.functions {
        match files.iter_mut().find(|(known, ..)| known == file) {
            Some((_, sum, count)) => {
                *sum += *took;
                *count += 1;
            }
            None => files.push((file.clone(), *took, 1)),
        }
    }
    files.sort_by_key(|a| std::cmp::Reverse(a.1));
    let mut functions = kept.functions.clone();
    functions.sort_by_key(|a| std::cmp::Reverse(a.2));
    let files: Vec<Value> = files
        .iter()
        .take(NAMED)
        .map(|(file, took, count)| json!({ "file": file, "ms": ms(*took), "functions": count }))
        .collect();
    let functions: Vec<Value> = functions
        .iter()
        .take(NAMED)
        .map(|(function, file, took)| json!({ "function": function, "file": file, "ms": ms(*took) }))
        .collect();
    json!({ "ms": ms(total), "cached": kept.cached, "phases": phases, "files": files, "functions": functions })
}

/// What was kept, for a person; none when a command's JSON said it.
pub fn text() -> Option<String> {
    if !keeping() || KEPT.lock().unwrap_or_else(|e| e.into_inner()).said {
        return None;
    }
    let timings = json();
    let time = |ms: &Value| {
        let ms = ms.as_f64().unwrap_or(0.0);
        match ms >= 1000.0 {
            true => format!("{:.2} s", ms / 1000.0),
            false => format!("{ms:.1} ms"),
        }
    };
    let cached = if timings["cached"] == true { ", from the cache" } else { "" };
    let what = KEPT.lock().unwrap_or_else(|e| e.into_inner()).what;
    let mut out = format!("the {what} took {}{cached}\n", time(&timings["ms"]));
    for phase in timings["phases"].as_array().into_iter().flatten() {
        out.push_str(&format!("  {:<26}{:>10}\n", phase["phase"].as_str().unwrap_or_default(), time(&phase["ms"])));
    }
    let files = timings["files"].as_array().cloned().unwrap_or_default();
    if !files.is_empty() {
        out.push_str("the files whose functions took the code generator longest, on one core:\n");
        for file in &files {
            let count = file["functions"].as_u64().unwrap_or(0);
            let functions = if count == 1 { "1 function".to_string() } else { format!("{count} functions") };
            let name = file["file"].as_str().unwrap_or_default();
            out.push_str(&format!("  {name:<40}{:>10}  {functions}\n", time(&file["ms"])));
        }
        out.push_str("the functions:\n");
        for function in timings["functions"].as_array().into_iter().flatten() {
            let name = function["function"].as_str().unwrap_or_default();
            let file = function["file"].as_str().unwrap_or_default();
            out.push_str(&format!("  {name:<40}{:>10}  {file}\n", time(&function["ms"])));
        }
    }
    Some(out)
}
