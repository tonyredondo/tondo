//! Panics: how one is raised, its name and exit status, the cleanups that
//! run as it unwinds and the end of a program nothing recovers; what it
//! carries beyond its message (language 15.7), the program's own code and
//! data, which `panic(message, code:, data:)` stages here just before it
//! panics; and the forms its report takes, the text of 15.7 and the JSON of
//! `TONDO_PANIC_FORMAT=json` (toolchain 7.5).

use super::*;
use crate::tasks::Task;
use std::cell::RefCell;
use std::fmt::Write;
use std::io::Write as _;
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
    /// Not dropped as the thread ends, which in a library would keep it
    /// loaded while the thread of its host lives: it is empty but between
    /// a panic's staging and its raising.
    static STAGED: std::mem::ManuallyDrop<RefCell<Staged>> = std::mem::ManuallyDrop::new(RefCell::new(Staged::default()));
}

fn text(s: *const Str) -> String {
    unsafe { crate::str_text(s) }.to_owned()
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
    /// The fields of the task's contexts marked `@log` (11.16).
    pub context: Vec<(String, String)>,
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
            let (name, data, context) = (number.map(crate::panic_name), staged.data, crate::contexts::shown(false));
            return Panic { code, name, message, data, given, location, context };
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
        Panic { code, name, message, data, given, location, context: crate::contexts::shown(false) }
    }

    /// The report's lines after the first: the location, and the fields of
    /// the contexts marked `@log`.
    pub(crate) fn place(&self) -> String {
        let mut text = format!("\n --> {}", self.location);
        if !self.context.is_empty() {
            let fields: Vec<String> = self.context.iter().map(|(name, value)| format!("{name}={value}")).collect();
            let _ = write!(text, "\ncontext: {}", fields.join(" "));
        }
        text
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
        let context = match self.context.is_empty() {
            true => "null".to_string(),
            false => {
                let mut fields: Vec<&(String, String)> = self.context.iter().collect();
                fields.sort();
                let members: Vec<String> = fields
                    .iter()
                    .map(|(name, value)| {
                        format!("{}:{}", crate::diagnostics::quoted(name), crate::diagnostics::quoted(value))
                    })
                    .collect();
                format!("{{{}}}", members.join(","))
            }
        };
        let mut out = format!(
            "{{\"code\":{},\"context\":{context},\"data\":{data},\"error_type\":null,\"format\":\"tondo-panic-0.1/1\",\"kind\":\"panic\",\"message\":{},\"name\":{name},\"recovered\":{recovered},\"source\":{},\"stack\":{stack}",
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
    super::config::config().panic_format.as_ref().is_some_and(|v| v == "json")
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
    /// Not a panic: the deadline of the task's `within` of this index
    /// passed, and the task unwinds to it (11.17).
    pub(crate) deadline: Option<usize>,
}

impl Record {
    fn report(&self) -> String {
        format!("{}{}{}", self.panic.headline(), self.panic.place(), self.stack)
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
    code != 13
        && code < 2000
        && task().is_some_and(|task| crate::live::holds_panics() || !task.recovers.is_empty() || belongs(task))
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
    let text = panic.headline();
    if crate::events::on() {
        crate::events::add(crate::events::What::Panicked(format!("{text} at {}", panic.location)));
    }
    crate::tasks::flight_step(|| crate::diagnostics::Did::Panicked(text.clone()));
    crate::debugger::recovering(&text);
    let record = match resumed {
        Some(stack) => Record { panic, frames: frames_of(&stack), stack, deadline: None },
        None => Record { panic, stack: crate::trace::report(), frames: crate::trace::report_json(), deadline: None },
    };
    if !task.paused.is_empty() {
        record.print(false, true);
    }
    task.unwinding = Some(Box::new(record));
    crate::tasks::attend(true);
    true
}

/// Asked where a statement of a live build unwinds from a panic that no
/// `recover` stops, under `tondo run --watch`: the task waits at the program's
/// innermost frame, the program going on, until the code changes. Then that
/// function starts again: the statement of its caller that called it, with all
/// the caller has, on the new code; or, without a caller, its own statement.
/// A panic of the standard library waits at the program's call.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_panic_held() -> u8 {
    let Some(task) = task() else { return 0 };
    let stops = !task.recovers.is_empty() || belongs(task) || !task.paused.is_empty();
    if !crate::live::holds_panics() || stops || task.unwinding.is_none() {
        return 0;
    }
    let fp = crate::trace::frame_pointer();
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let callers = crate::trace::walk(fp, bounds);
    let mut places = callers.filter_map(|address| Some(crate::trace::scopes_at(address).first()?.1.clone()));
    let Some(place) = places.next().filter(|place| !place.starts_with("<std>/")) else { return 0 };
    if !std::mem::take(&mut task.retrying) {
        let text = task.unwinding.as_ref().map_or(String::new(), |record| {
            record.print(false, false);
            record.panic.headline()
        });
        eprintln!("tondo: task {} waits here: save a fix, and this function starts again", task.id);
        // A debugger stops here first, and a fix may come while it does.
        let patches = crate::live::patches();
        crate::debugger::panicked(&text);
        let frame = crate::tasks::probe_of(unsafe { *(fp as *const usize) });
        if patches == crate::live::patches() && !crate::live::wait_for_patch(task.id, place, text, frame) {
            return 0;
        }
        // Its caller's statement calls it again.
        if places.any(|place| !place.starts_with("<std>/")) {
            task.retrying = true;
            return 0;
        }
    }
    task.unwinding = None;
    crate::tasks::attend(false);
    1
}

/// A task that ends in a panic a held statement did not keep: the program
/// ends, as it would have where the panic was.
pub(crate) fn unheld(task: &mut crate::tasks::Task) {
    if crate::live::holds_panics()
        && task.recovers.is_empty()
        && !belongs(task)
        && let Some(record) = task.unwinding.take()
    {
        record.print(false, false);
        crate::tasks::end_all(crate::PANIC_EXIT_CODE)
    }
}

/// What the running task panicked with, if it did, which ends that panic:
/// an expression a tool evaluates.
pub(crate) fn taken() -> Option<String> {
    let task = task()?;
    let record = task.unwinding.take()?;
    crate::tasks::attend(false);
    Some(record.panic.headline())
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
    // A deadline that passed unwinds to the `within` that set it (11.17).
    if task.expired {
        task.expired = false;
        crate::tasks::attend(false);
        if task.unwinding.is_none()
            && let Some(&(_, owner)) = task.withins.last()
        {
            let (code, message, location) = (String::new(), String::new(), String::new());
            let context = Vec::new();
            let panic = Panic { code, name: None, message, data: Vec::new(), given: false, location, context };
            let (stack, frames) = (String::new(), String::from("[]"));
            task.unwinding = Some(Box::new(Record { panic, stack, frames, deadline: Some(owner) }));
            crate::tasks::attend(true);
        }
    }
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

/// `tondo_rt_unwind_resume` where the function then returns: with the
/// frame of the compiled code that calls it, whose frame record holds the
/// caller's frame, then the return address.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_unwind_return() {
    std::arch::naked_asm!("mov rdi, rbp", "jmp {resume}", resume = sym resume_returning)
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_unwind_return() {
    std::arch::naked_asm!("mov x0, x29", "b {resume}", resume = sym resume_returning)
}

/// The function returns to its caller's way when the call was a
/// `try_call`: its return address becomes the call's handler, and the
/// caller unwinds from there without a check after each call.
unsafe extern "C" fn resume_returning(frame: *mut usize) {
    tondo_rt_unwind_resume();
    if task().is_some_and(|task| task.unwinding.is_some()) {
        let returns = unsafe { frame.add(1) };
        if let Some(handler) = crate::trace::handler(unsafe { *returns }) {
            unsafe { *returns = handler };
        }
    }
}

/// Where code with no cleanups of its own between two frames, as the thunk
/// of a function used as a value, returns when its callee unwinds: on to
/// its caller's way, as `tondo_rt_unwind_return` does, the panic untouched.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_unwind_pass() {
    std::arch::naked_asm!("mov rdi, rbp", "jmp {pass}", pass = sym pass_returning)
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_unwind_pass() {
    std::arch::naked_asm!("mov x0, x29", "b {pass}", pass = sym pass_returning)
}

unsafe extern "C" fn pass_returning(frame: *mut usize) {
    if task().is_some_and(|task| task.unwinding.is_some()) {
        let returns = unsafe { frame.add(1) };
        if let Some(handler) = crate::trace::handler(unsafe { *returns }) {
            unsafe { *returns = handler };
        }
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
/// abandoned: 1 and the panic is the one the standard library reads; 0 for
/// a deadline, which goes on to its `within` (11.17). Another panic that a
/// task of it delivered meanwhile is reported as suppressed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_recover_catch() -> u8 {
    let Some(task) = task() else { return 0 };
    settle(task);
    let deadline = matches!(task.paused.last(), Some(Some(record)) if record.deadline.is_some());
    if !deadline {
        task.caught = task.paused.pop().flatten();
    }
    if let Some(other) = task.recovers.pop().flatten().and_then(|owner| owner.close()) {
        other.print(false, true);
    }
    calm(task);
    u8::from(!deadline)
}

/// Where a `within` stops what unwinds to it: 1 for its own deadline; 0
/// for a panic or the deadline of a `within` around it, which go on.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_within_catch() -> u8 {
    let Some(task) = task() else { return 0 };
    settle(task);
    let level = task.withins.len().saturating_sub(1);
    crate::tasks::within_ends(task, level);
    task.withins.pop();
    match task.paused.last() {
        Some(Some(record)) if record.deadline == Some(level) => {
            task.paused.pop();
            1
        }
        _ => 0,
    }
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
    let (name, context) = (number.map(crate::panic_name), crate::contexts::shown(false));
    let panic = Panic { code, name, message, data: staged.data, given: true, location, context };
    let frames = frames_of(&stack);
    Record { panic, stack, frames, deadline: None }.print(true, false);
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

// ---------------------------------------------------------------------------
// Raising a panic
// ---------------------------------------------------------------------------

/// Process exit status of a panic (spec 20.10: distinct from error exits).
pub const PANIC_EXIT_CODE: i32 = 101;

pub(crate) fn panic_name(code: u32) -> &'static str {
    match code {
        1 => "bounds",
        2 => "zero-slice-step",
        3 => "integer-division-by-zero",
        4 => "overlapping-borrow",
        5 => "checked-overflow",
        6 => "array-shape-mismatch",
        7 => "assertion-failed",
        8 => "explicit-panic",
        9 => "duplicate-dynamic-map-key",
        10 => "invalid-shift-count",
        11 => "invalid-repeat-count",
        12 => "closed-channel",
        13 => "deadlock",
        14 => "reentrant-lock",
        15 => "invalid-environment-name",
        16 => "nul-in-c-string",
        17 => "invalid-panic-code",
        18 => "negative-exponent",
        19 => "lock-order",
        2001 => "test-skip-during-cleanup",
        2002 => "test-tag-conflict",
        2003 => "test-virtual-time-deadlock",
        2004 => "overlapping-test-virtual-time",
        2005 => "test-virtual-time-range",
        2006 => "test-artifact-conflict",
        2007 => "test-snapshot-mismatch",
        2008 => "test-snapshot-conflict",
        2009 => "test-bench-conflict",
        2010 => "test-fuzz-conflict",
        2011 => "test-fuzz-input-invalid",
        _ => "unknown",
    }
}

/// Each task keeps the cleanups of the scopes it executes, innermost last
/// (13.7). Generated code pushes one when a `defer` registers and pops it
/// before running it on a normal exit; a panic runs the rest. Frames stay
/// alive meanwhile: a panic ends the process, so it never unwinds the
/// native stack.
pub(crate) static PANICKING: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_push(code: tasks::Cleanup, closure: *mut u8) {
    tasks::push_cleanup(code, closure);
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_pop() {
    tasks::pop_cleanup();
    // A `defer` that ran before this one and panicked while a panic
    // unwinds was reported as suppressed, and is done (language 15.7).
    panics::defer_runs();
    tasks::defer_starts();
}

/// After a `defer` ran.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_end() {
    tasks::defer_ends();
}

pub(crate) fn run_cleanups() {
    tasks::run_cleanups();
}

/// Reports a language panic (spec 15.7, codes in 22.2), runs the pending
/// `defer` cleanups and terminates. A panic inside a cleanup is reported
/// as suppressed and the remaining cleanups still run. `message` may be
/// null; `location` is a static `file:line:column` string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic(code: u32, message: *const Str, location: *const Str) -> ! {
    unsafe { end_in_panic(code, message, location) }
}

/// A panic where compiled code unwinds when the call returns: one that a
/// `recover` of the running task stops is recorded, and the call returns
/// (language 15.9); any other ends the program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_unwind(code: u32, message: *const Str, location: *const Str) {
    if unsafe { panics::record(code, message, location) } {
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// A panic of the runtime: recorded when a `recover` of the running task
/// stops it, and the caller returns at once with a value nobody uses;
/// otherwise the program ends here.
pub(crate) fn raise(code: u32, message: &str, location: *const Str) {
    let message = str_new(message.as_bytes());
    if unsafe { panics::record(code, message, location) } {
        unsafe { tondo_rt_str_release(message) };
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// Reports a panic nothing recovers and ends the program.
pub(crate) unsafe fn end_in_panic(mut code: u32, message: *const Str, location: *const Str) -> ! {
    // `testing.failNow` reports as a failed assertion, which no `recover` stops.
    if code == panics::FAIL_NOW {
        code = 7;
    }
    flush_stdout();
    let message = (!message.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned());
    let mut location = String::from_utf8_lossy(unsafe { str_bytes(location) }).into_owned();
    // A panic inside the standard library points at the program's call.
    if location.starts_with("<std>/") {
        location = trace::caller().unwrap_or(location);
    }
    let suppressed = PANICKING.fetch_add(1, Ordering::SeqCst) > 0;
    // `testing.skip(reason)` ends a test as skipped, unless cleanup runs.
    if code == testing::SKIP && suppressed {
        code = 2001;
    } else if code == testing::SKIP {
        testing::terminal(code, &format!("{}\n --> {location}", message.unwrap_or_default()));
        run_cleanups();
        tasks::end_all(0)
    }
    // An assertion's data are its condition and the sides it compared.
    if code == 7
        && let Some(message) = &message
    {
        let line = |prefix: &str| message.lines().find_map(|l| l.strip_prefix(prefix)).map(str::to_string);
        let condition = line("  condition: ")
            .or_else(|| line("condition: "))
            .unwrap_or_else(|| message.lines().next().unwrap_or_default().to_string());
        let mut data = vec![("condition", panics::Datum::Text(condition))];
        if let (Some(left), Some(right)) = (line("  left:  "), line("  right: ")) {
            data.push(("left", panics::Datum::Text(left)));
            data.push(("right", panics::Datum::Text(right)));
        }
        panics::language_data(data);
    }
    let resumed = panics::resumed_stack();
    let panic = panics::Panic::new(code, panic_name(code), message.unwrap_or_default(), location);
    let text = panic.headline();
    let location = &panic.location;
    let prefix = if suppressed { "suppressed while unwinding: " } else { "" };
    if recorder::on() {
        let task = tasks::current_id();
        recorder::record(recorder::Event::Panic { task, message: format!("{text}\n --> {location}") });
    }
    if events::on() {
        events::add(events::What::Panicked(format!("{text} at {location}")));
    }
    // A debugger sees where the program's code panicked first.
    if !suppressed {
        debugger::panicked(&text);
    }
    let frames = resumed.as_deref().map(panics::frames_of);
    let stack = resumed.unwrap_or_else(trace::report);
    let report = format!("{prefix}{text}{}{stack}", panic.place());
    // A test build reports it to `tondo test` instead, with its data.
    testing::panic_data(&panic);
    if !testing::terminal(code, &report) {
        match panics::json_reports() {
            true => {
                let frames = frames.unwrap_or_else(trace::report_json);
                let _ = writeln!(std::io::stderr(), "{}", panic.json(&frames, false, suppressed));
            }
            false => {
                let _ = writeln!(std::io::stderr(), "{report}");
            }
        }
        if !suppressed {
            // Where the walk of the stack starts, before the frames of the
            // dump, which may lose it.
            diagnostics::faulted_at(0, trace::frame_pointer(), tasks::stack_bounds().unwrap_or((0, 0)));
            diagnostics::crash_dump("panic", &format!("{text}\n --> {location}"), trace::here_frames(), true);
        }
    }
    run_cleanups();
    // The other tasks run their cleanups too, before the process ends.
    tasks::end_all(PANIC_EXIT_CODE)
}

/// Panics with `code` and a message the runtime writes.
pub(crate) fn panic_with(code: u32, message: &str, location: *const Str) -> ! {
    let message = str_new(message.as_bytes());
    unsafe { tondo_rt_panic(code, message, location) }
}

/// The message of `index` outside the `len` elements of a `noun` (`P0001`).
pub(crate) fn out_of_bounds(index: i64, len: usize, noun: &str) -> String {
    if len == 0 {
        return format!("index {index} is out of bounds: the {noun} is empty");
    }
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("index {index} is out of bounds for {article} {noun} of length {len}: {}", valid_indices(len - 1, len))
}

/// The indices from 0 to `last`, and those from the end of `len` elements.
pub(crate) fn valid_indices(last: usize, len: usize) -> String {
    match (last, len) {
        (0, 0) => "the only valid index is 0".into(),
        (0, 1) => "the only valid indices are 0 and -1".into(),
        (_, 0) => format!("valid indices are 0 to {last}"),
        (_, 1) => format!("valid indices are 0 to {last}, and -1 from the end"),
        _ => format!("valid indices are 0 to {last}, and -{len} to -1 from the end"),
    }
}

/// A panic of compiled code whose message the runtime writes from the
/// values involved. `detail` holds the operation from bit 16, the
/// signedness of the right operand in bit 9 and of the left one in bit 8,
/// and the width of the left one in bits below; `ty` names its type.
/// Operations: `+ - * / %`, negation, `<< >>` from 0; for `P0001`, 0
/// indexes an array, 1 a C array and 2 a deque.
/// The values come in memory, `[a, b]`: as arguments, they would ask the
/// code that checks to keep them in the registers of a call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) -> ! {
    let message = unsafe { values_message(code, detail, values, ty) };
    panic_with(code, &message, location)
}

/// `tondo_rt_panic_values` where compiled code unwinds when it returns
/// (language 15.9).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values_unwind(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) {
    let message = unsafe { values_message(code, detail, values, ty) };
    raise(code, &message, location)
}

/// The message of a panic of compiled code, from its values, whose data it
/// stages.
pub(crate) unsafe fn values_message(code: u32, detail: u32, values: *const [u64; 4], ty: *const Str) -> String {
    // Two values of 128 bits, each extended by its signedness.
    let [a_low, a_high, b_low, b_high] = unsafe { *values };
    let (a, b) = ((a_high as i128) << 64 | a_low as i128, (b_high as i128) << 64 | b_low as i128);
    let (bits, op) = (detail & 0xff, detail >> 16);
    let show = |value: i128, signed: bool| if signed { value.to_string() } else { (value as u128).to_string() };
    let (left, right) = (show(a, detail & 0x100 != 0), show(b, detail & 0x200 != 0));
    let (a, b) = (a as i64, b as i64);
    let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
    let operator =
        ["+", "-", "*", "/", "%", "-", "<<", ">>", "pow", "nextPowerOfTwo"].get(op as usize).copied().unwrap_or("?");
    let range = || {
        let unused = 128 - bits.clamp(1, 128);
        match detail & 0x100 != 0 {
            true => format!("{} to {}", i128::MIN >> unused, i128::MAX >> unused),
            false => format!("0 to {}", u128::MAX >> unused),
        }
    };
    let int = |value: &str| value.parse::<i64>().map_or(panics::Datum::Text(value.to_string()), panics::Datum::Int);
    let data = match code {
        1 => vec![("index", panics::Datum::Int(a)), ("length", panics::Datum::Int(b))],
        3 => vec![("left", int(&left))],
        5 => vec![
            ("operation", panics::Datum::Text(operator.to_string())),
            ("left", int(&left)),
            ("right", int(&right)),
            ("type", panics::Datum::Text(ty.to_string())),
        ],
        6 => vec![("left", panics::Datum::Int(a)), ("right", panics::Datum::Int(b))],
        10 => vec![("count", int(&right)), ("width", panics::Datum::Int(i64::from(bits)))],
        18 => vec![("exponent", int(&right))],
        _ => Vec::new(),
    };
    panics::language_data(data);
    match code {
        1 if op == 1 => {
            format!("index {a} is out of bounds for the C array {ty}: the index must be from 0 to {}", b - 1)
        }
        1 if op == 2 => out_of_bounds(a, b as usize, "deque"),
        1 => out_of_bounds(a, b as usize, "array"),
        3 => format!("{left} {operator} 0"),
        5 if op == 5 => format!("-({left}) does not fit in {ty} ({})", range()),
        5 if op == 8 => format!("{left}.pow({right}) does not fit in {ty} ({})", range()),
        5 if op == 9 => format!("{left}.nextPowerOfTwo() does not fit in {ty} ({})", range()),
        5 if op == 10 => format!("{left} does not fit in {ty}"),
        18 => format!("{left}.pow({right}): the exponent must be 0 or more"),
        5 => format!("{left} {operator} {right} does not fit in {ty} ({})", range()),
        6 => format!("an array of length {a} cannot become the C array {ty}: it needs exactly {b} elements"),
        10 => format!("{left} {operator} {right}: a shift of {ty} must be from 0 to {}", bits - 1),
        _ => String::new(),
    }
}

/// Reports the error a fallible `main` returned (spec 20.10), of the type
/// `ty`: its text, or with `TONDO_PANIC_FORMAT=json` a line of
/// `tondo-panic-0.1/1` (toolchain 7.5).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_report_error(description: *const Str, ty: *const Str) {
    flush_stdout();
    let text = String::from_utf8_lossy(unsafe { str_bytes(description) });
    if panics::json_reports() {
        let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
        let _ = writeln!(
            std::io::stderr(),
            "{{\"code\":null,\"data\":null,\"error_type\":{},\"format\":\"tondo-panic-0.1/1\",\"kind\":\"error\",\"message\":{},\"name\":null,\"recovered\":false,\"source\":null,\"stack\":[]}}",
            diagnostics::quoted(&ty),
            diagnostics::quoted(&text),
        );
        return;
    }
    let _ = writeln!(std::io::stderr(), "error: {text}");
}

/// An internal invariant of the runtime or the generated code was broken.
pub(crate) fn runtime_fault(message: &str) -> ! {
    let _outside = replay::Outside::enter();
    flush_stdout();
    let _ = writeln!(std::io::stderr(), "tondo runtime fault: {message}{}", trace::report());
    diagnostics::crash_dump("fault", message, Vec::new(), false);
    std::process::abort()
}

/// Exit status when `TONDO_RT_CHECK_LEAKS` finds live strings at exit.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub const LEAK_EXIT_CODE: i32 = 99;

#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub fn check_leaks() -> Option<usize> {
    config::config().check_leaks.as_ref()?;
    // How long these waits take is no part of a recording.
    let _outside = replay::Outside::enter();
    tasks::wait_foreign_calls();
    tasks::wait_tasks_freed();
    // The contexts' initial values live as long as the program, and so do
    // the channels of signals.
    contexts::end();
    signals::end();
    // Garbage cycles are not leaks: the collector frees them. A thread of
    // the runtime may still be letting go of an object, as a read of a
    // stream that a reply had and a client closed: it gets a second.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        cycles::collect_all();
        let live = LIVE_OBJECTS.live();
        if live == 0 || std::time::Instant::now() >= deadline {
            return (live != 0).then_some(live);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
