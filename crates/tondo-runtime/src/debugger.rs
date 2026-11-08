//! The debugger of a debug build (toolchain 7.6). The compiler puts a probe
//! where each line starts, with the addresses of the function's bindings
//! and a bit for each that holds a value; while no debugger is attached a
//! probe only reads a flag. A tool attaches through the diagnostics
//! socket: it sets breakpoints, steps, and reads the bindings and stack of
//! the task that stopped. When one task stops every task stops at its next
//! probe, and they all go on together.

use super::tasks::tools;
use super::{Builder, Str, str_bytes};
use expression::{Evaluator, Expr};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

mod bindings;
mod commands;
mod expression;
mod stopping;
use bindings::*;
pub(crate) use commands::*;
pub(crate) use stopping::*;

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
    /// For a type with a `debug.View`, how it shows as it is.
    raw: Option<Show>,
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
    /// A `std.sync` cell, shared by every task that has it.
    Shared,
    Other,
}

struct Function {
    name: String,
    bindings: Vec<Binding>,
    /// Where the addresses of its bindings and which of them hold values
    /// are, from its frame pointer, when the compiler said.
    offsets: Option<(isize, isize)>,
}

struct Site {
    file: String,
    line: u32,
    function: usize,
    /// Its line goes on here from the code before, which a stop at the line
    /// already stopped at.
    goes_on: bool,
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
/// What each line of a file left in its frame, while a tool looks: the
/// bindings as the next line of that frame found them.
struct LineValues {
    file: String,
    /// When the tool stops looking if it does not ask again: one that went
    /// away leaves the program as it was.
    until: std::time::Instant,
    lines: HashMap<u32, Vec<(String, String)>>,
    /// The line each frame was at, and when each line was shown last.
    at: HashMap<(u64, usize), u32>,
    shown: HashMap<u32, std::time::Instant>,
}

static LINE_VALUES: Mutex<Option<LineValues>> = Mutex::new(None);
static VALUES_ON: AtomicBool = AtomicBool::new(false);
/// A line in a loop shows at most this often.
const VALUES_EVERY: std::time::Duration = std::time::Duration::from_millis(100);
const VALUES_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// `values <file>`: starts or goes on looking at a file of a build with
/// probes, and says what its lines left so far.
pub(crate) fn line_values_asked(file: &str) -> String {
    if !has_probes() {
        return "{\"error\":\"the values of lines need a build with probes: run it with --live\"}".to_string();
    }
    let mut looking = LINE_VALUES.lock().unwrap_or_else(|e| e.into_inner());
    let until = std::time::Instant::now() + VALUES_FOR;
    let values = looking.get_or_insert_with(|| LineValues {
        file: file.to_string(),
        until,
        lines: HashMap::new(),
        at: HashMap::new(),
        shown: HashMap::new(),
    });
    if values.file != file {
        values.file = file.to_string();
        values.lines.clear();
        values.at.clear();
    }
    values.until = until;
    VALUES_ON.store(true, Ordering::Relaxed);
    let lines = values.lines.iter().map(|(line, bindings)| {
        let bindings =
            bindings.iter().map(|(name, value)| format!("{{\"name\":{},\"value\":{}}}", quoted(name), quoted(value)));
        format!("\"{line}\":[{}]", bindings.collect::<Vec<_>>().join(","))
    });
    let lines = lines.collect::<Vec<_>>().join(",");
    drop(looking);
    format!("{{\"values\":{{{lines}}},\"held\":{}}}", super::live::held_json())
}

/// A frame a panic holds, for what reads its bindings.
fn held_frames(table: &Table, task: u64, probe: super::tasks::Probe) -> Vec<Frame> {
    let super::tasks::Probe { frame, site, addresses, alive } = probe;
    if let Some((addressed, living)) = table.sites.get(site).and_then(|s| table.offsets.get(s.function))
        && addressed.load(Ordering::Relaxed) == UNKNOWN
    {
        addressed.store(addresses as isize - frame as isize, Ordering::Relaxed);
        living.store(alive as isize - frame as isize, Ordering::Relaxed);
    }
    frames(table, &Stop { task, site, frame, bounds: (0, 0), addresses, alive, why: String::new() })
}

/// The bindings of a frame a panic holds, as JSON, each with its type.
pub(crate) fn frame_bindings(task: u64, probe: super::tasks::Probe) -> String {
    let Some(table) = table() else { return String::new() };
    let frames = held_frames(&table, task, probe);
    let bindings = visible(&table, &frames, 0).into_iter();
    let shown = bindings.map(|(binding, address, _)| {
        let value = clipped(&show(binding.show, address));
        format!("{{\"name\":{},\"type\":{},\"value\":{}}}", quoted(&binding.name), quoted(&binding.ty), quoted(&value))
    });
    shown.collect::<Vec<_>>().join(",")
}

/// Where the bindings named are in a frame a panic holds.
pub(crate) fn binding_addresses(task: u64, probe: super::tasks::Probe, names: &[&str]) -> Result<Vec<u64>, String> {
    let table = table().ok_or("the program has no probes")?;
    let frames = held_frames(&table, task, probe);
    let bound = visible(&table, &frames, 0);
    let find = |name: &&str| bound.iter().find(|(b, ..)| b.name == *name).map(|(_, address, _)| *address as u64);
    names.iter().map(|name| find(name).ok_or(format!("no binding `{name}` in that frame"))).collect()
}

/// At a probe, while a tool looks: what the line this frame was at left.
fn line_values(site: usize, frame: usize, addresses: usize, alive: usize) {
    if SHOWING.get() {
        return;
    }
    let Some(table) = table() else { return };
    let Some(here) = table.sites.get(site) else { return };
    let task = super::tasks::current_id();
    let now = std::time::Instant::now();
    let due = {
        let mut looking = LINE_VALUES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(values) = looking.as_mut() else { return };
        if now > values.until {
            VALUES_ON.store(false, Ordering::Relaxed);
            *looking = None;
            return;
        }
        if here.file != values.file && !values.file.ends_with(&format!("/{}", here.file)) {
            return;
        }
        // A line says what the next found; until then, what it started with,
        // which is all the last line of a function ever has.
        let was = values.at.insert((task, frame), here.line);
        let line = was.or((!values.lines.contains_key(&here.line)).then_some(here.line));
        line.filter(|line| values.shown.get(line).is_none_or(|at| now.duration_since(*at) >= VALUES_EVERY))
    };
    let Some(line) = due else { return };
    if let Some((addressed, living)) = table.offsets.get(here.function)
        && addressed.load(Ordering::Relaxed) == UNKNOWN
    {
        addressed.store(addresses as isize - frame as isize, Ordering::Relaxed);
        living.store(alive as isize - frame as isize, Ordering::Relaxed);
    }
    let stop = Stop { task, site, frame, bounds: (0, 0), addresses, alive, why: String::new() };
    let frames = frames(&table, &stop);
    let bindings = visible(&table, &frames, 0).into_iter();
    let found = bindings.map(|(binding, address, _)| (binding.name.clone(), clipped(&show(binding.show, address))));
    let found: Vec<(String, String)> = found.collect();
    if let Some(values) = LINE_VALUES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        values.lines.insert(line, found);
        values.shown.insert(line, now);
    }
}

