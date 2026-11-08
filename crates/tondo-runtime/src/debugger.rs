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
use bindings::*;
pub(crate) use commands::*;

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

// ---------------------------------------------------------------------------
// Stopping
// ---------------------------------------------------------------------------

/// How the debugger asked the program to go on: until a breakpoint, or for
/// one task, to the next line, the next one in its frame or an outer one,
/// or the first in an outer frame.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Run,
    /// One line on, in `frame` or in what it calls.
    Step {
        task: u64,
        frame: usize,
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
    /// Into the call of a function of the name `INTO` holds that the frame
    /// makes, or on to its next line.
    Into {
        task: u64,
        frame: usize,
    },
}

/// The name `step into` goes into.
static INTO: Mutex<String> = Mutex::new(String::new());

/// Whether a function is called `name`: by its whole name, as
/// `Stack.push`, or by its own, `push`, with a generic one's type
/// arguments or without; `loose`, also by the name after the last dot, as
/// a call `items.push` names `Array.push`.
fn named(function: &Function, name: &str, loose: bool) -> bool {
    let mut plain = String::new();
    let mut depth = 0;
    for c in function.name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let own = name.rsplit('.').next().unwrap_or(name);
    let by_own = loose || own == name;
    function.name == name || plain == name || (by_own && (plain == own || plain.ends_with(&format!(".{own}"))))
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
    /// Where the code of each stopped task is at, when known.
    code: Vec<(u64, usize)>,
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
    /// The moment of the history shown instead of the present.
    past: Option<usize>,
    /// The tasks that stay held while the program goes on.
    frozen: Vec<u64>,
}

/// Some task is frozen: probes look.
static FREEZING: AtomicBool = AtomicBool::new(false);

/// An expression a task's frame watches: the task stops on the line after
/// the one that changed its value, in that frame or a call it makes, until
/// the frame returns. One of a `std.sync` cell's value watches it from
/// every task, for as long as the watch lasts.
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
    /// When a change stops: its condition holds, and its count of such
    /// changes reaches `hits`.
    condition: Option<Expr>,
    count: u64,
    hits: Option<Hits>,
    /// The cell, which the watch keeps alive, the binding it was read from
    /// and how that shows it.
    shared: Option<(usize, String, Show)>,
}

impl Watch {
    /// Its value now, and whether its condition holds; `None` when it
    /// cannot be read now, as while a task changes a cell.
    fn now(&self, table: &Table) -> Option<(String, bool)> {
        let evaluate = |e: &mut Evaluator| {
            let value = e.evaluate(&self.expr).ok()?.text;
            Some((value, self.condition.as_ref().is_none_or(|c| e.holds(c))))
        };
        match &self.shared {
            None => evaluating(table, &self.frames, 0, evaluate),
            Some((cell, root, glue)) => {
                let read = |name: &str| match name == root {
                    true => Ok(show(*glue, cell as *const usize as usize)),
                    false => Err(format!("a watch of a shared value reads `{root}` only")),
                };
                unsafe {
                    super::sync::reading(*cell as *mut super::sync::SyncObject, || evaluate(&mut Evaluator::new(&read)))
                }
                .flatten()
            }
        }
    }
}

/// Removes the watches `unwanted` says, letting go of the cells they kept.
fn unwatch(state: &mut State, unwanted: impl Fn(&Watch) -> bool) {
    state.watches.retain(|watch| {
        let keep = !unwanted(watch);
        if let Some((cell, ..)) = watch.shared.as_ref().filter(|_| !keep) {
            unsafe { super::sync::tondo_rt_sync_release(*cell as *mut super::sync::SyncObject) };
        }
        keep
    });
    WATCHING.store(!state.watches.is_empty(), Ordering::Relaxed);
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
    /// The task it stops alone, when it names one, and the request it stops
    /// in alone: a text its method and path have.
    task: Option<u64>,
    request: Option<String>,
    /// `until`'s: it goes at the next stop, wherever that is.
    once: bool,
    /// The event it stops at, instead of a place.
    event: Option<Event>,
    /// It captures the stack and the bindings and goes on, instead of
    /// stopping (`capture`).
    capture: bool,
    /// Its condition, when only a compiled patch reads it.
    unparsed: Option<String>,
    /// Its condition compiled by live patches: for each, the code, the
    /// bindings it reads, by name, in order, and the sites it is for, those
    /// of the code it was compiled with, whose types it knows.
    compiled: Vec<(u64, Vec<String>, Vec<usize>)>,
}

/// Whether a compiled condition holds in a frame: its code called with the
/// addresses of the bindings it reads, on this task, its lines passing.
/// None when one of them is not there yet, for the evaluator to say.
fn compiled_holds(table: &Table, frames: &[Frame], code: u64, names: &[String]) -> Option<bool> {
    let bound = visible(table, frames, 0);
    let addresses: Option<Vec<u64>> =
        names.iter().map(|name| bound.iter().find(|(b, ..)| b.name == *name).map(|(_, a, _)| *a as u64)).collect();
    let addresses = addresses?;
    let condition: extern "C" fn(u64, u64) -> u8 = unsafe { std::mem::transmute(code as usize) };
    let _outside = super::replay::Outside::enter();
    let before = SHOWING.replace(true);
    let held = condition(addresses.as_ptr() as u64, 0) != 0;
    SHOWING.set(before);
    Some(held)
}

/// When a breakpoint stops by its count of hits: at the `n`th, from it on,
/// past it, or at every `n`th.
pub(crate) struct Hits {
    op: &'static str,
    n: u64,
}

impl Hits {
    pub(crate) fn parse(text: &str) -> Result<Hits, String> {
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

    pub(crate) fn reached(&self, count: u64) -> bool {
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
    code: Vec::new(),
    mode: Mode::Run,
    stopped: None,
    resumed: 0,
    held: Vec::new(),
    selected: None,
    breakpoints: Vec::new(),
    watches: Vec::new(),
    next_breakpoint: 1,
    past: None,
    frozen: Vec::new(),
});
static RESUMED: Condvar = Condvar::new();
/// The tool that debugs the program, which hears when it stops, and the
/// descriptor of its connection to the diagnostics thread.
static CLIENT: Mutex<Option<UnixStream>> = Mutex::new(None);
static CLIENT_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
/// The program started under `tondo debug`, which it waits for before
/// `main`; and it has not reached `main` yet.
pub(crate) static WAIT: AtomicBool = AtomicBool::new(false);

/// The debugger debugs the children the program starts too (`follow`).
static FOLLOW: AtomicBool = AtomicBool::new(false);

/// Whether a child the program starts is for the debugger too: one that is
/// a debug build of Tondo waits for it before its `main`, as `tondo debug`
/// starts a program.
pub(crate) fn following() -> bool {
    follows_children() && !super::replay::replaying()
}

/// The debugger asked to debug the children the program starts too.
pub(crate) fn follows_children() -> bool {
    attached() && FOLLOW.load(Ordering::Relaxed)
}

/// What gdb or lldb asks of the frame they show, of the program's function
/// at `pc` whose frame pointer is `fp`: `locals`, or `print <expression>`,
/// written to `path` as this debugger answers. The debugger calls it on
/// the thread it stopped; whether it could.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_to(
    path: *const std::ffi::c_char,
    command: *const std::ffi::c_char,
    pc: usize,
    fp: usize,
) -> i32 {
    let (Some(table), false, false) = (table(), path.is_null(), command.is_null()) else { return 0 };
    let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let command = unsafe { std::ffi::CStr::from_ptr(command) }.to_string_lossy().into_owned();
    std::fs::write(path, native_frame(&table, command.trim(), pc, fp)).is_ok() as i32
}

/// The answer to `command` in the frame of another debugger.
fn native_frame(table: &Table, command: &str, pc: usize, fp: usize) -> String {
    let error = |message: &str| format!("{{\"error\":{}}}", quoted(message));
    let Some(code) = super::trace::describe(pc + 1) else { return error("this frame is not the program's code") };
    let Some(function) = table.functions.iter().position(|f| f.name == code.function) else {
        return error(&format!("`{}` is not a function of the program", code.function));
    };
    let offsets = table.offsets.get(function).map(|(a, l)| (a.load(Ordering::Relaxed), l.load(Ordering::Relaxed)));
    let Some((addressed, living)) = offsets.filter(|&(a, _)| a != UNKNOWN) else {
        return error("the function has reached none of its lines yet: step to one");
    };
    let place = code.at.map_or(String::new(), |(file, line)| format!("{file}:{line} in {}", code.function));
    let at = |offset: isize| (fp as isize + offset) as usize;
    let frames =
        [Frame { place, function: Some(function), pointer: fp, addresses: at(addressed), alive: at(living), pc }];
    let (word, rest) = command.split_once(' ').unwrap_or((command, ""));
    match word {
        "locals" => format!("{{\"locals\":[{}]}}", locals_of(table, &frames, 0, super::tasks::current_id()).join(",")),
        "print" => match expression::parse(rest).and_then(|e| evaluating(table, &frames, 0, |ev| ev.evaluate(&e))) {
            Ok(evaluated) => format!("{{\"name\":{},{}}}", quoted(rest), described(&evaluated.text)),
            Err(message) => error(&message),
        },
        _ => error("gdb and lldb ask `locals` or `print <expression>`"),
    }
}

/// The program started a child the debugger follows, which a replay
/// replays from its own `recording`.
pub(crate) fn child_started(pid: u32, program: &str, recording: Option<&std::path::Path>) {
    let recording = recording.map_or(String::new(), |r| format!(",\"recording\":{}", quoted(&r.to_string_lossy())));
    tell(&format!("{{\"child\":{{\"pid\":{pid},\"program\":{}{recording}}}}}", quoted(program)));
}
static BEFORE_MAIN: AtomicBool = AtomicBool::new(true);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn tell(event: &str) {
    let _outside = super::replay::Outside::enter();
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(client, "{event}");
    }
}

