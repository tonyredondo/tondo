//! The contexts of the tasks (language 11.16). A task holds a table with a
//! `Ref` of each context's value; a `with` gives the task a new table for
//! the rest of its block and restores the old one at its end, and a task
//! shares the table of the one that spawns it. Tables never change once
//! made, so tasks share them without locks.

use super::{Glue, RefHeader, Str};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What gives a context's initial value: a function of the program. In a
/// live build, a function takes the state of a frame that moves to new
/// code, null in an ordinary call; another ignores it.
type Maker = unsafe extern "C" fn(*const u8) -> *mut RefHeader;
/// What shows the fields of a context marked `@log` or `@label`: names
/// and texts, each followed by a zero character.
type Shown = unsafe extern "C" fn(*const u8) -> *mut Str;

/// What writes a value as the debugger shows it.
type Show = unsafe extern "C" fn(*mut super::Builder, *const u8);

#[derive(Clone)]
struct Entry {
    maker: Maker,
    release: Option<Glue>,
    log: Option<Shown>,
    label: Option<Shown>,
    name: String,
    show: Option<Show>,
    computed: Vec<Arc<Computed>>,
    /// For a context with computed fields, each field's name, offset and
    /// show, for the tools.
    fields: Vec<(String, usize, Option<Show>)>,
}

/// A field whose initial value is computed (language 11.16): the function
/// that computes it, in a `Ref` of a record that holds only it, its name,
/// `Context.field`, and how far it got.
struct Computed {
    field: usize,
    maker: Maker,
    name: String,
    state: Mutex<State>,
}

enum State {
    Pending,
    /// A task computes it; those that read it meanwhile wait.
    Computing {
        task: u64,
        waiting: Vec<Arc<crate::tasks::Wait>>,
    },
    /// It lives as long as the program.
    Done(*mut RefHeader),
}

// The `Ref` it holds is shared, and never changes.
unsafe impl Send for State {}

impl Computed {
    /// The `Ref` that holds the value, if a task computed it.
    fn done(&self) -> Option<*mut RefHeader> {
        match *self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            State::Done(made) => Some(made),
            _ => None,
        }
    }

    /// The `Ref` that holds the value, which the first read computes while
    /// the others wait; none if computing it failed, panicked or was
    /// cancelled, which the next read tries again.
    fn value(&self, location: *const Str) -> Option<*mut RefHeader> {
        use crate::tasks::{CANCELLED, Wait, current_id, notify};
        let me = current_id();
        loop {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match &mut *state {
                State::Done(made) => return Some(*made),
                State::Computing { task, .. } if *task == me => {
                    drop(state);
                    let message = format!("the initial value of `{}` reads itself while it is computed", self.name);
                    super::raise(8, &message, location);
                    return None;
                }
                State::Computing { waiting, .. } => {
                    let wait = Wait::new();
                    waiting.push(wait.clone());
                    drop(state);
                    if wait.block_on_task() == CANCELLED {
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let State::Computing { waiting, .. } = &mut *state {
                            waiting.retain(|other| !Arc::ptr_eq(other, &wait));
                        }
                        return None;
                    }
                }
                State::Pending => {
                    *state = State::Computing { task: me, waiting: Vec::new() };
                    drop(state);
                    let made = unsafe { (self.maker)(std::ptr::null()) };
                    let after = if made.is_null() { State::Pending } else { State::Done(made) };
                    let before = std::mem::replace(&mut *self.state.lock().unwrap_or_else(|e| e.into_inner()), after);
                    if let State::Computing { waiting, .. } = before {
                        for wait in waiting {
                            if wait.claim(0) {
                                notify(wait.task);
                            }
                        }
                    }
                    return (!made.is_null()).then_some(made);
                }
            }
        }
    }
}

/// A `with` that changed fields of a context, for the tools (toolchain
/// 7.5, 7.6): where it is, which fields, by a bit each, the task that ran
/// it, and the one before.
pub(crate) struct Origin {
    at: String,
    fields: u64,
    task: u64,
    previous: Option<Arc<Origin>>,
}

/// The contexts the program registers as it starts, by index.
static REGISTERED: Mutex<Vec<Option<Entry>>> = Mutex::new(Vec::new());
static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
/// The table of the initial values, made when a task first reads one;
/// it lives as long as the program.
static ROOT: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());

pub(crate) struct Table {
    count: AtomicUsize,
    slots: Vec<*mut RefHeader>,
    /// The fields of each context a `with` set, a bit each: the others
    /// that are computed are read where they are computed.
    set: Vec<u64>,
    /// The fields marked `@label` its values give, for the task's labels.
    labels: Vec<(String, String)>,
    /// The last `with` that changed each context, none for an initial value.
    origins: Vec<Option<Arc<Origin>>>,
}

