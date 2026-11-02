//! The debugger of a debug build (toolchain 7.6). The compiler puts a probe
//! where each line starts, with the addresses of the function's bindings
//! and a bit for each that holds a value; while no debugger is attached a
//! probe only reads a flag. A tool attaches through the diagnostics
//! socket: it sets breakpoints, steps, and reads the bindings and stack of
//! the task that stopped. When one task stops every task stops at its next
//! probe, and they all go on together.

use super::{Builder, Str, str_bytes};
use expression::{Evaluator, Expr};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, RwLock};

mod expression;

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
    /// How `set` writes into it, among the table's layouts.
    layout: usize,
}

/// How `set` writes into a value (`codegen::write_layout`).
enum Layout {
    /// A number, a `Bool`, a `Char` or a `String`, by its type's name.
    Scalar(String),
    /// A record's or a tuple's fields: each one's name, offset, whether a
    /// pointer to it is there, and its layout.
    Fields(Vec<(String, usize, bool, usize)>),
    /// A plain enum: the size of its tag, and its variants.
    Plain(usize, Vec<String>),
    /// An array: the size of its elements, and their layout.
    Array(usize, usize),
    Other,
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

impl Site {
    /// Whether the site is in code the toolchain wrote, the standard
    /// library's or derived, as `<std>/json.to`.
    fn generated(&self) -> bool {
        self.file.starts_with('<')
    }
}

struct Table {
    functions: Vec<Function>,
    sites: Vec<Site>,
    layouts: Vec<Layout>,
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
/// The site of a task that waits, switched out at no probe.
const WAITING: usize = usize::MAX - 1;

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
/// A frame watches an expression: every probe looks.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// Steps stop in the standard library's code: always (`steps all`), or
/// for a step that starts there.
static STEPS_ALL: AtomicBool = AtomicBool::new(false);
static STEPS_STD: AtomicBool = AtomicBool::new(false);
/// The task that steps, or 0; and the function that returned last in it,
/// with what it returned, which the step's stop shows.
static STEPPING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RETURNED: Mutex<Option<(u64, String, String)>> = Mutex::new(None);

/// In a debug build, a function returns a value: while its task steps,
/// the debugger keeps what it returned, from the program's own code or,
/// for steps that go there, the standard library's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_returned(site: i64, glue: Show, value: *const u8) {
    let stepping = STEPPING.load(Ordering::Relaxed);
    if stepping == 0 || stepping != super::tasks::current_id() {
        return;
    }
    let Some(table) = table() else { return };
    let Some(at) = table.sites.get(site as usize) else { return };
    if at.generated() && !STEPS_STD.load(Ordering::Relaxed) {
        return;
    }
    let function = table.functions[at.function].name.clone();
    let shown = clipped(&show(glue, value as usize));
    *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = Some((stepping, function, shown));
}

/// Called by a debug build's process before anything else, or by a
/// library before its first call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_table(raw: *const Raw) {
    let (functions, sites, layouts) = unsafe { parse(raw, 0) };
    let marked = sites.iter().map(|_| AtomicBool::new(false)).collect();
    let offsets = functions.iter().map(|_| (AtomicIsize::new(UNKNOWN), AtomicIsize::new(UNKNOWN))).collect();
    let _ = TABLE.set(RwLock::new(Table { functions, sites, layouts, marked, offsets }));
    super::heap::track_objects();
}

/// Adds a patch's table: its functions after the program's, its sites
/// after those it numbers from; the breakpoints of its lines mark it too.
/// Gives how many sites it added.
pub(crate) fn add_table(raw: *const Raw) -> usize {
    let Some(lock) = TABLE.get() else { return 0 };
    let mut table = lock.write().unwrap_or_else(|e| e.into_inner());
    let (functions, sites, layouts) = unsafe { parse(raw, table.layouts.len()) };
    table.layouts.extend(layouts);
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

/// A table's functions, sites and layouts, the layouts numbered from
/// `base`, after those of the tables before it.
unsafe fn parse(raw: *const Raw, base: usize) -> (Vec<Function>, Vec<Site>, Vec<Layout>) {
    let raw = unsafe { &*raw };
    let text = String::from_utf8_lossy(unsafe { str_bytes(raw.text) }).into_owned();
    let glues = unsafe { std::slice::from_raw_parts(raw.glues, raw.count as usize) };
    let (mut functions, mut sites, mut layouts, mut next) = (Vec::new(), Vec::new(), Vec::new(), 0);
    let number = |text: &str| text.parse::<usize>().unwrap_or(0);
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            ["F", name] => functions.push(Function { name: name.to_string(), bindings: Vec::new() }),
            ["L", name, ty, layout] => {
                let show = glues[next];
                next += 1;
                if let Some(function) = functions.last_mut() {
                    let (name, ty, layout) = (name.to_string(), ty.to_string(), base + number(layout));
                    function.bindings.push(Binding { name, ty, show, layout });
                }
            }
            ["D", layout] => {
                let words: Vec<&str> = layout.split(' ').collect();
                layouts.push(match words.as_slice() {
                    ["s", ty] => Layout::Scalar(ty.to_string()),
                    ["r", fields @ ..] => Layout::Fields(
                        fields
                            .iter()
                            .filter_map(|field| {
                                let (name, at) = field.split_once('=')?;
                                let (offset, layout) = at.split_once(':')?;
                                let boxed = layout.starts_with('*');
                                let layout = base + number(layout.trim_start_matches('*'));
                                Some((name.to_string(), number(offset), boxed, layout))
                            })
                            .collect(),
                    ),
                    ["e", size, variants @ ..] => {
                        Layout::Plain(number(size), variants.iter().map(|v| v.to_string()).collect())
                    }
                    ["a", size, layout] => Layout::Array(number(size), base + number(layout)),
                    _ => Layout::Other,
                });
            }
            ["S", file, line, function] => sites.push(Site {
                file: file.to_string(),
                line: line.parse().unwrap_or(0),
                function: function.parse().unwrap_or(0),
            }),
            _ => {}
        }
    }
    (functions, sites, layouts)
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
    watches: Vec<Watch>,
    next_breakpoint: u32,
}