/// Whether a breakpoint of a request stops this task.
fn in_request(request: &Option<String>, task: u64) -> bool {
    request.as_ref().is_none_or(|text| super::tasks::request_of(task).is_some_and(|it| it.contains(text.as_str())))
}

/// The request a task works for, as a field of an answer.
fn request_field(task: u64) -> String {
    super::tasks::request_of(task).map_or(String::new(), |it| format!(",\"request\":{}", quoted(&it)))
}

/// Whether this build has probes, for a debugger to attach to.
pub(crate) fn has_probes() -> bool {
    TABLE.get().is_some()
}

/// A debugger is attached: probes look at what it asked for.
fn attached() -> bool {
    tools::listens(tools::DEBUGGER)
}

/// Every task stops at its next probe: one stopped, or the debugger asked.
static HOLD: AtomicBool = AtomicBool::new(false);
/// A frame watches an expression: every probe looks.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// Steps stop in the standard library's code: always (`steps all`), or
/// for a step that starts there.
static STEPS_ALL: AtomicBool = AtomicBool::new(false);
static STEPS_STD: AtomicBool = AtomicBool::new(false);
/// A line a task ran while the debugger recorded (`record`): where, its
/// stack, and the bindings its code could use as the line started, as the
/// debugger shows them, clipped.
struct Moment {
    task: u64,
    site: usize,
    stack: Vec<String>,
    bindings: Vec<(String, String, String)>,
    /// In a replay, the probes the program's thread passed before it.
    tick: u64,
}