/// The probe where a line starts: stops the task when the debugger asked
/// for it here. In a live build, whether the frame is to restart: the
/// statement after the probe then returns and calls its function again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_line(site: i64, addresses: *const usize, alive: *const u64) -> u8 {
    // The probes the program passes place a replay's calls: not those of
    // the code the debugger runs, to show a value or make a call.
    if super::replay::on() && !SHOWING.get() && CALLING.load(Ordering::Relaxed) != super::tasks::current_id() {
        super::replay::tick();
    }
    // A reload that converts values with identity stops every task at a
    // statement first.
    if super::live::barrier() {
        super::live::park();
    }
    // A recording of the last minutes keeps a copy of the program now and
    // then, which a replay goes on from.
    if super::replay::copy_due() {
        let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
        if let Some(stop) = copied(site as usize, frame, addresses as usize, alive as usize) {
            hold(state(), stop, Some(CHECKPOINT.into()));
        }
    }
    if VALUES_ON.load(Ordering::Relaxed) {
        let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
        line_values(site as usize, frame, addresses as usize, alive as usize);
    }
    if super::live::holds_panics() {
        let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
        let (site, addresses, alive) = (site as usize, addresses as usize, alive as usize);
        super::tasks::note_probe(super::tasks::Probe { frame, site, addresses, alive });
    }
    if !attached() {
        return super::live::moving(site);
    }
    // A call the debugger makes runs to its end, as a view's code does.
    let calling = CALLING.load(Ordering::Relaxed);
    if (calling != 0 && calling == super::tasks::current_id()) || SHOWING.get() {
        return 0;
    }
    let Some(mut table) = table() else { return 0 };
    let site = site as usize;
    super::live::forget_goto(super::tasks::current_id());
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
    let recording = RECORDING.load(Ordering::Relaxed) > 0;
    let waiting = STARTED.load(Ordering::Relaxed) || FREEZING.load(Ordering::Relaxed);
    let aiming = AIM.load(Ordering::Relaxed) != u64::MAX;
    // A replay that runs keeps a checkpoint now and then, looked at every
    // so many probes.
    let every = EVERY.load(Ordering::Relaxed);
    if every > 0 && super::replay::look_at(&CHECKPOINT_LOOK, 1 << 14) {
        let _outside = super::replay::Outside::enter();
        let here = Stop {
            task,
            site,
            frame,
            bounds: (0, 0),
            addresses: addresses as usize,
            alive: alive as usize,
            why: String::new(),
        };
        if let Some(stop) = periodic(&table, here, every) {
            let Some(again) = at_checkpoint(table, stop) else { return 0 };
            table = again;
        }
    }
    if !marked
        && !watching
        && !recording
        && !waiting
        && !aiming
        && !HOLD.load(Ordering::Relaxed)
        && state().mode == Mode::Run
    {
        return restarting(frame, task, site);
    }
    // What the debugger does here is not the program's, until the task
    // stops, when what the debugger asks it to run may be.
    let _outside = super::replay::Outside::enter();
    let quiet = super::heap::Quiet::enter();
    if recording {
        let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
        let (addresses, alive) = (addresses as usize, alive as usize);
        record(&table, &Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
    }
    let mut state = state();
    // A replay going to a probe (`seek`), or through the run to it, keeping
    // where breakpoints would stop and where a task's lines start (`scan`).
    if aiming {
        let tick = super::replay::ticks();
        let scanning = SCANNING.load(Ordering::Relaxed);
        // Breakpoints count their hits, as they would have, and stop
        // nowhere before the probe it goes to.
        scanned(&table, &mut state, task, site, frame, (addresses as usize, alive as usize), tick);
        let at = (addresses as usize, alive as usize);
        tracking(
            &table,
            Stop { task, site, frame, bounds: (0, 0), addresses: at.0, alive: at.1, why: String::new() },
            tick,
        );
        if AIM.load(Ordering::Relaxed) == tick {
            AIM.store(u64::MAX, Ordering::Relaxed);
            SCANNING.store(false, Ordering::Relaxed);
            drop(table);
            let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
            let (addresses, alive) = (addresses as usize, alive as usize);
            let stop = Stop { task, site, frame, bounds, addresses, alive, why: String::new() };
            drop(quiet);
            hold(state, stop, Some(if scanning { "scanned" } else { "replayed" }.to_string()));
            return restarting(frame, task, site);
        }
        return restarting(frame, task, site);
    }
    let frozen = state.frozen.contains(&task);
    // A task that started while a breakpoint stops tasks as they start, at
    // its first line of the program's code.
    let function = table.sites.get(site).map(|s| table.functions[s.function].name.as_str());
    let starts = STARTED.load(Ordering::Relaxed) && function.is_some_and(|f| !f.starts_with("spawn "));
    let started = starts.then(|| {
        let own = table.sites.get(site).is_some_and(|s| !s.generated());
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        let at = starting.iter().position(|(t, why)| *t == task && (why.is_none() || own));
        let found = at.map(|at| starting.swap_remove(at).1);
        STARTED.store(!starting.is_empty(), Ordering::Relaxed);
        found
    });
    let why = if frozen || state.stopped.is_some() || HOLD.load(Ordering::Relaxed) {
        // Another task stopped, or the debugger asked all to, or this task
        // stays where it is.
        if frozen || state.stopped.is_some() { None } else { Some("pause".to_string()) }
    } else if let Some(Some(told)) = started {
        let from = spawned(task).first().map_or(String::new(), |at| format!(", spawned at {at}"));
        Some(told.unwrap_or_else(|| format!("task {task} starts{from}")))
    } else {
        // A step goes over code the program's files do not have, the
        // standard library's and the derived, unless it started there.
        let standard = || table.sites.get(site).is_some_and(Site::generated);
        let goes_on = table.sites.get(site).is_some_and(|s| s.goes_on);
        let stepped = match state.mode {
            // The call asked for, made by the frame itself, or the frame's
            // next line.
            Mode::Into { task: t, frame: f } if t == task => {
                let function = table.sites.get(site).map(|s| &table.functions[s.function]);
                let caller = unsafe { *(frame as *const usize) };
                let into = INTO.lock().unwrap_or_else(|e| e.into_inner());
                (frame >= f && !standard()) || (caller == f && function.is_some_and(|f| named(f, &into, true)))
            }
            Mode::Into { .. } => false,
            Mode::Step { .. } | Mode::Next { .. } | Mode::Finish { .. }
                if !STEPS_STD.load(Ordering::Relaxed) && standard() =>
            {
                false
            }
            Mode::Run => false,
            // The line it stepped from going on is no new line.
            Mode::Step { task: t, frame: f } => t == task && !(frame == f && goes_on),
            Mode::Next { task: t, frame: f } => t == task && frame >= f && !(frame == f && goes_on),
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
            let evaluated = || match &breakpoint.condition {
                Some(c) => evaluating(&table, stack(), 0, |e| e.holds(c)),
                None => breakpoint.unparsed.is_none(),
            };
            let holds = || match breakpoint.compiled.iter().find(|(.., sites)| sites.contains(&site)) {
                Some((code, names, _)) => compiled_holds(&table, stack(), *code, names).unwrap_or_else(evaluated),
                None => evaluated(),
            };
            let here = breakpoint.sites.contains(&site)
                && breakpoint.task.is_none_or(|t| t == task)
                && in_request(&breakpoint.request, task)
                && holds();
            if !here {
                continue;
            }
            breakpoint.count += 1;
            if !breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                continue;
            }
            // A capture: the stack and the bindings, as the program goes on.
            if breakpoint.capture {
                let frames = stack();
                let locals = locals_of(&table, frames, 0, task).join(",");
                let places: Vec<String> = frames.iter().map(|f| quoted(&f.place)).collect();
                tell(&format!(
                    "{{\"captured\":{{\"id\":{},\"task\":{task},\"at\":{},\"locals\":[{locals}],\"stack\":[{}]}}}}",
                    breakpoint.id,
                    quoted(&place_in(&table, site)),
                    places.join(",")
                ));
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
        let until = hit.is_some_and(|id| state.breakpoints.iter().any(|b| b.id == id && b.once));
        match (hit, stepped) {
            (Some(_), _) if until => Some("until".to_string()),
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
    // `until`'s breakpoint goes at the stop.
    if why.is_some() && state.breakpoints.iter().any(|b| b.once) {
        state.breakpoints.retain(|b| !b.once);
        remark(&table, &state);
    }
    // A patch may come while the task waits.
    drop(table);
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let stop =
        Stop { task, site, frame, bounds, addresses: addresses as usize, alive: alive as usize, why: String::new() };
    drop(quiet);
    hold(state, stop, why);
    restarting(frame, task, site)
}

/// In a replay, the probe of the program's thread a `seek` or a `scan` goes
/// to; while scanning, the probes where breakpoints would have stopped,
/// and those where the lines of one task start.
static AIM: AtomicU64 = AtomicU64::new(u64::MAX);
static SCANNING: AtomicBool = AtomicBool::new(false);
static SCANNED: Mutex<(u64, std::collections::VecDeque<u64>, std::collections::VecDeque<u64>)> =
    Mutex::new((0, std::collections::VecDeque::new(), std::collections::VecDeque::new()));

/// What a scan keeps of each list, the last: what going back needs.
const KEPT: usize = 10_000;

fn kept_last(list: &mut std::collections::VecDeque<u64>, tick: u64) {
    if list.len() == KEPT {
        list.pop_front();
    }
    list.push_back(tick);
}

/// Forks the replay where its task stopped, on that task: the copy keeps
/// the program as it is here, without the debugger, and waits on a socket
/// of its own; each `resume <probe>` there forks it again into a replay
/// stopped here, which shows what the program writes from that probe on.
fn checkpoint(table: &Table) -> Result<u32, String> {
    if super::replay::threaded() {
        return Err("a copy of the replay would lose its threads of C".into());
    }
    // What the debugger's thread may hold is held across the fork, so that
    // the copy, without that thread, has it free: it asks for this one by
    // `on_task`, and may still be on its way to wait for it.
    let locks = (state(), CLIENT.lock(), WORKED.lock(), WORK.lock());
    let forked = super::tasks::fork_copy();
    drop(locks);
    match forked {
        -1 => Err(format!("cannot fork the replay: {}", std::io::Error::last_os_error())),
        0 => {
            kept(table);
            super::diagnostics::listen_again();
            Ok(0)
        }
        pid => Ok(pid as u32),
    }
}

/// In a replay, a copy of the process where its task stopped, which the
/// debugger goes back to rather than to the start. This thread holds no
/// lock of the table meanwhile: the copy, without it, could never add a
/// patch's.
fn take_checkpoint() -> String {
    let Some(stop) = state().stopped.clone().filter(|s| s.site < WAITING) else {
        return "{\"error\":\"the program runs: `pause` stops it, or a breakpoint\"}".to_string();
    };
    let tick = super::replay::ticks();
    let taken = on_task(stop.task, || match table() {
        Some(table) => checkpoint(&table),
        None => Err("the program is not a debug build".to_string()),
    });
    match taken {
        Some(Ok(pid)) => format!("{{\"checkpoint\":{{\"pid\":{pid},\"tick\":{tick}}}}}"),
        Some(Err(message)) => format!("{{\"error\":{}}}", quoted(&message)),
        None => "{\"error\":\"the stopped task did not take the checkpoint\"}".into(),
    }
}

/// In a replay, how often, in milliseconds, a program that runs keeps a
/// checkpoint at a probe (`checkpoints <ms>`), or never; and when it last
/// did.
static EVERY: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
/// The probe at which to see next if a checkpoint is due.
static CHECKPOINT_LOOK: AtomicU64 = AtomicU64::new(0);
/// So many probes make a checkpoint due too, as a replay may run them
/// faster than the time does.
const PROBES: u64 = 1 << 22;

/// How far into a value a dump keeps its parts, and how many of each.
const DUMPED_DEPTH: usize = 4;
const DUMPED_PARTS: usize = 100;
/// How long a dump takes to show the bindings, and how many that break it
/// it goes past.
const DUMPING: std::time::Duration = std::time::Duration::from_secs(3);
const BROKEN: usize = 16;

/// This process is the copy of the program that shows a dump's bindings.
static RENDERING: AtomicBool = AtomicBool::new(false);

pub(crate) fn rendering() -> bool {
    RENDERING.load(Ordering::Relaxed)
}

/// The parts of a value as a dump keeps them, to open it afterwards.
fn dumped_parts(text: &str, depth: usize) -> String {
    let part = |(name, part): &(String, super::shown::Part)| {
        let parts = dumped_parts(part.text, depth - 1);
        format!("{{\"name\":{},\"value\":{},\"parts\":{parts}}}", quoted(name), quoted(&clipped(part.text)))
    };
    let parts = match depth {
        0 => Vec::new(),
        _ => super::shown::parse(text).parts.iter().take(DUMPED_PARTS).map(part).collect(),
    };
    format!("[{}]", parts.join(","))
}

/// The frame of the code at `pc`, whose frame pointer is `fp`.
fn frame_at(table: &Table, pc: usize, fp: usize) -> Option<Frame> {
    let code = super::trace::describe(pc + 1)?;
    let (file, line) = code.at?;
    let function = table.functions.iter().position(|f| f.name == code.function)?;
    let (addressed, living) =
        table.offsets.get(function).map(|(a, l)| (a.load(Ordering::Relaxed), l.load(Ordering::Relaxed)))?;
    let (addresses, alive) = match addressed {
        UNKNOWN => (0, 0),
        _ => ((fp as isize + addressed) as usize, (fp as isize + living) as usize),
    };
    let place = format!("{file}:{line} in {}", code.function);
    Some(Frame { place, function: Some(function), pointer: fp, addresses, alive, pc })
}

/// The bindings of every frame of every task, for a dump: a line of JSON
/// each. Showing a value reads memory that may be broken, at a crash above
/// all, and takes locks others may hold: a copy of the program does it,
/// which may crash or hang without harm, and one that crashes goes on from
/// the next binding. `here` is where this thread's task stands.
pub(crate) fn values(here: Option<(Option<usize>, usize, (usize, usize))>) -> Vec<String> {
    rendered(here, false)
}

/// What the frame at the site `pc` holds, for a logpoint: as `values`, of
/// that frame alone, whose registers are all known.
pub(crate) fn frame_values(
    pc: usize,
    fp: usize,
    bounds: (usize, usize),
    registers: &[usize; 32],
    floats: &[usize; 32],
    condition: Option<String>,
) -> Vec<String> {
    *CONDITION.lock().unwrap_or_else(|e| e.into_inner()) = condition;
    site_registers(registers, floats, true);
    let found = rendered(Some((Some(pc), fp, bounds)), true);
    site_registers(registers, floats, false);
    found
}

/// While a logpoint shows what its site holds, the registers there are known.
pub(crate) fn site_registers(registers: &[usize; 32], floats: &[usize; 32], known: bool) {
    super::diagnostics::fault_registers(registers, known);
    FLOATS.iter().zip(floats).for_each(|(slot, &value)| slot.store(value, Ordering::Relaxed));
    LOGGING.store(known, Ordering::Relaxed);
}

/// Whether an expression reads.
pub(crate) fn valid_expression(text: &str) -> Result<(), String> {
    expression::parse(text).map(drop)
}

/// What must hold for the frame a logpoint shows to be shown.
static CONDITION: Mutex<Option<String>> = Mutex::new(None);

/// While a logpoint shows a frame, the low half of each register of numbers.
static LOGGING: AtomicBool = AtomicBool::new(false);
static FLOATS: [std::sync::atomic::AtomicUsize; 32] = [const { std::sync::atomic::AtomicUsize::new(0) }; 32];

/// Each place of the code where a logpoint can go: its address, its length,
/// its file and its line.
pub(crate) fn latent_sites() -> Vec<(usize, usize, String, u32, usize)> {
    let raw = DUMP_TABLE.load(Ordering::Relaxed);
    if raw.is_null() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(unsafe { str_bytes((*raw).text) }).into_owned();
    let (mut sites, mut function) = (Vec::new(), None);
    for line in text.lines() {
        match line.split('\t').collect::<Vec<_>>()[..] {
            ["F", index] => function = index.parse().ok().and_then(super::trace::function_start),
            ["P", at, length, file, line] => {
                if let (Some(start), Ok(at), Ok(length), Ok(line)) =
                    (function, at.parse::<usize>(), length.parse(), line.parse())
                {
                    sites.push((start + at, length, file.to_string(), line, start));
                }
            }
            _ => {}
        }
    }
    sites
}

fn rendered(here: Option<(Option<usize>, usize, (usize, usize))>, alone: bool) -> Vec<String> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let table = table();
    if table.is_none() && DUMP_TABLE.load(Ordering::Relaxed).is_null() {
        return Vec::new();
    }
    let _outside = super::replay::Outside::enter();
    let deadline = std::time::Instant::now() + DUMPING;
    let (mut lines, mut skipped) = (Vec::new(), 0);
    for _ in 0..BROKEN {
        let mut ends = [0; 2];
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            break;
        }
        match unsafe { libc::fork() } {
            -1 => break,
            0 => {
                unsafe { libc::close(ends[0]) };
                render(table.as_deref(), here, lines.len() + skipped, ends[1], alone);
                unsafe { libc::_exit(0) };
            }
            pid => {
                unsafe { libc::close(ends[1]) };
                unsafe { libc::fcntl(ends[0], libc::F_SETFL, libc::O_NONBLOCK) };
                let mut pipe = unsafe { std::fs::File::from_raw_fd(ends[0]) };
                let (mut text, mut part, mut ended) = (Vec::new(), [0u8; 8192], false);
                while !ended && std::time::Instant::now() < deadline {
                    match pipe.read(&mut part) {
                        Ok(0) => ended = true,
                        Ok(read) => text.extend_from_slice(&part[..read]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            let mut ready = libc::pollfd { fd: ends[0], events: libc::POLLIN, revents: 0 };
                            unsafe { libc::poll(&mut ready, 1, 50) };
                        }
                        Err(_) => ended = true,
                    }
                }
                let mut status = 0;
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                }
                // What came whole: a line cut short has no end.
                let end = text.iter().rposition(|&b| b == b'\n').map_or(0, |at| at + 1);
                let whole = text[..end].split(|&b| b == b'\n').filter(|line| !line.is_empty());
                lines.extend(whole.map(|line| String::from_utf8_lossy(line).into_owned()));
                // It ended by itself, or it stands at the binding that
                // broke it: on past that one.
                let done = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
                if done || !ended {
                    break;
                }
                skipped += 1;
            }
        }
    }
    lines
}

