//! The debugger of a debug build (toolchain 7.6). The compiler puts a probe
//! where each line starts, with the addresses of the function's bindings
//! and a bit for each that holds a value; while no debugger is attached a
//! probe only reads a flag. A tool attaches through the diagnostics
//! socket: it sets breakpoints, steps, and reads the bindings and stack of
//! the task that stopped. When one task stops every task stops at its next
//! probe, and they all go on together.

use super::{Builder, Str, str_bytes};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

/// Shows a value: appends it, as the debugger writes it, to a builder.
type Show = unsafe extern "C" fn(*mut Builder, *const u8);

/// The table the compiler writes (`codegen::debug_table`).
#[repr(C)]
pub struct Raw {
    text: *const Str,
    glues: *const Show,
    count: u64,
}

struct Binding {
    name: String,
    ty: String,
    show: Show,
}

struct Function {
    name: String,
    bindings: Vec<Binding>,
}

struct Site {
    file: String,
    line: u32,
    function: usize,
}

struct Table {
    functions: Vec<Function>,
    sites: Vec<Site>,
    /// Whether each site has a breakpoint, which a probe reads first.
    marked: Vec<AtomicBool>,
}

static TABLE: OnceLock<Table> = OnceLock::new();
/// A debugger is attached: probes look at what it asked for.
static ATTACHED: AtomicBool = AtomicBool::new(false);
/// Every task stops at its next probe: one stopped, or the debugger asked.
static HOLD: AtomicBool = AtomicBool::new(false);

/// Called by a debug build before `main`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_table(raw: *const Raw) {
    let raw = unsafe { &*raw };
    let text = String::from_utf8_lossy(unsafe { str_bytes(raw.text) }).into_owned();
    let glues = unsafe { std::slice::from_raw_parts(raw.glues, raw.count as usize) };
    let (mut functions, mut sites, mut next) = (Vec::new(), Vec::new(), 0);
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            ["F", name] => functions.push(Function { name: name.to_string(), bindings: Vec::new() }),
            ["L", name, ty] => {
                let show = glues[next];
                next += 1;
                if let Some(function) = functions.last_mut() {
                    function.bindings.push(Binding { name: name.to_string(), ty: ty.to_string(), show });
                }
            }
            ["S", file, line, function] => sites.push(Site {
                file: file.to_string(),
                line: line.parse().unwrap_or(0),
                function: function.parse().unwrap_or(0),
            }),
            _ => {}
        }
    }
    let marked = sites.iter().map(|_| AtomicBool::new(false)).collect();
    let _ = TABLE.set(Table { functions, sites, marked });
    if WAIT.load(Ordering::Relaxed) {
        start_stopped();
    }
}

/// Under `tondo debug`, the program waits a while for the debugger, then
/// stands still before `main` until it says go on, so that it can set its
/// breakpoints first.
fn start_stopped() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ATTACHED.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // The debugger's `attach` made the stop at the start; it may have gone
    // on already.
    let mut state = state();
    BEFORE_MAIN.store(false, Ordering::Relaxed);
    while state.stopped.as_ref().is_some_and(|stop| stop.site == usize::MAX) {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
}

// ---------------------------------------------------------------------------
// Stopping
// ---------------------------------------------------------------------------

/// How the debugger asked the program to go on: until a breakpoint, or for
/// one task, to the next line, the next one in its frame or an outer one,
/// or the first in an outer frame.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Run,
    Step { task: u64 },
    Next { task: u64, frame: usize },
    Finish { task: u64, frame: usize },
}

/// Where the task that stopped is: its probe, the frame of its function
/// and its stack's bounds, and its bindings.
#[derive(Clone)]
struct Stop {
    task: u64,
    site: usize,
    frame: usize,
    bounds: (usize, usize),
    addresses: usize,
    alive: usize,
    why: String,
}

struct State {
    mode: Mode,
    stopped: Option<Stop>,
    /// Counts the times the program went on; a stopped task waits for it to
    /// change.
    resumed: u64,
    /// Where each held task is, by its number.
    held: Vec<(u64, usize)>,
    breakpoints: Vec<Breakpoint>,
    next_breakpoint: u32,
}

struct Breakpoint {
    id: u32,
    sites: Vec<usize>,
    condition: Option<Condition>,
}

/// `name op literal`, against a binding's value as the debugger shows it.
struct Condition {
    name: String,
    op: String,
    value: String,
}

