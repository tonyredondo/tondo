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
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, RwLock};

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
    /// Where each function keeps the addresses of its bindings and the bits
    /// of those that hold a value, from its frame pointer, once one of its
    /// probes ran with a debugger attached: the frames of a stack show their
    /// bindings from them.
    offsets: Vec<(AtomicIsize, AtomicIsize)>,
}

/// An offset not known yet.
const UNKNOWN: isize = isize::MIN;

/// The program's table, to which a live build's patches add theirs. A
/// lock taken before `STATE`, and never held while a task waits.
static TABLE: OnceLock<RwLock<Table>> = OnceLock::new();

fn table() -> Option<std::sync::RwLockReadGuard<'static, Table>> {
    TABLE.get().map(|table| table.read().unwrap_or_else(|e| e.into_inner()))
}
/// A debugger is attached: probes look at what it asked for.
static ATTACHED: AtomicBool = AtomicBool::new(false);
/// Every task stops at its next probe: one stopped, or the debugger asked.
static HOLD: AtomicBool = AtomicBool::new(false);

/// Called by a debug build's process before anything else, or by a
/// library before its first call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_table(raw: *const Raw) {
    let (functions, sites) = unsafe { parse(raw) };
    let marked = sites.iter().map(|_| AtomicBool::new(false)).collect();
    let offsets = functions.iter().map(|_| (AtomicIsize::new(UNKNOWN), AtomicIsize::new(UNKNOWN))).collect();
    let _ = TABLE.set(RwLock::new(Table { functions, sites, marked, offsets }));
    super::heap::track_objects();
}

/// Adds a patch's table: its functions after the program's, its sites
/// after those it numbers from; the breakpoints of its lines mark it too.
/// Gives how many sites it added.
pub(crate) fn add_table(raw: *const Raw) -> usize {
    let (functions, sites) = unsafe { parse(raw) };
    let Some(lock) = TABLE.get() else { return 0 };
    let mut table = lock.write().unwrap_or_else(|e| e.into_inner());
    let (offset, first) = (table.functions.len(), table.sites.len());
    let added = functions.iter().map(|_| (AtomicIsize::new(UNKNOWN), AtomicIsize::new(UNKNOWN)));
    table.offsets.extend(added);
    table.functions.extend(functions);
    for mut site in sites {
        site.function += offset;
        table.sites.push(site);
        table.marked.push(AtomicBool::new(false));
    }
    let mut state = state();
    for breakpoint in &mut state.breakpoints {
        let Some(&known) = breakpoint.sites.first() else { continue };
        let (file, line) = (table.sites[known].file.clone(), table.sites[known].line);
        let added = (first..table.sites.len()).filter(|&i| table.sites[i].file == file && table.sites[i].line == line);
        breakpoint.sites.extend(added);
    }
    remark(&table, &state);
    table.sites.len() - first
}

/// Whether the program is a debug build.
pub(crate) fn built() -> bool {
    TABLE.get().is_some()
}

/// The number of sites, which a patch's start from.
pub(crate) fn sites() -> usize {
    table().map_or(0, |table| table.sites.len())
}

unsafe fn parse(raw: *const Raw) -> (Vec<Function>, Vec<Site>) {
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
    (functions, sites)
}

/// Under `tondo debug`, a debug build waits a while for the debugger, then
/// stands still before `main` until it says go on, so that it can set its
/// breakpoints first.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn start_stopped() {
    if TABLE.get().is_none() || !WAIT.load(Ordering::Relaxed) {
        return;
    }
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
    Step {
        task: u64,
    },
    Next {
        task: u64,
        frame: usize,
    },
    Finish {
        task: u64,
        frame: usize,
    },
    /// A frame restarts: stop where its function's new call starts.
    Restarted {
        task: u64,
    },
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
    /// Where each held task is, and its frame.
    held: Vec<Stop>,
    /// The task and frame whose bindings `locals` and `print` show; the
    /// stopped task's own by default.
    selected: Option<(u64, usize)>,
    breakpoints: Vec<Breakpoint>,
    next_breakpoint: u32,
}

struct Breakpoint {
    id: u32,
    sites: Vec<usize>,
    condition: Option<Condition>,
    /// How many times its condition held here, and from which count on it
    /// stops; and the message it writes instead of stopping (a logpoint).
    count: u64,
    hits: Option<Hits>,
    log: Option<String>,
}