/// Where a task stands in a dump: its id, the code of a fault, its frame,
/// its stack, and for one switched out, its registers and where it was.
type TaskStop = (u64, Option<usize>, usize, (usize, usize), Option<super::unwinding::Switched>);

/// In the copy of the program, shows the bindings from the `from`th on,
/// a line each on `out`.
fn render(
    table: Option<&Table>,
    here: Option<(Option<usize>, usize, (usize, usize))>,
    from: usize,
    out: i32,
    alone: bool,
) {
    RENDERING.store(true, Ordering::Relaxed);
    // A value that breaks this copy only skips that binding: at once, as
    // a core takes seconds, and without a word on the program's stderr.
    extern "C" fn broke(_: i32) {
        unsafe { libc::_exit(1) };
    }
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = broke as *const () as usize;
        action.sa_flags = libc::SA_ONSTACK;
        for signal in [libc::SIGSEGV, libc::SIGBUS, libc::SIGABRT, libc::SIGILL, libc::SIGFPE] {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
        libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 });
        libc::dup2(libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY), 2);
    }
    let _quiet = super::heap::Quiet::enter();
    let mut stops: Vec<TaskStop> =
        here.into_iter().map(|(pc, fp, bounds)| (super::tasks::current_id(), pc, fp, bounds, None)).collect();
    if !alone {
        stops.extend(super::tasks::standing().into_iter().map(|(id, fp, bounds, saved)| (id, None, fp, bounds, saved)));
    }
    let mut index = 0;
    // A logpoint's condition reads all the frame has, so what it shows
    // waits for that.
    let condition = alone.then(|| CONDITION.lock().unwrap_or_else(|e| e.into_inner()).clone()).flatten();
    let mut waiting: Vec<(String, String, String)> = Vec::new();
    let mut show = |task: u64, frame: usize, place: &str, name: &str, ty: &str, glue: Show, address: usize| {
        index += 1;
        if index <= from {
            return;
        }
        let text = shown_here(glue, address);
        let line = format!(
            "{{\"task\":{task},\"frame\":{frame},\"at\":{},\"name\":{},\"type\":{},\"value\":{},\"parts\":{}}}\n",
            quoted(place),
            quoted(name),
            quoted(ty),
            quoted(&clipped(&text)),
            dumped_parts(&text, DUMPED_DEPTH),
        );
        match condition.is_some() {
            true => waiting.push((name.to_string(), text, line)),
            false => drop(unsafe { libc::write(out, line.as_ptr().cast(), line.len()) }),
        }
    };
    let release = table.is_none().then(dump_functions);
    for (task, pc, frame, bounds, switched) in stops {
        // Past the frames of the Rust library, which may have no pointers.
        let frame = if pc.is_none() { super::trace::through(frame, bounds) } else { frame };
        if let Some(table) = table {
            let stop = Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: String::new() };
            let mut stack = frames(table, &stop);
            stack.splice(0..0, pc.and_then(|pc| frame_at(table, pc, frame)));
            for at in 0..stack.len() {
                for (binding, address, _) in visible(table, &stack, at) {
                    show(task, at, &stack[at].place, &binding.name, &binding.ty, binding.show, address);
                }
            }
        } else if let Some(functions) = &release {
            let top = pc.map(|pc| (frame, pc + 1));
            let (mut at, mut registers): (_, Registers) = (0, [None; 32]);
            // A fault says all of them, for the frame it stopped.
            if pc.is_some() {
                registers = super::diagnostics::faulted_registers();
            }
            // One that switched out left them where it did, in the
            // runtime's frame that called the switch.
            if let Some((saved, address)) = switched {
                registers = super::unwinding::restored(address, frame, &saved, bounds);
            }
            let (kept_at, kept) = PANIC_REGISTERS.get();
            for (fp, address) in top.into_iter().chain(super::trace::callers(frame, bounds)) {
                // The runtime's frames saved what the program's code had.
                let Some((function, start)) = super::trace::function_at(address) else {
                    registers = super::unwinding::restored(address, fp, &registers, bounds);
                    continue;
                };
                if fp == kept_at {
                    for (&register, &value) in CALLEE_SAVED.iter().zip(&kept) {
                        registers[register] = Some(value);
                    }
                }
                let offset = (address - 1 - start) as u32;
                let found = functions.get(&function);
                // What a panic kept is in this frame, if it panicked.
                let numbers = PANIC_NUMBERS.get().addr();
                let numbers = (fp == kept_at && numbers != 0).then_some(numbers);
                let scopes = super::trace::scopes_at(address);
                for (record, place) in scopes.iter().filter(|(_, place)| !place.contains(" in spawn")) {
                    let line = place.split(" in ").next().and_then(|at| at.rsplit(':').next());
                    let line: u32 = line.and_then(|line| line.parse().ok()).unwrap_or(u32::MAX);
                    let bound = found.iter().flat_map(|f| &f.bindings);
                    for binding in bound.filter(|b| b.scope == *record && b.from <= line) {
                        let mut word = 0usize;
                        let kept = numbers.zip(binding.keep).and_then(|(numbers, index)| {
                            let bits = unsafe { *(numbers as *const u64) };
                            (bits & (1 << index) != 0).then_some(numbers + (index + 1) * 8)
                        });
                        if let Some(address) = kept.or_else(|| binding.found(fp, offset, &registers, &mut word)) {
                            show(task, at, place, &binding.name, &binding.ty, binding.show, address);
                        }
                    }
                    at += 1;
                }
                if alone {
                    break;
                }
                let saved = found.map_or(&[][..], |f| &f.saved);
                registers = caller_registers(&registers, fp, saved);
            }
        }
    }
    let Some(condition) = &condition else { return };
    let read = |name: &str| {
        let found = waiting.iter().find(|(binding, ..)| binding == name);
        found.map(|(_, text, _)| text.clone()).ok_or(format!("no binding `{name}` holds a value here"))
    };
    if expression::parse(condition).is_ok_and(|expr| Evaluator::new(&read).holds(&expr)) {
        for (.., line) in &waiting {
            unsafe { libc::write(out, line.as_ptr().cast(), line.len()) };
        }
    }
}

