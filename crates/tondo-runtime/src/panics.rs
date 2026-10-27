//! What a panic carries beyond its message (language 15.7): the program's
//! own code and data, which `panic(message, code:, data:)` stages here just
//! before it panics, and the forms its report takes, the text of 15.7 and
//! the JSON of `TONDO_PANIC_FORMAT=json` (toolchain 7.5).

use super::{Str, str_bytes};
use crate::tasks::Task;
use std::cell::RefCell;
use std::fmt::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// A value of a panic's data: `Int | Float | Bool | String`.
#[derive(Clone)]
pub(crate) enum Datum {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl Datum {
    /// As `Display` shows the member it holds.
    fn text(&self) -> String {
        match self {
            Datum::Int(n) => n.to_string(),
            Datum::Float(x) => format!("{x:?}"),
            Datum::Bool(b) => b.to_string(),
            Datum::Text(text) => text.clone(),
        }
    }

    /// As JSON: a number, a boolean or a string, and a float that is not
    /// finite as its text.
    fn json(&self) -> String {
        match self {
            Datum::Int(n) => n.to_string(),
            Datum::Float(x) if x.is_finite() => format!("{x:?}"),
            Datum::Float(x) => crate::diagnostics::quoted(&format!("{x:?}")),
            Datum::Bool(b) => b.to_string(),
            Datum::Text(text) => crate::diagnostics::quoted(text),
        }
    }
}

/// What the program gave the panic it is about to raise.
#[derive(Default)]
struct Staged {
    code: Option<String>,
    data: Vec<(String, Datum)>,
    /// Whether the language gave the data, from the values its message
    /// already shows, rather than the program.
    language: bool,
    /// For `panic.resume()`: the code, location and stack of the panic it
    /// raises again.
    resumed: Option<(String, String, String)>,
}

/// The data of a panic of the language, which its message already shows
/// (22.2 of the language).
pub(crate) fn language_data(data: Vec<(&str, Datum)>) {
    STAGED.with(|staged| {
        let mut staged = staged.borrow_mut();
        staged.data = data.into_iter().map(|(key, value)| (key.to_string(), value)).collect();
        staged.language = true;
    });
}

thread_local! {
    static STAGED: RefCell<Staged> = RefCell::new(Staged::default());
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_code(code: *const Str) {
    let code = text(code);
    STAGED.with(|staged| staged.borrow_mut().code = Some(code));
}

fn stage(key: *const Str, datum: Datum) {
    let key = text(key);
    STAGED.with(|staged| {
        let data = &mut staged.borrow_mut().data;
        data.retain(|(k, _)| *k != key);
        data.push((key, datum));
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_int(key: *const Str, value: i64) {
    stage(key, Datum::Int(value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_float(key: *const Str, value: f64) {
    stage(key, Datum::Float(value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_bool(key: *const Str, value: u8) {
    stage(key, Datum::Bool(value != 0));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_text(key: *const Str, value: *const Str) {
    stage(key, Datum::Text(text(value)));
}

/// Whether a code is the program's own: segments of a lowercase letter
/// followed by letters and digits, separated by dots (15.7).
pub(crate) fn own_code(code: &str) -> bool {
    !code.is_empty()
        && code.split('.').all(|segment| {
            let mut chars = segment.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
        })
}

/// A panic as its report shows it.
pub(crate) struct Panic {
    /// `P0001`, or the program's own code.
    pub code: String,
    /// The stable name of a code of the language, as `bounds`.
    pub name: Option<&'static str>,
    pub message: String,
    pub data: Vec<(String, Datum)>,
    /// Whether the program gave the data, which the text then shows.
    pub given: bool,
    pub location: String,
}

impl Panic {
    /// The panic that `code` of the language raises with `message`, and
    /// what the program staged for it: its own code and its data. A code
    /// that is not one is `P0017`, with what it was as `code`.
    pub(crate) fn new(code: u32, name: &'static str, message: String, location: String) -> Panic {
        let staged = STAGED.with(|staged| std::mem::take(&mut *staged.borrow_mut()));
        // Raised again by `panic.resume()`: as it was.
        if let Some((code, location, _)) = staged.resumed {
            let number = code.strip_prefix('P').and_then(|c| c.parse::<u32>().ok());
            let given = number.is_none_or(|n| n == 8 || n == 17);
            return Panic { code, name: number.map(crate::panic_name), message, data: staged.data, given, location };
        }
        let mut given = !staged.data.is_empty() && !staged.language;
        let mut data = staged.data;
        let (code, name) = match staged.code {
            Some(own) if own_code(&own) => (own, None),
            Some(other) => {
                data.retain(|(key, _)| key != "code");
                data.push(("code".into(), Datum::Text(other)));
                given = true;
                ("P0017".into(), Some("invalid-panic-code"))
            }
            None => (format!("P{code:04}"), Some(name)),
        };
        Panic { code, name, message, data, given, location }
    }

    /// The report's first lines: `panic[code]: name: message`, and the
    /// data the program gave, a line each.
    pub(crate) fn headline(&self) -> String {
        let mut text = format!("panic[{}]: ", self.code);
        if let Some(name) = self.name {
            text.push_str(name);
            if !self.message.is_empty() {
                text.push_str(": ");
            }
        }
        text.push_str(&self.message);
        if self.given {
            for (key, value) in &self.data {
                let _ = write!(text, "\n  {key}: {}", value.text());
            }
        }
        text
    }

    /// The data as a JSON object, its keys in order, or `null`.
    pub(crate) fn data_json(&self) -> String {
        let mut data: Vec<&(String, Datum)> = self.data.iter().collect();
        data.sort_by(|a, b| a.0.cmp(&b.0));
        match data.is_empty() {
            true => "null".to_string(),
            false => {
                let members: Vec<String> = data
                    .iter()
                    .map(|(key, value)| format!("{}:{}", crate::diagnostics::quoted(key), value.json()))
                    .collect();
                format!("{{{}}}", members.join(","))
            }
        }
    }

    /// The report as one line of `tondo-panic-0.1/1`, with the frames of
    /// `stack`, a JSON array.
    pub(crate) fn json(&self, stack: &str, recovered: bool, suppressed: bool) -> String {
        let data = self.data_json();
        let name = self.name.map_or("null".to_string(), crate::diagnostics::quoted);
        let mut out = format!(
            "{{\"code\":{},\"data\":{data},\"error_type\":null,\"format\":\"tondo-panic-0.1/1\",\"kind\":\"panic\",\"message\":{},\"name\":{name},\"recovered\":{recovered},\"source\":{},\"stack\":{stack}",
            crate::diagnostics::quoted(&self.code),
            crate::diagnostics::quoted(&self.message),
            source(&self.location),
        );
        if suppressed {
            out.push_str(",\"suppressed\":true");
        }
        out.push('}');
        out
    }
}

/// A `file:line:column` location as JSON, or `null`.
pub(crate) fn source(location: &str) -> String {
    let mut parts = location.rsplitn(3, ':');
    let (column, line, file) = (parts.next(), parts.next(), parts.next());
    match (file, line.and_then(|l| l.parse::<u32>().ok()), column.and_then(|c| c.parse::<u32>().ok())) {
        (Some(file), Some(line), Some(column)) => {
            format!("{{\"column\":{column},\"file\":{},\"line\":{line}}}", crate::diagnostics::quoted(file))
        }
        _ => "null".into(),
    }
}

/// Whether the program reports its failures as JSON, for tools.
pub(crate) fn json_reports() -> bool {
    static JSON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *JSON.get_or_init(|| std::env::var_os("TONDO_PANIC_FORMAT").is_some_and(|v| v == "json"))
}

// ---------------------------------------------------------------------------
// `recover` (language 15.9)
// ---------------------------------------------------------------------------

/// `testing.failNow`, which ends a test as a failed assertion and which no
/// `recover` stops.
pub const FAIL_NOW: u32 = 2012;

/// A panic on its way to a `recover`: what it says, and its stack as the
/// report writes it and as JSON.
pub(crate) struct Record {
    pub(crate) panic: Panic,
    /// The lines after the location: `stack:` and the frames.
    pub(crate) stack: String,
    pub(crate) frames: String,
}

impl Record {
    fn report(&self) -> String {
        format!("{}\n --> {}{}", self.panic.headline(), self.panic.location, self.stack)
    }

    /// Writes the report to standard error, as text or as JSON.
    fn print(&self, recovered: bool, suppressed: bool) {
        crate::flush_stdout();
        let text = match json_reports() {
            true => self.panic.json(&self.frames, recovered, suppressed),
            false if suppressed => format!("suppressed while unwinding: {}", self.report()),
            false => self.report(),
        };
        let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("{text}\n"));
    }
}

fn task<'a>() -> Option<&'a mut Task> {
    unsafe { crate::tasks::current().as_mut() }
}

/// Whether a panic with `code` stops at a `recover` of the running task:
/// one of the language's, but a deadlock and those of the test runner, in
/// a task running a `recover` or belonging to one.
fn recoverable(code: u32) -> bool {
    code != 13 && code < 2000 && task().is_some_and(|task| !task.recovers.is_empty() || belongs(task))
}

fn belongs(task: &Task) -> bool {
    task.owner.is_some() && !task.free.load(Ordering::Acquire)
}

/// A `recover` that tasks started in its block belong to (language 15.9):
/// the first panic one of them ends with stops the task that runs it.
pub(crate) struct Owner(Mutex<Stop>);

struct Stop {
    /// The task that runs the `recover`, `None` once it ended.
    task: Option<usize>,
    panic: Option<Box<Record>>,
}

impl Owner {
    fn state(&self) -> std::sync::MutexGuard<'_, Stop> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The `recover` ends: what a task of it delivered and the running
    /// task has not taken.
    fn close(&self) -> Option<Box<Record>> {
        let mut state = self.state();
        state.task = None;
        state.panic.take()
    }
}

/// The `recover` a task started now belongs to: the innermost one the
/// running task runs, or the one that task belongs to.
pub(crate) fn owner_of_new() -> Option<Arc<Owner>> {
    let task = task()?;
    let at = task as *mut Task as usize;
    match task.recovers.last_mut() {
        Some(owner) => {
            let stop = || Arc::new(Owner(Mutex::new(Stop { task: Some(at), panic: None })));
            Some(owner.get_or_insert_with(stop).clone())
        }
        None => task.owner.clone().filter(|_| belongs(task)),
    }
}

/// A task ended with `panic`: its `recover` takes it, while it runs, and
/// stops; otherwise it waits in the task's `Join`.
pub(crate) fn deliver(task: &Task, panic: Box<Record>) -> Option<Box<Record>> {
    let Some(owner) = task.owner.as_ref().filter(|_| belongs(task)) else { return Some(panic) };
    let mut state = owner.state();
    let Some(at) = state.task.filter(|_| state.panic.is_none()) else { return Some(panic) };
    state.panic = Some(panic);
    // Under the lock: the task cannot leave the `recover` meanwhile.
    unsafe { crate::tasks::alarm(at as *mut Task) };
    None
}

/// The panic a task of one of its `recover`s delivered goes on in the
/// running task, which unwinds from here.
fn take_alarm(task: &mut Task) {
    if !task.alarm.swap(false, Ordering::AcqRel) {
        return;
    }
    crate::tasks::attend(false);
    let delivered: Vec<Box<Record>> =
        task.recovers.iter().rev().flatten().filter_map(|o| o.state().panic.take()).collect();
    for panic in delivered {
        rethrow(panic);
    }
}

/// Ends the alarm when no `recover` of the task holds a panic any more.
fn calm(task: &mut Task) {
    if !task.alarm.load(Ordering::Acquire) {
        return;
    }
    let states: Vec<_> = task.recovers.iter().flatten().map(|o| o.state()).collect();
    if states.iter().all(|state| state.panic.is_none()) && task.alarm.swap(false, Ordering::AcqRel) {
        crate::tasks::attend(false);
    }
}

/// Records a panic that a `recover` stops, which compiled code unwinds
/// from when the call returns; false for one that ends the program. A
/// panic while another unwinds is the first's: one in a cleanup is
/// reported as suppressed, and its own cleanup unwinds it.
pub(crate) unsafe fn record(code: u32, message: *const Str, location: *const Str) -> bool {
    if !recoverable(code) {
        return false;
    }
    let Some(task) = task() else { return false };
    if task.unwinding.is_some() {
        return true;
    }
    let message = match message.is_null() {
        true => String::new(),
        false => text(message),
    };
    let mut location = text(location);
    if location.starts_with("<std>/") {
        location = crate::trace::caller().unwrap_or(location);
    }
    let resumed = resumed_stack();
    let panic = Panic::new(code, crate::panic_name(code), message, location);
    let record = match resumed {
        Some(stack) => Record { panic, frames: frames_of(&stack), stack },
        None => Record { panic, stack: crate::trace::report(), frames: crate::trace::report_json() },
    };
    if !task.paused.is_empty() {
        record.print(false, true);
    }
    task.unwinding = Some(Box::new(record));
    crate::tasks::attend(true);
    true
}

/// A panic that a task ended with goes on in the task that waits for it:
/// recorded when a `recover` there stops it, or the end of the program.
pub(crate) fn rethrow(record: Box<Record>) {
    let code = record.panic.code.strip_prefix('P').and_then(|c| c.parse().ok()).unwrap_or(8);
    let Some(task) = task() else { return };
    // In a cleanup, it is one more panic while another unwinds.
    if !task.paused.is_empty() {
        record.print(false, true);
        return;
    }
    if task.unwinding.is_some() {
        return;
    }
    if recoverable(code) {
        task.unwinding = Some(record);
        crate::tasks::attend(true);
        return;
    }
    record.print(false, false);
    crate::tasks::end_all(crate::PANIC_EXIT_CODE)
}

/// Whether the running task runs the cleanups of a frame a panic unwinds.
pub(crate) fn cleaning() -> bool {
    task().is_some_and(|task| !task.paused.is_empty())
}

/// Whether the running task unwinds from a panic: compiled code asks after
/// a call that may panic.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unwinding() -> u8 {
    let Some(task) = task() else { return 0 };
    take_alarm(task);
    u8::from(task.unwinding.is_some())
}

/// At the start of a frame's cleanups on the way of a panic: the panic
/// waits, so that the cleanups run as code does.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unwind_hold() {
    let Some(task) = task() else { return };
    let held = task.unwinding.take();
    if held.is_some() {
        crate::tasks::attend(false);
    }
    task.paused.push(held);
}

/// After the cleanups: the panic goes on, to the caller.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unwind_resume() {
    let Some(task) = task() else { return };
    settle(task);
    if let Some(held) = task.paused.pop().flatten() {
        task.unwinding = Some(held);
        crate::tasks::attend(true);
    }
}

/// Drops a panic that a cleanup suppressed, already reported.
fn settle(task: &mut crate::tasks::Task) {
    if !task.paused.is_empty() && task.unwinding.take().is_some() {
        crate::tasks::attend(false);
    }
}

/// Before a `defer` runs: one that ran before it and panicked is done.
pub(crate) fn defer_runs() {
    if let Some(task) = task() {
        settle(task);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_recover_enter() {
    if let Some(task) = task() {
        task.recovers.push(None);
    }
}

/// The block ends as code does, its tasks done: 1 if one of them ended
/// with a panic, which waits for the `recover` to stop it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_recover_end() -> u8 {
    let Some(task) = task() else { return 0 };
    if let Some(panic) = task.recovers.last().and_then(|o| o.as_ref()?.close()) {
        rethrow(panic);
    }
    take_alarm(task);
    match task.unwinding.take() {
        Some(panic) => {
            crate::tasks::attend(false);
            task.paused.push(Some(panic));
            1
        }
        None => {
            task.recovers.pop();
            calm(task);
            0
        }
    }
}

/// The block ends another way, by `return`, `break`, `?`, `fail` or a
/// cancellation: a panic of a task of it goes on from here.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_recover_leave() {
    let Some(task) = task() else { return };
    let delivered = task.recovers.pop().flatten().and_then(|owner| owner.close());
    calm(task);
    if let Some(panic) = delivered {
        rethrow(panic);
    }
}

/// Where a `recover` stops a panic, after the cleanups of what it
/// abandoned: the panic is the one the standard library reads. Another
/// that a task of it delivered meanwhile is reported as suppressed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_recover_catch() {
    let Some(task) = task() else { return };
    settle(task);
    task.caught = task.paused.pop().flatten();
    if let Some(other) = task.recovers.pop().flatten().and_then(|owner| owner.close()) {
        other.print(false, true);
    }
    calm(task);
}

fn caught<T>(read: impl FnOnce(&Record) -> T, otherwise: T) -> T {
    match task().and_then(|task| task.caught.as_deref()) {
        Some(record) => read(record),
        None => otherwise,
    }
}

/// A text of the caught panic, or an empty one.
fn caught_text(read: impl FnOnce(&Record) -> &str) -> *mut Str {
    str_of(caught(|r| read(r).to_string(), String::new()).as_str())
}

fn str_of(text: &str) -> *mut Str {
    crate::str_new(text.as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_code() -> *mut Str {
    caught_text(|r| &r.panic.code)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_message() -> *mut Str {
    caught_text(|r| &r.panic.message)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_location() -> *mut Str {
    caught_text(|r| &r.panic.location)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_stack() -> *mut Str {
    caught_text(|r| r.stack.trim_start_matches('\n'))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_count() -> i64 {
    caught(|r| r.panic.data.len() as i64, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_key(index: i64) -> *mut Str {
    caught_text(|r| r.panic.data.get(index as usize).map_or("", |(k, _)| k))
}

/// The kind of a datum: 0 `Int`, 1 `Float`, 2 `Bool`, 3 `String`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_kind(index: i64) -> i64 {
    let kind = |datum: &Datum| match datum {
        Datum::Int(_) => 0,
        Datum::Float(_) => 1,
        Datum::Bool(_) => 2,
        Datum::Text(_) => 3,
    };
    caught(|r| r.panic.data.get(index as usize).map_or(3, |(_, d)| kind(d)), 3)
}

fn datum<T>(index: i64, read: impl FnOnce(&Datum) -> Option<T>, otherwise: T) -> T {
    caught(|r| r.panic.data.get(index as usize).and_then(|(_, d)| read(d)), None).unwrap_or(otherwise)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_int(index: i64) -> i64 {
    datum(index, |d| if let Datum::Int(n) = d { Some(*n) } else { None }, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_float(index: i64) -> f64 {
    datum(index, |d| if let Datum::Float(x) = d { Some(*x) } else { None }, 0.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_bool(index: i64) -> u8 {
    datum(index, |d| if let Datum::Bool(b) = d { Some(u8::from(*b)) } else { None }, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_caught_text(index: i64) -> *mut Str {
    let text = datum(index, |d| if let Datum::Text(t) = d { Some(t.clone()) } else { None }, String::new());
    str_of(&text)
}

/// The stable name of a code of the language, as `bounds`; empty for the
/// program's own.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_name(code: *const Str) -> *mut Str {
    let code = text(code);
    let number = code.strip_prefix('P').and_then(|c| c.parse::<u32>().ok());
    str_of(number.map_or("", crate::panic_name))
}

/// `panic.resume()`: the next panic raised is this one again, with its
/// code, location and stack, and the data staged with it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_restage(code: *const Str, location: *const Str, stack: *const Str) {
    let (code, location, stack) = (text(code), text(location), text(stack));
    STAGED.with(|staged| {
        let mut staged = staged.borrow_mut();
        staged.resumed = Some((code, location, format!("\n{}", stack.trim_start_matches('\n'))));
    });
}

/// A panic the program recovered, written as a report of one, with the
/// data staged: as a server writes the panic of a request it answered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_report(
    code: *const Str,
    message: *const Str,
    location: *const Str,
    stack: *const Str,
) {
    let staged = STAGED.with(|staged| std::mem::take(&mut *staged.borrow_mut()));
    let (code, message, location) = (text(code), text(message), text(location));
    let stack = format!("\n{}", text(stack).trim_start_matches('\n'));
    let number = code.strip_prefix('P').and_then(|c| c.parse::<u32>().ok());
    // Its data shows, with what the report adds, as which request it was.
    let panic = Panic { code, name: number.map(crate::panic_name), message, data: staged.data, given: true, location };
    let frames = frames_of(&stack);
    Record { panic, stack, frames }.print(true, false);
}

/// The frames of a stack as the report writes it, as JSON (toolchain 7.5).
pub(crate) fn frames_of(stack: &str) -> String {
    let mut task: Option<usize> = None;
    let mut out = String::from("[");
    for line in stack.lines() {
        match line {
            "stack:" => task = Some(0),
            "in a task spawned at:" => task = Some(task.map_or(1, |t| t + 1)),
            _ if line.starts_with("  ") => {
                let Some(task) = task else { continue };
                let (line, times) = match line.strip_suffix(" times)").and_then(|l| l.rsplit_once("  (")) {
                    Some((line, times)) => (line, times.parse().unwrap_or(1)),
                    None => (line, 1),
                };
                let Some((function, place)) = line.trim_start().rsplit_once("  ") else { continue };
                if out.len() > 1 {
                    out.push(',');
                }
                let _ = write!(
                    out,
                    "{{\"function\":{},\"source\":{},\"task\":{task},\"times\":{times}}}",
                    crate::diagnostics::quoted(function.trim_end()),
                    source(place.trim()),
                );
            }
            _ => {}
        }
    }
    out.push(']');
    out
}

/// Whether the running task unwinds from a panic, as a function of the
/// runtime asks after a call that may have raised one.
pub(crate) fn unwinding() -> bool {
    tondo_rt_unwinding() != 0
}

/// The stack of a panic `panic.resume()` raises again, as its report wrote
/// it, while it is staged.
pub(crate) fn resumed_stack() -> Option<String> {
    STAGED.with(|staged| staged.borrow().resumed.as_ref().map(|(_, _, stack)| stack.clone()))
}