/// When a breakpoint stops by its count of hits: at the `n`th, from it on,
/// past it, or at every `n`th.
struct Hits {
    op: &'static str,
    n: u64,
}

impl Hits {
    fn parse(text: &str) -> Result<Hits, String> {
        let text = text.trim();
        for op in [">=", ">", "==", "%"] {
            if let Some(n) = text.strip_prefix(op).and_then(|n| n.trim().parse().ok()) {
                return Ok(Hits { op, n });
            }
        }
        match text.parse() {
            Ok(n) => Ok(Hits { op: "==", n }),
            Err(_) => Err(format!("a count of hits is `n`, `== n`, `>= n`, `> n` or `% n`, not `{text}`")),
        }
    }

    fn reached(&self, count: u64) -> bool {
        match self.op {
            ">=" => count >= self.n,
            ">" => count > self.n,
            "%" => self.n > 0 && count.is_multiple_of(self.n),
            _ => count == self.n,
        }
    }
}

/// A logpoint's message: each `{path}` in it replaced by what it names
/// there, a text without its quotes.
fn logged(table: &Table, frame: &Frame, message: &str) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else { break };
        let path = rest[open + 1..open + close].trim();
        match resolve(table, frame, path) {
            Ok(found) => {
                let text = &found.text;
                let unquoted = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
                out.push_str(unquoted.unwrap_or(text));
            }
            Err(_) => out.push_str(&format!("{{{path}?}}")),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
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
    selected: None,
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
/// for it here. In a live build, whether the frame is to restart: the
/// statement after the probe then returns and calls its function again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_line(site: i64, addresses: *const usize, alive: *const u64) -> u8 {
    // A reload that converts values with identity stops every task at a
    // statement first.
    if super::live::barrier() {
        super::live::park();
    }
    if !ATTACHED.load(Ordering::Relaxed) {
        return super::live::moving(site);
    }
    let Some(table) = table() else { return 0 };
    let site = site as usize;
    // The frame of the function that has the probe.
    let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
    let task = super::tasks::current_id();
    // Where this function keeps its bindings, for the stacks it is in.
    if let Some((addressed, living)) = table.sites.get(site).and_then(|s| table.offsets.get(s.function))
        && addressed.load(Ordering::Relaxed) == UNKNOWN
    {
        addressed.store(addresses as isize - frame as isize, Ordering::Relaxed);
        living.store(alive as isize - frame as isize, Ordering::Relaxed);
    }
    let marked = table.marked.get(site).is_some_and(|m| m.load(Ordering::Relaxed));
    if !marked && !HOLD.load(Ordering::Relaxed) && state().mode == Mode::Run {
        return restarting(frame, task, site);
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
            // The first line of the call that restarted the frame.
            Mode::Restarted { task: t } => {
                let function = table.sites.get(site).map(|s| table.functions[s.function].name.as_str());
                t == task && function == Some(RESTARTED.lock().unwrap_or_else(|e| e.into_inner()).as_str())
            }
        };
        // A breakpoint whose condition holds counts the hit; it stops when
        // its count says so, or writes its message and goes on.
        let mut hit = None;
        for breakpoint in state.breakpoints.iter_mut() {
            let here = breakpoint.sites.contains(&site)
                && breakpoint
                    .condition
                    .as_ref()
                    .is_none_or(|c| holds(c, &table, site, addresses as usize, alive as usize));
            if !here {
                continue;
            }
            breakpoint.count += 1;
            if !breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                continue;
            }
            match &breakpoint.log {
                Some(message) => {
                    let function = table.sites.get(site).map(|s| s.function);
                    let frame =
                        Frame { place: String::new(), function, addresses: addresses as usize, alive: alive as usize };
                    let text = logged(&table, &frame, message);
                    tell(&format!(
                        "{{\"logged\":{},\"task\":{task},\"at\":{}}}",
                        quoted(&text),
                        quoted(&place_in(&table, site))
                    ));
                }
                None => {
                    hit.get_or_insert(breakpoint.id);
                }
            }
        }
        match (hit, stepped) {
            (Some(id), _) => Some(format!("breakpoint {id}")),
            (None, true) if matches!(state.mode, Mode::Restarted { .. }) => {
                RESTART_FRAME.store(0, Ordering::Relaxed);
                Some("restart".to_string())
            }
            (None, true) => Some("step".to_string()),
            (None, false) => return restarting(frame, task, site),
        }
    };
    // A patch may come while the task waits.
    drop(table);
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let stop =
        Stop { task, site, frame, bounds, addresses: addresses as usize, alive: alive as usize, why: String::new() };
    hold(state, stop, why);
    restarting(frame, task, site)
}