/// An expression a task's frame watches: the task stops on the line after
/// the one that changed its value, in that frame or a call it makes, until
/// the frame returns.
struct Watch {
    id: u32,
    task: u64,
    expr: Expr,
    text: String,
    /// The frame, and those that called it, which live while it does.
    frames: Vec<Frame>,
    /// The value as it was, and the site of the task's last probe.
    value: String,
    last: usize,
}

struct Breakpoint {
    id: u32,
    sites: Vec<usize>,
    condition: Option<Expr>,
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

/// A logpoint's message: each `{expression}` in it replaced by its value
/// there, a text without its quotes.
fn logged(table: &Table, frames: &[Frame], message: &str) -> String {
    let mut out = String::new();
    for (text, expr) in pieces(message) {
        match expr.map(|expr| evaluating(table, frames, 0, |e| e.evaluate(&expr?).map(|v| v.value.written()))) {
            None => out.push_str(text),
            Some(Ok(value)) => out.push_str(&value),
            Some(Err(_)) => out.push_str(&format!("{{{text}?}}")),
        }
    }
    out
}

/// A logpoint's message in pieces: a text, or an expression in braces with
/// its text.
fn pieces(message: &str) -> Vec<(&str, Option<Result<Expr, String>>)> {
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        out.push((&rest[..open], None));
        let text = rest[open + 1..open + close].trim();
        out.push((text, Some(expression::parse(text))));
        rest = &rest[open + close + 1..];
    }
    out.push((rest, None));
    out
}

