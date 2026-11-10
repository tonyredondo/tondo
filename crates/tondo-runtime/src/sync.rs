//! `std.sync` (standard library 14.4.4): shared state behind locks that
//! park tasks instead of blocking threads, semaphores and barriers.

use super::cycles::{self, Kind, Taken, Visit, Visitor};
use super::tasks::{CANCELLED, Task, Wait, current, lock, notify, stops, stops_task, unlock};
use super::{Glue, LIVE_OBJECTS, Str};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// The state of a `Mutex`, `RwLock`, `Atomic` or `Once`, and its lock:
/// several tasks may hold it to read, one to change it. Taking a free lock
/// is one atomic operation on `state`; a task that finds it taken spins a
/// little and then parks in `waiting`. Releasing it wakes one waiter, which
/// tries again, so a task that is running can take it first instead of
/// waiting for the woken one to be scheduled. A woken task that still loses
/// after waiting `STARVATION` turns the lock to starving: then nobody takes
/// it ahead of the queue, and releasing it hands it to the first waiter.
/// `rc` counts the handles.
#[repr(C)]
pub struct SyncObject {
    rc: AtomicUsize,
    /// `WRITER`, `PARKED`, and the number of readers in units of `READER`.
    state: AtomicUsize,
    /// The value: after the cell, in its allocation, until a reload puts
    /// one of another type on the heap. Compiled code reads it at 16
    /// (`SYNC_VALUE` in codegen).
    value: *mut u8,
    /// Guards `waiting` and `changes`.
    queue: AtomicBool,
    /// Tasks waiting to take it, whether to change it, and since when.
    waiting: VecDeque<(*const Wait, bool, std::time::Instant)>,
    /// Tasks in `updateWhen` waiting for the value to change.
    changes: Vec<*const Wait>,
    size: usize,
    align: usize,
    /// The size and alignment the cell was made with room for.
    room: (usize, usize),
    release: Option<Glue>,
    /// Visits the state for the cycle collector, when it may refer back
    /// to the cell, which is then registered with it.
    visit: Option<Visit>,
    /// The task that last let go of the lock to change the value, which
    /// the debugger's watch of the cell stops.
    writer: std::sync::atomic::AtomicU64,
    /// The task that holds it to change it, where it took it, and the cell
    /// that task took before and still holds: a task's exclusive locks are
    /// a chain through their cells (`Task.locked`), which taking one
    /// writes in the cell just taken, rather than a list of the task's.
    owner: AtomicUsize,
    taken_at: std::sync::atomic::AtomicPtr<Str>,
    below: std::sync::atomic::AtomicPtr<SyncObject>,
}

const _: () = assert!(std::mem::offset_of!(SyncObject, value) == 16);

/// A cell's allocation with room for a value of `size` bytes after it, and
/// where the value starts.
fn cell_layout((size, align): (usize, usize)) -> (std::alloc::Layout, usize) {
    let value = std::alloc::Layout::from_size_align(size.max(1), align).unwrap();
    std::alloc::Layout::new::<SyncObject>().extend(value).unwrap()
}

/// Frees a value a reload put on the heap; the one in the cell's room goes
/// with the cell.
unsafe fn free_value(o: &SyncObject, value: *mut u8, size: usize, align: usize) {
    let room = unsafe { (o as *const SyncObject as *mut u8).add(cell_layout(o.room).1) };
    if value != room {
        unsafe { std::alloc::dealloc(value, std::alloc::Layout::from_size_align(size.max(1), align).unwrap()) };
    }
}

/// The task that last changed the cell's value, or 0.
pub(crate) fn last_writer(object: usize) -> u64 {
    unsafe { (*(object as *const SyncObject)).writer.load(Ordering::Relaxed) }
}

const WRITER: usize = 1;
/// Some task waits in `waiting` or `changes`.
const PARKED: usize = 2;
/// Releases hand the lock over in order; nobody takes it ahead.
const STARVING: usize = 4;
const READER: usize = 8;