/// How many lines the history keeps, or 0 while it does not record; and
/// those lines, oldest first.
static RECORDING: AtomicUsize = AtomicUsize::new(0);
static HISTORY: Mutex<std::collections::VecDeque<Moment>> = Mutex::new(std::collections::VecDeque::new());

/// Keeps the line a task starts in the history: the program's own lines,
/// as steps go.
fn record(table: &Table, here: &Stop) {
    if table.sites.get(here.site).is_none_or(Site::generated) {
        return;
    }
    let frames = frames(table, here);
    let bindings = visible(table, &frames, 0)
        .into_iter()
        .map(|(b, address, _)| (b.name.clone(), b.ty.clone(), clipped(&show(b.show, address))))
        .collect();
    let stack = frames.iter().map(|f| f.place.clone()).collect();
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    while history.len() >= RECORDING.load(Ordering::Relaxed).max(1) {
        history.pop_front();
    }
    let tick = super::replay::ticks();
    history.push_back(Moment { task: here.task, site: here.site, stack, bindings, tick });
}

/// The line `goto` moves a stopped task's frame to as it goes on, where
/// its stack shows it until then.
static MOVED_TO: Mutex<Option<(u64, u32)>> = Mutex::new(None);

/// A place, `file:line in function`, at another line.
fn moved_place(place: &str, line: u32) -> String {
    match place.split_once(" in ").and_then(|(at, function)| Some((at.rsplit_once(':')?.0, function))) {
        Some((file, function)) => format!("{file}:{line} in {function}"),
        None => place.to_string(),
    }
}

/// A call the debugger asks a held task to make (`call`): the task, the
/// code, and the addresses of the bindings it reads; and what it gave.
static CALL: Mutex<Option<(u64, u64, Vec<u64>)>> = Mutex::new(None);
/// The task that makes such a call, whose probes let it run to its end.
static CALLING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CALLED: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static CALLED_DONE: Condvar = Condvar::new();

/// `call <slot> <binding>...`: the stopped task calls the function a live
/// patch put in the slot, which `tondo` wrote for an expression, with the
/// addresses of those bindings of the frame looked at; gives what it
/// returned, or that it still runs after a while, as when it waits for
/// another task the stop holds.
fn called(table: &Table, rest: &str) -> Result<String, String> {
    let mut words = rest.split_whitespace();
    let slot: u64 = words.next().and_then(|w| w.parse().ok()).ok_or("call takes a slot and the bindings it reads")?;
    let code = super::live::slot_code(slot).filter(|&c| c != 0).ok_or("the program is not a live build")?;
    let (frames, index) = selected(table)?;
    let bound = visible(table, &frames, index);
    let addresses = words
        .map(|name| bound.iter().find(|(b, ..)| b.name == name).map(|(_, a, _)| *a as u64).ok_or(missing(name, &bound)))
        .collect::<Result<Vec<u64>, String>>()?;
    let task = {
        let state = state();
        let stop = state.stopped.as_ref().ok_or("the program runs: `pause` stops it, or a breakpoint")?;
        state.selected.map_or(stop.task, |(task, _)| task)
    };
    *CALLED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let asking = state();
    *CALL.lock().unwrap_or_else(|e| e.into_inner()) = Some((task, code, addresses));
    drop(asking);
    RESUMED.notify_all();
    let called = CALLED.lock().unwrap_or_else(|e| e.into_inner());
    let (mut called, _) = CALLED_DONE
        .wait_timeout_while(called, std::time::Duration::from_secs(10), |c| c.is_none())
        .unwrap_or_else(|e| e.into_inner());
    match called.take() {
        Some(Ok(text)) => Ok(text),
        Some(Err(message)) => Err(message),
        None => Err("the call still runs after 10 seconds: it may wait for a task the stop holds".into()),
    }
}