static STATE: Mutex<State> = Mutex::new(State {
    mode: Mode::Run,
    stopped: None,
    resumed: 0,
    held: Vec::new(),
    breakpoints: Vec::new(),
    next_breakpoint: 1,
});
static RESUMED: Condvar = Condvar::new();
/// The tool that debugs the program, which hears when it stops, and the
/// descriptor of its connection to the diagnostics thread.
static CLIENT: Mutex<Option<UnixStream>> = Mutex::new(None);
static CLIENT_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
/// The program started under `tondo debug`, which it waits for before
/// `main`; and it has not reached `main` yet.
pub(crate) static WAIT: AtomicBool = AtomicBool::new(false);
static BEFORE_MAIN: AtomicBool = AtomicBool::new(true);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn tell(event: &str) {
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(client, "{event}");
    }
}

/// The probe where a line starts: stops the task when the debugger asked
/// for it here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_line(site: i64, addresses: *const usize, alive: *const u64) {
    if !ATTACHED.load(Ordering::Relaxed) {
        return;
    }
    let Some(table) = TABLE.get() else { return };
    let site = site as usize;
    // The frame of the function that has the probe.
    let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
    let task = super::tasks::current_id();
    let marked = table.marked.get(site).is_some_and(|m| m.load(Ordering::Relaxed));
    if !marked && !HOLD.load(Ordering::Relaxed) && state().mode == Mode::Run {
        return;
    }
    let mut state = state();
    let why = if state.stopped.is_some() || HOLD.load(Ordering::Relaxed) {
        // Another task stopped, or the debugger asked all to.
        if state.stopped.is_some() { None } else { Some("pause".to_string()) }
    } else {
        let stepped = match state.mode {
            Mode::Run => false,
            Mode::Step { task: t } => t == task,
            Mode::Next { task: t, frame: f } => t == task && frame >= f,
            Mode::Finish { task: t, frame: f } => t == task && frame > f,
        };
        let hit = state.breakpoints.iter().find(|b| {
            b.sites.contains(&site)
                && b.condition.as_ref().is_none_or(|c| holds(c, table, site, addresses as usize, alive as usize))
        });
        match (hit, stepped) {
            (Some(breakpoint), _) => Some(format!("breakpoint {}", breakpoint.id)),
            (None, true) => Some("step".to_string()),
            (None, false) => return,
        }
    };
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    if let Some(why) = why {
        let stop = Stop { task, site, frame, bounds, addresses: addresses as usize, alive: alive as usize, why };
        tell(&stopped_event(&stop));
        state.stopped = Some(stop);
        state.mode = Mode::Run;
        HOLD.store(true, Ordering::Relaxed);
    }
    state.held.push((task, site));
    // Out of Tondo code while it waits, so collections and dumps go on.
    let paused = super::tasks::pause_here();
    let resumed = state.resumed;
    while state.resumed == resumed {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    state.held.retain(|&(t, _)| t != task);
    drop(state);
    if paused {
        super::tasks::enter_tondo();
    }
}

fn stopped_event(stop: &Stop) -> String {
    format!(
        "{{\"stopped\":{{\"why\":{},\"task\":{},\"at\":{}}}}}",
        quoted(&stop.why),
        stop.task,
        quoted(&place(stop.site))
    )
}

/// A probe's place: `file:line in function`.
fn place(site: usize) -> String {
    let Some(table) = TABLE.get() else { return String::new() };
    let Some(at) = table.sites.get(site) else { return "the start of the program".into() };
    format!("{}:{} in {}", at.file, at.line, table.functions[at.function].name)
}

/// Goes on as `mode` says.
fn resume(mode: Mode) {
    let mut state = state();
    state.mode = mode;
    state.stopped = None;
    HOLD.store(false, Ordering::Relaxed);
    state.resumed += 1;
    RESUMED.notify_all();
}

// ---------------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------------

/// The bindings of the function at a probe that hold a value: name, type
/// and value as the debugger shows it.
fn bindings(table: &Table, site: usize, addresses: usize, alive: usize) -> Vec<(String, String, String)> {
    let Some(at) = table.sites.get(site) else { return Vec::new() };
    let function = &table.functions[at.function];
    let mut out = Vec::new();
    for (index, binding) in function.bindings.iter().enumerate() {
        let word = unsafe { *((alive as *const u64).add(index / 64)) };
        if word & (1 << (index % 64)) == 0 {
            continue;
        }
        let address = unsafe { *((addresses as *const usize).add(index)) };
        out.push((binding.name.clone(), binding.ty.clone(), show(binding.show, address)));
    }
    out
}