/// How long a woken task may lose to others before the lock starves.
const STARVATION: std::time::Duration = std::time::Duration::from_millis(1);
/// The index a wait is claimed with when the lock was handed to its task.
const HANDED: usize = 1;

const SHARED: u8 = 0;
const TRY: u8 = 2;
const SPIN_ROUNDS: u32 = 8;

/// A new cell for a value of `size` bytes, which compiled code stores at
/// the address the cell keeps at 16.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sync_new(
    size: usize,
    align: usize,
    release: Option<Glue>,
    visit: Option<Visit>,
) -> *mut SyncObject {
    let align = align.max(8);
    let (layout, offset) = cell_layout((size, align));
    LIVE_OBJECTS.made();
    let cell = unsafe { std::alloc::alloc(layout) } as *mut SyncObject;
    if cell.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    unsafe {
        cell.write(SyncObject {
            rc: AtomicUsize::new(1),
            state: AtomicUsize::new(0),
            queue: AtomicBool::new(false),
            waiting: VecDeque::new(),
            changes: Vec::new(),
            value: (cell as *mut u8).add(offset),
            size,
            align,
            room: (size, align),
            release,
            visit,
            writer: std::sync::atomic::AtomicU64::new(0),
            owner: AtomicUsize::new(0),
            taken_at: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            below: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        })
    };
    if visit.is_some() {
        cycles::remember(cell as *const u8, Kind::Sync);
    }
    cell
}

/// Visits the state for the cycle collector.
pub(crate) unsafe fn visit(visitor: &mut Visitor, cell: *mut u8) {
    let o = unsafe { &*(cell as *const SyncObject) };
    unsafe { visitor.visit(o.visit, o.value) };
}

/// Takes the state out of a garbage cell.
pub(crate) unsafe fn clear(cell: *mut u8) -> Option<Taken> {
    let o = unsafe { &mut *(cell as *mut SyncObject) };
    let release = o.release.take()?;
    let mut state = vec![0u64; o.size.div_ceil(8).max(1)];
    unsafe { std::ptr::copy_nonoverlapping(o.value, state.as_mut_ptr() as *mut u8, o.size) };
    Some(Box::new(move || unsafe { release(state.as_mut_ptr() as *mut u8) }))
}

/// A guard is the object's address, plus 1 when it holds it to read.
fn object(guard: usize) -> *mut SyncObject {
    (guard & !1) as *mut SyncObject
}

/// A cell's value, of another type now: what releases and visits it goes
/// with it, and the old value goes with what released it.
pub(crate) unsafe fn sync_commit(
    cell: usize,
    value: *mut u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    visit: Option<cycles::Visit>,
) {
    let o = unsafe { &mut *(cell as *mut SyncObject) };
    let (old, old_size, old_align, old_release) = (o.value, o.size, o.align, o.release);
    let remembered = o.visit.is_some();
    (o.value, o.size, o.align, o.release, o.visit) = (value, size, align, release, visit);
    match (remembered, visit.is_some()) {
        (false, true) => cycles::remember(cell as *const u8, Kind::Sync),
        (true, false) => cycles::forget(cell as *const u8),
        _ => {}
    }
    unsafe {
        if let Some(release) = old_release {
            release(old);
        }
        free_value(o, old, old_size, old_align);
    }
}

/// Whether a task holds a cell's lock, and so its value.
pub(crate) fn sync_locked(cell: usize) -> bool {
    let state = unsafe { (*(cell as *mut SyncObject)).state.load(Ordering::Acquire) };
    state & (WRITER | !(READER - 1)) != 0
}

/// A cell's value, which a reload converts.
pub(crate) fn sync_value(cell: usize) -> *mut u8 {
    unsafe { (*(cell as *mut SyncObject)).value }
}