/// Holds the task until the program goes on: as the one that stopped,
/// `why`, or as one of those the stop holds.
fn hold(mut state: std::sync::MutexGuard<'static, State>, mut stop: Stop, why: Option<String>) {
    let task = stop.task;
    if let Some(why) = why {
        stop.why = why;
        tell(&stopped_event(&stop));
        state.stopped = Some(stop.clone());
        state.mode = Mode::Run;
        HOLD.store(true, Ordering::Relaxed);
    }
    state.held.push(stop);
    // Out of Tondo code while it waits, so collections and dumps go on.
    let paused = super::tasks::pause_here();
    let resumed = state.resumed;
    while state.resumed == resumed {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    state.held.retain(|held| held.task != task);
    drop(state);
    if paused {
        super::tasks::enter_tondo();
    }
}

/// Whether a panic stops the program while a debugger is attached
/// (`catch panic`, the default, or `catch none`).
static CATCH: AtomicBool = AtomicBool::new(true);

/// A task panics: with a debugger attached that catches panics, it stops
/// where the program's own code panicked, before it unwinds, the panic's
/// text as why, and the others at their next probe.
pub(crate) fn panicked(message: &str) {
    if !ATTACHED.load(Ordering::Relaxed) || !CATCH.load(Ordering::Relaxed) {
        return;
    }
    let Some(table) = table() else { return };
    let task = super::tasks::current_id();
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let mut found = None;
    for (frame, address) in super::trace::callers(super::trace::frame_pointer(), bounds) {
        let Some(described) = super::trace::describe(address) else { continue };
        let Some((file, line)) = described.at else { continue };
        let function = table.functions.iter().position(|f| f.name == described.function);
        let Some(function) = function.filter(|_| !file.starts_with("<std>/") && frame != 0) else { continue };
        let (addressed, living) = &table.offsets[function];
        let (addressed, living) = (addressed.load(Ordering::Relaxed), living.load(Ordering::Relaxed));
        // The probe of the statement that panicked, where the stop shows.
        let site = table.sites.iter().enumerate().filter(|(_, s)| s.function == function && s.line <= line);
        let Some((site, _)) = site.max_by_key(|(_, s)| s.line) else { continue };
        if addressed == UNKNOWN {
            continue;
        }
        let (addresses, alive) = ((frame as isize + addressed) as usize, (frame as isize + living) as usize);
        found = Some(Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
        break;
    }
    drop(table);
    if let Some(stop) = found {
        hold(state(), stop, Some(message.to_string()));
    }
}

/// The frame the debugger asked to restart, and its task; and the function
/// whose new call stops at its first line.
static RESTART_FRAME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RESTART_TASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RESTARTED: Mutex<String> = Mutex::new(String::new());

/// Whether the frame at a probe is the one to restart (1), or moves to new
/// code (2).
fn restarting(frame: usize, task: u64, site: usize) -> u8 {
    let asked = RESTART_FRAME.load(Ordering::Relaxed);
    match asked != 0 && asked == frame && RESTART_TASK.load(Ordering::Relaxed) == task {
        true => 1,
        false => super::live::moving(site as i64),
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
pub(crate) fn place(site: usize) -> String {
    let Some(table) = table() else { return String::new() };
    place_in(&table, site)
}

fn place_in(table: &Table, site: usize) -> String {
    let Some(at) = table.sites.get(site) else { return "the start of the program".into() };
    format!("{}:{} in {}", at.file, at.line, table.functions[at.function].name)
}

/// How the program goes on once the debugger has the answer that says it
/// runs: a program that ends at once would end before the answer went.
static AFTER_ANSWER: Mutex<Option<Mode>> = Mutex::new(None);

fn go_after_answer(mode: Mode) {
    *AFTER_ANSWER.lock().unwrap_or_else(|e| e.into_inner()) = Some(mode);
}

/// Called once an answer went to the debugger.
pub(crate) fn answered() {
    let mode = AFTER_ANSWER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mode) = mode {
        resume(mode);
    }
}

/// Goes on as `mode` says.
fn resume(mode: Mode) {
    if !matches!(mode, Mode::Restarted { .. }) {
        RESTART_FRAME.store(0, Ordering::Relaxed);
    }
    let mut state = state();
    state.mode = mode;
    state.stopped = None;
    state.selected = None;
    HOLD.store(false, Ordering::Relaxed);
    state.resumed += 1;
    RESUMED.notify_all();
}

// ---------------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------------

/// A frame of a held task: where it is, its function, and where its
/// bindings and the bits of those that hold a value are, or 0 when that is
/// not known: a function none of whose probes ran with a debugger
/// attached.
struct Frame {
    place: String,
    function: Option<usize>,
    addresses: usize,
    alive: usize,
}

/// The frames of a held task, innermost first, as `stack` lists them.
fn frames(table: &Table, held: &Stop) -> Vec<Frame> {
    let function = table.sites.get(held.site).map(|s| s.function);
    let place = place_in(table, held.site);
    let mut out = vec![Frame { place, function, addresses: held.addresses, alive: held.alive }];
    if held.site == usize::MAX {
        return out;
    }
    for (fp, address) in super::trace::callers(held.frame, held.bounds) {
        let Some(described) = super::trace::describe(address) else { continue };
        let Some((file, line)) = described.at else { continue };
        if described.function.starts_with("spawn") {
            continue;
        }
        let function = table.functions.iter().position(|f| f.name == described.function);
        let offsets = function
            .and_then(|f| table.offsets.get(f))
            .map(|(a, l)| (a.load(Ordering::Relaxed), l.load(Ordering::Relaxed)));
        let (addresses, alive) = match offsets {
            Some((a, l)) if a != UNKNOWN && fp != 0 => ((fp as isize + a) as usize, (fp as isize + l) as usize),
            _ => (0, 0),
        };
        out.push(Frame { place: format!("{file}:{line} in {}", described.function), function, addresses, alive });
    }
    out
}

/// The bindings of a frame that hold a value, with where each is.
fn bound<'t>(table: &'t Table, frame: &Frame) -> Vec<(&'t Binding, usize)> {
    let Some(function) = frame.function.filter(|_| frame.addresses != 0) else { return Vec::new() };
    let mut out = Vec::new();
    for (index, binding) in table.functions[function].bindings.iter().enumerate() {
        let word = unsafe { *((frame.alive as *const u64).add(index / 64)) };
        if word & (1 << (index % 64)) == 0 {
            continue;
        }
        out.push((binding, unsafe { *((frame.addresses as *const usize).add(index)) }));
    }
    out
}

/// What a path names in a frame: a binding, then its parts, as
/// `user.name` or `items[2]`: the binding's type when it is the binding,
/// the part's text, and its own parts with their texts and how many parts
/// each has.
struct Found {
    ty: Option<String>,
    text: String,
    parts: Vec<(String, String, usize)>,
}

fn resolve(table: &Table, frame: &Frame, path: &str) -> Result<Found, String> {
    let root = path.find(['.', '[']).map_or(path, |i| &path[..i]);
    let rest = &path[root.len()..];
    let bound = bound(table, frame);
    let Some((binding, address)) = bound.iter().find(|(b, _)| b.name == root) else {
        let names: Vec<&str> = bound.iter().map(|(b, _)| b.name.as_str()).collect();
        return Err(match names.is_empty() {
            true => format!("no binding `{root}` holds a value here"),
            false => format!("no binding `{root}` holds a value here; these do: {}", names.join(", ")),
        });
    };
    let whole = show(binding.show, *address, WHOLE);
    let value = super::shown::parse(&whole);
    let Some(part) = super::shown::find(&value, rest) else {
        return Err(format!("`{path}` is not a part of `{root}`, which is {}", clipped(&whole)));
    };
    let parts = part.parts.iter().map(|(name, p)| (name.clone(), clipped(p.text), p.parts.len())).collect();
    let ty = rest.is_empty().then(|| binding.ty.clone());
    Ok(Found { ty, text: clipped(part.text), parts })
}

/// How much of a value the debugger shows, and how much it reads to find
/// a part of it.
const SHOWN: usize = 4096;
const WHOLE: usize = 1 << 20;

/// A value as the debugger shows it, at most `limit` bytes.
fn show(glue: Show, address: usize, limit: usize) -> String {
    let mut builder = super::Builder::new(0);
    unsafe { glue(&mut builder, address as *const u8) };
    let text = builder.finish();
    let shown = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    unsafe { super::tondo_rt_str_release(text) };
    clip(shown, limit)
}

fn clipped(text: &str) -> String {
    clip(text.to_string(), SHOWN)
}

fn clip(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
    text
}

/// Whether a breakpoint's condition holds at a probe: numbers compare as
/// numbers, and anything else as the text the debugger shows, a string
/// without its quotes.
fn holds(condition: &Condition, table: &Table, site: usize, addresses: usize, alive: usize) -> bool {
    let function = table.sites.get(site).map(|s| s.function);
    let frame = Frame { place: String::new(), function, addresses, alive };
    let Ok(Found { text: value, .. }) = resolve(table, &frame, &condition.name) else { return false };
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
    let Some(table) = table() else {
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
    state().breakpoints.clear();
    if let Some(table) = table() {
        table.marked.iter().for_each(|m| m.store(false, Ordering::Relaxed));
    }
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
    let table = table()?;
    let table = &*table;
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
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        "restart" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let Some(at) = table.sites.get(stop.site) else {
                return Some("{\"error\":\"the program has not started: `continue` starts it\"}".into());
            };
            let function = table.functions[at.function].name.clone();
            if !super::live::on() {
                return Some("{\"error\":\"a frame restarts in a live build: debug the program with --live\"}".into());
            }
            if function.starts_with("closure in ") {
                return Some(
                    "{\"error\":\"a closure's frame does not restart: step out of it with `finish`, and restart its caller\"}"
                        .into(),
                );
            }
            RESTART_TASK.store(stop.task, Ordering::Relaxed);
            RESTART_FRAME.store(stop.frame, Ordering::Relaxed);
            *RESTARTED.lock().unwrap_or_else(|e| e.into_inner()) = function;
            go_after_answer(Mode::Restarted { task: stop.task });
            "{\"running\":true}".into()
        }
        "step" | "next" | "finish" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let mode = match word {
                "step" => Mode::Step { task: stop.task },
                "next" => Mode::Next { task: stop.task, frame: stop.frame },
                _ => Mode::Finish { task: stop.task, frame: stop.frame },
            };
            go_after_answer(mode);
            "{\"running\":true}".into()
        }
        "pause" => {
            HOLD.store(true, Ordering::Relaxed);
            "{\"pausing\":true}".into()
        }
        "set" => {
            let Some((name, value)) = rest.split_once(' ') else {
                return Some("{\"error\":\"set takes a binding and its new value, as `set count 3`\"}".into());
            };
            if state().selected.is_some_and(|(_, frame)| frame != 0) {
                let message = "a binding changes in the frame where its task stopped: `frame 0`";
                return Some(format!("{{\"error\":{}}}", quoted(message)));
            }
            let frame = match selected(table) {
                Ok(frame) => frame,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let bound = bound(table, &frame);
            let Some((binding, address)) = bound.iter().find(|(b, _)| b.name == name) else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("no binding `{name}` holds a value here"))));
            };
            if let Err(message) = unsafe { write(&binding.ty, *address, value.trim()) } {
                return Some(format!("{{\"error\":{}}}", quoted(&message)));
            }
            let shown = show(binding.show, *address, SHOWN);
            format!(
                "{{\"name\":{},\"type\":{},\"value\":{},\"children\":0}}",
                quoted(name),
                quoted(&binding.ty),
                quoted(&shown)
            )
        }
        "catch" => match rest {
            "panic" | "none" => {
                CATCH.store(rest == "panic", Ordering::Relaxed);
                format!("{{\"catch\":{}}}", quoted(rest))
            }
            _ => "{\"error\":\"catch takes `panic`, to stop where a task panics, or `none`\"}".into(),
        },
        "select" => {
            // A task and its frame, or a frame of the task looked at.
            let numbers: Vec<Option<u64>> = rest.split_whitespace().map(|w| w.parse().ok()).collect();
            let mut state = state();
            let current = state.selected.map(|(task, _)| task).or(state.stopped.as_ref().map(|s| s.task));
            let (task, frame) = match numbers[..] {
                [Some(task), Some(frame)] => (task, frame),
                [Some(frame)] if current.is_some() => (current.unwrap_or(0), frame),
                _ => {
                    return Some(
                        "{\"error\":\"select takes a frame, or a task and its frame, as `select 1 0`\"}".into(),
                    );
                }
            };
            let Some(held) = state.held.iter().find(|h| h.task == task).cloned() else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
            };
            let count = frames(table, &held).len() as u64;
            if frame >= count {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} has {count} frames"))));
            }
            state.selected = Some((task, frame as usize));
            format!("{{\"selected\":{{\"task\":{task},\"frame\":{frame}}}}}")
        }
        "locals" | "print" | "children" => {
            let frame = match selected(table) {
                Ok(frame) => frame,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            if word == "locals" {
                if frame.addresses == 0 && frame.function.is_some() {
                    let message =
                        "this frame's bindings are not known: none of its function's lines ran since the debugger came";
                    return Some(format!("{{\"error\":{}}}", quoted(message)));
                }
                let items: Vec<String> = bound(table, &frame)
                    .into_iter()
                    .map(|(binding, address)| {
                        let whole = show(binding.show, address, WHOLE);
                        let parts = super::shown::parse(&whole).parts.len();
                        format!(
                            "{{\"name\":{},\"type\":{},\"value\":{},\"children\":{parts}}}",
                            quoted(&binding.name),
                            quoted(&binding.ty),
                            quoted(&clipped(&whole))
                        )
                    })
                    .collect();
                return Some(format!("{{\"locals\":[{}]}}", items.join(",")));
            }
            let found = match resolve(table, &frame, rest) {
                Ok(found) => found,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            if word == "print" {
                let ty = found.ty.as_deref().map_or("null".to_string(), quoted);
                return Some(format!(
                    "{{\"name\":{},\"type\":{ty},\"value\":{},\"children\":{}}}",
                    quoted(rest),
                    quoted(&found.text),
                    found.parts.len()
                ));
            }
            let items: Vec<String> = found
                .parts
                .iter()
                .map(|(name, text, parts)| {
                    format!(
                        "{{\"name\":{},\"path\":{},\"value\":{},\"children\":{parts}}}",
                        quoted(name),
                        quoted(&format!("{rest}{name}")),
                        quoted(text)
                    )
                })
                .collect();
            format!("{{\"children\":[{}]}}", items.join(","))
        }
        // The contexts of the task looked at, and which `with` set each
        // field (language 11.16).
        "context" => {
            let state = state();
            let task = state.selected.map(|(task, _)| task).or(state.stopped.as_ref().map(|s| s.task));
            drop(state);
            let Some(task) = task.and_then(super::tasks::by_id) else { return Some(not_stopped()) };
            let contexts: Vec<String> = unsafe { super::contexts::described(task) }
                .into_iter()
                .map(|context| {
                    let fields: Vec<String> = context
                        .fields
                        .iter()
                        .map(|field| {
                            let (at, inherited) = match &field.set {
                                Some((at, inherited)) => (quoted(at), *inherited),
                                None => ("null".to_string(), false),
                            };
                            format!(
                                "{{\"name\":{},\"value\":{},\"children\":{},\"set\":{at},\"inherited\":{inherited}}}",
                                quoted(&field.name),
                                quoted(&clipped(&field.value)),
                                field.parts
                            )
                        })
                        .collect();
                    format!("{{\"name\":{},\"fields\":[{}]}}", quoted(&context.name), fields.join(","))
                })
                .collect();
            format!("{{\"contexts\":[{}]}}", contexts.join(","))
        }
        "stack" => {
            let state = state();
            let task = match rest.parse::<u64>() {
                Ok(task) => Some(task),
                Err(_) => state.stopped.as_ref().map(|s| s.task),
            };
            let Some(task) = task else { return Some(not_stopped()) };
            let held = state.held.iter().find(|h| h.task == task).cloned().or(state.stopped.clone());
            drop(state);
            let Some(held) = held.filter(|h| h.task == task) else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
            };
            let frames: Vec<String> = frames(table, &held).iter().map(|f| quoted(&f.place)).collect();
            format!("{{\"stack\":[{}]}}", frames.join(","))
        }
        "tasks" => {
            let state = state();
            let held: Vec<String> = state
                .held
                .iter()
                .map(|held| {
                    let why = match &state.stopped {
                        Some(stop) if stop.task == held.task => stop.why.clone(),
                        _ => "held".to_string(),
                    };
                    format!(
                        "{{\"task\":{},\"at\":{},\"why\":{}}}",
                        held.task,
                        quoted(&place_in(table, held.site)),
                        quoted(&why)
                    )
                })
                .collect();
            format!("{{\"tasks\":[{}]}}", held.join(","))
        }
        _ => return None,
    })
}