/// A value as the debugger shows it, at most some thousands of bytes.
fn show(glue: Show, address: usize) -> String {
    let builder = super::tondo_rt_sb_new();
    unsafe { glue(builder, address as *const u8) };
    let text = unsafe { super::tondo_rt_sb_finish(builder) };
    let mut shown = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    unsafe { super::tondo_rt_str_release(text) };
    if shown.len() > 4096 {
        let mut end = 4096;
        while !shown.is_char_boundary(end) {
            end -= 1;
        }
        shown.truncate(end);
        shown.push_str("...");
    }
    shown
}

/// Whether a breakpoint's condition holds at a probe: numbers compare as
/// numbers, and anything else as the text the debugger shows, a string
/// without its quotes.
fn holds(condition: &Condition, table: &Table, site: usize, addresses: usize, alive: usize) -> bool {
    let Some((_, _, value)) = bindings(table, site, addresses, alive).into_iter().find(|(n, ..)| *n == condition.name)
    else {
        return false;
    };
    let unquote = |text: &str| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(text).to_string();
    let (left, right) = (unquote(&value), unquote(&condition.value));
    let order = match (left.parse::<f64>(), right.parse::<f64>()) {
        (Ok(a), Ok(b)) => a.partial_cmp(&b),
        _ => Some(left.cmp(&right)),
    };
    use std::cmp::Ordering::*;
    matches!(
        (condition.op.as_str(), order),
        ("==", Some(Equal))
            | ("!=", Some(Less | Greater))
            | ("<", Some(Less))
            | ("<=", Some(Less | Equal))
            | (">", Some(Greater))
            | (">=", Some(Greater | Equal))
    )
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Attaches the tool on `client` as the debugger: it hears when the
/// program stops.
pub(crate) fn attach(client: &UnixStream) -> String {
    let Some(table) = TABLE.get() else {
        return "{\"error\":\"the program was not built with --debug\"}".into();
    };
    let mut slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return "{\"error\":\"another tool debugs this program\"}".into();
    }
    use std::os::fd::AsRawFd;
    *slot = client.try_clone().ok();
    CLIENT_FD.store(client.as_raw_fd(), Ordering::Relaxed);
    drop(slot);
    // A program that waits for the debugger stands still before `main`
    // from now on, until it says go on.
    if WAIT.load(Ordering::Relaxed) && BEFORE_MAIN.load(Ordering::Relaxed) {
        let stop =
            Stop { task: 1, site: usize::MAX, frame: 0, bounds: (0, 0), addresses: 0, alive: 0, why: "start".into() };
        tell(&stopped_event(&stop));
        state().stopped = Some(stop);
    }
    ATTACHED.store(true, Ordering::Relaxed);
    format!("{{\"debugging\":true,\"lines\":{}}}", table.sites.len())
}

/// The debugger went away: the program goes on as if it never came.
pub(crate) fn detach() {
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CLIENT_FD.store(-1, Ordering::Relaxed);
    ATTACHED.store(false, Ordering::Relaxed);
    let mut state = state();
    state.breakpoints.clear();
    if let Some(table) = TABLE.get() {
        table.marked.iter().for_each(|m| m.store(false, Ordering::Relaxed));
    }
    drop(state);
    resume(Mode::Run);
}

/// Whether the connection on `fd` is the debugger's.
pub(crate) fn is_client(fd: i32) -> bool {
    CLIENT_FD.load(Ordering::Relaxed) == fd
}

/// The program ended: the debugger hears it.
pub(crate) fn exited() {
    tell("{\"exited\":true}");
}

