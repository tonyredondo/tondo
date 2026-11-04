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
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
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
    *CALL.lock().unwrap_or_else(|e| e.into_inner()) = Some((task, code, addresses));
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
    /// The task it stops alone, when it names one.
    task: Option<u64>,
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
    ATTACHED.load(Ordering::Relaxed) && FOLLOW.load(Ordering::Relaxed)
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
    let frames = [Frame { place, function: Some(function), pointer: fp, addresses: at(addressed), alive: at(living) }];
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
    if !ATTACHED.load(Ordering::Relaxed) {
        return super::live::moving(site);
    }
    // A call the debugger makes runs to its end, as a view's code does.
    let calling = CALLING.load(Ordering::Relaxed);
    if (calling != 0 && calling == super::tasks::current_id()) || SHOWING.get() {
        return 0;
    }
    let Some(table) = table() else { return 0 };
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
    if every > 0 && super::replay::ticks().is_multiple_of(1 << 14) {
        let _outside = super::replay::Outside::enter();
        periodic(
            &table,
            Stop {
                task,
                site,
                frame,
                bounds: (0, 0),
                addresses: addresses as usize,
                alive: alive as usize,
                why: String::new(),
            },
            every,
        );
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
    let started = STARTED.load(Ordering::Relaxed) && function.is_some_and(|f| !f.starts_with("spawn ")) && {
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        let found = starting.iter().position(|&t| t == task).map(|at| starting.swap_remove(at)).is_some();
        STARTED.store(!starting.is_empty(), Ordering::Relaxed);
        found
    };
    let why = if frozen || state.stopped.is_some() || HOLD.load(Ordering::Relaxed) {
        // Another task stopped, or the debugger asked all to, or this task
        // stays where it is.
        if frozen || state.stopped.is_some() { None } else { Some("pause".to_string()) }
    } else if started {
        let from = spawned(task).first().map_or(String::new(), |at| format!(", spawned at {at}"));
        Some(format!("task {task} starts{from}"))
    } else {
        // A step goes over code the program's files do not have, the
        // standard library's and the derived, unless it started there.
        let standard = || table.sites.get(site).is_some_and(Site::generated);
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
            let evaluated = || match &breakpoint.condition {
                Some(c) => evaluating(&table, stack(), 0, |e| e.holds(c)),
                None => breakpoint.unparsed.is_none(),
            };
            let holds = || match breakpoint.compiled.iter().find(|(.., sites)| sites.contains(&site)) {
                Some((code, names, _)) => compiled_holds(&table, stack(), *code, names).unwrap_or_else(evaluated),
                None => evaluated(),
            };
            let here = breakpoint.sites.contains(&site) && breakpoint.task.is_none_or(|t| t == task) && holds();
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
    match unsafe { libc::fork() } {
        -1 => Err(format!("cannot fork the replay: {}", std::io::Error::last_os_error())),
        0 => {
            kept(table);
            Ok(0)
        }
        pid => Ok(pid as u32),
    }
}

/// In a replay, how often, in milliseconds, a program that runs keeps a
/// checkpoint at a probe (`checkpoints <ms>`), or never; and when it last
/// did.
static EVERY: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
/// So many probes make a checkpoint due too, as a replay may run them
/// faster than the time does.
const PROBES: u64 = 1 << 22;

/// A checkpoint at a probe of a replay that runs, when one is due: the
/// debugger's state and connection held across the fork, so that the copy
/// has none another thread held; the debugger hears where it is. A replay
/// forked from it later stops at that probe.
fn periodic(table: &Table, mut here: Stop, every: u64) {
    if super::replay::threaded() {
        return;
    }
    let (now, tick) = (std::time::Instant::now(), super::replay::ticks());
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let due = |(at, probe): (std::time::Instant, u64)| {
            now.duration_since(at).as_millis() >= every as u128 || tick - probe >= PROBES
        };
        if last.is_some_and(|last| !due(last)) {
            return;
        }
        *last = Some((now, tick));
    }
    let state = state();
    let client = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    // A debugger that left would never hear of it.
    if client.is_none() {
        return;
    }
    match unsafe { libc::fork() } {
        0 => {
            drop(client);
            drop(state);
            kept(table);
            here.bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
            hold(self::state(), here, Some("checkpoint".into()));
        }
        -1 => {}
        pid => {
            drop(client);
            drop(state);
            tell(&format!("{{\"checkpointed\":{{\"pid\":{pid},\"tick\":{}}}}}", super::replay::ticks()));
        }
    }
}