/// What a build without probes says of a binding, for its dumps.
struct DumpBinding {
    name: String,
    ty: String,
    show: Show,
    /// Where it is from the frame pointer, when it has a slot there.
    slot: Option<isize>,
    /// Behind what the ranges find, a pointer to it.
    pointer: bool,
    /// Over which code it is where.
    ranges: Vec<(u32, u32, Place)>,
    /// Its place among what an explicit panic keeps.
    keep: Option<usize>,
    /// The inlined call it is of, or `u32::MAX`.
    scope: u32,
    /// The first line that gives a slot its value: before it, what is there
    /// is not the binding's.
    from: u32,
}

/// Where a value is: at an offset from the canonical frame address, or in
/// a register, by its number.
#[derive(Clone, Copy)]
enum Place {
    Frame(isize),
    Register(usize),
    /// A register of numbers, which only a logpoint saves.
    Float(usize),
}

/// What a dump reads of a function of a build without probes.
#[derive(Default)]
struct DumpFunction {
    bindings: Vec<DumpBinding>,
    /// Where it saves its caller's registers, from the canonical frame
    /// address, by number.
    saved: Vec<(usize, isize)>,
}

/// The registers of a frame, those known.
use super::unwinding::Registers;

/// The callee-saved registers, as the ABI says.
#[cfg(target_arch = "x86_64")]
const CALLEE_SAVED: &[usize] = &[3, 12, 13, 14, 15];
#[cfg(target_arch = "aarch64")]
const CALLEE_SAVED: &[usize] = &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28];

thread_local! {
    /// The frame pointer and callee-saved registers before a panic.
    static PANIC_REGISTERS: std::cell::Cell<(usize, [usize; 10])> = const { std::cell::Cell::new((0, [0; 10])) };
}

thread_local! {
    /// What an explicit panic kept: the bits of those it holds, then each.
    static PANIC_NUMBERS: std::cell::Cell<*const u64> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// Called before a panic the program wrote, with what it keeps.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_keep_numbers(kept: *const u64) {
    PANIC_NUMBERS.set(kept);
}

/// Called by what keeps the registers: the frame pointer of the code that
/// panics, then the callee-saved registers.
extern "C" fn registers_kept(registers: *const usize) {
    let mut kept = [0; 10];
    let fp = unsafe { *registers };
    for (index, slot) in kept.iter_mut().enumerate().take(CALLEE_SAVED.len()) {
        *slot = unsafe { *registers.add(index + 1) };
    }
    PANIC_REGISTERS.set((fp, kept));
}

/// Keeps the registers of the code that is about to panic, which the
/// runtime's own frames would overwrite on the way to a dump.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_keep_registers() {
    std::arch::naked_asm!(
        "sub rsp, 56",
        "mov [rsp], rbp",
        "mov [rsp + 8], rbx",
        "mov [rsp + 16], r12",
        "mov [rsp + 24], r13",
        "mov [rsp + 32], r14",
        "mov [rsp + 40], r15",
        "mov rdi, rsp",
        "call {kept}",
        "add rsp, 56",
        "ret",
        kept = sym registers_kept,
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_keep_registers() {
    std::arch::naked_asm!(
        "sub sp, sp, #112",
        "stp x29, x19, [sp]",
        "stp x20, x21, [sp, #16]",
        "stp x22, x23, [sp, #32]",
        "stp x24, x25, [sp, #48]",
        "stp x26, x27, [sp, #64]",
        "stp x28, x30, [sp, #80]",
        "mov x0, sp",
        "bl {kept}",
        "ldr x30, [sp, #88]",
        "add sp, sp, #112",
        "ret",
        kept = sym registers_kept,
    )
}

impl DumpBinding {
    /// Where the binding is at a place of the code of the frame at `fp`:
    /// in memory, or a copy of a register in `word`.
    fn found(&self, fp: usize, offset: u32, registers: &Registers, word: &mut usize) -> Option<usize> {
        if let Some(slot) = self.slot {
            return Some((fp as isize + slot) as usize);
        }
        let &(.., place) = self.ranges.iter().find(|&&(start, end, _)| start <= offset && offset < end)?;
        let value = match place {
            Place::Frame(at) => {
                let address = (fp as isize + 16 + at) as usize;
                if !self.pointer {
                    return Some(address);
                }
                peek(address, 8).try_into().ok().map(usize::from_ne_bytes)?
            }
            Place::Register(number) => (*registers.get(number)?)?,
            Place::Float(number) => {
                let known = LOGGING.load(Ordering::Relaxed);
                known.then(|| FLOATS.get(number).map(|float| float.load(Ordering::Relaxed)))??
            }
        };
        if self.pointer {
            return Some(value);
        }
        *word = value;
        Some(word as *mut usize as usize)
    }
}

/// The registers of the caller of the frame at `fp`, of a function that
/// saves these.
fn caller_registers(registers: &Registers, fp: usize, saved: &[(usize, isize)]) -> Registers {
    let mut caller: Registers = [None; 32];
    for &register in CALLEE_SAVED {
        caller[register] = registers[register];
    }
    for &(register, at) in saved {
        let word = peek((fp as isize + 16 + at) as usize, 8).try_into().ok().map(usize::from_ne_bytes);
        if let Some(slot) = caller.get_mut(register) {
            *slot = word;
        }
    }
    caller
}

/// The table of what the dumps of a build without probes show.
static DUMP_TABLE: AtomicPtr<Raw> = AtomicPtr::new(std::ptr::null_mut());

/// Called by a program without probes before anything else.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_dump_table(raw: *mut Raw) {
    DUMP_TABLE.store(raw, Ordering::Relaxed);
}

/// The bindings of each function, by its number, as `tondo_rt_dump_table`
/// has them, which only a dump reads.
fn dump_functions() -> std::collections::HashMap<usize, DumpFunction> {
    let mut functions: std::collections::HashMap<usize, DumpFunction> = Default::default();
    let raw = DUMP_TABLE.load(Ordering::Relaxed);
    if raw.is_null() {
        return functions;
    }
    let raw = unsafe { &*raw };
    let text = String::from_utf8_lossy(unsafe { str_bytes(raw.text) }).into_owned();
    let glues = unsafe { std::slice::from_raw_parts(raw.glues, raw.count as usize) };
    let (mut current, mut next) = (0, 0);
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let function = functions.entry(current).or_default();
        match fields[..] {
            ["F", index] => current = index.parse().unwrap_or(0),
            ["C", register, at] => {
                if let (Ok(register), Ok(at)) = (register.parse(), at.parse()) {
                    function.saved.push((register, at));
                }
            }
            ["B", name, ty, kind, value, keep, scope, from, _] => {
                let (name, ty) = (name.to_string(), ty.to_string());
                let show = glues[next];
                next += 1;
                let value: isize = value.parse().unwrap_or(0);
                let (slot, pointer) = if kind == "s" { (Some(value), false) } else { (None, value == 1) };
                let keep = keep.parse().ok();
                let scope = scope.parse().unwrap_or(u32::MAX);
                let from = from.parse().unwrap_or(0);
                let ranges = Vec::new();
                function.bindings.push(DumpBinding { name, ty, show, slot, pointer, ranges, keep, scope, from });
            }
            ["R", start, end, kind, at] => {
                let place = match kind {
                    "f" => at.parse().ok().map(Place::Frame),
                    "x" => at.parse().ok().map(Place::Float),
                    _ => at.parse().ok().map(Place::Register),
                };
                if let (Some(binding), Ok(start), Ok(end), Some(place)) =
                    (function.bindings.last_mut(), start.parse(), end.parse(), place)
                {
                    binding.ranges.push((start, end, place));
                }
            }
            _ => {}
        }
    }
    functions
}