/// Changes a binding of a type the debugger writes, at `address`, to the
/// value `text` says: a number, `true` or `false`, a character, or a text,
/// in quotes or not.
unsafe fn write(ty: &str, address: usize, text: &str) -> Result<(), String> {
    let wrong = || format!("`{text}` is not a value of type {ty}");
    macro_rules! number {
        ($t:ty) => {{
            let value: $t = text.parse().map_err(|_| wrong())?;
            unsafe { *(address as *mut $t) = value };
        }};
    }
    match ty {
        "Int" => number!(i64),
        "Int32" => number!(i32),
        "Int16" => number!(i16),
        "Int8" => number!(i8),
        "UInt64" => number!(u64),
        "UInt32" => number!(u32),
        "UInt16" => number!(u16),
        "UInt8" | "Byte" => number!(u8),
        "Float" => number!(f64),
        "Float32" => number!(f32),
        "Bool" => {
            let value: bool = text.parse().map_err(|_| wrong())?;
            unsafe { *(address as *mut u8) = value as u8 };
        }
        "Char" => {
            let inner = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')).unwrap_or(text);
            let mut chars = inner.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return Err(wrong()) };
            unsafe { *(address as *mut u32) = c as u32 };
        }
        "String" => {
            let inner = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
            let value = match inner {
                Some(inner) => {
                    inner.replace("\\n", "\n").replace("\\t", "\t").replace("\\\"", "\"").replace("\\\\", "\\")
                }
                None => text.to_string(),
            };
            let old = unsafe { *(address as *const *mut Str) };
            unsafe { *(address as *mut *mut Str) = super::str_new(value.as_bytes()) };
            unsafe { super::tondo_rt_str_release(old) };
        }
        _ => {
            return Err(format!(
                "a binding of type {ty} does not change here: numbers, Bool, Char, Byte and String do"
            ));
        }
    }
    Ok(())
}