/// The address of the value of a cell, given it or a guard on it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_value(handle: usize) -> *mut u8 {
    if handle == 0 { std::ptr::null_mut() } else { unsafe { (*object(handle)).value } }
}

/// Takes the lock if it is free for this kind of hold.
fn try_take(o: &SyncObject, exclusive: bool) -> bool {
    let mut state = o.state.load(Ordering::Relaxed);
    loop {
        let next = if exclusive {
            if state & !PARKED != 0 {
                return false;
            }
            state | WRITER
        } else {
            if state & (WRITER | STARVING) != 0 {
                return false;
            }
            state + READER
        };
        match o.state.compare_exchange_weak(state, next, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(seen) => state = seen,
        }
    }
}

/// The locks a task holds, read once for each operation: finding the
/// running task costs a call.
fn held(task: *mut Task) -> &'static mut Vec<super::tasks::Held> {
    unsafe { &mut (*task).held }
}

/// Records a lock the task took to read, in room a task that ended left
/// at its first.
fn hold(holds: &mut Vec<super::tasks::Held>, lock: super::tasks::Held) {
    if holds.capacity() == 0 {
        *holds = super::tasks::held_room();
    }
    holds.push(lock);
}

/// Records that `task` took the cell, to change it (`exclusive`), at `at`.
fn took(o: &SyncObject, task: *mut Task, exclusive: bool, at: *const Str) {
    if !exclusive {
        return hold(held(task), (o as *const SyncObject as usize, false, at));
    }
    let locked = unsafe { &(*task).locked };
    o.owner.store(task as usize, Ordering::Relaxed);
    o.taken_at.store(at as *mut Str, Ordering::Relaxed);
    o.below.store(locked.load(Ordering::Relaxed) as *mut SyncObject, Ordering::Relaxed);
    locked.store(o as *const SyncObject as usize, Ordering::Relaxed);
}

/// Takes the cell out of what `task` holds: of its chain, where it is most
/// often the last taken, when it held it to change it.
fn gave_up(o: &SyncObject, task: *mut Task, exclusive: bool) {
    let address = o as *const SyncObject as usize;
    if !exclusive {
        let holds = held(task);
        if let Some(index) = holds.iter().position(|&(h, _, _)| h == address) {
            holds.swap_remove(index);
        }
        return;
    }
    o.owner.store(0, Ordering::Relaxed);
    let below = o.below.load(Ordering::Relaxed);
    let locked = unsafe { &(*task).locked };
    if locked.load(Ordering::Relaxed) == address {
        return locked.store(below as usize, Ordering::Relaxed);
    }
    let mut at = locked.load(Ordering::Relaxed) as *mut SyncObject;
    while let Some(cell) = unsafe { at.as_ref() } {
        let next = cell.below.load(Ordering::Relaxed);
        if next as usize == address {
            return cell.below.store(below, Ordering::Relaxed);
        }
        at = next;
    }
}

/// The locks a task holds, in the order it took them, for a dump, a
/// debugger and the order of locks.
pub(crate) fn locks_of(task: &Task) -> Vec<super::tasks::Held> {
    let mut locks = Vec::new();
    let mut at = task.locked.load(Ordering::Relaxed) as *const SyncObject;
    while let Some(cell) = unsafe { at.as_ref() } {
        locks.push((at as usize, true, cell.taken_at.load(Ordering::Relaxed) as *const Str));
        at = cell.below.load(Ordering::Relaxed);
    }
    locks.reverse();
    locks.extend(task.held.iter().copied());
    locks
}