/// A checkpoint at a probe of a replay that runs, when one is due: the
/// debugger's state and connection held across the fork, so that the copy
/// has none another thread held; the debugger hears where it is. A replay
/// forked from it later stops at that probe.
fn periodic(table: &Table, mut here: Stop, every: u64) -> Option<Stop> {
    if super::replay::threaded() {
        return None;
    }
    let (now, tick) = (std::time::Instant::now(), super::replay::ticks());
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let due = |(at, probe): (std::time::Instant, u64)| {
            now.duration_since(at).as_millis() >= every as u128 || tick - probe >= PROBES
        };
        if last.is_some_and(|last| !due(last)) {
            return None;
        }
        *last = Some((now, tick));
    }
    let busy = idle_debugger()?;
    let state = state();
    let client = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    // A debugger that left would never hear of it.
    if client.is_none() {
        return None;
    }
    match super::tasks::fork_copy() {
        0 => {
            drop((busy, client, state));
            kept(table);
            here.bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
            Some(here)
        }
        -1 => None,
        pid => {
            drop((busy, client, state));
            tell(&format!("{{\"checkpointed\":{{\"pid\":{pid},\"tick\":{}}}}}", super::replay::ticks()));
            None
        }
    }
}

/// While the debugger's thread runs a command it may hold the table, which
/// a copy could never have back: a copy waits for one that does not run,
/// and a command for a copy that is being taken. Not a lock: the copy that
/// a command itself asks for has to be able to clear it.
static COMMANDING: AtomicBool = AtomicBool::new(false);
static COPYING: AtomicBool = AtomicBool::new(false);

struct Commanding;

impl Commanding {
    fn enter() -> Commanding {
        COMMANDING.store(true, Ordering::SeqCst);
        while COPYING.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        Commanding
    }
}

impl Drop for Commanding {
    fn drop(&mut self) {
        COMMANDING.store(false, Ordering::SeqCst);
    }
}

struct Copying;

impl Drop for Copying {
    fn drop(&mut self) {
        COPYING.store(false, Ordering::SeqCst);
    }
}

fn idle_debugger() -> Option<Copying> {
    COPYING.store(true, Ordering::SeqCst);
    let copying = Copying;
    (!COMMANDING.load(Ordering::SeqCst)).then_some(copying)
}

/// Stands a copy at its checkpoint, without the table its caller read: a
/// patch may come, which needs it. Gives it back.
fn at_checkpoint(
    table: std::sync::RwLockReadGuard<'static, Table>,
    stop: Stop,
) -> Option<std::sync::RwLockReadGuard<'static, Table>> {
    drop(table);
    hold(state(), stop, Some(CHECKPOINT.into()));
    self::table()
}

/// Why a replay forked from a checkpoint is stopped.
const CHECKPOINT: &str = "checkpoint";

/// A copy of a run being recorded, at a probe: as a checkpoint, held until
/// a replay goes on from here, of what was recorded after it.
fn copied(site: usize, frame: usize, addresses: usize, alive: usize) -> Option<Stop> {
    let table = table()?;
    let _outside = super::replay::Outside::enter();
    let task = super::tasks::current_id();
    let busy = idle_debugger()?;
    let state = state();
    let client = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let mut parent = Some((busy, client, state));
    let mut here = None;
    super::replay::copy(|segment| {
        drop(parent.take());
        let debugger = kept(&table);
        super::replay::replay_from(segment, debugger);
        let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
        here = Some(Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
    });
    here
}

/// A checkpoint, until a replay forked from it goes on from here, which
/// returns. Only this thread is left: the debugger's connection and its
/// settings were the parent's. Gives the process of the debugger that
/// asked.
fn kept(table: &Table) -> i32 {
    use std::io::Write;
    COMMANDING.store(false, Ordering::SeqCst);
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tools::listen(tools::DEBUGGER, false);
    {
        let mut state = state();
        state.breakpoints.clear();
        state.watches.clear();
        state.next_breakpoint = 1;
        remark(table, &state);
    }
    WATCHING.store(false, Ordering::Relaxed);
    EVERY.store(0, Ordering::Relaxed);
    RECORDING.store(0, Ordering::Relaxed);
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clear();
    CATCH.store(true, Ordering::Relaxed);
    CATCH_ERRORS.store(false, Ordering::Relaxed);
    STEPS_STD.store(false, Ordering::Relaxed);
    // The replays it forks end without it waiting for them. What it has
    // open but its standard streams is its parent's: the sockets of the
    // tools, which would seem to listen once the parent ended.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
    let open = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }.clamp(1024, 1 << 16) as i32;
    for fd in 3..open {
        unsafe { libc::close(fd) };
    }
    // Nor does it keep its parent's output open: each replay it forks
    // writes where the one that asks for it says.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        libc::dup2(null, 0);
        libc::dup2(null, 1);
        libc::dup2(null, 2);
        libc::close(null);
    }
    let path = super::diagnostics::sockets().join(format!("{}.checkpoint", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = std::os::unix::net::UnixListener::bind(&path) else { unsafe { libc::_exit(0) } };
    // One the debugger never heard of, or forgot, ends after an hour
    // unused, or what `--hold` says; any, once the debugger is gone.
    let mut idle = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&listener), events: libc::POLLIN, revents: 0 };
    let mut unused = std::time::Duration::ZERO;
    loop {
        match unsafe { libc::poll(&mut idle, 1, 10_000) } {
            0 => {
                unused += std::time::Duration::from_secs(10);
                if unused >= super::replay::hold_for() || super::replay::orphaned() {
                    let _ = std::fs::remove_file(&path);
                    unsafe { libc::_exit(0) };
                }
                continue;
            }
            // A signal came.
            ..0 => continue,
            _ => unused = std::time::Duration::ZERO,
        }
        let Ok((mut stream, _)) = listener.accept() else { continue };
        let (line, given) = received(&stream);
        let asked = line.trim().strip_prefix("resume ").and_then(|asked| asked.split_once(' '));
        let Some((shown, debugger)) = asked.and_then(|(s, d)| Some((s.parse::<u64>().ok()?, d.parse::<i32>().ok()?)))
        else {
            let _ = std::fs::remove_file(&path);
            unsafe { libc::_exit(0) };
        };
        match unsafe { libc::fork() } {
            0 => {
                drop(listener);
                unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) };
                // Its output, where the one that asked for it said.
                if let [out, error] = given[..] {
                    unsafe {
                        libc::dup2(out, 1);
                        libc::dup2(error, 2);
                    }
                }
                given.iter().for_each(|&fd| unsafe {
                    libc::close(fd);
                });
                super::replay::shown_from(shown);
                return debugger;
            }
            pid => {
                given.iter().for_each(|&fd| unsafe {
                    libc::close(fd);
                });
                let _ = writeln!(stream, "{pid}");
            }
        }
    }
}