/// An event the debugger hears once the answer to its command went.
static TELL_AFTER: Mutex<Option<String>> = Mutex::new(None);

/// `back [n | until <condition>]`, `forward [n | until <condition>]` and
/// `reverse`: the moment of the stopped task's history `locals`, `print`
/// and `stack` show: n lines before or after the one shown, the nearest
/// where the condition holds, or the last before it at a breakpoint;
/// forward past the last is the present again.
fn moved(table: &Table, word: &str, rest: &str) -> Result<String, String> {
    moved_because(table, word, rest, None)
}

/// `moved`, with what the stop says it is for.
fn moved_because(table: &Table, word: &str, rest: &str, because: Option<String>) -> Result<String, String> {
    let because = because.map_or(String::new(), |b| format!(",\"because\":{}", quoted(&b)));
    let until = match rest.strip_prefix("until ") {
        Some(condition) => Some(expression::parse(condition).map_err(|m| format!("the condition: {m}"))?),
        None => None,
    };
    let count = rest.parse().unwrap_or(1);
    let mut state = state();
    let stop = state.stopped.clone().ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let task = state.selected.map_or(stop.task, |(task, _)| task);
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mine: Vec<usize> = (0..history.len()).filter(|&i| history[i].task == task).collect();
    if mine.is_empty() {
        return Err("the history has no line of this task: `record` keeps them from now on".into());
    }
    // The last line recorded is where the task is now.
    let present = mine.len() - 1;
    let at = state.past.and_then(|past| mine.iter().position(|&i| i == past)).unwrap_or(present);
    // Whether the condition holds at a moment, as its bindings were.
    let holds = |moment: &Moment, condition: &Expr| {
        let read = |name: &str| match moment.bindings.iter().find(|(n, ..)| n == name) {
            Some((.., text)) => Ok(text.clone()),
            None => Err(format!("no binding `{name}`")),
        };
        Evaluator::new(&read).holds(condition)
    };
    let to = match (word, &until) {
        ("back", Some(condition)) => {
            let found = mine[..at].iter().rposition(|&i| holds(&history[i], condition));
            found.ok_or(format!("no line before this one in task {task}'s history has `{}`", &rest[6..]))?
        }
        ("forward", Some(condition)) => {
            let found = mine[at + 1..].iter().position(|&i| holds(&history[i], condition)).map(|i| at + 1 + i);
            found.ok_or(format!("no line after this one in task {task}'s history has `{}`", &rest[6..]))?
        }
        _ => match word {
            "back" => at.saturating_sub(count),
            "reverse" => {
                let marked =
                    |&&i: &&usize| table.marked.get(history[i].site).is_some_and(|m| m.load(Ordering::Relaxed));
                mine[..at].iter().rposition(|i| marked(&i)).unwrap_or(0)
            }
            _ => (at + count).min(present),
        },
    };
    if to == at {
        return Err(match word {
            "forward" => "this is the present: `continue` or a step goes on from here".into(),
            _ => format!(
                "no line of task {task}'s history comes before this one: `record` keeps them from when it starts"
            ),
        });
    }
    state.past = (to < present).then_some(mine[to]);
    state.selected = None;
    let moment = &history[mine[to]];
    let at = place_in(table, moment.site);
    let back = present - to;
    let tick = if super::replay::on() { format!(",\"tick\":{}", moment.tick) } else { String::new() };
    *TELL_AFTER.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!(
        "{{\"stopped\":{{\"why\":\"history\",\"task\":{task},\"at\":{},\"back\":{back}{tick}{because}}}}}",
        quoted(&at)
    ));
    Ok("{\"running\":true}".into())
}

/// `why <expression>`: the line of the stopped task's history that gave
/// the expression the value it has where the task is shown: the last line
/// before which it had another, or none. The history shows that line.
fn why(table: &Table, text: &str) -> Result<String, String> {
    let ((changed, now), at) = {
        let state = state();
        let (task, mine, at) = shown_lines(&state)?;
        let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        let found = origin(&history, &mine, at, text).map_err(|e| e.replace("{task}", &task.to_string()))?;
        (found, at)
    };
    let count = (at - changed).to_string();
    let because = format!("`{text}` became {} on this line", clipped(&now));
    moved_because(table, "back", &count, Some(because))
}