/// The frame `locals` and `print` look at: the one `select` chose, or the
/// stopped task's own.
fn selected(table: &Table) -> Result<Frame, String> {
    let state = state();
    let not = "the program runs: `pause` stops it, or a breakpoint".to_string();
    let (task, index) = state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0))).ok_or(not.clone())?;
    let held = state.held.iter().find(|h| h.task == task).cloned().or(state.stopped.clone()).ok_or(not)?;
    drop(state);
    frames(table, &held).into_iter().nth(index).ok_or_else(|| format!("task {task} has no frame {index}"))
}

/// `break <file>:<line> [if <path> <op> <value>] [hits <count>] [log <message>]`:
/// the probes of that line,
/// or of the next line that has one in the file.
fn breakpoint(table: &Table, text: &str) -> String {
    let error = |message: String| format!("{{\"error\":{}}}", quoted(&message));
    let (text, log) = match text.split_once(" log ") {
        Some((text, log)) => (text, Some(log.to_string())),
        None => (text, None),
    };
    let (text, hits) = match text.split_once(" hits ") {
        Some((text, hits)) => match Hits::parse(hits) {
            Ok(hits) => (text, Some(hits)),
            Err(message) => return error(message),
        },
        None => (text, None),
    };
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
    state.breakpoints.push(Breakpoint { id, sites, condition, count: 0, hits, log });
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