/// Panics with `P0014` when `task` holds the cell in a way that taking it
/// again, to change it when `exclusive`, would wait for itself forever.
fn reentrant(o: &SyncObject, task: *mut Task, exclusive: bool, location: *const Str) -> bool {
    let address = o as *const SyncObject as usize;
    let found = match o.owner.load(Ordering::Relaxed) == task as usize {
        true => Some((true, o.taken_at.load(Ordering::Relaxed) as *const Str)),
        false if exclusive => held(task).iter().find(|&&(h, ..)| h == address).map(|&(_, _, at)| (false, at)),
        false => None,
    };
    let Some((alone, taken)) = found else { return false };
    let how = |exclusive: bool| if exclusive { "to change its value" } else { "to read it" };
    let mut message = format!(
        "this task already holds this lock {}, so taking it again {} would wait for itself forever",
        how(alone),
        how(exclusive)
    );
    if !taken.is_null() {
        message.push_str(&format!(": it took it at {}", String::from_utf8_lossy(unsafe { super::str_bytes(taken) })));
    }
    // A `recover` stops it: the code unwinds and lets the lock go.
    super::raise(14, &message, location);
    true
}

/// Takes the lock to read (`mode` 0), to change (1), or to change if it is
/// free (2). Returns the guard, or 0 when `try` finds it taken or the task
/// is cancelled (in a `defer`, only when it would wait) or unwinds. Taking it again from the task that holds it,
/// in a way that would wait forever, panics with `P0014`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_lock(object: *mut SyncObject, mode: u8, location: *const Str) -> usize {
    // One that has to wait touches it as much as one that takes it.
    super::tasks::touch(object as usize);
    let guard = unsafe { taken(object, mode, location) };
    if guard != 0 && super::race::on() {
        match mode {
            SHARED => super::race::read(object as usize),
            _ => super::race::wrote(object as usize, false),
        }
    }
    guard
}

unsafe fn taken(object: *mut SyncObject, mode: u8, location: *const Str) -> usize {
    let task = current();
    if stops_task(task) {
        return 0;
    }
    let exclusive = mode != SHARED;
    let address = object as usize;
    let o = unsafe { &mut *object };
    // A lock the task holds is taken, so the first try finds whether it
    // does; the order of locks asks first.
    let mut checked = super::lockorder::on();
    if checked {
        if reentrant(o, task, exclusive, location) {
            return 0;
        }
        let locks = locks_of(unsafe { &*task });
        if !locks.is_empty()
            && let Some(message) = super::lockorder::taking(address, exclusive, location, &locks)
        {
            super::raise(19, &message, location);
            return 0;
        }
    }
    let guard = address | (!exclusive) as usize;
    let mut spins = 0;
    let mut since = None;
    let mut woken = false;
    loop {
        if try_take(o, exclusive) {
            took(o, task, exclusive, location);
            return guard;
        }
        if !std::mem::replace(&mut checked, true) && reentrant(o, task, exclusive, location) {
            return 0;
        }
        if mode == TRY {
            return 0;
        }
        // Locks are usually held briefly: spin a little before parking.
        if spins < SPIN_ROUNDS {
            for _ in 0..1 << spins {
                std::hint::spin_loop();
            }
            spins += 1;
            continue;
        }
        let wait = Wait::new();
        lock(&o.queue);
        o.state.fetch_or(PARKED, Ordering::AcqRel);
        if try_take(o, exclusive) {
            unlock(&o.queue);
            took(o, task, exclusive, location);
            return guard;
        }
        // A task woken before goes first, and starves the lock once it
        // has lost for too long.
        let since = *since.get_or_insert_with(std::time::Instant::now);
        if woken {
            o.waiting.push_front((Arc::as_ptr(&wait), exclusive, since));
            if since.elapsed() > STARVATION {
                o.state.fetch_or(STARVING, Ordering::AcqRel);
            }
        } else {
            o.waiting.push_back((Arc::as_ptr(&wait), exclusive, since));
        }
        unlock(&o.queue);
        super::debugger::lock_taken(object as usize);
        match super::tasks::blocked_in(super::tasks::LOCK, object as usize, || wait.block_on_task()) {
            CANCELLED => {
                lock(&o.queue);
                o.waiting.retain(|&(w, ..)| w != Arc::as_ptr(&wait));
                unlock(&o.queue);
                return 0;
            }
            HANDED => {
                took(o, task, exclusive, location);
                return guard;
            }
            _ => {
                woken = true;
                spins = 0;
            }
        }
    }
}