static STATE: Mutex<State> = Mutex::new(State {
    mode: Mode::Run,
    stopped: None,
    resumed: 0,
    held: Vec::new(),
    selected: None,
    breakpoints: Vec::new(),
    watches: Vec::new(),
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
    let watching = WATCHING.load(Ordering::Relaxed);
    if !marked && !watching && !HOLD.load(Ordering::Relaxed) && state().mode == Mode::Run {
        return restarting(frame, task, site);
    }
    let mut state = state();
    let why = if state.stopped.is_some() || HOLD.load(Ordering::Relaxed) {
        // Another task stopped, or the debugger asked all to.
        if state.stopped.is_some() { None } else { Some("pause".to_string()) }
    } else {
        // A step goes over code the program's files do not have, the
        // standard library's and the derived, unless it started there.
        let standard = || table.sites.get(site).is_some_and(Site::generated);
        let stepped = match state.mode {
            Mode::Step { .. } | Mode::Next { .. } | Mode::Finish { .. }
                if !STEPS_STD.load(Ordering::Relaxed) && standard() =>
            {
                false
            }
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
        // its count says so, or writes its message and goes on. Both read
        // what the code here can use, as `locals` shows it.
        let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
        let here = Stop {
            task,
            site,
            frame,
            bounds,
            addresses: addresses as usize,
            alive: alive as usize,
            why: String::new(),
        };
        let stack = std::cell::OnceCell::new();
        let stack = || stack.get_or_init(|| frames(&table, &here));
        let mut hit = None;
        for breakpoint in state.breakpoints.iter_mut() {
            let here = breakpoint.sites.contains(&site)
                && breakpoint.condition.as_ref().is_none_or(|c| evaluating(&table, stack(), 0, |e| e.holds(c)));
            if !here {
                continue;
            }
            breakpoint.count += 1;
            if !breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                continue;
            }
            match &breakpoint.log {
                Some(message) => {
                    let text = logged(&table, stack(), message);
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
        let changed = if watching { watched(&table, &mut state, task, frame, site) } else { None };
        match (hit, stepped) {
            (Some(id), _) => Some(format!("breakpoint {id}")),
            _ if changed.is_some() => changed,
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

/// At a probe of a task, its watches: one whose frame returned ends, and
/// the first whose value changed gives why the task stops: its expression,
/// its values, and the line that changed it.
fn watched(table: &Table, state: &mut State, task: u64, frame: usize, site: usize) -> Option<String> {
    let mut changed = None;
    state.watches.retain_mut(|watch| {
        if watch.task != task {
            return true;
        }
        // The frame returned when the task runs in an outer one, or another
        // call took its place.
        let watched = &watch.frames[0];
        let function = table.sites.get(site).map(|s| s.function);
        if frame > watched.pointer || (frame == watched.pointer && function != watched.function) {
            tell(&format!("{{\"unwatched\":{},\"why\":\"its frame returned\"}}", watch.id));
            return false;
        }
        let now = evaluating(table, &watch.frames, 0, |e| e.evaluate(&watch.expr).map(|v| v.text));
        if let Some(now) = now.ok().filter(|now| *now != watch.value) {
            let (before, after) = (clipped(&watch.value), clipped(&now));
            let at = place_in(table, watch.last);
            let why = format!("watch {}: {} changed from {before} to {after} at {at}", watch.id, watch.text);
            changed.get_or_insert(why);
            watch.value = now;
        }
        watch.last = site;
        true
    });
    WATCHING.store(!state.watches.is_empty(), Ordering::Relaxed);
    changed
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
/// (`catch panic`, the default, or `catch none`), and whether an error does
/// (`catch error`).
static CATCH: AtomicBool = AtomicBool::new(true);
static CATCH_ERRORS: AtomicBool = AtomicBool::new(false);

/// In a debug build, an error leaves a function where it starts: a `fail`
/// of the program's code, or a `?` that passes one on from the standard
/// library. With a debugger that catches errors, the task stops there, the
/// error as why, and the others at their next probe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_failed(
    site: i64,
    addresses: *const usize,
    alive: *const u64,
    glue: Show,
    error: *const u8,
) {
    if !ATTACHED.load(Ordering::Relaxed) || !CATCH_ERRORS.load(Ordering::Relaxed) {
        return;
    }
    let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
    let task = super::tasks::current_id();
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let (addresses, alive) = (addresses as usize, alive as usize);
    let stop = Stop { task, site: site as usize, frame, bounds, addresses, alive, why: String::new() };
    let why = format!("error: {}", clipped(&show(glue, error as usize)));
    hold(state(), stop, Some(why));
}

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
        "{{\"stopped\":{{\"why\":{},\"task\":{},\"at\":{}{}}}}}",
        quoted(&stop.why),
        stop.task,
        quoted(&place(stop.site)),
        returned(stop)
    )
}

/// What a function returned in the step that stopped, as an answer's
/// field.
fn returned(stop: &Stop) -> String {
    match RETURNED.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some((task, function, value)) if *task == stop.task && stop.why == "step" => {
            format!(",\"returned\":{{\"function\":{},\"value\":{}}}", quoted(function), quoted(value))
        }
        _ => String::new(),
    }
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
    let stepping = match mode {
        Mode::Step { task } | Mode::Next { task, .. } | Mode::Finish { task, .. } => task,
        Mode::Run | Mode::Restarted { .. } => 0,
    };
    STEPPING.store(stepping, Ordering::Relaxed);
    *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
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

/// A frame of a held task: where it is, its function, its frame pointer,
/// and where its bindings and the bits of those that hold a value are, or
/// 0 when that is not known: a function none of whose probes ran with a
/// debugger attached.
#[derive(Clone)]
struct Frame {
    place: String,
    function: Option<usize>,
    pointer: usize,
    addresses: usize,
    alive: usize,
}

/// The frames of a held task, innermost first, as `stack` lists them; of
/// one that waits, those of the program's functions its stack has.
fn frames(table: &Table, held: &Stop) -> Vec<Frame> {
    let function = table.sites.get(held.site).map(|s| s.function);
    let place = place_in(table, held.site);
    let first = Frame { place, function, pointer: held.frame, addresses: held.addresses, alive: held.alive };
    let mut out = if held.site == WAITING { Vec::new() } else { vec![first] };
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
        let place = format!("{file}:{line} in {}", described.function);
        out.push(Frame { place, function, pointer: fp, addresses, alive });
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
/// and the whole text of what it names.
struct Found {
    ty: Option<String>,
    text: String,
}

fn resolve(table: &Table, frames: &[Frame], index: usize, path: &str) -> Result<Found, String> {
    let root = path.find(['.', '[']).map_or(path, |i| &path[..i]);
    let rest = &path[root.len()..];
    let bound = visible(table, frames, index);
    let Some((binding, address, _)) = bound.iter().find(|(b, ..)| b.name == root) else {
        return Err(missing(root, &bound));
    };
    let whole = show(binding.show, *address);
    if rest.is_empty() {
        return Ok(Found { ty: Some(binding.ty.clone()), text: whole });
    }
    match super::shown::find(&whole, rest) {
        Some(part) => Ok(Found { ty: None, text: part.to_string() }),
        None => Err(format!("`{path}` is not a part of `{root}`, which is {}", clipped(&whole))),
    }
}

fn missing(name: &str, bound: &[(&Binding, usize, Option<&str>)]) -> String {
    let names: Vec<&str> = bound.iter().map(|(b, ..)| b.name.as_str()).collect();
    match names.is_empty() {
        true => format!("no binding `{name}` holds a value here"),
        false => format!("no binding `{name}` holds a value here; these do: {}", names.join(", ")),
    }
}

/// Runs `f` with an evaluator of expressions that reads the bindings the
/// code of a frame can use.
fn evaluating<T>(table: &Table, frames: &[Frame], index: usize, f: impl FnOnce(&mut Evaluator) -> T) -> T {
    let bound = visible(table, frames, index);
    let read = |name: &str| match bound.iter().find(|(b, ..)| b.name == name) {
        Some((binding, address, _)) => Ok(show(binding.show, *address)),
        None => Err(missing(name, &bound)),
    };
    f(&mut Evaluator::new(&read))
}

/// How much of a value's text the debugger shows; it reads all of it to
/// find a part.
const SHOWN: usize = 4096;

/// A value as the debugger shows it.
fn show(glue: Show, address: usize) -> String {
    let mut builder = super::Builder::new(0);
    unsafe { glue(&mut builder, address as *const u8) };
    let text = builder.finish();
    let shown = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    unsafe { super::tondo_rt_str_release(text) };
    shown
}

fn clipped(text: &str) -> String {
    let mut text = text.to_string();
    if text.len() > SHOWN {
        let mut end = SHOWN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
    text
}

/// What an answer says of a value: its text, clipped, how many parts it
/// has, and whether they go by position, which a tool shows in pages.
fn described(text: &str) -> String {
    let value = super::shown::parse(text);
    let indexed = if value.indexed && !value.parts.is_empty() { ",\"indexed\":true" } else { "" };
    format!("\"value\":{},\"children\":{}{indexed}", quoted(&clipped(text)), value.parts.len())
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
    let mut state = state();
    state.breakpoints.clear();
    state.watches.clear();
    WATCHING.store(false, Ordering::Relaxed);
    drop(state);
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
            state.watches.retain(|w| w.id != id);
            WATCHING.store(!state.watches.is_empty(), Ordering::Relaxed);
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
            let in_std = table.sites.get(stop.site).is_some_and(Site::generated);
            STEPS_STD.store(STEPS_ALL.load(Ordering::Relaxed) || in_std, Ordering::Relaxed);
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
        "set" => match set(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // Whether steps go into the standard library's code.
        "steps" => match rest {
            "all" | "program" => {
                STEPS_ALL.store(rest == "all", Ordering::Relaxed);
                format!("{{\"steps\":{}}}", quoted(rest))
            }
            _ => "{\"error\":\"steps takes `program`, to step over the standard library's code, or `all`\"}".into(),
        },
        // An expression the frame looked at watches, until it returns.
        "watch" => {
            let (frames, index) = match selected(table) {
                Ok(selected) => selected,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let expr = match expression::parse(rest) {
                Ok(expr) => expr,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let value = match evaluating(table, &frames, index, |e| e.evaluate(&expr)) {
                Ok(evaluated) => evaluated.text,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let mut state = state();
            let Some((task, _)) = state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0))) else {
                return Some(not_stopped());
            };
            let last = state.held.iter().find(|h| h.task == task).map_or(usize::MAX, |h| h.site);
            let id = state.next_breakpoint;
            state.next_breakpoint += 1;
            let frames = frames[index..].to_vec();
            let answer = format!("{{\"watch\":{{\"id\":{id},{}}}}}", described(&value));
            state.watches.push(Watch { id, task, expr, text: rest.to_string(), frames, value, last });
            WATCHING.store(true, Ordering::Relaxed);
            answer
        }
        // What stops the program: panics, errors, both or none.
        "catch" => {
            let kinds: Vec<&str> = rest.split_whitespace().collect();
            let known = |kinds: &[&str]| kinds.iter().all(|k| matches!(*k, "panic" | "error"));
            if !(known(&kinds) && !kinds.is_empty() || kinds == ["none"]) {
                let message =
                    "catch takes `panic`, to stop where a task panics, `error`, where an error starts, both, or `none`";
                return Some(format!("{{\"error\":{}}}", quoted(message)));
            }
            CATCH.store(kinds.contains(&"panic"), Ordering::Relaxed);
            CATCH_ERRORS.store(kinds.contains(&"error"), Ordering::Relaxed);
            let kinds: Vec<String> = kinds.iter().filter(|k| **k != "none").map(|k| quoted(k)).collect();
            format!("{{\"catch\":[{}]}}", kinds.join(","))
        }
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
            let Some(held) = stop_of(&state, task) else {
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
            let (frames, index) = match selected(table) {
                Ok(selected) => selected,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            if word == "locals" {
                let frame = &frames[index];
                if frame.addresses == 0 && frame.function.is_some() {
                    let message =
                        "this frame's bindings are not known: none of its function's lines ran since the debugger came";
                    return Some(format!("{{\"error\":{}}}", quoted(message)));
                }
                let items: Vec<String> = visible(table, &frames, index)
                    .into_iter()
                    .map(|(binding, address, from)| {
                        let whole = show(binding.show, address);
                        let from = from.map_or(String::new(), |from| format!(",\"from\":{}", quoted(from)));
                        format!(
                            "{{\"name\":{},\"type\":{},{}{from}}}",
                            quoted(&binding.name),
                            quoted(&binding.ty),
                            described(&whole)
                        )
                    })
                    .collect();
                // In the frame a step stopped in, what a function returned.
                let state = state();
                let looked = state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0)));
                let returned = match &state.stopped {
                    Some(stop) if looked == Some((stop.task, 0)) => returned(stop),
                    _ => String::new(),
                };
                return Some(format!("{{\"locals\":[{}]{returned}}}", items.join(",")));
            }
            if word == "print" {
                return Some(printed(table, &frames, index, rest));
            }
            // The parts of a value by its path, all or a page of them:
            // `children <path> [<start> <count>]`.
            let (path, page) = match rest.rsplitn(3, ' ').collect::<Vec<_>>()[..] {
                [count, start, path] => match (start.parse::<usize>(), count.parse::<usize>()) {
                    (Ok(start), Ok(count)) => (path.trim(), Some((start, count))),
                    _ => (rest, None),
                },
                _ => (rest, None),
            };
            let found = match resolve(table, &frames, index, path) {
                Ok(found) => found,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let parts = super::shown::parse(&found.text).parts;
            let total = parts.len();
            let (start, count) = page.unwrap_or((0, total));
            let items: Vec<String> = parts
                .iter()
                .skip(start)
                .take(count)
                .map(|(name, part)| {
                    format!(
                        "{{\"name\":{},\"path\":{},{}}}",
                        quoted(name),
                        quoted(&format!("{path}{name}")),
                        described(part.text)
                    )
                })
                .collect();
            format!("{{\"children\":[{}],\"total\":{total}}}", items.join(","))
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
            let held = stop_of(&state, task);
            drop(state);
            let Some(held) = held else {
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
            // While the program stands still, the tasks that wait, or are
            // ready to run again, too: where in the program each is.
            let mut waiting = Vec::new();
            if state.stopped.is_some() {
                for (task, ..) in super::tasks::waiting() {
                    let Some(stop) = stop_of(&state, task).filter(|s| s.site == WAITING) else { continue };
                    let at = frames(table, &stop).first().map_or(String::new(), |f| f.place.clone());
                    let (at, why) = (quoted(&at), quoted(&stop.why));
                    waiting.push(format!("{{\"task\":{task},\"at\":{at},\"why\":{why}}}"));
                }
            }
            format!("{{\"tasks\":[{}]}}", held.into_iter().chain(waiting).collect::<Vec<_>>().join(","))
        }
        _ => return None,
    })
}

/// `set <target> [=] <value>`: changes a binding of the frame where the
/// task stopped, or a part of one, `user.age` or `items[i]`, to what the
/// expression gives: a number, a `Bool`, a `Char`, a `String` or a plain
/// enum's variant, as `Role.Admin`. An array's item changes when no other
/// value shares the array. Gives what the target is now.
fn set(table: &Table, text: &str) -> Result<String, String> {
    let usage =
        || "set takes what changes and its new value, as `set count count + 1` or `set user.age = 37`".to_string();
    // The target ends at a space outside brackets and quotes.
    let (mut depth, mut quoted) = (0i32, false);
    let space = text.char_indices().find_map(|(at, c)| {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' if !quoted => depth += 1,
            ']' | ')' if !quoted => depth -= 1,
            ' ' if !quoted && depth == 0 => return Some(at),
            _ => {}
        }
        None
    });
    let (target, value) = text.split_once(" = ").or_else(|| space.map(|at| text.split_at(at))).ok_or_else(usage)?;
    let (target, value) = (target.trim(), value.trim());
    if state().selected.is_some_and(|(_, frame)| frame != 0) {
        return Err("a binding changes in the frame where its task stopped: `frame 0`".into());
    }
    let (frames, index) = selected(table)?;
    let mut steps = Vec::new();
    let mut place = &expression::parse(target)?;
    let root = loop {
        match place {
            Expr::Name(name) => break name.clone(),
            Expr::Field(inner, field) => {
                steps.push(Ok(field.clone()));
                place = inner;
            }
            Expr::Index(inner, index) => {
                steps.push(Err(&**index));
                place = inner;
            }
            _ => return Err(format!("`{target}` is not a binding or a part of one")),
        }
    };
    let bound = bound(table, &frames[index]);
    let Some(&(binding, mut address)) = bound.iter().find(|(b, _)| b.name == root) else {
        return Err(format!("no binding `{root}` holds a value here"));
    };
    let mut layout = binding.layout;
    for step in steps.into_iter().rev() {
        match (step, &table.layouts[layout]) {
            (Ok(field), Layout::Fields(fields)) => {
                let Some(&(_, offset, boxed, inner)) = fields.iter().find(|(name, ..)| *name == field) else {
                    return Err(format!("`{target}` has no field `{field}`"));
                };
                address += offset;
                if boxed {
                    address = unsafe { *(address as *const usize) };
                }
                layout = inner;
            }
            (Err(at), Layout::Array(size, inner)) => {
                let at = match evaluating(table, &frames, index, |e| e.evaluate(at))?.value {
                    expression::Value::Int(at) => at,
                    other => return Err(format!("an index is an Int, not {}", other.shown())),
                };
                let slot = address as *mut *mut super::ArrayHeader;
                let length = if unsafe { *slot }.is_null() { 0 } else { unsafe { (**slot).len } };
                let Some(at) = usize::try_from(at).ok().filter(|&at| at < length) else {
                    return Err(format!("{at} is not an index of `{target}`, which has {length} items"));
                };
                // An array another value shares is copied first, as the
                // program does, when its items hold nothing counted.
                if !unsafe { super::unique(*slot) } {
                    let plain = match &table.layouts[*inner] {
                        Layout::Scalar(ty) => ty != "String",
                        Layout::Plain(..) => true,
                        _ => false,
                    };
                    if !plain {
                        let message = "another value shares this array, which a change here would change too";
                        return Err(format!("{message}: change the binding that holds it"));
                    }
                    unsafe { super::tondo_rt_array_unique(slot, *size, 0, None, None) };
                }
                address = unsafe { super::element(*slot, *size, at) } as usize;
                layout = *inner;
            }
            _ => {
                return Err(format!(
                    "the debugger does not change `{target}`: it changes numbers, Bool, Char, String and plain enums, in records, tuples and arrays"
                ));
            }
        }
    }
    let evaluated = evaluating(table, &frames, index, |e| e.evaluate(&expression::parse(value)?));
    match (&table.layouts[layout], evaluated) {
        (Layout::Scalar(ty), Ok(evaluated)) => unsafe { write(ty, address, &evaluated.value)? },
        // A text without its quotes, as it was written before.
        (Layout::Scalar(ty), Err(_)) if ty == "String" => unsafe {
            write(ty, address, &expression::Value::Text(value.to_string()))?
        },
        (Layout::Plain(size, variants), Ok(evaluated)) => {
            let shown = evaluated.value.shown();
            let name = shown.rsplit('.').next().unwrap_or_default();
            let Some(tag) = variants.iter().position(|v| v == name) else {
                return Err(format!("`{value}` is none of its variants: {}", variants.join(", ")));
            };
            unsafe { std::ptr::copy_nonoverlapping(&(tag as u64).to_le_bytes()[0], address as *mut u8, *size) };
        }
        (_, Err(message)) => return Err(message),
        _ => return Err(format!("the debugger does not change `{target}`, a value of type {}", binding.ty)),
    }
    Ok(printed(table, &frames, index, target))
}

/// Writes a number, a `Bool`, a `Char` or a `String` of the type `ty` at
/// `address`.
unsafe fn write(ty: &str, address: usize, value: &expression::Value) -> Result<(), String> {
    use expression::Value;
    let wrong = || format!("{} is not a value of type {ty}", value.shown());
    macro_rules! int {
        ($t:ty) => {{
            let Value::Int(n) = value else { return Err(wrong()) };
            let n = <$t>::try_from(*n).map_err(|_| wrong())?;
            unsafe { (address as *mut $t).write_unaligned(n) };
        }};
    }
    macro_rules! float {
        ($t:ty) => {{
            let x = match value {
                Value::Float(x) => *x,
                Value::Int(n) => *n as f64,
                _ => return Err(wrong()),
            };
            unsafe { (address as *mut $t).write_unaligned(x as $t) };
        }};
    }
    match (ty, value) {
        ("Int", _) => int!(i64),
        ("Int32", _) => int!(i32),
        ("Int16", _) => int!(i16),
        ("Int8", _) => int!(i8),
        ("Int128", _) => int!(i128),
        ("UInt", _) => int!(u64),
        ("UInt128", _) => int!(u128),
        ("UInt32", _) => int!(u32),
        ("UInt16", _) => int!(u16),
        ("UInt8" | "Byte", _) => int!(u8),
        ("Float", _) => float!(f64),
        ("Float32", _) => float!(f32),
        ("Bool", Value::Bool(b)) => unsafe { *(address as *mut u8) = *b as u8 },
        ("Char", Value::Char(c)) => unsafe { *(address as *mut u32) = *c as u32 },
        ("String", Value::Text(text)) => {
            let old = unsafe { *(address as *const *mut Str) };
            unsafe { *(address as *mut *mut Str) = super::str_new(text.as_bytes()) };
            unsafe { super::tondo_rt_str_release(old) };
        }
        _ => return Err(wrong()),
    }
    Ok(())
}

/// Where a task the debugger looks at is: held at a probe, or, while the
/// program stands still, waiting.
fn stop_of(state: &State, task: u64) -> Option<Stop> {
    if let Some(held) = state.held.iter().chain(&state.stopped).find(|h| h.task == task) {
        return Some(held.clone());
    }
    state.stopped.as_ref()?;
    let (_, why, frame, bounds) = super::tasks::waiting().into_iter().find(|(id, ..)| *id == task)?;
    Some(Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: why.into() })
}

/// The frame `locals` and `print` look at: the one `select` chose, or the
/// stopped task's own.
fn selected(table: &Table) -> Result<(Vec<Frame>, usize), String> {
    let state = state();
    let not = "the program runs: `pause` stops it, or a breakpoint".to_string();
    let (task, index) = state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0))).ok_or(not.clone())?;
    let held = stop_of(&state, task).ok_or(not)?;
    drop(state);
    let frames = frames(table, &held);
    match index < frames.len() {
        true => Ok((frames, index)),
        false => Err(format!("task {task} has no frame {index}")),
    }
}

/// The function that the function `name` is written in, by how the
/// compiler names it: a closure's or a cleanup's, and for a test or a
/// suite, any suite, those around it being the ones on its stack.
fn written_in(name: &str) -> Option<(&str, bool)> {
    if name.starts_with("test ") || name.starts_with("suite ") {
        return Some(("suite ", true));
    }
    name.strip_prefix("closure in ").or(name.strip_prefix("defer in ")).map(|outer| (outer, false))
}

/// The bindings the code of a frame can use, with where each is: its own,
/// then those of the functions it is written in whose frames are further
/// down its stack, as a closure's maker or a test's suites, each with the
/// function it is from.
fn visible<'t>(table: &'t Table, frames: &[Frame], index: usize) -> Vec<(&'t Binding, usize, Option<&'t str>)> {
    let name = |frame: &Frame| frame.function.map(|f| table.functions[f].name.as_str());
    let mut out: Vec<(&Binding, usize, Option<&str>)> =
        bound(table, &frames[index]).into_iter().map(|(b, a)| (b, a, None)).collect();
    let mut at = index;
    while let Some((outer, prefix)) = name(&frames[at]).and_then(written_in) {
        let found = (at + 1..frames.len()).find(|&below| {
            name(&frames[below]).is_some_and(|n| if prefix { n.starts_with(outer) } else { n == outer })
        });
        let Some(below) = found else { break };
        let from = name(&frames[below]);
        for (binding, address) in bound(table, &frames[below]) {
            if !out.iter().any(|(b, ..)| b.name == binding.name) {
                out.push((binding, address, from));
            }
        }
        at = below;
    }
    out
}

