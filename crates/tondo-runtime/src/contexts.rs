//! The contexts of the tasks (language 11.16). A task holds a table with a
//! `Ref` of each context's value; a `with` gives the task a new table for
//! the rest of its block and restores the old one at its end, and a task
//! shares the table of the one that spawns it. Tables never change once
//! made, so tasks share them without locks.

use super::{Glue, RefHeader, Str};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What gives a context's initial value: a function of the program. In a
/// live build, a function takes the state of a frame that moves to new
/// code, null in an ordinary call; another ignores it.
type Maker = unsafe extern "C" fn(*const u8) -> *mut RefHeader;
/// What shows the fields of a context marked `@log` or `@label`: names
/// and texts, each followed by a zero character.
type Shown = unsafe extern "C" fn(*const u8) -> *mut Str;

#[derive(Clone, Copy)]
struct Entry {
    maker: Maker,
    release: Option<Glue>,
    log: Option<Shown>,
    label: Option<Shown>,
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
    /// The fields marked `@label` its values give, for the task's labels.
    labels: Vec<(String, String)>,
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
        registered.iter().map(|entry| entry.expect("every context is registered")).collect()
    })
}

/// Each context's maker and the release of its record, as the program
/// starts.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_context_register(
    index: usize,
    maker: Maker,
    release: Option<Glue>,
    log: Option<Shown>,
    label: Option<Shown>,
) {
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if registered.len() <= index {
        registered.resize(index + 1, None);
    }
    registered[index] = Some(Entry { maker, release, log, label });
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
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots, labels: Vec::new() }));
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

/// A `with`: the task's table becomes a new one with `value` for the
/// context; the old one, which the block restores, is given back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_set(index: usize, value: *mut RefHeader) -> *mut Table {
    let Some(held) = current() else { return std::ptr::null_mut() };
    let base = table(Some(held));
    let mut slots = unsafe { (*base).slots.clone() };
    slots[index] = value;
    for &slot in &slots {
        unsafe { (*slot).rc.fetch_add(1, Ordering::Relaxed) };
    }
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots, labels: Vec::new() }));
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