/// Takes the lock for a waiter it is handed to, if it is free for it.
fn grant(o: &SyncObject, exclusive: bool) -> bool {
    let mut state = o.state.load(Ordering::Relaxed);
    loop {
        let next = if exclusive {
            if state & WRITER != 0 || state >= READER {
                return false;
            }
            state | WRITER
        } else {
            if state & WRITER != 0 {
                return false;
            }
            state + READER
        };
        match o.state.compare_exchange_weak(state, next, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(seen) => state = seen,
        }
    }
}

fn ungrant(o: &SyncObject, exclusive: bool) {
    if exclusive {
        o.state.fetch_and(!WRITER, Ordering::Release);
    } else {
        o.state.fetch_sub(READER, Ordering::Release);
    }
}

/// With the queue locked: the tasks waiting in `updateWhen`, after a change.
unsafe fn wake_changes(o: &mut SyncObject) -> Vec<*mut Task> {
    o.changes.drain(..).filter(|&w| unsafe { (*w).claim(0) }).map(|w| unsafe { (*w).task }).collect()
}

/// With the queue locked, after a release: wakes one task waiting for the
/// lock or, when it starves, hands the lock to the first writer or to every
/// reader in a row at the head of the queue. Returns the tasks, and how long
/// they waited, for `profile.startContention`.
unsafe fn wake_lockers(o: &mut SyncObject) -> (Vec<*mut Task>, std::time::Duration) {
    let mut woken = Vec::new();
    let mut delay = std::time::Duration::ZERO;
    unsafe {
        if o.state.load(Ordering::Acquire) & STARVING != 0 {
            while let Some(&(wait, exclusive, since)) = o.waiting.front() {
                if !grant(o, exclusive) {
                    break;
                }
                o.waiting.pop_front();
                if (*wait).claim(HANDED) {
                    woken.push((*wait).task);
                    delay += since.elapsed();
                    if exclusive {
                        break;
                    }
                } else {
                    ungrant(o, exclusive);
                }
            }
            if o.waiting.is_empty() {
                o.state.fetch_and(!STARVING, Ordering::AcqRel);
            }
        } else {
            while let Some((wait, _, since)) = o.waiting.pop_front() {
                if (*wait).claim(0) {
                    woken.push((*wait).task);
                    delay += since.elapsed();
                    break;
                }
            }
        }
    }
    if o.waiting.is_empty() && o.changes.is_empty() {
        o.state.fetch_and(!PARKED, Ordering::AcqRel);
    }
    (woken, delay)
}

/// Releases the lock a guard holds; nothing for 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_unlock(guard: usize) {
    if guard == 0 {
        return;
    }
    let exclusive = guard & 1 == 0;
    let address = object(guard) as usize;
    super::tasks::touch(address);
    let task = current();
    unsafe {
        let o = &mut *object(guard);
        gave_up(o, task, exclusive);
        let before = if exclusive {
            // Only a debugger's watch of a cell reads who changed it last.
            if super::debugger::watching() {
                o.writer.store((*task).id, Ordering::Relaxed);
            }
            // The bit is set, as this task holds the lock: subtracting it
            // takes one instruction where clearing it loops on a compare.
            o.state.fetch_sub(WRITER, Ordering::Release)
        } else {
            o.state.fetch_sub(READER, Ordering::Release)
        };
        if before & PARKED != 0 {
            lock(&o.queue);
            let mut woken = if exclusive { wake_changes(o) } else { Vec::new() };
            let (lockers, delay) = wake_lockers(o);
            woken.extend(lockers);
            unlock(&o.queue);
            woken.into_iter().for_each(notify);
            // Where the lock others waited for is let go.
            if !delay.is_zero() {
                super::profiling::contended(delay);
            }
        }
    }
    if super::race::on() {
        match exclusive {
            true => super::tasks::may_switch(),
            false => super::tasks::may_lose_update(),
        }
    }
}