/// A task's table; none until a `with` changes it, which is the initial
/// values'.
pub(crate) struct Held(*mut Table);

impl Held {
    pub(crate) const NONE: Held = Held(std::ptr::null_mut());
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

fn entries() -> &'static [Entry] {
    ENTRIES.get_or_init(|| {
        let registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        registered.iter().map(|entry| entry.clone().expect("every context is registered")).collect()
    })
}

/// Each context's maker and the release of its record, as the program
/// starts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_register(
    index: usize,
    maker: Maker,
    release: Option<Glue>,
    log: Option<Shown>,
    label: Option<Shown>,
    name: *const Str,
    show: Option<Show>,
) {
    let name = String::from_utf8_lossy(unsafe { super::str_bytes(name) }).into_owned();
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if registered.len() <= index {
        registered.resize(index + 1, None);
    }
    registered[index] =
        Some(Entry { maker, release, log, label, name, show, computed: Vec::new(), fields: Vec::new() });
}

/// A field of a context with computed fields, as the tools show it: its
/// offset in the record, how it shows, and its name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_shows(index: usize, offset: usize, show: Option<Show>, name: *const Str) {
    let name = String::from_utf8_lossy(unsafe { super::str_bytes(name) }).into_owned();
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = registered[index].as_mut().expect("the context is registered first");
    entry.fields.push((name, offset, show));
}

/// A field of the context at `index` whose initial value is computed, and
/// the function that computes it, as the program starts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_computed(index: usize, field: usize, maker: Maker, name: *const Str) {
    let name = String::from_utf8_lossy(unsafe { super::str_bytes(name) }).into_owned();
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = registered[index].as_mut().expect("the context is registered first");
    entry.computed.push(Arc::new(Computed { field, maker, name, state: Mutex::new(State::Pending) }));
}

/// A context of a task, as the tools show it.
pub(crate) struct Described {
    pub name: String,
    pub fields: Vec<Field>,
}

/// A field of a context: its text, how many parts it has, and the `with`
/// that gave it its value, with whether another task ran it, none for its
/// initial value.
pub(crate) struct Field {
    pub name: String,
    pub value: String,
    pub parts: usize,
    pub set: Option<(String, bool)>,
}

/// The contexts of `task`, which runs no Tondo code meanwhile.
pub(crate) unsafe fn described(task: *mut crate::tasks::Task) -> Vec<Described> {
    let Some(task) = (unsafe { task.as_ref() }) else { return Vec::new() };
    let table = table(Some(&task.context));
    let mut out = Vec::new();
    for (index, entry) in entries().iter().enumerate() {
        let Some(show) = entry.show else { continue };
        let slot = unsafe { (&(*table).slots)[index] };
        if !entry.fields.is_empty() {
            let set = unsafe { (&(*table).set)[index] };
            let origin = unsafe { (&(*table).origins)[index].clone() };
            out.push(Described {
                name: entry.name.clone(),
                fields: unsafe { fields(entry, slot, set, origin, task.id) },
            });
            continue;
        }
        let mut builder = super::Builder::new(0);
        unsafe { show(&mut builder, super::ref_payload(slot as *mut u8)) };
        let text = builder.finish();
        let whole = String::from_utf8_lossy(unsafe { super::str_bytes(text) }).into_owned();
        unsafe { super::tondo_rt_str_release(text) };
        let parsed = super::shown::parse(&whole);
        let origin = unsafe { (&(*table).origins)[index].clone() };
        let fields = parsed
            .parts
            .iter()
            .enumerate()
            .map(|(at, (name, part))| {
                let mut node = origin.as_deref();
                while let Some(found) = node.filter(|o| o.fields & 1 << at.min(63) == 0) {
                    node = found.previous.as_deref();
                }
                let set = node.map(|o| (o.at.clone(), o.task != task.id));
                let name = name.trim_start_matches('.').to_string();
                Field { name, value: part.text.to_string(), parts: part.parts.len(), set }
            })
            .collect();
        out.push(Described { name: entry.name.clone(), fields });
    }
    out
}