/// The stopped task's lines in the history, and which of them the
/// debugger shows.
fn shown_lines(state: &State) -> Result<(u64, Vec<usize>, usize), String> {
    let stop = state.stopped.clone().ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let task = state.selected.map_or(stop.task, |(task, _)| task);
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mine: Vec<usize> = (0..history.len()).filter(|&i| history[i].task == task).collect();
    let present = mine.len().checked_sub(1).ok_or("the history has no line of this task: `record` keeps them")?;
    let at = state.past.and_then(|past| mine.iter().position(|&i| i == past)).unwrap_or(present);
    Ok((task, mine, at))
}

/// Of the task's lines `mine`, the one before line `at` that gave the
/// expression the value it has as line `at` starts: the last that started
/// with another value. Gives that line and the value.
fn origin(
    history: &std::collections::VecDeque<Moment>,
    mine: &[usize],
    at: usize,
    text: &str,
) -> Result<(usize, String), String> {
    let expr = expression::parse(text).map_err(|m| format!("the expression: {m}"))?;
    let shown = |moment: &Moment| {
        let read = |name: &str| match moment.bindings.iter().find(|(n, ..)| n == name) {
            Some((.., text)) => Ok(text.clone()),
            None => Err(format!("no binding `{name}`")),
        };
        Evaluator::new(&read).evaluate(&expr).ok().map(|e| e.text)
    };
    let now = shown(&history[mine[at]]).ok_or(format!("`{text}` has no value here"))?;
    match mine[..at].iter().rposition(|&i| shown(&history[i]).as_ref() != Some(&now)) {
        Some(changed) => Ok((changed, now)),
        None => Err(format!(
            "`{text}` has been {} since the first line of task {{task}}'s history: `record` keeps more of it",
            clipped(&now)
        )),
    }
}

/// What an expression gives as a moment of the history starts, read from
/// its bindings as they were; none where it has no value.
fn shown_at(moment: &Moment, expr: &Expr) -> Option<String> {
    let read = |name: &str| match moment.bindings.iter().find(|(n, ..)| n == name) {
        Some((.., text)) => Ok(text.clone()),
        None => Err(format!("no binding `{name}`")),
    };
    Evaluator::new(&read).evaluate(expr).ok().map(|e| e.text)
}

/// The most a `timeline` or a `ran` gives, the newest.
const LISTED: usize = 200;

/// `effects`: what each line of the call the debugger shows changed the
/// last time it ran, as the history has it: the bindings whose values the
/// call's next line started with were others, or new.
fn effects(table: &Table) -> Result<String, String> {
    let (_, mine, at) = shown_lines(&state())?;
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let now = &history[mine[at]];
    let depth = now.stack.len();
    // The call's lines, back to where it started: those of its callees have
    // its callers under them too.
    let within = |moment: &Moment| {
        moment.stack.len() >= depth && moment.stack[moment.stack.len() + 1 - depth..] == now.stack[1..]
    };
    let mut call: Vec<&Moment> = mine[..=at]
        .iter()
        .rev()
        .map(|&i| &history[i])
        .take_while(|moment| within(moment))
        .filter(|moment| moment.stack.len() == depth)
        .collect();
    call.reverse();
    let mut lines: Vec<(usize, String)> = Vec::new();
    for pair in call.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        let changed: Vec<String> = after
            .bindings
            .iter()
            .filter(|(name, _, value)| !before.bindings.iter().any(|(n, _, v)| n == name && v == value))
            .map(|(name, _, value)| format!("{{\"name\":{},\"value\":{}}}", quoted(name), quoted(value)))
            .collect();
        lines.retain(|(site, _)| *site != before.site);
        lines.push((before.site, changed.join(",")));
    }
    let lines: Vec<String> = lines
        .iter()
        .filter_map(|(site, changed)| {
            let site = table.sites.get(*site)?;
            Some(format!("{{\"file\":{},\"line\":{},\"changed\":[{changed}]}}", quoted(&site.file), site.line))
        })
        .collect();
    Ok(format!("{{\"effects\":[{}]}}", lines.join(",")))
}

