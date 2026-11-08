//! The bindings of a stopped task (docs/refactor.md, phase 6): what each of
//! its frames holds, read where the probes said, and shown as the debugger
//! writes values.

use super::*;

/// A frame of a held task: where it is, its function, its frame pointer,
/// and where its bindings and the bits of those that hold a value are, or
/// 0 when that is not known: a function none of whose probes ran with a
/// debugger attached.
#[derive(Clone)]
pub(super) struct Frame {
    pub(super) place: String,
    pub(super) function: Option<usize>,
    pub(super) pointer: usize,
    pub(super) addresses: usize,
    pub(super) alive: usize,
    /// Where its code is at, in the frames below the first.
    pub(super) pc: usize,
}

/// The frames of a held task, innermost first, as `stack` lists them; of
/// one that waits, those of the program's functions its stack has.
pub(super) fn frames(table: &Table, held: &Stop) -> Vec<Frame> {
    let function = table.sites.get(held.site).map(|s| s.function);
    let place = place_in(table, held.site);
    let first = Frame { place, function, pointer: held.frame, addresses: held.addresses, alive: held.alive, pc: 0 };
    let mut out = if held.site == WAITING { Vec::new() } else { vec![first] };
    if held.site == usize::MAX {
        return out;
    }
    for (fp, address) in crate::trace::callers(held.frame, held.bounds) {
        let Some(described) = crate::trace::describe(address) else { continue };
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
        out.push(Frame { place, function, pointer: fp, addresses, alive, pc: address });
    }
    out
}

/// The bindings of a frame that hold a value, with where each is.
pub(super) fn bound<'t>(table: &'t Table, frame: &Frame) -> Vec<(&'t Binding, usize)> {
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
pub(super) struct Found {
    pub(super) ty: Option<String>,
    pub(super) text: String,
}

pub(super) fn resolve(table: &Table, frames: &[Frame], index: usize, path: &str) -> Result<Found, String> {
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
    match crate::shown::find(&whole, rest) {
        Some(part) => Ok(Found { ty: None, text: part.to_string() }),
        None => Err(format!("`{path}` is not a part of `{root}`, which is {}", clipped(&whole))),
    }
}

pub(super) fn missing(name: &str, bound: &[(&Binding, usize, Option<&str>)]) -> String {
    let names: Vec<&str> = bound.iter().map(|(b, ..)| b.name.as_str()).collect();
    match names.is_empty() {
        true => format!("no binding `{name}` holds a value here"),
        false => format!("no binding `{name}` holds a value here; these do: {}", names.join(", ")),
    }
}

/// Runs `f` with an evaluator of expressions that reads the bindings the
/// code of a frame can use.
pub(super) fn evaluating<T>(table: &Table, frames: &[Frame], index: usize, f: impl FnOnce(&mut Evaluator) -> T) -> T {
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
pub(super) const SHOWN: usize = 4096;

/// A value as the debugger shows it.
pub(super) fn show(glue: Show, address: usize) -> String {
    // A program with views shows through the program's code, which runs on
    // a task: the stopped one, when the debugger's thread asks.
    if VIEWED.load(Ordering::Relaxed) && crate::tasks::current().is_null() {
        let task = state().stopped.as_ref().filter(|s| s.site < WAITING).map(|s| s.task);
        if let Some(shown) = task.and_then(|task| on_task(task, || shown_here(glue, address))) {
            return shown;
        }
    }
    shown_here(glue, address)
}

/// A value as the debugger shows it, on this thread; a view's code that
/// runs meanwhile passes its probes.
pub(super) fn shown_here(glue: Show, address: usize) -> String {
    let before = SHOWING.replace(true);
    let mut builder = crate::Builder::new(0);
    unsafe { glue(&mut builder, address as *const u8) };
    let text = builder.finish();
    let shown = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    unsafe { crate::tondo_rt_str_release(text) };
    SHOWING.set(before);
    shown
}

thread_local! {
    /// This thread shows a value: the code of a view runs to its end.
    pub(super) static SHOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The program has types with a `debug.View`.
pub(super) static VIEWED: AtomicBool = AtomicBool::new(false);

/// Work the debugger's thread asks a held task to do on its own stack: the
/// task and the work; and whether it is done.
#[allow(clippy::type_complexity)]
pub(super) static WORK: Mutex<Option<(u64, Box<dyn FnOnce() + Send>)>> = Mutex::new(None);
pub(super) static WORKED: Mutex<bool> = Mutex::new(false);
pub(super) static WORK_DONE: Condvar = Condvar::new();

/// Runs `work` on the held task `task`, as the debugger's thread waits:
/// `None` when the task does not take it within a while.
pub(super) fn on_task<T: Send + 'static>(task: u64, work: impl FnOnce() -> T + Send) -> Option<T> {
    // Not on this thread's stack: a checkpoint forks in the job and goes on
    // in the copy without this thread, whose stack the next thread there gets.
    let out: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
    let place = out.clone();
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        let done = work();
        *place.lock().unwrap_or_else(|e| e.into_inner()) = Some(done);
    });
    // The job borrows what this thread holds, which waits until it ran.
    let job: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(job) };
    *WORKED.lock().unwrap_or_else(|e| e.into_inner()) = false;
    let asking = state();
    *WORK.lock().unwrap_or_else(|e| e.into_inner()) = Some((task, job));
    drop(asking);
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
    out.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub(super) fn clipped(text: &str) -> String {
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
pub(super) fn described(text: &str) -> String {
    let value = crate::shown::parse(text);
    let indexed = if value.indexed && !value.parts.is_empty() { ",\"indexed\":true" } else { "" };
    format!("\"value\":{},\"children\":{}{indexed}", quoted(&clipped(text)), value.parts.len())
}
