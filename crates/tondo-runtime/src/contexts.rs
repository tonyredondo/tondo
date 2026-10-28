//! The contexts of the tasks (language 11.16). A task holds a table with a
//! `Ref` of each context's value; a `with` gives the task a new table for
//! the rest of its block and restores the old one at its end, and a task
//! shares the table of the one that spawns it. Tables never change once
//! made, so tasks share them without locks.

use super::{Glue, RefHeader};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, OnceLock};

/// What gives a context's initial value: a function of the program.
type Maker = unsafe extern "C" fn() -> *mut RefHeader;

#[derive(Clone, Copy)]
struct Entry {
    maker: Maker,
    release: Option<Glue>,
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
pub extern "C" fn tondo_rt_context_register(index: usize, maker: Maker, release: Option<Glue>) {
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if registered.len() <= index {
        registered.resize(index + 1, None);
    }
    registered[index] = Some(Entry { maker, release });
}

fn root() -> *mut Table {
    let root = ROOT.load(Ordering::Acquire);
    if !root.is_null() {
        return root;
    }
    let slots = entries().iter().map(|entry| unsafe { (entry.maker)() }).collect();
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots }));
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
    let made = Box::into_raw(Box::new(Table { count: AtomicUsize::new(1), slots }));
    std::mem::replace(&mut held.0, made)
}

/// The end of a `with`'s block: the table before it comes back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_context_restore(saved: *mut Table) {
    let Some(held) = current() else { return };
    let ended = std::mem::replace(&mut held.0, saved);
    unsafe { release(ended) };
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