/// Inside `updateWhen`: gives the lock up until another task changes the
/// value, then takes it again and returns the guard; returns 0, holding
/// nothing, if the task is cancelled meanwhile.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_await(guard: usize) -> usize {
    unsafe {
        let object = object(guard);
        let o = &mut *object;
        let wait = Wait::new();
        // This task changed the value, and is registered before it releases
        // the lock, so no later change is missed.
        lock(&o.queue);
        let mut woken = wake_changes(o);
        o.changes.push(Arc::as_ptr(&wait));
        o.state.fetch_or(PARKED, Ordering::AcqRel);
        o.writer.store(super::tasks::current_id(), Ordering::Relaxed);
        let location = o.taken_at.load(Ordering::Relaxed) as *const Str;
        gave_up(o, current(), true);
        o.state.fetch_and(!WRITER, Ordering::Release);
        let (lockers, delay) = wake_lockers(o);
        woken.extend(lockers);
        unlock(&o.queue);
        if !delay.is_zero() {
            super::profiling::contended(delay);
        }
        woken.into_iter().for_each(notify);
        if wait.block_on_task() == CANCELLED {
            lock(&o.queue);
            o.changes.retain(|&w| w != Arc::as_ptr(&wait));
            unlock(&o.queue);
            return 0;
        }
        tondo_rt_sync_lock(object, 1, location)
    }
}

/// Another handle of a cell, which the debugger keeps while it watches
/// the cell's value.
pub(crate) unsafe fn retain(object: *mut SyncObject) {
    unsafe { (*object).rc.fetch_add(1, Ordering::Relaxed) };
}

/// Runs `read` with the cell held to read, as the debugger reads its value
/// while the program runs; `None`, without waiting, while a task holds it
/// to change it, or tasks wait for it in order.
pub(crate) unsafe fn reading<T>(object: *mut SyncObject, read: impl FnOnce() -> T) -> Option<T> {
    let o = unsafe { &mut *object };
    let mut state = o.state.load(Ordering::Acquire);
    loop {
        if state & (WRITER | STARVING) != 0 {
            return None;
        }
        match o.state.compare_exchange_weak(state, state + READER, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => break,
            Err(now) => state = now,
        }
    }
    let out = read();
    if o.state.fetch_sub(READER, Ordering::Release) & PARKED != 0 {
        lock(&o.queue);
        let (lockers, _) = unsafe { wake_lockers(o) };
        unlock(&o.queue);
        lockers.into_iter().for_each(notify);
    }
    Some(out)
}

/// Releases a handle; the last one frees the value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_release(object: *mut SyncObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        if (*object).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        if (*object).visit.is_some() {
            cycles::forget(object as *const u8);
        }
        super::live::forget_object(object as usize);
        if super::lockorder::on() {
            super::lockorder::forget(object as usize);
        }
        if super::race::on() {
            super::race::forget(object as usize);
        }
        let o = &*object;
        if let Some(release) = o.release {
            release(o.value);
        }
        free_value(o, o.value, o.size, o.align);
        let layout = cell_layout(o.room).0;
        std::ptr::drop_in_place(object);
        std::alloc::dealloc(object as *mut u8, layout);
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Semaphore
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SemaphoreObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    permits: usize,
    waiting: VecDeque<*const Wait>,
}

/// A permit taken from a semaphore; the last reference gives it back.
#[repr(C)]
pub struct PermitObject {
    rc: AtomicUsize,
    semaphore: *mut SemaphoreObject,
}

/// `sync.Semaphore(permits)`; a count that is not positive panics with
/// `P0001`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_semaphore_new(permits: i64, location: *const Str) -> *mut SemaphoreObject {
    if permits <= 0 {
        super::raise(1, &format!("a semaphore needs at least 1 permit, and this one has {permits}"), location);
        return std::ptr::null_mut();
    }
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(SemaphoreObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        permits: permits as usize,
        waiting: VecDeque::new(),
    }))
}