/// A line a checkpoint was sent, with the descriptors that came with it.
fn received(stream: &std::os::unix::net::UnixStream) -> (String, Vec<i32>) {
    let fd = std::os::fd::AsRawFd::as_raw_fd(stream);
    let mut data = [0u8; 256];
    let mut control = [0u8; 64];
    let mut part = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let read = unsafe { libc::recvmsg(fd, &mut message, 0) };
    let mut given = Vec::new();
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while let Some(found) = header.as_ref() {
            if found.cmsg_level == libc::SOL_SOCKET && found.cmsg_type == libc::SCM_RIGHTS {
                // musl's length is a `u32`.
                #[allow(clippy::unnecessary_cast)]
                let count = (found.cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<i32>();
                let fds = libc::CMSG_DATA(header).cast::<i32>();
                given.extend((0..count).map(|i| fds.add(i).read_unaligned()));
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    (String::from_utf8_lossy(&data[..read.max(0) as usize]).into_owned(), given)
}

/// What a scan follows as it goes (`track`, `count`): expressions, each in
/// a frame, whose values it keeps when they change, each change with the
/// probe and the line before it, which gave it, whatever task ran it; and
/// for one line, each time a task starts it, with an expression's value
/// then, the last ones.
#[derive(Default)]
struct Tracked {
    followed: Vec<Followed>,
    before: Option<(u64, usize)>,
    line: Option<Counted>,
    /// Each task's last line of the program's code, by probe and site.
    lines: std::collections::BTreeMap<u64, (u64, usize)>,
    /// Where each value a channel holds was sent from, the oldest first:
    /// a line and its task; `None` for one sent before the scan began.
    messages: std::collections::BTreeMap<usize, std::collections::VecDeque<Option<Origin>>>,
    /// The line each task ended on, by its join.
    ends: std::collections::BTreeMap<usize, Origin>,
    /// What a line received, by its probe and site: where it was sent from,
    /// or where the task it waited for ended; the last ones.
    received: std::collections::VecDeque<((u64, usize), Origin)>,
    /// Where each line of the program's code that runs goes: its task, its
    /// probe and its site, which another run of the program compares.
    trail: Option<std::io::BufWriter<std::fs::File>>,
    /// What went wrong writing it, which the scan says when it ends.
    trail_failed: Option<String>,
}

/// A line of a task: its probe, its site and the task.
type Origin = (u64, usize, u64);

/// What a scan keeps of a value that changes hands: `from` is the task
/// whose line it comes from, `to` the one whose line takes it; `None` when
/// what was sent before the scan began.
fn handed_over(from: impl FnOnce(&mut Tracked) -> Option<Origin>, to: u64) {
    // The scan's own work, which the recording does not have.
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let (Some(origin), Some(&line)) = (from(tracked), tracked.lines.get(&to)) else { return };
    if tracked.received.len() == LISTED {
        tracked.received.pop_front();
    }
    tracked.received.push_back((line, origin));
}

fn origin_of(tracked: &Tracked, task: u64) -> Option<Origin> {
    tracked.lines.get(&task).map(|&(tick, site)| (tick, site, task))
}

/// The task `from` sent a value into the channel, which held `held`.
pub(crate) fn sent(channel: usize, held: usize, from: u64) {
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let origin = origin_of(tracked, from);
    tracked.messages.entry(channel).or_insert_with(|| vec![None; held].into()).push_back(origin);
}

/// A task ended, and its stack goes: a scan follows its frames no longer.
pub(crate) fn task_ended(task: u64) {
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    for followed in tracked.iter_mut().flat_map(|t| &mut t.followed).filter(|f| f.anchor.task == task) {
        followed.anchor.live = None;
    }
}

/// The task `by` took the oldest value of the channel, which held `held`.
pub(crate) fn taken(channel: usize, held: usize, by: u64) {
    let take = |t: &mut Tracked| t.messages.entry(channel).or_insert_with(|| vec![None; held].into()).pop_front()?;
    handed_over(take, by);
}

/// The task `from` gave a value to `to`, which waited for it.
pub(crate) fn handed(from: u64, to: u64) {
    handed_over(|t| origin_of(t, from), to);
}

/// The current task ends, its result in `join`.
pub(crate) fn ended(join: usize) {
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tracked) = tracked.as_mut()
        && let Some(origin) = origin_of(tracked, super::tasks::current_id())
    {
        tracked.ends.insert(join, origin);
    }
}

/// The current task takes the result in `join`.
pub(crate) fn joined(join: usize) {
    handed_over(|t| t.ends.get(&join).copied(), super::tasks::current_id());
}

/// A frame as another replay of the same run finds it, where addresses
/// may differ: its task, how far its frame is from the top of the task's
/// stack, and its function; and where it is while it lives.
struct Anchor {
    task: u64,
    depth: usize,
    function: usize,
    live: Option<usize>,
}

impl Anchor {
    fn parse(text: &str) -> Option<Anchor> {
        let mut parts = text.split(':').map(str::parse::<u64>);
        let (Some(Ok(task)), Some(Ok(depth)), Some(Ok(function)), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Anchor { task, depth: depth as usize, function: function as usize, live: None })
    }
}

/// `anchor [<expression>]`: the frame `locals` looks at, as `track` takes
/// it; or the one further down the stack that has the binding the
/// expression reads, as a closure's maker.
fn anchor(table: &Table, text: &str) -> Result<String, String> {
    let (frames, mut index) = selected(table)?;
    if !text.is_empty() {
        let expr = expression::parse(text).map_err(|m| format!("the expression: {m}"))?;
        let mut read = Vec::new();
        expression::names(&expr, &mut read);
        let name = |frame: &Frame| frame.function.map(|f| table.functions[f].name.as_str());
        let has = |frame: &Frame| read.first().is_some_and(|r| bound(table, frame).iter().any(|(b, _)| &b.name == r));
        // As `visible` finds it.
        while !has(&frames[index]) {
            let Some((outer, prefix)) = name(&frames[index]).and_then(written_in) else { break };
            let below = (index + 1..frames.len()).find(|&below| {
                name(&frames[below]).is_some_and(|n| if prefix { n.starts_with(outer) } else { n == outer })
            });
            let Some(below) = below else { break };
            index = below;
        }
    }
    let state = state();
    let task = state.selected.map_or(state.stopped.as_ref().map_or(0, |s| s.task), |(task, _)| task);
    let high = stop_of(&state, task).map_or(0, |held| held.bounds.1);
    let frame = &frames[index];
    match (frame.function, high.checked_sub(frame.pointer)) {
        (Some(function), Some(depth)) if high != 0 => Ok(format!("{{\"anchor\":\"{task}:{depth}:{function}\"}}")),
        _ => Err("this frame is not one of the program's that a replay finds again".into()),
    }
}

/// An expression a scan follows in a frame: its text, its last value, and
/// its last changes, each with the probe and the site of the line that
/// gave it.
struct Followed {
    anchor: Anchor,
    expr: Expr,
    text: String,
    last: Option<String>,
    changes: std::collections::VecDeque<(Option<(u64, usize)>, String)>,
    /// The first change to each value, as the first time a condition held,
    /// which the last changes may no longer have.
    firsts: Vec<(Option<(u64, usize)>, String)>,
}

/// A line a scan counts: its sites, the expression it shows, and its last
/// runs, each with the probe, the task and the value.
struct Counted {
    sites: Vec<usize>,
    expr: Option<Expr>,
    runs: std::collections::VecDeque<(u64, u64, Option<String>)>,
    times: u64,
}

static TRACKED: Mutex<Option<Tracked>> = Mutex::new(None);

/// At a probe of a scan, what it follows.
fn tracking(table: &Table, here: Stop, tick: u64) {
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let (task, site) = (here.task, here.site);
    // The innermost frame alone, which is what a line's expression reads:
    // not the callers, whose names are looked up.
    let function = table.sites.get(site).map(|s| s.function);
    if let Some(line) = tracked.line.as_mut()
        && line.sites.contains(&site)
    {
        let innermost = [Frame {
            place: String::new(),
            function,
            pointer: here.frame,
            addresses: here.addresses,
            alive: here.alive,
            pc: 0,
        }];
        let value =
            line.expr.as_ref().and_then(|e| evaluating(table, &innermost, 0, |ev| ev.evaluate(e)).ok().map(|v| v.text));
        line.times += 1;
        if line.runs.len() == LISTED {
            line.runs.pop_front();
        }
        line.runs.push_back((tick, task, value));
    }
    // A followed frame lives from a probe of its own until its task runs
    // in a caller, or another call takes its place.
    let mut depth = None;
    for anchor in tracked.followed.iter_mut().map(|f| &mut f.anchor).filter(|a| a.task == task) {
        let depth = *depth
            .get_or_insert_with(|| super::tasks::stack_bounds().map_or(0, |(_, high)| high).wrapping_sub(here.frame));
        if depth == anchor.depth && function == Some(anchor.function) {
            anchor.live = Some(here.frame);
        } else if depth <= anchor.depth {
            anchor.live = None;
        }
    }
    // A value changes between two lines of the program's code, of any task:
    // the first of them gave it, or a call it made.
    if table.sites.get(site).is_none_or(Site::generated) {
        return;
    }
    if let Some(trail) = tracked.trail.as_mut() {
        let _outside = super::replay::Outside::enter();
        let line = [task, tick, site as u64].map(u64::to_le_bytes).concat();
        if let Err(wrong) = trail.write_all(&line) {
            tracked.trail_failed = Some(wrong.to_string());
        }
    }
    let before = tracked.before.replace((tick, site));
    tracked.lines.insert(task, (tick, site));
    for followed in tracked.followed.iter_mut() {
        let anchor = &followed.anchor;
        let Some(pointer) = anchor.live else { continue };
        let Some((addressed, living)) = table.offsets.get(anchor.function) else { continue };
        if addressed.load(Ordering::Relaxed) == UNKNOWN {
            continue;
        }
        let at = |offset: &AtomicIsize| (pointer as isize + offset.load(Ordering::Relaxed)) as usize;
        let frame = [Frame {
            place: String::new(),
            function: Some(anchor.function),
            pointer,
            addresses: at(addressed),
            alive: at(living),
            pc: 0,
        }];
        let now = evaluating(table, &frame, 0, |ev| ev.evaluate(&followed.expr)).ok().map(|v| v.text);
        if let Some(value) = now.as_ref().filter(|now| Some(*now) != followed.last.as_ref()) {
            if followed.changes.len() == LISTED {
                followed.changes.pop_front();
            }
            followed.changes.push_back((before, value.clone()));
            if followed.firsts.len() < LISTED && !followed.firsts.iter().any(|(_, v)| v == value) {
                followed.firsts.push((before, value.clone()));
            }
        }
        followed.last = now;
    }
}

/// `track <frame> <expression>`, `count <file>:<line> [<expression>]` and
/// `tracked`: what the next scan follows, and what it found.
fn track(table: &Table, word: &str, rest: &str) -> Result<String, String> {
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    match word {
        // Where the values tasks receive come from, as the next scan goes.
        "track" if rest.trim().is_empty() => {
            tools::listen(tools::FOLLOWING, true);
            tracked.get_or_insert_with(Tracked::default);
            Ok("{\"tracking\":true}".into())
        }
        "track" => {
            let (anchor, text) =
                rest.split_once(' ').ok_or("track takes a frame, as `anchor` gives it, and an expression")?;
            let anchor = Anchor::parse(anchor).ok_or("track takes a frame first, as `anchor` gives it")?;
            let expr = expression::parse(text.trim()).map_err(|m| format!("the expression: {m}"))?;
            tools::listen(tools::FOLLOWING, true);
            tracked.get_or_insert_with(Tracked::default).followed.push(Followed {
                anchor,
                expr,
                text: text.trim().to_string(),
                last: None,
                changes: Default::default(),
                firsts: Vec::new(),
            });
            Ok("{\"tracking\":true}".into())
        }
        "trail" => {
            let _outside = super::replay::Outside::enter();
            let file = std::fs::File::create(rest).map_err(|e| format!("cannot write {rest}: {e}"))?;
            tracked.get_or_insert_with(Tracked::default).trail = Some(std::io::BufWriter::new(file));
            Ok("{\"trailing\":true}".into())
        }
        "count" => {
            let (at, text) = rest.split_once(' ').map_or((rest, None), |(at, text)| (at, Some(text.trim())));
            let sites = line_sites(table, at)?;
            let expr = text.map(expression::parse).transpose().map_err(|m| format!("the expression: {m}"))?;
            tracked.get_or_insert_with(Tracked::default).line =
                Some(Counted { sites, expr, runs: Default::default(), times: 0 });
            Ok("{\"counting\":true}".into())
        }
        _ => {
            tools::listen(tools::FOLLOWING, false);
            let mut found = tracked.take().unwrap_or_default();
            if let Some(mut trail) = found.trail.take() {
                let _outside = super::replay::Outside::enter();
                std::io::Write::flush(&mut trail).map_err(|e| format!("cannot write the lines of the run: {e}"))?;
                if let Some(wrong) = found.trail_failed.take() {
                    return Err(format!("cannot write the lines of the run: {wrong}"));
                }
            }
            let received: Vec<String> = found
                .received
                .iter()
                .map(|&((line, here), (tick, site, task))| {
                    let (here, at) = (quoted(&place_in(table, here)), quoted(&place_in(table, site)));
                    format!(
                        "{{\"tick\":{line},\"at\":{here},\"from\":{{\"tick\":{tick},\"at\":{at},\"task\":{task}}}}}"
                    )
                })
                .collect();
            let followed: Vec<String> = found
                .followed
                .iter()
                .map(|Followed { text, changes, firsts, .. }| {
                    let shown = |(before, value): &(Option<(u64, usize)>, String)| {
                        let (at, tick) = match before {
                            Some((tick, site)) => (quoted(&place_in(table, *site)), tick.to_string()),
                            None => ("null".into(), "null".into()),
                        };
                        format!("{{\"at\":{at},\"tick\":{tick},\"value\":{}}}", quoted(&clipped(value)))
                    };
                    let changes: Vec<String> = changes.iter().map(shown).collect();
                    let firsts: Vec<String> = firsts.iter().map(shown).collect();
                    format!(
                        "{{\"expression\":{},\"changes\":[{}],\"firsts\":[{}]}}",
                        quoted(text),
                        changes.join(","),
                        firsts.join(",")
                    )
                })
                .collect();
            let line = found.line.map_or(String::new(), |Counted { sites, runs, times, .. }| {
                let runs: Vec<String> = runs
                    .iter()
                    .map(|(tick, task, value)| {
                        let value =
                            value.as_ref().map_or(String::new(), |v| format!(",\"value\":{}", quoted(&clipped(v))));
                        format!("{{\"task\":{task},\"tick\":{tick}{value}}}")
                    })
                    .collect();
                format!(",\"ran\":[{}],\"times\":{times},\"at\":{}", runs.join(","), quoted(&place_in(table, sites[0])))
            });
            Ok(format!("{{\"tracked\":[{}]{line},\"received\":[{}]}}", followed.join(","), received.join(",")))
        }
    }
}

/// What a scan keeps at a probe: the tick, when the scanned task's line
/// starts there or a breakpoint that stops would stop there.
fn scanned(
    table: &Table,
    state: &mut State,
    task: u64,
    site: usize,
    frame: usize,
    (addresses, alive): (usize, usize),
    tick: u64,
) {
    let mut scan = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
    if scan.0 == task && table.sites.get(site).is_some_and(|s| !s.generated()) {
        kept_last(&mut scan.2, tick);
    }
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let here = Stop { task, site, frame, bounds, addresses, alive, why: String::new() };
    let stack = std::cell::OnceCell::new();
    let stack = || stack.get_or_init(|| frames(table, &here));
    for breakpoint in state.breakpoints.iter_mut().filter(|b| b.log.is_none() && !b.capture && b.event.is_none()) {
        let here = breakpoint.sites.contains(&site)
            && breakpoint.task.is_none_or(|t| t == task)
            && in_request(&breakpoint.request, task)
            && breakpoint.condition.as_ref().is_none_or(|c| evaluating(table, stack(), 0, |e| e.holds(c)));
        if here {
            breakpoint.count += 1;
            if breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                kept_last(&mut scan.1, tick);
            }
        }
    }
}

/// At a probe of a task, its watches: one whose frame returned ends, and
/// the first whose value changed gives why the task stops: its expression,
/// its values, and the line that changed it.
fn watched(table: &Table, state: &mut State, task: u64, frame: usize, site: usize) -> Option<String> {
    let mut changed = None;
    let mut ended = Vec::new();
    for watch in state.watches.iter_mut() {
        if watch.shared.is_none() {
            if watch.task != task {
                continue;
            }
            // The frame returned when the task runs in an outer one, or
            // another call took its place.
            let watched = &watch.frames[0];
            let function = table.sites.get(site).map(|s| s.function);
            if frame > watched.pointer || (frame == watched.pointer && function != watched.function) {
                tell(&format!("{{\"unwatched\":{},\"why\":\"its frame returned\"}}", watch.id));
                ended.push(watch.id);
                continue;
            }
        }
        // A cell's change stops the task that made it, at its next probe,
        // not another that runs meanwhile.
        if let Some((cell, ..)) = watch.shared
            && ![0, task].contains(&super::sync::last_writer(cell))
        {
            continue;
        }
        if let Some((now, holds)) = watch.now(table).filter(|(now, _)| *now != watch.value) {
            let (before, after) = (clipped(&watch.value), clipped(&now));
            watch.value = now;
            watch.count += u64::from(holds);
            if holds && watch.hits.as_ref().is_none_or(|hits| hits.reached(watch.count)) {
                // A shared value changes in some task: the line is that of
                // the frame's own changes only.
                let at = match watch.shared {
                    None => format!(" at {}", place_in(table, watch.last)),
                    Some(_) => String::new(),
                };
                changed
                    .get_or_insert(format!("watch {}: {} changed from {before} to {after}{at}", watch.id, watch.text));
            }
        }
        watch.last = site;
    }
    unwatch(state, |watch| ended.contains(&watch.id));
    changed
}

/// Holds the task until the program goes on: as the one that stopped,
/// `why`, or as one of those the stop holds.
fn hold(mut state: std::sync::MutexGuard<'static, State>, mut stop: Stop, why: Option<String>) {
    // What the debugger has the task do meanwhile is not the program's.
    let _outside = super::replay::Outside::enter();
    let task = stop.task;
    let frozen = |state: &State| state.frozen.contains(&task);
    // Where its code is: the call of the probe that stopped it.
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let code = super::trace::callers(super::trace::frame_pointer(), bounds).find(|&(fp, _)| fp == stop.frame);
    state.code.retain(|&(held, _)| held != task);
    state.code.extend(code.map(|(_, address)| (task, address)));
    if let Some(why) = why {
        stop.why = why;
        tell(&stopped_event(&stop));
        state.stopped = Some(stop.clone());
        state.mode = Mode::Run;
        HOLD.store(true, Ordering::Relaxed);
        // A replay forked from a checkpoint listens once it is there, so
        // that the debugger finds it stopped.
        if stop.why == CHECKPOINT {
            super::diagnostics::listen_again();
        }
    }
    state.held.push(stop);
    // Out of Tondo code while it waits, so collections and dumps go on.
    let paused = super::tasks::pause_here();
    let resumed = state.resumed;
    let mut aside = false;
    // A frozen task stays while the others go on. What the debugger's thread
    // asks is looked at before each wait, with `state`, which it holds to ask:
    // the next request may come while the last one ends.
    while state.resumed == resumed || frozen(&state) {
        // The others go on: another worker takes this one's place. Only then,
        // since every task a stop holds would ask for one.
        if state.resumed != resumed && !aside {
            super::tasks::step_aside();
            aside = true;
        }
        // Work the debugger's thread asks of this task, as showing a value
        // through a view.
        let work = WORK.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(t, _)| *t == task);
        if let Some((_, work)) = work {
            drop(state);
            if paused {
                super::tasks::enter_tondo();
            }
            work();
            if paused {
                super::tasks::pause_here();
            }
            *WORKED.lock().unwrap_or_else(|e| e.into_inner()) = true;
            WORK_DONE.notify_all();
            state = self::state();
            continue;
        }
        // A call the debugger asks this task to make, on its own stack.
        let asked = CALL.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(t, ..)| *t == task);
        if let Some((_, code, addresses)) = asked {
            drop(state);
            if paused {
                super::tasks::enter_tondo();
            }
            let call: extern "C" fn(u64, u64) -> *mut Str = unsafe { std::mem::transmute(code as usize) };
            CALLING.store(task, Ordering::Relaxed);
            let made = call(addresses.as_ptr() as u64, 0);
            CALLING.store(0, Ordering::Relaxed);
            let text = (!made.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(made) }).into_owned());
            unsafe { super::tondo_rt_str_release(made) };
            if paused {
                super::tasks::pause_here();
            }
            *CALLED.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(text.ok_or_else(|| "the call panicked".to_string()));
            CALLED_DONE.notify_all();
            state = self::state();
            continue;
        }
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    state.held.retain(|held| held.task != task);
    drop(state);
    if aside {
        super::tasks::step_back();
    }
    if paused {
        super::tasks::enter_tondo();
    }
}