/// The fields of a context with computed fields, one by one: each from the
/// task's record, or, for a computed one no `with` set, from where it was
/// computed, if it was.
unsafe fn fields(entry: &Entry, slot: *mut RefHeader, set: u64, origin: Option<Arc<Origin>>, task: u64) -> Vec<Field> {
    let mut out = Vec::new();
    for (at, (name, offset, show)) in entry.fields.iter().enumerate() {
        let bit = 1u64 << at.min(63);
        let holder = match entry.computed.iter().find(|c| c.field == at) {
            Some(computed) if set & bit == 0 => computed.done(),
            _ => Some(slot),
        };
        let value = match (holder, show) {
            (None, _) => "(computed when first read)".to_string(),
            (Some(_), None) => "()".to_string(),
            (Some(holder), Some(show)) => {
                let mut builder = super::Builder::new(0);
                unsafe { show(&mut builder, (super::ref_payload(holder as *mut u8) as *const u8).add(*offset)) };
                let text = builder.finish();
                let value = String::from_utf8_lossy(unsafe { super::str_bytes(text) }).into_owned();
                unsafe { super::tondo_rt_str_release(text) };
                value
            }
        };
        let mut node = origin.as_deref();
        while let Some(found) = node.filter(|o| o.fields & bit == 0) {
            node = found.previous.as_deref();
        }
        let set = node.map(|o| (o.at.clone(), o.task != task));
        let parts = super::shown::parse(&value).parts.len();
        out.push(Field { name: name.clone(), value, parts, set });
    }
    out
}

/// The names and texts of the fields of the running task's contexts marked
/// `@log`, or `@label`, that do not have their initial values. A `Display`
/// that panics while they are shown shows none.
pub(crate) fn shown(label: bool) -> Vec<(String, String)> {
    thread_local! {
        static SHOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if SHOWING.with(|showing| showing.replace(true)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in entries() {
        let Some(show) = (if label { entry.label } else { entry.log }) else { continue };
        let text = unsafe { show(std::ptr::null()) };
        let mut parts = unsafe { super::str_bytes(text) }.split(|&b| b == 0);
        while let (Some(name), Some(value)) = (parts.next(), parts.next()) {
            out.push((String::from_utf8_lossy(name).into_owned(), String::from_utf8_lossy(value).into_owned()));
        }
        unsafe { super::tondo_rt_str_release(text) };
    }
    SHOWING.with(|showing| showing.set(false));
    out
}

/// `std.log`'s fields of the contexts: those marked `@log`, each name and
/// text followed by a zero character.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_context_logged() -> *mut Str {
    let mut text = Vec::new();
    for (name, value) in shown(false) {
        text.extend_from_slice(name.as_bytes());
        text.push(0);
        text.extend_from_slice(value.as_bytes());
        text.push(0);
    }
    super::str_new(&text)
}

/// Whether a context has fields marked `@label`.
fn labelled() -> bool {
    static LABELLED: OnceLock<bool> = OnceLock::new();
    *LABELLED.get_or_init(|| entries().iter().any(|entry| entry.label.is_some()))
}

/// The labels the running task's contexts give (`std.profile`).
pub(crate) fn labels() -> Vec<(String, String)> {
    let Some(held) = current() else { return Vec::new() };
    match held.0.is_null() {
        true => Vec::new(),
        false => unsafe { (*held.0).labels.clone() },
    }
}

fn root() -> *mut Table {
    let root = ROOT.load(Ordering::Acquire);
    if !root.is_null() {
        return root;
    }
    let slots = entries().iter().map(|entry| unsafe { (entry.maker)(std::ptr::null()) }).collect();
    let origins = vec![None; entries().len()];
    let set = vec![0; entries().len()];
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots, set, labels: Vec::new(), origins }));
    match ROOT.compare_exchange(std::ptr::null_mut(), made, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(other) => {
            unsafe { free(made) };
            other
        }
    }
}

fn current<'a>() -> Option<&'a mut Held> {
    unsafe { crate::tasks::current().as_mut() }.map(|task| &mut task.context)
}

/// The table a task reads: its own, or the initial values'.
fn table(held: Option<&Held>) -> *mut Table {
    match held {
        Some(held) if !held.0.is_null() => held.0,
        _ => root(),
    }
}

/// What a task it spawns starts with: a share of the running task's table.
pub(crate) fn inherited() -> Held {
    let Some(held) = current() else { return Held::NONE };
    if !held.0.is_null() {
        unsafe { (*held.0).count.fetch_add(1, Ordering::Relaxed) };
    }
    Held(held.0)
}

/// The `Ref` of a context's value where the task runs, which its table
/// holds while it runs.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_context_get(index: usize) -> *mut RefHeader {
    let table = table(current().map(|held| &*held));
    unsafe { (&(*table).slots)[index] }
}

/// The `Ref` that holds `field` of a context where the task runs, a field
/// whose initial value is computed: the task's, if a `with` set it, or the
/// one its first read computes, at `location`. If computing it failed, the
/// task's, whose zeros the code does not use: it unwinds.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_context_field(index: usize, field: usize, location: *const Str) -> *mut RefHeader {
    let table = table(current().map(|held| &*held));
    let slot = unsafe { (&(*table).slots)[index] };
    if unsafe { (&(*table).set)[index] } & 1 << field.min(63) != 0 {
        return slot;
    }
    let computed = entries()[index].computed.iter().find(|c| c.field == field);
    computed.and_then(|c| c.value(location)).unwrap_or(slot)
}