/// `break <file>:<line> [if <condition>] [hits <count>] [log <message>]`:
/// the probes of that line, or of the next line that has one in the file;
/// `break fn <name> ...`, those where each function of that name starts.
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
    let sites = match at.strip_prefix("fn ") {
        Some(name) => function_sites(table, name.trim()),
        None => line_sites(table, at),
    };
    let sites = match sites {
        Ok(sites) => sites,
        Err(message) => return error(message),
    };
    let condition = match condition.map(expression::parse) {
        Some(Err(message)) => return error(format!("the condition: {message}")),
        Some(Ok(condition)) => Some(condition),
        None => None,
    };
    // What the condition and the message read are bindings there.
    let mut exprs: Vec<&Expr> = condition.iter().collect();
    let pieces = log.as_deref().map(pieces).unwrap_or_default();
    for (_, expr) in &pieces {
        match expr {
            Some(Ok(expr)) => exprs.push(expr),
            Some(Err(message)) => return error(format!("the message: {message}")),
            None => {}
        }
    }
    let functions: std::collections::BTreeSet<usize> = sites.iter().map(|&s| table.sites[s].function).collect();
    let usable: Vec<&str> = functions.into_iter().flat_map(|f| usable(table, f)).collect();
    let mut read = Vec::new();
    exprs.iter().for_each(|expr| expression::names(expr, &mut read));
    if let Some(name) = read.iter().find(|n| expression::binding_name(n) && !usable.contains(&n.as_str())) {
        let place = place_in(table, sites[0]);
        return error(format!("`{name}` is not a binding at {place}; these are: {}", usable.join(", ")));
    }
    let mut state = state();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let at = &table.sites[sites[0]];
    let answer = format!("{{\"breakpoint\":{{\"id\":{id},\"file\":{},\"line\":{}}}}}", quoted(&at.file), at.line);
    state.breakpoints.push(Breakpoint { id, sites, condition, count: 0, hits, log });
    remark(table, &state);
    answer
}