/// Whether a panic stops the program while a debugger is attached
/// (`catch panic`, the default, or `catch none`), and whether an error does
/// (`catch error`).
static CATCH: AtomicBool = AtomicBool::new(true);
static CATCH_ERRORS: AtomicBool = AtomicBool::new(false);
/// The panics `catch panic` names, by code or name, and the errors `catch
/// error` names, by type; none names, all.
static CAUGHT: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));

/// Whether a panic's headline, `panic[P0001]: bounds: ...`, is of a code
/// or name `catch` named.
fn caught_panic(headline: &str) -> bool {
    let wanted = &CAUGHT.lock().unwrap_or_else(|e| e.into_inner()).0;
    let Some((code, rest)) = headline.strip_prefix("panic[").and_then(|h| h.split_once("]: ")) else {
        return wanted.is_empty();
    };
    let name = rest.split(':').next().unwrap_or_default();
    wanted.is_empty() || wanted.iter().any(|w| w == code || w == name)
}

/// Whether an error of the type `ty`, shown as `shown`, is of one `catch`
/// named: by the type's name, with its module or without, or for a union,
/// by the member it is.
fn caught_error(ty: &str, shown: &str) -> bool {
    let wanted = &CAUGHT.lock().unwrap_or_else(|e| e.into_inner()).1;
    let ty = match ty.contains(" | ") {
        true => shown.split_once(": ").map_or(ty, |(member, _)| member),
        false => ty,
    };
    let own = ty.rsplit('.').next().unwrap_or(ty);
    wanted.is_empty() || wanted.iter().any(|w| w == ty || w == own)
}

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
    ty: *const Str,
) {
    if !attached() || !CATCH_ERRORS.load(Ordering::Relaxed) {
        return;
    }
    if calling_here() {
        return;
    }
    let shown = show(glue, error as usize);
    if !caught_error(&String::from_utf8_lossy(unsafe { str_bytes(ty) }), &shown) {
        return;
    }
    let frame = unsafe { *(super::trace::frame_pointer() as *const usize) };
    let task = super::tasks::current_id();
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let (addresses, alive) = (addresses as usize, alive as usize);
    let stop = Stop { task, site: site as usize, frame, bounds, addresses, alive, why: String::new() };
    let why = format!("error: {}", clipped(&shown));
    hold(state(), stop, Some(why));
}