/// `timeline <expression>`: the lines of the stopped task's history, up
/// to the one shown, that gave the expression another value: the line
/// before the first that starts with it. Each with its place, how many
/// lines back, its probe in a replay, and the value.
fn timeline(table: &Table, text: &str) -> Result<String, String> {
    let expr = expression::parse(text).map_err(|m| format!("the expression: {m}"))?;
    let state = state();
    let (_, mine, at) = shown_lines(&state)?;
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut changes = Vec::new();
    let mut last: Option<String> = None;
    for (index, &i) in mine[..=at].iter().enumerate() {
        let now = shown_at(&history[i], &expr);
        if now.is_some() && now != last {
            let gave = index.saturating_sub(1);
            let moment = &history[mine[gave]];
            changes.push(format!(
                "{{\"at\":{},\"back\":{},\"tick\":{},\"value\":{}}}",
                quoted(&place_in(table, moment.site)),
                at - gave,
                moment.tick,
                quoted(&clipped(now.as_deref().unwrap_or_default()))
            ));
        }
        last = now;
    }
    let from = changes.len().saturating_sub(LISTED);
    Ok(format!("{{\"timeline\":[{}],\"changes\":{}}}", changes[from..].join(","), changes.len()))
}

/// `ran <file>:<line> [<expression>]`: each line of the history, of any
/// task, that starts at that line, newest last: the task, its probe in a
/// replay, and the expression's value then.
fn ran(table: &Table, rest: &str) -> Result<String, String> {
    let (at, text) = rest.split_once(' ').map_or((rest, None), |(at, text)| (at, Some(text.trim())));
    let sites = line_sites(table, at)?;
    let expr = text.map(expression::parse).transpose().map_err(|m| format!("the expression: {m}"))?;
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let runs: Vec<String> = history
        .iter()
        .filter(|moment| sites.contains(&moment.site))
        .map(|moment| {
            let value = expr.as_ref().and_then(|expr| shown_at(moment, expr));
            let value = value.map_or(String::new(), |v| format!(",\"value\":{}", quoted(&clipped(&v))));
            format!("{{\"task\":{},\"tick\":{}{value}}}", moment.task, moment.tick)
        })
        .collect();
    let from = runs.len().saturating_sub(LISTED);
    let place = place_in(table, sites[0]);
    Ok(format!("{{\"ran\":[{}],\"times\":{},\"at\":{}}}", runs[from..].join(","), runs.len(), quoted(&place)))
}

/// `causes <expression>...`: where each got the value it has as the line
/// the debugger shows starts, without moving there: the line, how many
/// lines back, and its probe in a replay.
fn causes(table: &Table, rest: &str) -> Result<String, String> {
    let state = state();
    let (_, mine, at) = shown_lines(&state)?;
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut found = Vec::new();
    for text in rest.split_whitespace() {
        let Ok((changed, now)) = origin(&history, &mine, at, text) else { continue };
        let moment = &history[mine[changed]];
        found.push(format!(
            "{{\"name\":{},\"value\":{},\"at\":{},\"back\":{},\"tick\":{}}}",
            quoted(text),
            quoted(&clipped(&now)),
            quoted(&place_in(table, moment.site)),
            at - changed,
            moment.tick
        ));
    }
    Ok(format!("{{\"causes\":[{}]}}", found.join(",")))
}

/// The moment of the history the debugger shows, while it shows one.
fn past(state: &State) -> Option<(usize, std::sync::MutexGuard<'static, std::collections::VecDeque<Moment>>)> {
    state.past.map(|past| (past, HISTORY.lock().unwrap_or_else(|e| e.into_inner())))
}