/// The probes of a line, `file:line`, or of the next line with code.
fn line_sites(table: &Table, at: &str) -> Result<Vec<usize>, String> {
    let Some((file, line)) = at.rsplit_once(':') else {
        return Err(format!("a breakpoint is `file:line`, as `main.to:12`, or `fn name`, not `{at}`"));
    };
    let Ok(line) = line.parse::<u32>() else { return Err(format!("`{line}` is not a line number")) };
    // A file as the compiler named it, or a path that ends with it, as an
    // editor gives.
    let matches = |site: &Site| {
        site.file == file || site.file.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{}", site.file))
    };
    let Some(found) = table.sites.iter().filter(|s| matches(s) && s.line >= line).map(|s| s.line).min() else {
        let files: std::collections::BTreeSet<&str> = table.sites.iter().map(|s| s.file.as_str()).collect();
        return Err(format!(
            "no code at {file}:{line} or after it; the program's files are {}",
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    };
    Ok(table.sites.iter().enumerate().filter(|(_, s)| matches(s) && s.line == found).map(|(i, _)| i).collect())
}

/// The probes where the functions called `name` start: by their whole
/// name, as `Stack.push`, or by their own, `push`; a generic one in each
/// of its types.
fn function_sites(table: &Table, name: &str) -> Result<Vec<usize>, String> {
    let plain = |full: &str| {
        let mut out = String::new();
        let mut depth = 0;
        for c in full.chars() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    };
    let named = |function: &Function| {
        let plain = plain(&function.name);
        function.name == name || plain == name || plain.ends_with(&format!(".{name}"))
    };
    let mut sites = Vec::new();
    for (index, _) in table.functions.iter().enumerate().filter(|(_, f)| named(f)) {
        let of = || table.sites.iter().enumerate().filter(|(_, s)| s.function == index);
        let Some(first) = of().map(|(_, s)| s.line).min() else { continue };
        sites.extend(of().filter(|(_, s)| s.line == first).map(|(i, _)| i));
    }
    if sites.is_empty() {
        let near: Vec<&str> = table
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .filter(|f| f.to_lowercase().contains(&name.to_lowercase()) && !f.starts_with("closure in "))
            .take(10)
            .collect();
        return Err(match near.is_empty() {
            true => format!("no function `{name}` has code in the program"),
            false => format!("no function `{name}` has code in the program; these do: {}", near.join(", ")),
        });
    }
    Ok(sites)
}

/// The names of the bindings the code of a function can use: its own, and
/// those of the functions it is written in.
fn usable(table: &Table, function: usize) -> Vec<&str> {
    let mut out: Vec<&str> = table.functions[function].bindings.iter().map(|b| b.name.as_str()).collect();
    let mut name = table.functions[function].name.as_str();
    while let Some((outer, prefix)) = written_in(name) {
        let around =
            table.functions.iter().filter(|f| if prefix { f.name.starts_with(outer) } else { f.name == outer });
        let around: Vec<&Function> = around.collect();
        out.extend(around.iter().flat_map(|f| f.bindings.iter().map(|b| b.name.as_str())));
        match around.first() {
            Some(first) if !prefix => name = first.name.as_str(),
            _ => break,
        }
    }
    out
}

/// `print <expression>`: its value in the frame looked at, with its type
/// when it has one, and the path that names it when it is a binding or a
/// part of one, whose parts `children` gives. A path that is not an
/// expression, as a map's key that is a record, reads as a path.
fn printed(table: &Table, frames: &[Frame], index: usize, text: &str) -> String {
    let expr = match expression::parse(text) {
        Ok(expr) => expr,
        Err(message) => {
            return match resolve(table, frames, index, text) {
                Ok(found) => {
                    let ty = found.ty.as_deref().map_or("null".to_string(), quoted);
                    format!(
                        "{{\"name\":{},\"type\":{ty},{},\"path\":{}}}",
                        quoted(text),
                        described(&found.text),
                        quoted(text)
                    )
                }
                Err(_) => format!("{{\"error\":{}}}", quoted(&message)),
            };
        }
    };
    let evaluated = match evaluating(table, frames, index, |e| e.evaluate(&expr)) {
        Ok(evaluated) => evaluated,
        Err(message) => return format!("{{\"error\":{}}}", quoted(&message)),
    };
    let ty = match (&expr, &evaluated.path) {
        (Expr::Name(name), Some(_)) => {
            visible(table, frames, index).iter().find(|(b, ..)| b.name == *name).map(|(b, ..)| b.ty.clone())
        }
        (_, Some(_)) => None,
        (_, None) => evaluated.value.ty().map(str::to_string),
    };
    let ty = ty.as_deref().map_or("null".to_string(), quoted);
    let path = evaluated.path.as_deref().map_or(String::new(), |path| format!(",\"path\":{}", quoted(path)));
    format!("{{\"name\":{},\"type\":{ty},{}{path}}}", quoted(text), described(&evaluated.text))
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