/// A `with` at `at`, which changes `fields`, a bit each: the task's table
/// becomes a new one with `value` for the context; the old one, which the
/// block restores, is given back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_set(
    index: usize,
    value: *mut RefHeader,
    fields: u64,
    at: *const Str,
) -> *mut Table {
    let Some(held) = current() else { return std::ptr::null_mut() };
    let base = table(Some(held));
    let mut slots = unsafe { (*base).slots.clone() };
    slots[index] = value;
    for &slot in &slots {
        unsafe { (*slot).rc.fetch_add(1, Ordering::Relaxed) };
    }
    let mut set = unsafe { (*base).set.clone() };
    set[index] |= fields;
    let mut origins = unsafe { (*base).origins.clone() };
    let at = String::from_utf8_lossy(unsafe { super::str_bytes(at) }).into_owned();
    let previous = origins[index].take();
    origins[index] = Some(Arc::new(Origin { at, fields, task: super::tasks::current_id(), previous }));
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots, set, labels: Vec::new(), origins }));
    let saved = std::mem::replace(&mut held.0, made);
    // Its labels, which the new values give.
    if labelled() {
        let labels = shown(true);
        unsafe { (*made).labels = labels };
        super::labels::refresh();
    }
    saved
}

/// The end of a `with`'s block: the table before it comes back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_restore(saved: *mut Table) {
    let Some(held) = current() else { return };
    let ended = std::mem::replace(&mut held.0, saved);
    unsafe { release(ended) };
    if labelled() {
        super::labels::refresh();
    }
}

/// The instant deadlines count from, which a task's inherited one is kept
/// as nanoseconds since, `u64::MAX` for none.
fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

/// The deadline of the running task: its innermost `within`'s, or the one
/// of the block that spawned it (11.17).
fn deadline() -> Option<Instant> {
    let task = unsafe { crate::tasks::current().as_ref() }?;
    if let Some(&(at, _)) = task.withins.last() {
        return Some(at);
    }
    match task.inherited.load(Ordering::Acquire) {
        u64::MAX => None,
        nanos => Some(base() + Duration::from_nanos(nanos)),
    }
}

/// The running task's deadline for a task it spawns, which reads it.
pub(crate) fn deadline_nanos() -> u64 {
    deadline().map_or(u64::MAX, |at| at.saturating_duration_since(base()).as_nanos() as u64)
}

/// `within limit { ... }` starts: its deadline is the nearer of `limit`
/// from now and the one that holds.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_within_enter(limit: i64) {
    let Some(task) = (unsafe { crate::tasks::current().as_mut() }) else { return };
    let at = Instant::now() + Duration::from_nanos(limit.max(0) as u64);
    let level = task.withins.len();
    let held = match task.withins.last() {
        Some(&(outer, owner)) if outer <= at => (outer, owner),
        _ => (at, level),
    };
    task.withins.push(held);
}

/// The block of a `within` ends: a deadline of its that passed with no
/// check after it is gone with it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_within_leave() {
    let Some(task) = (unsafe { crate::tasks::current().as_mut() }) else { return };
    let level = task.withins.len().saturating_sub(1);
    if task.withins.pop().is_some_and(|(_, owner)| owner == level) && task.expired {
        task.expired = false;
        crate::tasks::attend(false);
    }
}

/// How long until the running task's deadline, in nanoseconds: zero once it
/// passed, -1 without one.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_deadline_remaining() -> i64 {
    match deadline() {
        Some(at) => at.saturating_duration_since(Instant::now()).as_nanos().min(i64::MAX as u128) as i64,
        None => -1,
    }
}

/// The program ends: the initial values go, for the check of leaks.
pub(crate) fn end() {
    let root = ROOT.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !root.is_null() {
        unsafe { free(root) };
    }
    let Some(entries) = ENTRIES.get() else { return };
    for entry in entries {
        for computed in &entry.computed {
            let mut state = computed.state.lock().unwrap_or_else(|e| e.into_inner());
            if let State::Done(made) = std::mem::replace(&mut *state, State::Pending) {
                unsafe { super::tondo_rt_ref_release(made, entry.release) };
            }
        }
    }
}

unsafe fn release(table: *mut Table) {
    if table.is_null() || table == ROOT.load(Ordering::Relaxed) {
        return;
    }
    if unsafe { (*table).count.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    fence(Ordering::Acquire);
    unsafe { free(table) };
}

unsafe fn free(table: *mut Table) {
    let table = unsafe { Box::from_raw(table) };
    for (slot, entry) in table.slots.iter().zip(entries()) {
        unsafe { super::tondo_rt_ref_release(*slot, entry.release) };
    }
}