/// `locals`, `print` and `children` in a moment of the history: the
/// bindings of its innermost frame, as they were.
fn in_past(moment: &Moment, word: &str, rest: &str, frame: usize) -> String {
    if frame != 0 {
        let message = "the history keeps the bindings of the innermost frame: `frame 0`";
        return format!("{{\"error\":{}}}", quoted(message));
    }
    let read = |name: &str| match moment.bindings.iter().find(|(n, ..)| n == name) {
        Some((.., text)) => Ok(text.clone()),
        None => Err(format!("no binding `{name}` held a value there")),
    };
    match word {
        "locals" => {
            let items: Vec<String> = moment
                .bindings
                .iter()
                .map(|(name, ty, text)| {
                    format!("{{\"name\":{},\"type\":{},{}}}", quoted(name), quoted(ty), described(text))
                })
                .collect();
            format!("{{\"locals\":[{}]}}", items.join(","))
        }
        "print" => {
            let evaluated = expression::parse(rest).and_then(|expr| Evaluator::new(&read).evaluate(&expr));
            match evaluated {
                Ok(evaluated) => {
                    let ty = moment.bindings.iter().find(|(n, ..)| n == rest).map(|(_, ty, _)| ty.as_str());
                    let ty = ty.or(evaluated.value.ty()).map_or("null".to_string(), quoted);
                    let path = evaluated.path.as_deref().map_or(String::new(), |p| format!(",\"path\":{}", quoted(p)));
                    format!("{{\"name\":{},\"type\":{ty},{}{path}}}", quoted(rest), described(&evaluated.text))
                }
                Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
            }
        }
        _ => {
            let root = rest.find(['.', '[']).map_or(rest, |i| &rest[..i]);
            let found = read(root).and_then(|whole| {
                super::shown::find(&whole, &rest[root.len()..])
                    .map(str::to_string)
                    .ok_or(format!("`{rest}` is not a part of `{root}`"))
            });
            match found {
                Ok(text) => children_of(rest, &text, None),
                Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
            }
        }
    }
}

/// The parts of a value's text, all or a page of them, each with its path.
fn children_of(path: &str, text: &str, page: Option<(usize, usize)>) -> String {
    let parts = super::shown::parse(text).parts;
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

/// The task that steps, or 0; and the function that returned last in it,
/// with what it returned, which the step's stop shows.
static STEPPING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RETURNED: Mutex<Option<(u64, String, String)>> = Mutex::new(None);

/// Where the value of a `Ref` (`reference`), or of a `std.sync` cell, is,
/// for the debugger to show it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_state(handle: usize, reference: u8) -> *const u8 {
    match reference {
        1 => super::ref_payload(handle as *mut u8),
        _ => unsafe { super::sync::tondo_rt_sync_value(handle) },
    }
}

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
    VIEWED.store(functions.iter().any(|f| f.bindings.iter().any(|b| b.raw.is_some())), Ordering::Relaxed);
    let marked = sites.iter().map(|_| AtomicBool::new(false)).collect();
    let offsets = functions
        .iter()
        .map(|f| {
            let (addresses, alive) = f.offsets.unwrap_or((UNKNOWN, UNKNOWN));
            (AtomicIsize::new(addresses), AtomicIsize::new(alive))
        })
        .collect();
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
    let added = functions.iter().map(|f| {
        let (addresses, alive) = f.offsets.unwrap_or((UNKNOWN, UNKNOWN));
        (AtomicIsize::new(addresses), AtomicIsize::new(alive))
    });
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
        let same = |site: &Site| site.file == file && site.line == line && !site.goes_on;
        let added = (first..table.sites.len()).filter(|&i| same(&table.sites[i]));
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
            ["F", name] => functions.push(Function { name: name.to_string(), bindings: Vec::new(), offsets: None }),
            ["O", addresses, alive] => {
                if let (Some(function), Ok(addresses), Ok(alive)) =
                    (functions.last_mut(), addresses.parse(), alive.parse())
                {
                    function.offsets = Some((addresses, alive));
                }
            }
            ["L", name, ty, layout, viewed] => {
                let show = glues[next];
                next += 1;
                // A view's binding has the glue that shows it as it is next.
                let raw = (*viewed == "1").then(|| {
                    next += 1;
                    glues[next - 1]
                });
                if let Some(function) = functions.last_mut() {
                    let (name, ty, layout) = (name.to_string(), ty.to_string(), base + number(layout));
                    function.bindings.push(Binding { name, ty, show, layout, raw });
                }
            }
            // The program has secrets, which any of its texts may hold.
            ["H"] => super::heap::hide_texts(),
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
                    ["h"] => Layout::Shared,
                    _ => Layout::Other,
                });
            }
            ["S", file, line, function, goes_on] => sites.push(Site {
                file: file.to_string(),
                line: line.parse().unwrap_or(0),
                function: function.parse().unwrap_or(0),
                goes_on: *goes_on == "1",
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
    while !attached() && std::time::Instant::now() < deadline {
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