/// Answers a command of the debugger, or `None` for one it does not know.
pub(crate) fn command(line: &str) -> Option<String> {
    let table = TABLE.get()?;
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let stopped = || state().stopped.clone();
    let not_stopped = || "{\"error\":\"the program runs: `pause` stops it, or a breakpoint\"}".to_string();
    Some(match word {
        "break" => breakpoint(table, rest),
        "delete" => {
            let id: u32 = rest.parse().unwrap_or(0);
            let mut state = state();
            state.breakpoints.retain(|b| b.id != id);
            remark(table, &state);
            format!("{{\"deleted\":{id}}}")
        }
        "continue" => {
            resume(Mode::Run);
            "{\"running\":true}".into()
        }
        "step" | "next" | "finish" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let mode = match word {
                "step" => Mode::Step { task: stop.task },
                "next" => Mode::Next { task: stop.task, frame: stop.frame },
                _ => Mode::Finish { task: stop.task, frame: stop.frame },
            };
            resume(mode);
            "{\"running\":true}".into()
        }
        "pause" => {
            HOLD.store(true, Ordering::Relaxed);
            "{\"pausing\":true}".into()
        }
        "locals" | "print" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let found = bindings(table, stop.site, stop.addresses, stop.alive);
            if word == "print" {
                return Some(match found.iter().find(|(name, ..)| name == rest) {
                    Some((name, ty, value)) => {
                        format!("{{\"name\":{},\"type\":{},\"value\":{}}}", quoted(name), quoted(ty), quoted(value))
                    }
                    None => format!("{{\"error\":{}}}", quoted(&format!("no binding `{rest}` holds a value here"))),
                });
            }
            let items: Vec<String> = found
                .iter()
                .map(|(name, ty, value)| {
                    format!("{{\"name\":{},\"type\":{},\"value\":{}}}", quoted(name), quoted(ty), quoted(value))
                })
                .collect();
            format!("{{\"locals\":[{}]}}", items.join(","))
        }
        "stack" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let mut frames = vec![quoted(&place(stop.site))];
            if stop.site == usize::MAX {
                return Some(format!("{{\"stack\":[{}]}}", frames.join(",")));
            }
            for address in super::trace::walk(stop.frame, stop.bounds) {
                if let Some(described) = super::trace::describe(address)
                    && let Some((file, line)) = described.at
                    && !described.function.starts_with("spawn")
                {
                    frames.push(quoted(&format!("{file}:{line} in {}", described.function)));
                }
            }
            format!("{{\"stack\":[{}]}}", frames.join(","))
        }
        "tasks" => {
            let state = state();
            let held: Vec<String> = state
                .held
                .iter()
                .map(|&(task, site)| {
                    let why = match &state.stopped {
                        Some(stop) if stop.task == task => stop.why.clone(),
                        _ => "held".to_string(),
                    };
                    format!("{{\"task\":{task},\"at\":{},\"why\":{}}}", quoted(&place(site)), quoted(&why))
                })
                .collect();
            format!("{{\"tasks\":[{}]}}", held.join(","))
        }
        _ => return None,
    })
}

/// `break <file>:<line> [if <name> <op> <value>]`: the probes of that line,
/// or of the next line that has one in the file.
fn breakpoint(table: &Table, text: &str) -> String {
    let error = |message: String| format!("{{\"error\":{}}}", quoted(&message));
    let (at, condition) = match text.split_once(" if ") {
        Some((at, condition)) => (at.trim(), Some(condition.trim())),
        None => (text.trim(), None),
    };
    let Some((file, line)) = at.rsplit_once(':') else {
        return error(format!("a breakpoint is `file:line`, as `main.to:12`, not `{at}`"));
    };
    let Ok(line) = line.parse::<u32>() else { return error(format!("`{line}` is not a line number")) };
    // A file as the compiler named it, or a path that ends with it, as an
    // editor gives.
    let matches = |site: &Site| {
        site.file == file || site.file.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{}", site.file))
    };
    let Some(found) = table.sites.iter().filter(|s| matches(s) && s.line >= line).map(|s| s.line).min() else {
        let files: std::collections::BTreeSet<&str> = table.sites.iter().map(|s| s.file.as_str()).collect();
        return error(format!(
            "no code at {file}:{line} or after it; the program's files are {}",
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    };
    let condition = match condition.map(parse_condition) {
        Some(Err(message)) => return error(message),
        Some(Ok(condition)) => Some(condition),
        None => None,
    };
    let sites = table.sites.iter().enumerate().filter(|(_, s)| matches(s) && s.line == found).map(|(i, _)| i).collect();
    let mut state = state();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    state.breakpoints.push(Breakpoint { id, sites, condition });
    remark(table, &state);
    let file = table.sites.iter().find(|s| matches(s) && s.line == found).map_or(file, |s| s.file.as_str());
    format!("{{\"breakpoint\":{{\"id\":{id},\"file\":{},\"line\":{found}}}}}", quoted(file))
}

fn parse_condition(text: &str) -> Result<Condition, String> {
    for op in ["==", "!=", "<=", ">=", "<", ">"] {
        if let Some((name, value)) = text.split_once(op) {
            let (name, value) = (name.trim(), value.trim());
            if !name.is_empty() && !value.is_empty() && !name.contains(' ') {
                return Ok(Condition { name: name.into(), op: op.into(), value: value.into() });
            }
        }
    }
    Err(format!("a condition is `name op value` with ==, !=, <, <=, > or >=, as `count > 3`, not `{text}`"))
}

/// Marks the sites that have a breakpoint.
fn remark(table: &Table, state: &State) {
    for (site, marked) in table.marked.iter().enumerate() {
        marked.store(state.breakpoints.iter().any(|b| b.sites.contains(&site)), Ordering::Relaxed);
    }
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