/// A task panics: with a debugger attached that catches panics, it stops
/// where the program's own code panicked, before it unwinds, the panic's
/// text as why, and the others at their next probe.
pub(crate) fn panicked(message: &str) {
    if calling_here() {
        return;
    }
    if !attached() || !CATCH.load(Ordering::Relaxed) || !caught_panic(message) {
        return;
    }
    stop_here(message.to_string());
}

/// A panic a `recover` will stop, which ends nothing: a breakpoint on that
/// stops where the program's code panicked.
pub(crate) fn recovering(message: &str) {
    if attached() && EVENTS.load(Ordering::Relaxed) & ON_RECOVER != 0 && !calling_here() {
        stop_here(message.to_string());
    }
}

/// The events breakpoints stop at (`break on ...`), a bit each, and the
/// least wait in milliseconds that stops: a task's start, a channel's send
/// or receive, a long wait.
static EVENTS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static LONGEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
const ON_SPAWN: u8 = 1;
const ON_SEND: u8 = 2;
const ON_RECEIVE: u8 = 4;
const ON_WAIT: u8 = 8;
const ON_LOCK: u8 = 16;
const ON_REQUEST: u8 = 32;
const ON_RECOVER: u8 = 64;

/// An event a breakpoint stops at.
#[derive(Clone, Copy, PartialEq)]
enum Event {
    Spawn,
    Send,
    Receive,
    Wait(u64),
    Lock,
    Request,
    Recover,
}

/// The tasks that started while a breakpoint stops tasks as they start,
/// which stop at their first line; those that took a request, at their
/// first of the program's own, saying which.
static STARTING: Mutex<Vec<(u64, Option<String>)>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);

/// A task starts.
pub(crate) fn began(task: u64) {
    if EVENTS.load(Ordering::Relaxed) & ON_SPAWN != 0 {
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).push((task, None));
        STARTED.store(true, Ordering::Relaxed);
    }
}

/// A task sends on a channel, or receives from one: a breakpoint on that
/// stops it there, before.
pub(crate) fn channel_used(kind: u8, channel: usize) {
    let event = if kind == super::tasks::SENDING { ON_SEND } else { ON_RECEIVE };
    if EVENTS.load(Ordering::Relaxed) & event == 0 || calling_here() {
        return;
    }
    let (count, capacity) = super::tasks::channel_fill(channel);
    let filled = capacity.map_or(format!("{count}"), |capacity| format!("{count} of {capacity}"));
    let verb = if event == ON_SEND { "sends on" } else { "receives from" };
    stop_here(format!("{verb} a channel that holds {filled}"));
}

/// A task finds a lock taken: a breakpoint on that stops it, before it waits.
pub(crate) fn lock_taken(object: usize) {
    if !attached() || EVENTS.load(Ordering::Relaxed) & ON_LOCK == 0 || calling_here() {
        return;
    }
    let held: Vec<String> =
        super::tasks::holders(object).iter().map(|(t, _, at)| format!("task {t} holds, taken at {at}")).collect();
    stop_here(format!("wants a std.sync lock {}", held.join(" and ")));
}

/// A task takes a request to answer: a breakpoint on that stops it where
/// the program's handler starts.
pub(crate) fn serving(method: &str, path: &str) {
    if attached() && EVENTS.load(Ordering::Relaxed) & ON_REQUEST != 0 {
        let told = Some(format!("serves {method} {path}"));
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).push((super::tasks::current_id(), told));
        STARTED.store(true, Ordering::Relaxed);
    }
}

/// A task's wait ended after `waited`: a breakpoint on waits longer stops
/// it there.
pub(crate) fn waited(task: u64, waited: std::time::Duration) {
    let longest = LONGEST.load(Ordering::Relaxed);
    if waited.as_millis() < u128::from(longest) || calling_here() {
        return;
    }
    use super::tasks::{JOIN, LOCK, RECEIVING, SENDING};
    let what = match super::tasks::blocked_of(task) {
        Some((LOCK, _)) => "for a std.sync lock".to_string(),
        Some((RECEIVING, _)) => "to receive from a channel".to_string(),
        Some((SENDING, _)) => "to send on a channel".to_string(),
        Some((JOIN, object)) => {
            super::tasks::joined(object).map_or("for a task to end".into(), |t| format!("for task {t} to end"))
        }
        _ => "in a wait".to_string(),
    };
    stop_here(format!("waited {} ms {what}", waited.as_millis()));
}

/// Whether the running task makes a call the debugger asked for, or runs
/// a view's code.
fn calling_here() -> bool {
    let calling = CALLING.load(Ordering::Relaxed);
    (calling != 0 && calling == super::tasks::current_id()) || SHOWING.get()
}

#[cfg_attr(not(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))), allow(dead_code))]
/// Stops the running task where it is for good: each `continue` stops it
/// there again. Without a debugger the program ends, saying why.
pub(crate) fn stop_for_good(why: String) -> ! {
    // Each stop holds the task until the debugger lets it go.
    loop {
        if !attached() || !stop_here(why.clone()) {
            break;
        }
    }
    // Straight to the descriptor: the program may be writing to its
    // standard error, whose lock it holds.
    let message = format!("tondo: {why}\n");
    unsafe { libc::write(2, message.as_ptr().cast(), message.len()) };
    std::process::exit(70);
}

/// Stops the running task, from inside the runtime, where the program's
/// own code called it, `why` as why, and the others at their next probe;
/// gives whether it did, which it cannot outside the program's code.
fn stop_here(why: String) -> bool {
    let Some(table) = table() else { return false };
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
    let Some(stop) = found else { return false };
    hold(state(), stop, Some(why));
    true
}

/// What a task waits for, in words, and the tasks that keep it waiting:
/// those that hold the lock it wants, or the one it waits to finish.
pub(crate) fn waits_for(task: u64) -> Option<(String, Vec<u64>)> {
    use super::tasks::{JOIN, LOCK, RECEIVING, SENDING};
    let (kind, object) = super::tasks::blocked_of(task)?;
    let filled = |object: usize| {
        let (count, capacity) = super::tasks::channel_fill(object);
        capacity.map_or(format!("{count}"), |capacity| format!("{count} of {capacity}"))
    };
    Some(match kind {
        LOCK => {
            let holders = super::tasks::holders(object);
            let held: Vec<String> = holders.iter().map(|(t, _, at)| format!("task {t} holds, taken at {at}")).collect();
            let text = match held.is_empty() {
                true => "a std.sync lock".to_string(),
                false => format!("a std.sync lock {}", held.join(" and ")),
            };
            (text, holders.iter().map(|(t, ..)| *t).collect())
        }
        RECEIVING => (format!("a value of a channel that holds {}", filled(object)), Vec::new()),
        SENDING => (format!("room in a channel that holds {}", filled(object)), Vec::new()),
        JOIN => match super::tasks::joined(object) {
            Some(joined) => (format!("task {joined} to end"), vec![joined]),
            None => ("a task to end".to_string(), Vec::new()),
        },
        _ => return None,
    })
}

/// A cycle of tasks that wait for each other, through locks and joins, in
/// words: a deadlock's cause.
pub(crate) fn cycle(tasks: &[u64]) -> Option<String> {
    // From the lowest task, so that the same cycle reads the same.
    let mut tasks = tasks.to_vec();
    tasks.sort_unstable();
    let edges: Vec<(u64, String, Vec<u64>)> =
        tasks.iter().filter_map(|&t| waits_for(t).map(|(text, on)| (t, text, on))).collect();
    for &(start, ..) in &edges {
        let mut path = vec![start];
        let mut at = start;
        while let Some((_, _, on)) = edges.iter().find(|(t, ..)| *t == at) {
            let Some(&next) = on.first() else { break };
            if next == start {
                let told: Vec<String> = path
                    .iter()
                    .filter_map(|t| edges.iter().find(|(e, ..)| e == t))
                    .map(|(t, text, _)| format!("task {t} waits for {text}"))
                    .collect();
                return Some(told.join("; "));
            }
            if path.contains(&next) {
                break;
            }
            path.push(next);
            at = next;
        }
    }
    None
}

/// Every task waits and none can wake the others (`P0013`): with a
/// debugger attached that catches it, the program stops first, where the
/// main task waits, so that `tasks`, `stack` and `locals` show what each
/// one waits for; it ends when the debugger goes on.
pub(crate) fn deadlocked(headline: &str) {
    if !attached() || !CATCH.load(Ordering::Relaxed) || !caught_panic(headline) {
        return;
    }
    let Some(table) = table() else { return };
    let waiting = super::tasks::waiting();
    let Some(&(task, _, frame, bounds)) = waiting.first() else { return };
    // The tasks that wait for each other, when locks or joins tie them.
    let ids: Vec<u64> = waiting.iter().map(|(id, ..)| *id).collect();
    let headline = match cycle(&ids) {
        Some(cycle) => format!("{headline}: {cycle}"),
        None => headline.to_string(),
    };
    let headline = headline.as_str();
    let stop = Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: headline.to_string() };
    let at = frames(&table, &stop).first().map_or(String::new(), |f| f.place.clone());
    drop(table);
    let mut state = state();
    tell(&format!("{{\"stopped\":{{\"why\":{},\"task\":{task},\"at\":{}}}}}", quoted(headline), quoted(&at)));
    state.stopped = Some(stop);
    HOLD.store(true, Ordering::Relaxed);
    let resumed = state.resumed;
    while state.resumed == resumed {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
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
        // The frame the debugger moves to another line.
        false if super::live::going(task, frame, site as i64) => 2,
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
    if let Some(event) = TELL_AFTER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        tell(&event);
    }
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
        Mode::Step { task, .. } | Mode::Next { task, .. } | Mode::Finish { task, .. } | Mode::Into { task, .. } => task,
        Mode::Run | Mode::Restarted { .. } => 0,
    };
    STEPPING.store(stepping, Ordering::Relaxed);
    *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let mut state = state();
    state.mode = mode;
    state.stopped = None;
    state.selected = None;
    state.past = None;
    HOLD.store(false, Ordering::Relaxed);
    state.resumed += 1;
    RESUMED.notify_all();
}