fn new_permit(semaphore: *mut SemaphoreObject) -> *mut PermitObject {
    unsafe { (*semaphore).rc.fetch_add(1, Ordering::Relaxed) };
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(PermitObject { rc: AtomicUsize::new(1), semaphore }))
}

/// `acquire()`, or `tryAcquire()` with `try` 1: a permit, or null when
/// none is free for `try` or the task is cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_semaphore_acquire(
    semaphore: *mut SemaphoreObject,
    try_only: u8,
) -> *mut PermitObject {
    if stops() {
        return std::ptr::null_mut();
    }
    unsafe {
        let s = &mut *semaphore;
        lock(&s.lock);
        if s.permits > 0 {
            s.permits -= 1;
            unlock(&s.lock);
            return new_permit(semaphore);
        }
        if try_only != 0 {
            unlock(&s.lock);
            return std::ptr::null_mut();
        }
        let wait = Wait::new();
        s.waiting.push_back(Arc::as_ptr(&wait));
        unlock(&s.lock);
        if wait.block_on_task() == CANCELLED {
            lock(&s.lock);
            s.waiting.retain(|&w| w != Arc::as_ptr(&wait));
            unlock(&s.lock);
            return std::ptr::null_mut();
        }
        // The task that released a permit handed it over.
        new_permit(semaphore)
    }
}

/// Releases a `Permit`; the last reference gives the permit back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_permit_release(permit: *mut PermitObject) {
    if permit.is_null() {
        return;
    }
    unsafe {
        if (*permit).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let permit = Box::from_raw(permit);
        LIVE_OBJECTS.gone();
        let s = &mut *permit.semaphore;
        lock(&s.lock);
        let mut woken = None;
        while let Some(wait) = s.waiting.pop_front() {
            if (*wait).claim(0) {
                woken = Some((*wait).task);
                break;
            }
        }
        if woken.is_none() {
            s.permits += 1;
        }
        unlock(&s.lock);
        woken.into_iter().for_each(notify);
        tondo_rt_semaphore_release(permit.semaphore);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_semaphore_release(semaphore: *mut SemaphoreObject) {
    if semaphore.is_null() {
        return;
    }
    unsafe {
        if (*semaphore).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(semaphore));
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Barrier
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct BarrierObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    parties: usize,
    /// The tasks of this round waiting for the rest.
    arrived: Vec<*const Wait>,
}

/// `sync.Barrier(parties)`; a count that is not positive panics with
/// `P0001`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_barrier_new(parties: i64, location: *const Str) -> *mut BarrierObject {
    if parties <= 0 {
        super::raise(1, &format!("a barrier needs at least 1 party, and this one has {parties}"), location);
        return std::ptr::null_mut();
    }
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(BarrierObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        parties: parties as usize,
        arrived: Vec::new(),
    }))
}