/// A checkpoint, until a replay forked from it goes on from here, which
/// returns. Only this thread is left: the debugger's connection and its
/// settings were the parent's.
fn kept(table: &Table) {
    use std::io::Write;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    ATTACHED.store(false, Ordering::Relaxed);
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
    for fd in 3..1024 {
        unsafe { libc::close(fd) };
    }
    // Nor does it keep its parent's output open: each replay it forks
    // writes where the one that asks for it says.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        libc::dup2(null, 1);
        libc::dup2(null, 2);
        libc::close(null);
    }
    let path = super::diagnostics::sockets().join(format!("{}.checkpoint", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = std::os::unix::net::UnixListener::bind(&path) else { unsafe { libc::_exit(0) } };
    // One the debugger never heard of, or forgot, ends after an hour
    // unused; any, once the debugger is gone.
    let mut idle = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&listener), events: libc::POLLIN, revents: 0 };
    let mut unused = std::time::Duration::ZERO;
    loop {
        match unsafe { libc::poll(&mut idle, 1, 10_000) } {
            0 => {
                unused += std::time::Duration::from_secs(10);
                if unused >= std::time::Duration::from_secs(3600) || super::replay::orphaned() {
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
        let Some(shown) = line.trim().strip_prefix("resume ").and_then(|t| t.parse::<u64>().ok()) else {
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
                super::diagnostics::listen_again();
                return;
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
}

/// A line of a task: its probe, its site and the task.
type Origin = (u64, usize, u64);

/// A scan follows where values a task receives come from.
static FOLLOWING: AtomicBool = AtomicBool::new(false);

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
    if !FOLLOWING.load(Ordering::Relaxed) {
        return;
    }
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let origin = origin_of(tracked, from);
    tracked.messages.entry(channel).or_insert_with(|| vec![None; held].into()).push_back(origin);
}

/// A task ended, and its stack goes: a scan follows its frames no longer.
pub(crate) fn task_ended(task: u64) {
    if !FOLLOWING.load(Ordering::Relaxed) {
        return;
    }
    let _outside = super::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    for followed in tracked.iter_mut().flat_map(|t| &mut t.followed).filter(|f| f.anchor.task == task) {
        followed.anchor.live = None;
    }
}

/// The task `by` took the oldest value of the channel, which held `held`.
pub(crate) fn taken(channel: usize, held: usize, by: u64) {
    if FOLLOWING.load(Ordering::Relaxed) {
        let take =
            |t: &mut Tracked| t.messages.entry(channel).or_insert_with(|| vec![None; held].into()).pop_front()?;
        handed_over(take, by);
    }
}

/// The task `from` gave a value to `to`, which waited for it.
pub(crate) fn handed(from: u64, to: u64) {
    if FOLLOWING.load(Ordering::Relaxed) {
        handed_over(|t| origin_of(t, from), to);
    }
}

/// The current task ends, its result in `join`.
pub(crate) fn ended(join: usize) {
    if !FOLLOWING.load(Ordering::Relaxed) {
        return;
    }
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
    if FOLLOWING.load(Ordering::Relaxed) {
        handed_over(|t| t.ends.get(&join).copied(), super::tasks::current_id());
    }
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
            FOLLOWING.store(true, Ordering::Relaxed);
            tracked.get_or_insert_with(Tracked::default);
            Ok("{\"tracking\":true}".into())
        }
        "track" => {
            let (anchor, text) =
                rest.split_once(' ').ok_or("track takes a frame, as `anchor` gives it, and an expression")?;
            let anchor = Anchor::parse(anchor).ok_or("track takes a frame first, as `anchor` gives it")?;
            let expr = expression::parse(text.trim()).map_err(|m| format!("the expression: {m}"))?;
            FOLLOWING.store(true, Ordering::Relaxed);
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
        "count" => {
            let (at, text) = rest.split_once(' ').map_or((rest, None), |(at, text)| (at, Some(text.trim())));
            let sites = line_sites(table, at)?;
            let expr = text.map(expression::parse).transpose().map_err(|m| format!("the expression: {m}"))?;
            tracked.get_or_insert_with(Tracked::default).line =
                Some(Counted { sites, expr, runs: Default::default(), times: 0 });
            Ok("{\"counting\":true}".into())
        }
        _ => {
            FOLLOWING.store(false, Ordering::Relaxed);
            let found = tracked.take().unwrap_or_default();
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
    // A frozen task stays while the others go on.
    while state.resumed == resumed || frozen(&state) {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
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
        }
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
    if !ATTACHED.load(Ordering::Relaxed) || !CATCH_ERRORS.load(Ordering::Relaxed) {
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
    if !ATTACHED.load(Ordering::Relaxed) || !CATCH.load(Ordering::Relaxed) || !caught_panic(message) {
        return;
    }
    stop_here(message.to_string());
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

/// An event a breakpoint stops at.
#[derive(Clone, Copy, PartialEq)]
enum Event {
    Spawn,
    Send,
    Receive,
    Wait(u64),
}

/// The tasks that started while a breakpoint stops tasks as they start,
/// which stop at their first line.
static STARTING: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);

/// A task starts.
pub(crate) fn began(task: u64) {
    if ATTACHED.load(Ordering::Relaxed) && EVENTS.load(Ordering::Relaxed) & ON_SPAWN != 0 {
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).push(task);
        STARTED.store(true, Ordering::Relaxed);
    }
}

/// A task sends on a channel, or receives from one: a breakpoint on that
/// stops it there, before.
pub(crate) fn channel_used(kind: u8, channel: usize) {
    let event = if kind == super::tasks::SENDING { ON_SEND } else { ON_RECEIVE };
    if !ATTACHED.load(Ordering::Relaxed) || EVENTS.load(Ordering::Relaxed) & event == 0 || calling_here() {
        return;
    }
    let (count, capacity) = super::tasks::channel_fill(channel);
    let filled = capacity.map_or(format!("{count}"), |capacity| format!("{count} of {capacity}"));
    let verb = if event == ON_SEND { "sends on" } else { "receives from" };
    stop_here(format!("{verb} a channel that holds {filled}"));
}

/// A task's wait ended after `waited`: a breakpoint on waits longer stops
/// it there.
pub(crate) fn waited(task: u64, waited: std::time::Duration) {
    let longest = LONGEST.load(Ordering::Relaxed);
    if !ATTACHED.load(Ordering::Relaxed) || waited.as_millis() < u128::from(longest) || calling_here() {
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
        if !ATTACHED.load(Ordering::Relaxed) || !stop_here(why.clone()) {
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
fn waits_for(task: u64) -> Option<(String, Vec<u64>)> {
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
    if !ATTACHED.load(Ordering::Relaxed) || !CATCH.load(Ordering::Relaxed) || !caught_panic(headline) {
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
        Mode::Step { task } | Mode::Next { task, .. } | Mode::Finish { task, .. } | Mode::Into { task, .. } => task,
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
    // `raw()`: a viewed value as it is.
    let (whole, rest) = match (rest.strip_prefix(".raw()"), binding.raw) {
        (Some(after), Some(raw)) => (show(raw, *address), after),
        _ => (show(binding.show, *address), rest),
    };
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
    let read = |name: &str| {
        // `x.raw()`: a viewed binding as it is.
        let (name, raw) = name.strip_suffix(".raw()").map_or((name, false), |n| (n, true));
        match bound.iter().find(|(b, ..)| b.name == name) {
            Some((binding, address, _)) if raw => match binding.raw {
                Some(glue) => Ok(show(glue, *address)),
                None => Err(format!("`{name}` has no view: it shows as it is")),
            },
            Some((binding, address, _)) => Ok(show(binding.show, *address)),
            None => Err(missing(name, &bound)),
        }
    };
    f(&mut Evaluator::new(&read))
}

/// How much of a value's text the debugger shows; it reads all of it to
/// find a part.
const SHOWN: usize = 4096;

/// A value as the debugger shows it.
fn show(glue: Show, address: usize) -> String {
    // A program with views shows through the program's code, which runs on
    // a task: the stopped one, when the debugger's thread asks.
    if VIEWED.load(Ordering::Relaxed) && super::tasks::current().is_null() {
        let task = state().stopped.as_ref().filter(|s| s.site < WAITING).map(|s| s.task);
        if let Some(shown) = task.and_then(|task| on_task(task, || shown_here(glue, address))) {
            return shown;
        }
    }
    shown_here(glue, address)
}

/// A value as the debugger shows it, on this thread; a view's code that
/// runs meanwhile passes its probes.
fn shown_here(glue: Show, address: usize) -> String {
    let before = SHOWING.replace(true);
    let mut builder = super::Builder::new(0);
    unsafe { glue(&mut builder, address as *const u8) };
    let text = builder.finish();
    let shown = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    unsafe { super::tondo_rt_str_release(text) };
    SHOWING.set(before);
    shown
}

thread_local! {
    /// This thread shows a value: the code of a view runs to its end.
    static SHOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The program has types with a `debug.View`.
static VIEWED: AtomicBool = AtomicBool::new(false);

/// Work the debugger's thread asks a held task to do on its own stack: the
/// task and the work; and whether it is done.
#[allow(clippy::type_complexity)]
static WORK: Mutex<Option<(u64, Box<dyn FnOnce() + Send>)>> = Mutex::new(None);
static WORKED: Mutex<bool> = Mutex::new(false);
static WORK_DONE: Condvar = Condvar::new();

/// Runs `work` on the held task `task`, as the debugger's thread waits:
/// `None` when the task does not take it within a while.
fn on_task<T: Send>(task: u64, work: impl FnOnce() -> T + Send) -> Option<T> {
    let mut out: Option<T> = None;
    let place = &mut out as *mut Option<T> as usize;
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || unsafe { *(place as *mut Option<T>) = Some(work()) });
    // The job borrows what this thread holds, which waits until it ran.
    let job: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(job) };
    *WORKED.lock().unwrap_or_else(|e| e.into_inner()) = false;
    *WORK.lock().unwrap_or_else(|e| e.into_inner()) = Some((task, job));
    RESUMED.notify_all();
    let worked = WORKED.lock().unwrap_or_else(|e| e.into_inner());
    let (worked, timeout) = WORK_DONE
        .wait_timeout_while(worked, std::time::Duration::from_secs(5), |done| !*done)
        .unwrap_or_else(|e| e.into_inner());
    if timeout.timed_out() {
        // Not taken: it goes. Taken: it runs, and this thread waits for it.
        if WORK.lock().unwrap_or_else(|e| e.into_inner()).take().is_some() {
            return None;
        }
        drop(WORK_DONE.wait_while(worked, |done| !*done));
    }
    out
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
    // A replay without a debugger keeps no more checkpoints, nor does the
    // program start children for it.
    EVERY.store(0, Ordering::Relaxed);
    FOLLOW.store(false, Ordering::Relaxed);
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CLIENT_FD.store(-1, Ordering::Relaxed);
    ATTACHED.store(false, Ordering::Relaxed);
    let mut state = state();
    state.breakpoints.clear();
    state.frozen.clear();
    FREEZING.store(false, Ordering::Relaxed);
    unwatch(&mut state, |_| true);
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
    // A scan to the end gives what it followed as the program ends.
    let following = TRACKED.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let tracked = match (following, table()) {
        (true, Some(table)) => track(&table, "tracked", "").map_or(String::new(), |t| format!(",\"tracked\":{t}")),
        _ => String::new(),
    };
    tell(&format!("{{\"exited\":true{tracked}}}"));
}

/// The program crashed in a replay, as it did in its recording: a scan
/// gives what it followed all the same, which is what a crash is searched
/// for, unless the crash came with a lock this takes.
pub(crate) fn crashed() {
    let free = TRACKED.try_lock().is_ok() && CLIENT.try_lock().is_ok();
    if free && super::replay::replaying() {
        exited();
    }
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
        "break" => match rest.strip_prefix("on ") {
            Some(event) => event_breakpoint(table, event),
            None => breakpoint(table, rest),
        },
        "delete" => {
            let id: u32 = rest.parse().unwrap_or(0);
            let mut state = state();
            state.breakpoints.retain(|b| b.id != id);
            unwatch(&mut state, |w| w.id == id);
            remark(table, &state);
            format!("{{\"deleted\":{id}}}")
        }
        "continue" => {
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        // In a replay, the probes the program's thread passed (`tick`), a
        // run on to one of them (`seek <tick>`), or through to it keeping
        // where breakpoints would stop and where a task's lines start
        // (`scan <tick> [<task>]`), which `scanned` gives.
        "tick" if super::replay::replaying() => format!("{{\"tick\":{}}}", super::replay::ticks()),
        "seek" | "scan" if super::replay::replaying() => {
            let mut words = rest.split_whitespace();
            let Some(tick) = words.next().and_then(|t| t.parse::<u64>().ok()) else {
                return Some(format!("{{\"error\":\"{word} takes a probe's tick, as `tick` gives\"}}"));
            };
            if tick <= super::replay::ticks() {
                return Some(
                    "{\"error\":\"the replay is past that probe: a replay of the recording from its start goes back\"}"
                        .into(),
                );
            }
            let task = words.next().and_then(|t| t.parse().ok()).unwrap_or(0);
            *SCANNED.lock().unwrap_or_else(|e| e.into_inner()) = (task, Default::default(), Default::default());
            SCANNING.store(word == "scan", Ordering::Relaxed);
            AIM.store(tick, Ordering::Relaxed);
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        // In a replay, a copy of the process where its task stopped, which
        // the debugger goes back to rather than to the start.
        "checkpoint" if super::replay::replaying() => {
            let Some(stop) = stopped().filter(|s| s.site < WAITING) else { return Some(not_stopped()) };
            let tick = super::replay::ticks();
            match on_task(stop.task, || checkpoint(table)) {
                Some(Ok(pid)) => format!("{{\"checkpoint\":{{\"pid\":{pid},\"tick\":{tick}}}}}"),
                Some(Err(message)) => format!("{{\"error\":{}}}", quoted(&message)),
                None => "{\"error\":\"the stopped task did not take the checkpoint\"}".into(),
            }
        }
        "checkpoints" if super::replay::replaying() => {
            let every: u64 = rest.parse().unwrap_or(0);
            EVERY.store(every, Ordering::Relaxed);
            format!("{{\"checkpoints\":{every}}}")
        }
        "scanned" => {
            let scan = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
            let list = |ticks: &std::collections::VecDeque<u64>| {
                ticks.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
            };
            format!("{{\"hits\":[{}],\"lines\":[{}]}}", list(&scan.1), list(&scan.2))
        }
        "why" => match why(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "anchor" => match anchor(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "track" | "count" | "tracked" => match track(table, word, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "effects" => match effects(table) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "timeline" => match timeline(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "ran" => match ran(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "causes" => match causes(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `until <line>` or `until <file>:<line>`: runs until that line, or
        // the next stop, wherever that is.
        "until" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let at = match rest.parse::<u32>() {
                Ok(line) => match table.sites.get(stop.site) {
                    Some(site) => format!("{}:{line}", site.file),
                    None => return Some("{\"error\":\"until takes a line of a file, as `until main.to:12`\"}".into()),
                },
                Err(_) => rest.to_string(),
            };
            let sites = match line_sites(table, &at) {
                Ok(sites) => sites,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let mut state = state();
            let id = state.next_breakpoint;
            state.next_breakpoint += 1;
            let task = None;
            state.breakpoints.push(Breakpoint {
                id,
                sites,
                condition: None,
                count: 0,
                hits: None,
                log: None,
                task,
                once: true,
                event: None,
                capture: false,
                unparsed: None,
                compiled: Vec::new(),
            });
            remark(table, &state);
            drop(state);
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        // The children the program starts, debugged too, or no longer.
        "follow" => {
            let on = rest.trim() != "off";
            FOLLOW.store(on, Ordering::Relaxed);
            format!("{{\"following\":{on}}}")
        }
        // The history: kept from now on, or no longer.
        "record" => {
            let lines = match rest {
                "off" => 0,
                "" => 10_000,
                lines => match lines.parse::<usize>() {
                    Ok(lines) if lines > 0 => lines,
                    _ => {
                        return Some(
                            "{\"error\":\"record takes how many lines it keeps, as `record 10000`, or `off`\"}".into(),
                        );
                    }
                },
            };
            RECORDING.store(lines, Ordering::Relaxed);
            if lines == 0 {
                HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clear();
                state().past = None;
            }
            format!("{{\"recording\":{lines}}}")
        }
        "back" | "forward" | "reverse" => match moved(table, word, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // A step in the past goes forward in the history.
        "step" | "next" | "finish" if state().past.is_some() => match moved(table, "forward", "1") {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
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
                // A call of the line, by its function's name.
                "step" if rest.starts_with("into ") => {
                    *INTO.lock().unwrap_or_else(|e| e.into_inner()) = rest["into ".len()..].trim().to_string();
                    Mode::Into { task: stop.task, frame: stop.frame }
                }
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
        // `goto <line> <move>`: the stopped frame goes on at the statement
        // of that line of its function, by the move `tondo` worked out from
        // the live build's points, as soon as it goes on; until then it
        // shows there.
        "goto" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            if !super::live::on() {
                return Some(
                    "{\"error\":\"a frame goes to another line in a live build: debug the program with --live\"}"
                        .into(),
                );
            }
            let (line, hex) = rest.split_once(' ').unwrap_or((rest, ""));
            let bytes: Option<Vec<u8>> = (0..hex.len())
                .step_by(2)
                .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
                .collect();
            let (Ok(line), Some(bytes)) = (line.parse::<u32>(), bytes) else {
                return Some("{\"error\":\"goto takes a line and a move as tondo writes it\"}".into());
            };
            if let Err(message) = super::live::goto(stop.task, stop.frame, stop.site as i64, &bytes) {
                return Some(format!("{{\"error\":{}}}", quoted(&message)));
            }
            *MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()) = Some((stop.task, line));
            let at = moved_place(&place_in(table, stop.site), line);
            *TELL_AFTER.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("{{\"stopped\":{{\"why\":\"goto\",\"task\":{},\"at\":{}}}}}", stop.task, quoted(&at)));
            // The stop at the target line comes next, as after a step.
            format!("{{\"running\":true,\"goto\":{line}}}")
        }
        // What a call of the program's code gives, on the stopped task.
        "call" => match called(table, rest) {
            Ok(text) => format!("{{\"called\":{}}}", quoted(&text)),
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `freeze <task>`: it stays held, at its next line if it runs, while
        // the others go on; `thaw <task>` lets it go on.
        "freeze" | "thaw" => {
            let Ok(task) = rest.parse::<u64>() else {
                return Some(format!(
                    "{{\"error\":{}}}",
                    quoted(&format!("{word} takes a task's number, as `tasks` lists them"))
                ));
            };
            let mut state = state();
            state.frozen.retain(|&t| t != task);
            if word == "freeze" {
                state.frozen.push(task);
            }
            FREEZING.store(!state.frozen.is_empty(), Ordering::Relaxed);
            let frozen: Vec<String> = state.frozen.iter().map(u64::to_string).collect();
            drop(state);
            RESUMED.notify_all();
            format!("{{\"frozen\":[{}]}}", frozen.join(","))
        }
        "retainers" => match retainers(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "memory" => match memory() {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `bindings <breakpoint>`: the file of its lines, and the bindings
        // its condition reads there with their types, for `tondo` to
        // compile it.
        "bindings" => {
            let id: u32 = rest.parse().unwrap_or(0);
            let state = state();
            let Some(breakpoint) = state.breakpoints.iter().find(|b| b.id == id) else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("no breakpoint {id}"))));
            };
            let Some(&first) = breakpoint.sites.first() else {
                return Some("{\"error\":\"the breakpoint is at no line\"}".into());
            };
            let functions: std::collections::BTreeSet<usize> =
                breakpoint.sites.iter().map(|&s| table.sites[s].function).collect();
            let mut bindings: Vec<(&str, &str)> = Vec::new();
            for function in functions {
                let mut name = table.functions[function].name.as_str();
                let mut around = vec![function];
                while let Some((outer, false)) = written_in(name) {
                    let Some(found) = table.functions.iter().position(|f| f.name == outer) else { break };
                    around.push(found);
                    name = outer;
                }
                bindings.extend(around.iter().flat_map(|&f| &table.functions[f].bindings).map(|b| (&*b.name, &*b.ty)));
            }
            // A name bound with two types, which the patch could not tell
            // apart, stays out: a condition that reads it is evaluated.
            let mut out: Vec<String> = Vec::new();
            for (index, &(name, ty)) in bindings.iter().enumerate() {
                let earliest = bindings.iter().position(|(n, _)| *n == name) == Some(index);
                if earliest && bindings.iter().all(|(n, t)| *n != name || *t == ty) {
                    out.push(format!("{{\"name\":{},\"type\":{}}}", quoted(name), quoted(ty)));
                }
            }
            format!("{{\"file\":{},\"bindings\":[{}]}}", quoted(&table.sites[first].file), out.join(","))
        }
        // The conditions only compiled code reads whose breakpoints have
        // lines of code they were not compiled for, after a reload.
        "conditions" => {
            let state = state();
            let pending = state.breakpoints.iter().filter_map(|b| {
                let text = b.unparsed.as_ref()?;
                let known = |s: &usize| b.compiled.iter().any(|(.., sites)| sites.contains(s));
                (!b.sites.iter().all(known)).then(|| format!("{{\"id\":{},\"condition\":{}}}", b.id, quoted(text)))
            });
            format!("{{\"conditions\":[{}]}}", pending.collect::<Vec<_>>().join(","))
        }
        // `condition <breakpoint> <slot> <binding>...`: its condition as a
        // live patch compiled it, which the breakpoint calls from now on.
        "condition" => {
            let mut words = rest.split_whitespace();
            let (Some(id), Some(slot)) =
                (words.next().and_then(|w| w.parse::<u32>().ok()), words.next().and_then(|w| w.parse().ok()))
            else {
                return Some("{\"error\":\"condition takes a breakpoint, a slot and the bindings it reads\"}".into());
            };
            let Some(code) = super::live::slot_code(slot).filter(|&c| c != 0) else {
                return Some("{\"error\":\"the program is not a live build\"}".into());
            };
            let names = words.map(String::from).collect();
            let mut state = state();
            match state.breakpoints.iter_mut().find(|b| b.id == id) {
                Some(breakpoint) => {
                    let known: Vec<usize> = breakpoint.compiled.iter().flat_map(|(.., s)| s.clone()).collect();
                    let sites = breakpoint.sites.iter().filter(|s| !known.contains(s)).copied().collect();
                    breakpoint.compiled.push((code, names, sites));
                    format!("{{\"condition\":{{\"id\":{id},\"compiled\":true}}}}")
                }
                None => format!("{{\"error\":{}}}", quoted(&format!("no breakpoint {id}"))),
            }
        }
        // Whether steps go into the standard library's code.
        "steps" => match rest {
            "all" | "program" => {
                STEPS_ALL.store(rest == "all", Ordering::Relaxed);
                format!("{{\"steps\":{}}}", quoted(rest))
            }
            _ => "{\"error\":\"steps takes `program`, to step over the standard library's code, or `all`\"}".into(),
        },
        // An expression the frame looked at watches, until it returns.
        "watch" => match watch(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // What stops the program: panics, errors, both or none.
        // `catch panic [<code or name>...] error [<type>...]`, or `none`.
        "catch" => {
            let (mut panics, mut errors): (Option<Vec<String>>, Option<Vec<String>>) = (None, None);
            let mut last = None;
            for word in rest.split_whitespace() {
                match (word, last) {
                    ("panic", _) => (panics, last) = (Some(Vec::new()), Some(true)),
                    ("error", _) => (errors, last) = (Some(Vec::new()), Some(false)),
                    ("none", None) => last = Some(true),
                    (name, Some(true)) if panics.is_some() => panics.get_or_insert_default().push(name.into()),
                    (name, Some(false)) => errors.get_or_insert_default().push(name.into()),
                    _ => {
                        let message = "catch takes `panic` and its codes or names, as `panic bounds P0005`, to stop where \
                            a task panics, `error` and its types, as `error ConfigError`, where an error starts, or `none`";
                        return Some(format!("{{\"error\":{}}}", quoted(message)));
                    }
                }
            }
            if last.is_none() {
                return Some("{\"error\":\"catch takes `panic`, `error`, both, or `none`\"}".into());
            }
            CATCH.store(panics.is_some(), Ordering::Relaxed);
            CATCH_ERRORS.store(errors.is_some(), Ordering::Relaxed);
            let kinds: Vec<String> = [("panic", &panics), ("error", &errors)]
                .iter()
                .filter_map(|(kind, names)| {
                    names.as_ref().map(|names| quoted(format!("{kind} {}", names.join(" ")).trim_end()))
                })
                .collect();
            *CAUGHT.lock().unwrap_or_else(|e| e.into_inner()) =
                (panics.unwrap_or_default(), errors.unwrap_or_default());
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
        "locals" | "print" | "children" if state().past.is_some() => {
            let state = state();
            let frame = state.selected.map_or(0, |(_, frame)| frame);
            let Some((past, history)) = past(&state) else { return Some(not_stopped()) };
            in_past(&history[past], word, rest, frame)
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
                let looked = {
                    let state = state();
                    state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0)))
                };
                let items = locals_of(table, &frames, index, looked.map_or(0, |(task, _)| task));
                // In the frame a step stopped in, what a function returned.
                let state = state();
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
            let answer = children_of(path, &found.text, page);
            // A viewed binding's value as it is comes first, as `raw()`.
            let bound = visible(table, &frames, index);
            let viewed = bound.iter().find(|(b, ..)| b.name == path && b.raw.is_some());
            match viewed.and_then(|(binding, address, _)| Some(show(binding.raw?, *address))) {
                Some(raw) => {
                    let item = format!(
                        "{{\"name\":\"raw()\",\"path\":{},{}}}",
                        quoted(&format!("{path}.raw()")),
                        described(&raw)
                    );
                    let more = if answer.starts_with("{\"children\":[]") { "" } else { "," };
                    answer.replacen("{\"children\":[", &format!("{{\"children\":[{item}{more}"), 1)
                }
                None => answer,
            }
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
        "stack" if state().past.is_some() => {
            let state = state();
            let Some((past, history)) = past(&state) else { return Some(not_stopped()) };
            let moment = &history[past];
            // Another task than the one whose history it shows is as now.
            if rest.parse::<u64>().is_ok_and(|task| task != moment.task) {
                let task: u64 = rest.parse().unwrap_or(0);
                let Some(held) = stop_of(&state, task) else {
                    return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
                };
                let frames: Vec<String> = frames(table, &held).iter().map(|f| quoted(&f.place)).collect();
                return Some(format!("{{\"stack\":[{}]}}", frames.join(",")));
            }
            let frames: Vec<String> = moment.stack.iter().map(|f| quoted(f)).collect();
            format!("{{\"stack\":[{}]}}", frames.join(","))
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
            let mut frames: Vec<String> = frames(table, &held).iter().map(|f| f.place.clone()).collect();
            let spawned: Vec<String> = spawned(task).iter().map(|s| quoted(s)).collect();
            // A frame `goto` moves shows where it goes on.
            if let (Some((_, line)), Some(first)) =
                (MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()).filter(|(t, _)| *t == task), frames.first_mut())
            {
                *first = moved_place(first, line);
            }
            let frames: Vec<String> = frames.iter().map(|f| quoted(f)).collect();
            // Where the task was spawned, and its spawner: the stack's
            // cause.
            let spawned = match spawned.is_empty() {
                true => String::new(),
                false => format!(",\"spawned\":[{}]", spawned.join(",")),
            };
            format!("{{\"stack\":[{}]{spawned}}}", frames.join(","))
        }
        "snapshot" => match snapshot(table) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "tasks" => {
            let state = state();
            let held: Vec<String> = state
                .held
                .iter()
                .map(|held| {
                    let why = match &state.stopped {
                        Some(stop) if stop.task == held.task => stop.why.clone(),
                        _ if state.frozen.contains(&held.task) => "frozen".to_string(),
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
                    // What it waits for, and the tasks that hold that.
                    let waits = waits_for(task).map_or(String::new(), |(text, on)| {
                        let on: Vec<String> = on.iter().map(u64::to_string).collect();
                        format!(",\"for\":{},\"on\":[{}]", quoted(&text), on.join(","))
                    });
                    waiting.push(format!("{{\"task\":{task},\"at\":{at},\"why\":{why}{waits}}}"));
                }
            }
            format!("{{\"tasks\":[{}]}}", held.into_iter().chain(waiting).collect::<Vec<_>>().join(","))
        }
        _ => return None,
    })
}

/// `watch <expression> [if <condition>] [hits <count>]`, in the frame
/// looked at: of a binding that holds a `std.sync` cell, or a part of its
/// value, from every task.
fn watch(table: &Table, text: &str) -> Result<String, String> {
    let (text, hits) = match text.split_once(" hits ") {
        Some((text, hits)) => (text, Some(Hits::parse(hits)?)),
        None => (text, None),
    };
    let (text, condition) = match text.split_once(" if ") {
        Some((text, condition)) => {
            (text.trim(), Some(expression::parse(condition).map_err(|m| format!("the condition: {m}"))?))
        }
        None => (text.trim(), None),
    };
    let (frames, index) = selected(table)?;
    let expr = expression::parse(text)?;
    let evaluated = evaluating(table, &frames, index, |e| e.evaluate(&expr))?;
    // A binding of a cell, as `hits` in `hits.value`.
    let root = evaluated.path.as_deref().map(|path| path.split(['.', '[']).next().unwrap_or(path));
    let bound = visible(table, &frames, index);
    let shared = root.and_then(|root| bound.iter().find(|(b, ..)| b.name == root)).and_then(|(binding, address, _)| {
        matches!(table.layouts[binding.layout], Layout::Shared).then(|| {
            let cell = unsafe { *(*address as *const usize) };
            unsafe { super::sync::retain(cell as *mut super::sync::SyncObject) };
            (cell, binding.name.clone(), binding.show)
        })
    });
    let mut state = state();
    let (task, _) = state
        .selected
        .or(state.stopped.as_ref().map(|s| (s.task, 0)))
        .ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let last = state.held.iter().find(|h| h.task == task).map_or(usize::MAX, |h| h.site);
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let everywhere = if shared.is_some() { ",\"everywhere\":true" } else { "" };
    let answer = format!("{{\"watch\":{{\"id\":{id},{}{everywhere}}}}}", described(&evaluated.text));
    let (frames, value) = (frames[index..].to_vec(), evaluated.text);
    let text = text.to_string();
    state.watches.push(Watch { id, task, expr, text, frames, value, last, condition, count: 0, hits, shared });
    WATCHING.store(true, Ordering::Relaxed);
    Ok(answer)
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
    let (address, layout, ty) = located(table, &frames, index, target, true)?;
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
        _ => return Err(format!("the debugger does not change `{target}`, a value of type {ty}")),
    }
    Ok(printed(table, &frames, index, target))
}

/// An object of the heap as `retainers` and `memory` name it: its size,
/// where it was made, and a text's first characters.
fn object_named(objects: &super::heap::Objects, at: usize) -> String {
    let (_, size, site, text) = &objects.list[at];
    let made = made_at(*site);
    match text {
        Some(text) => format!("the text {:?} of {size} bytes, made at {made}", clip(text, 40)),
        None => format!("an object of {size} bytes, made at {made}"),
    }
}

fn made_at(site: usize) -> String {
    super::trace::describe(site)
        .and_then(|d| d.at.map(|(file, line)| format!("{file}:{line} in {}", d.function)))
        .unwrap_or_else(|| "a place the debugger does not know".into())
}

fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text.to_string(),
    }
}

/// `retainers <path>`: what keeps the value a binding or a part of one
/// holds alive, when it is an object of the heap: the stacks of the tasks
/// that point to it, and the shortest chains of objects from a stack to it.
fn retainers(table: &Table, target: &str) -> Result<String, String> {
    let (frames, index) = selected(table)?;
    let (address, ..) = located(table, &frames, index, target, false)?;
    let object = unsafe { *(address as *const usize) };
    let objects = super::heap::objects().ok_or("the program does not keep its objects: a debug build does")?;
    let Ok(found) = objects.list.binary_search_by_key(&object, |o| o.0) else {
        return Err(format!("`{target}` is not an object of the heap: its value is in its frame, or in another value"));
    };
    let mut by: Vec<Vec<usize>> = vec![Vec::new(); objects.list.len()];
    for &(from, to) in &objects.edges {
        by[to].push(from);
    }
    let stacks = |at: usize| -> Vec<u64> { objects.roots.iter().filter(|r| r.1 == at).map(|r| r.0).collect() };
    // From the object back to the stacks, the nearest first.
    let mut came: Vec<Option<usize>> = vec![None; objects.list.len()];
    let mut seen = vec![false; objects.list.len()];
    let mut queue = std::collections::VecDeque::from([found]);
    seen[found] = true;
    let mut chains = Vec::new();
    while let Some(at) = queue.pop_front() {
        if at != found && !stacks(at).is_empty() {
            let mut chain = vec![format!("the stack of task {}", stacks(at)[0])];
            let mut step = Some(at);
            while let Some(here) = step {
                chain.push(object_named(&objects, here));
                step = came[here];
            }
            chains.push(format!("[{}]", chain.iter().map(|c| quoted(c)).collect::<Vec<_>>().join(",")));
            if chains.len() == 3 {
                break;
            }
        }
        for &from in &by[at] {
            if !seen[from] {
                seen[from] = true;
                came[from] = Some(at);
                queue.push_back(from);
            }
        }
    }
    let held: Vec<String> = stacks(found).iter().map(u64::to_string).collect();
    Ok(format!(
        "{{\"retainers\":{{\"object\":{},\"stacks\":[{}],\"chains\":[{}]}}}}",
        quoted(&object_named(&objects, found)),
        held.join(","),
        chains.join(",")
    ))
}

/// What `memory` counted last time, by where the objects were made.
static COUNTED: Mutex<Vec<(usize, usize, usize)>> = Mutex::new(Vec::new());

/// `memory`: the program's objects by where they were made, the places
/// with the most bytes first, and how each grew since `memory` last
/// counted.
fn memory() -> Result<String, String> {
    let objects = super::heap::objects().ok_or("the program does not keep its objects: a debug build does")?;
    let mut by: std::collections::HashMap<usize, (usize, usize)> = std::collections::HashMap::new();
    for &(_, size, site, _) in &objects.list {
        let place = by.entry(site).or_default();
        place.0 += 1;
        place.1 += size;
    }
    let mut counted = COUNTED.lock().unwrap_or_else(|e| e.into_inner());
    let before = std::mem::take(&mut *counted);
    let mut places: Vec<(usize, (usize, usize))> = by.into_iter().collect();
    places.sort_by_key(|&(site, (_, bytes))| (std::cmp::Reverse(bytes), site));
    *counted = places.iter().map(|&(site, (count, bytes))| (site, count, bytes)).collect();
    let rows: Vec<String> = places
        .iter()
        .take(20)
        .map(|&(site, (count, bytes))| {
            let grew = match before.iter().find(|b| b.0 == site) {
                Some(&(_, was, was_bytes)) => (count as i64 - was as i64, bytes as i64 - was_bytes as i64),
                None if before.is_empty() => (0, 0),
                None => (count as i64, bytes as i64),
            };
            format!(
                "{{\"at\":{},\"objects\":{count},\"bytes\":{bytes},\"more\":{},\"more_bytes\":{}}}",
                quoted(&made_at(site)),
                grew.0,
                grew.1
            )
        })
        .collect();
    let (count, bytes) = (objects.list.len(), objects.list.iter().map(|o| o.1).sum::<usize>());
    Ok(format!("{{\"memory\":{{\"objects\":{count},\"bytes\":{bytes},\"places\":[{}]}}}}", rows.join(",")))
}

/// Where a binding of a frame, or a part of one, as `user.age` or
/// `items[i]`, is in memory, its layout and the binding's type; `writing`
/// first copies an array another value shares, as a change does.
fn located(
    table: &Table,
    frames: &[Frame],
    index: usize,
    target: &str,
    writing: bool,
) -> Result<(usize, usize, String), String> {
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
                let at = match evaluating(table, frames, index, |e| e.evaluate(at))?.value {
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
                if writing && !unsafe { super::unique(*slot) } {
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
            _ if writing => {
                return Err(format!(
                    "the debugger does not change `{target}`: it changes numbers, Bool, Char, String and plain enums, in records, tuples and arrays"
                ));
            }
            _ => return Err(format!("the debugger finds the parts of records, tuples and arrays, not `{target}`")),
        }
    }
    Ok((address, layout, binding.ty.clone()))
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
    // `capture`, last: what it takes, without stopping.
    let (text, capture) = match text.strip_suffix(" capture") {
        Some(text) => (text, true),
        None => (text, false),
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
    // One task's, `task <id>`.
    let (at, task) = match at.split_once(" task ") {
        Some((at, task)) => match task.trim().parse::<u64>() {
            Ok(task) => (at.trim(), Some(task)),
            Err(_) => return error(format!("`task` takes a task's number, as `tasks` lists them, not `{task}`")),
        },
        None => (at, None),
    };
    let sites = match at.strip_prefix("fn ") {
        Some(name) => function_sites(table, name.trim()),
        None => line_sites(table, at),
    };
    let sites = match sites {
        Ok(sites) => sites,
        Err(message) => return error(message),
    };
    // In a live build, `tondo` compiles the condition: one the debugger
    // does not read, as a call, stops nowhere until it does.
    let written = condition.filter(|_| super::live::on());
    let (condition, unparsed) = match condition.map(expression::parse) {
        Some(Err(message)) if written.is_some() => (None, Some(message)),
        Some(Err(message)) if condition.is_some_and(|c| c.contains('(')) => {
            return error(format!(
                "the condition: {message}; a live build, debugged with --live, compiles a condition that calls the program's functions"
            ));
        }
        Some(Err(message)) => return error(format!("the condition: {message}")),
        Some(Ok(condition)) => (Some(condition), None),
        None => (None, None),
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
    // Its condition, which `tondo` compiles in a live build.
    let shown = match (written, &unparsed) {
        (Some(c), Some(why)) => format!(",\"condition\":{},\"unparsed\":{}", quoted(c), quoted(why)),
        (Some(c), None) => format!(",\"condition\":{}", quoted(c)),
        (None, _) => String::new(),
    };
    let answer =
        format!("{{\"breakpoint\":{{\"id\":{id},\"file\":{},\"line\":{}{shown}}}}}", quoted(&at.file), at.line);
    let (event, compiled) = (None, Vec::new());
    let unparsed = unparsed.and(written).map(String::from);
    let breakpoint =
        Breakpoint { id, sites, condition, unparsed, count: 0, hits, log, task, once: false, event, capture, compiled };
    state.breakpoints.push(breakpoint);
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
    let mut sites = Vec::new();
    for (index, _) in table.functions.iter().enumerate().filter(|(_, f)| named(f, name, false)) {
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

/// The bindings the code of a frame can use, as `locals` gives them: each
/// one changed since `locals` last showed that frame says so.
fn locals_of(table: &Table, frames: &[Frame], index: usize, task: u64) -> Vec<String> {
    let key = (task, frames[index].pointer, frames[index].function);
    let shown: Vec<(String, String)> = visible(table, frames, index)
        .into_iter()
        .map(|(binding, address, _)| (binding.name.clone(), show(binding.show, address)))
        .collect();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let before = seen.iter().find(|(k, _)| *k == key).map(|(_, texts)| texts.clone());
    let items = visible(table, frames, index)
        .into_iter()
        .zip(&shown)
        .map(|((binding, _, from), (_, whole))| {
            let mut from = from.map_or(String::new(), |from| format!(",\"from\":{}", quoted(from)));
            // A view opens to its value as it is, too.
            if binding.raw.is_some() {
                from.push_str(",\"viewed\":true");
            }
            let was = before.as_ref().and_then(|b| b.iter().find(|(n, _)| *n == binding.name)).map(|(_, t)| t);
            let changed = if was.is_some_and(|was| was != whole) { ",\"changed\":true" } else { "" };
            format!(
                "{{\"name\":{},\"type\":{},{}{from}{changed}}}",
                quoted(&binding.name),
                quoted(&binding.ty),
                described(whole)
            )
        })
        .collect();
    seen.retain(|(k, _)| *k != key);
    seen.push((key, shown));
    // A few frames' worth: those a session looks at.
    if seen.len() > 64 {
        seen.remove(0);
    }
    items
}

/// What `locals` last showed of each frame, by its task, frame pointer and
/// function, which the next shows changes against.
#[allow(clippy::type_complexity)]
static SEEN: Mutex<Vec<((u64, usize, Option<usize>), Vec<(String, String)>)>> = Mutex::new(Vec::new());

/// Where a task was spawned, and its spawner, as `file:line in function`:
/// what its stack goes on with.
fn spawned(task: u64) -> Vec<String> {
    super::tasks::spawned_at(task)
        .into_iter()
        .filter_map(|address| {
            let described = super::trace::describe(address)?;
            let (file, line) = described.at?;
            (!described.function.starts_with("spawn")).then(|| format!("{file}:{line} in {}", described.function))
        })
        .collect()
}

/// `snapshot`: what a model needs of a stop in one answer: why and where
/// the task stopped, each frame of its stack with its bindings, where it
/// was spawned, the other tasks, and the last lines of its history.
fn snapshot(table: &Table) -> Result<String, String> {
    let state = state();
    let stop = state.stopped.clone().ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let held = stop_of(&state, stop.task).unwrap_or(stop.clone());
    drop(state);
    let frames = frames(table, &held);
    let described: Vec<String> = (0..frames.len())
        .map(|index| {
            let locals = match frames[index].addresses == 0 && frames[index].function.is_some() {
                true => String::new(),
                false => locals_of(table, &frames, index, stop.task).join(","),
            };
            format!("{{\"at\":{},\"locals\":[{locals}]}}", quoted(&frames[index].place))
        })
        .collect();
    let spawned: Vec<String> = spawned(stop.task).iter().map(|s| quoted(s)).collect();
    let tasks = command("tasks").unwrap_or_default();
    let tasks = tasks.strip_prefix("{\"tasks\":").and_then(|t| t.strip_suffix('}')).unwrap_or("[]").to_string();
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut lines: Vec<String> = history
        .iter()
        .rev()
        .filter(|m| m.task == stop.task)
        .skip(1)
        .take(10)
        .map(|m| quoted(&place_in(table, m.site)))
        .collect();
    lines.reverse();
    Ok(format!(
        "{{\"snapshot\":{{\"task\":{},\"why\":{},\"frames\":[{}],\"spawned\":[{}],\"tasks\":{tasks},\"before\":[{}]{}}}}}",
        stop.task,
        quoted(&stop.why),
        described.join(","),
        spawned.join(","),
        lines.join(","),
        returned(&stop)
    ))
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

/// Marks the sites that have a breakpoint, and the events some stop at.
fn remark(table: &Table, state: &State) {
    for (site, marked) in table.marked.iter().enumerate() {
        marked.store(state.breakpoints.iter().any(|b| b.sites.contains(&site)), Ordering::Relaxed);
    }
    let (mut events, mut longest) = (0, u64::MAX);
    for event in state.breakpoints.iter().filter_map(|b| b.event) {
        events |= match event {
            Event::Spawn => ON_SPAWN,
            Event::Send => ON_SEND,
            Event::Receive => ON_RECEIVE,
            Event::Wait(ms) => {
                longest = longest.min(ms);
                ON_WAIT
            }
        };
    }
    EVENTS.store(events, Ordering::Relaxed);
    LONGEST.store(longest, Ordering::Relaxed);
    // Tasks that started stop no more once no breakpoint asks.
    if events & ON_SPAWN == 0 {
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).clear();
        STARTED.store(false, Ordering::Relaxed);
    }
}

/// `break on spawn|send|receive|wait <ms>`: a breakpoint at an event.
fn event_breakpoint(table: &Table, text: &str) -> String {
    let event = match text.split_whitespace().collect::<Vec<_>>()[..] {
        ["spawn"] => Event::Spawn,
        ["send"] => Event::Send,
        ["receive"] => Event::Receive,
        ["wait", ms] if ms.parse::<u64>().is_ok() => Event::Wait(ms.parse().unwrap_or(0)),
        _ => {
            let message = "break on takes `spawn`, `send`, `receive` or `wait <ms>`, as `break on wait 100`";
            return format!("{{\"error\":{}}}", quoted(message));
        }
    };
    let mut state = state();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let (sites, condition, hits, log, task) = (Vec::new(), None, None, None, None);
    let event = Some(event);
    let (capture, compiled, unparsed) = (false, Vec::new(), None);
    let breakpoint =
        Breakpoint { id, sites, condition, unparsed, count: 0, hits, log, task, once: false, event, capture, compiled };
    state.breakpoints.push(breakpoint);
    remark(table, &state);
    format!("{{\"breakpoint\":{{\"id\":{id},\"on\":{}}}}}", quoted(text.trim()))
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