/// `barrier.wait()`: 1 for the task that completes the round, which
/// releases the others; 0 for them, and for a cancelled task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_barrier_wait(barrier: *mut BarrierObject) -> u8 {
    if stops() {
        return 0;
    }
    unsafe {
        let b = &mut *barrier;
        lock(&b.lock);
        if b.arrived.len() + 1 == b.parties {
            let woken: Vec<*mut Task> = b.arrived.drain(..).filter(|&w| (*w).claim(0)).map(|w| (*w).task).collect();
            unlock(&b.lock);
            woken.into_iter().for_each(notify);
            return 1;
        }
        let wait = Wait::new();
        b.arrived.push(Arc::as_ptr(&wait));
        unlock(&b.lock);
        if wait.block_on_task() == CANCELLED {
            lock(&b.lock);
            b.arrived.retain(|&w| w != Arc::as_ptr(&wait));
            unlock(&b.lock);
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_barrier_release(barrier: *mut BarrierObject) {
    if barrier.is_null() {
        return;
    }
    unsafe {
        if (*barrier).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(barrier));
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Atomics of integers, `Bool`, `Char` and `Byte`
// ---------------------------------------------------------------------------

/// The value of an atomic cell, `width` bytes wide, read or changed with
/// hardware atomics; values travel zero-extended in a `u64`.
macro_rules! word {
    ($object:expr, $width:expr, |$atomic:ident| $body:expr) => {{
        let value = unsafe { (*$object).value };
        match $width {
            1 => {
                let $atomic = unsafe { &*(value as *const std::sync::atomic::AtomicU8) };
                ($body) as u64
            }
            2 => {
                let $atomic = unsafe { &*(value as *const std::sync::atomic::AtomicU16) };
                ($body) as u64
            }
            4 => {
                let $atomic = unsafe { &*(value as *const std::sync::atomic::AtomicU32) };
                ($body) as u64
            }
            _ => {
                let $atomic = unsafe { &*(value as *const std::sync::atomic::AtomicU64) };
                ($body) as u64
            }
        }
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_load(object: *mut SyncObject, width: u8) -> u64 {
    let value = word!(object, width, |a| a.load(Ordering::SeqCst));
    if super::race::on() {
        super::race::read(object as usize);
        super::tasks::may_lose_update();
    }
    value
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_store(object: *mut SyncObject, width: u8, value: u64) {
    if super::race::on() {
        super::tasks::may_switch();
        super::race::wrote(object as usize, false);
    }
    word!(object, width, |a| {
        a.store(value as _, Ordering::SeqCst);
        0u8
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_swap(object: *mut SyncObject, width: u8, value: u64) -> u64 {
    if super::race::on() {
        super::tasks::may_switch();
        super::race::wrote(object as usize, false);
    }
    word!(object, width, |a| a.swap(value as _, Ordering::SeqCst))
}

/// Stores `desired` if the value is `expected`; returns the value seen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_exchange(
    object: *mut SyncObject,
    width: u8,
    expected: u64,
    desired: u64,
) -> u64 {
    if super::race::on() {
        super::tasks::may_switch();
        super::race::wrote(object as usize, true);
    }
    word!(
        object,
        width,
        |a| match a.compare_exchange(expected as _, desired as _, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(seen) | Err(seen) => seen,
        }
    )
}

/// `add(delta)` on an integer of `width` bytes, signed when `signed`;
/// returns the new value, or panics with `P0005` when it does not fit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_add(
    object: *mut SyncObject,
    width: u8,
    signed: u8,
    delta: u64,
    location: *const Str,
) -> u64 {
    if super::race::on() {
        super::tasks::may_switch();
        super::race::wrote(object as usize, true);
    }
    let bits = width as u32 * 8;
    let decode = |raw: u64| -> i128 {
        if signed != 0 {
            let shift = 64 - bits;
            (((raw << shift) as i64) >> shift) as i128
        } else {
            (raw & (u64::MAX >> (64 - bits))) as i128
        }
    };
    let (low, high) =
        if signed != 0 { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) } else { (0, (1i128 << bits) - 1) };
    let delta = decode(delta);
    loop {
        let current = unsafe { tondo_rt_atomic_load(object, width) };
        let next = decode(current) + delta;
        if next < low || next > high {
            let name = match (signed != 0, bits) {
                (true, 64) => "Int".to_string(),
                (true, _) => format!("Int{bits}"),
                (false, _) => format!("UInt{bits}"),
            };
            let message =
                format!("the atomic {} + {} does not fit in {name} ({low} to {high})", decode(current), delta);
            super::raise(5, &message, location);
            return current;
        }
        let next = next as u64 & (u64::MAX >> (64 - bits));
        if unsafe { tondo_rt_atomic_exchange(object, width, current, next) } == current {
            return next;
        }
    }
}
