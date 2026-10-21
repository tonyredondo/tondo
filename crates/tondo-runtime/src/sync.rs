//! `std.sync` (standard library 14.4.4): shared state behind locks that
//! park tasks instead of blocking threads, semaphores and barriers.

use super::tasks::{CANCELLED, Task, Wait, cancelled, current, lock, notify, unlock};
use super::{Glue, LIVE_OBJECTS, Str, tondo_rt_panic};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// The state of a `Mutex`, `RwLock`, `Atomic`, `Once` or shared collection,
/// and its lock: several tasks may hold it to read, one to change it. `rc`
/// counts the program's handles.
#[repr(C)]
pub struct SyncObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    /// The tasks holding it to read, once per hold, and the one holding it
    /// to change it.
    readers: Vec<*mut Task>,
    writer: *mut Task,
    /// Tasks waiting to take it, and whether to change it.
    waiting: VecDeque<(*const Wait, bool)>,
    /// Tasks in `updateWhen` waiting for the value to change.
    changes: Vec<*const Wait>,
    value: *mut u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
}

const SHARED: u8 = 0;
const TRY: u8 = 2;

/// A new cell for a value of `size` bytes, which compiled code stores at
/// `tondo_rt_sync_value`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sync_new(size: usize, align: usize, release: Option<Glue>) -> *mut SyncObject {
    let align = align.max(8);
    let layout = std::alloc::Layout::from_size_align(size.max(1), align).unwrap();
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(SyncObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        readers: Vec::new(),
        writer: std::ptr::null_mut(),
        waiting: VecDeque::new(),
        changes: Vec::new(),
        value: unsafe { std::alloc::alloc(layout) },
        size,
        align,
        release,
    }))
}

/// A guard is the object's address, plus 1 when it holds it to read.
fn object(guard: usize) -> *mut SyncObject {
    (guard & !1) as *mut SyncObject
}

/// The address of the value of a cell, given it or a guard on it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_value(handle: usize) -> *mut u8 {
    if handle == 0 { std::ptr::null_mut() } else { unsafe { (*object(handle)).value } }
}

fn free_for(o: &SyncObject, exclusive: bool) -> bool {
    o.writer.is_null() && (!exclusive || o.readers.is_empty())
}

fn hold(o: &mut SyncObject, task: *mut Task, exclusive: bool) {
    if exclusive {
        o.writer = task;
    } else {
        o.readers.push(task);
    }
}

/// Takes the lock to read (`mode` 0), to change (1), or to change if it is
/// free (2). Returns the guard, or 0 when `try` finds it taken or the task
/// is cancelled. Taking it again from the task that holds it panics with
/// `P0014`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_lock(object: *mut SyncObject, mode: u8, location: *const Str) -> usize {
    if cancelled() {
        return 0;
    }
    let me = current();
    let exclusive = mode != SHARED;
    let guard = object as usize | (!exclusive) as usize;
    unsafe {
        let o = &mut *object;
        lock(&o.lock);
        if o.writer == me || (exclusive && o.readers.contains(&me)) {
            unlock(&o.lock);
            tondo_rt_panic(14, std::ptr::null(), location);
        }
        if free_for(o, exclusive) {
            hold(o, me, exclusive);
            unlock(&o.lock);
            return guard;
        }
        if mode == TRY {
            unlock(&o.lock);
            return 0;
        }
        let wait = Wait::new();
        o.waiting.push_back((Arc::as_ptr(&wait), exclusive));
        unlock(&o.lock);
        if wait.block() == CANCELLED {
            lock(&o.lock);
            o.waiting.retain(|&(w, _)| w != Arc::as_ptr(&wait));
            unlock(&o.lock);
            return 0;
        }
        // Whoever released it made this task a holder before notifying it.
        guard
    }
}

/// Gives up a hold, with the object locked, and hands the lock to the
/// tasks waiting for it; returns the tasks to notify.
unsafe fn give_up(o: &mut SyncObject, exclusive: bool) -> Vec<*mut Task> {
    let mut woken = Vec::new();
    unsafe {
        if exclusive {
            o.writer = std::ptr::null_mut();
            // The value may have changed: `updateWhen` checks it again.
            for wait in o.changes.drain(..) {
                if (*wait).claim(0) {
                    woken.push((*wait).task);
                }
            }
        } else {
            let me = current();
            if let Some(index) = o.readers.iter().position(|&r| r == me) {
                o.readers.swap_remove(index);
            }
        }
        while let Some(&(wait, wants)) = o.waiting.front() {
            if !free_for(o, wants) {
                break;
            }
            o.waiting.pop_front();
            if (*wait).claim(0) {
                hold(o, (*wait).task, wants);
                woken.push((*wait).task);
            }
        }
    }
    woken
}

/// Releases the lock a guard holds; nothing for 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_unlock(guard: usize) {
    if guard == 0 {
        return;
    }
    unsafe {
        let o = &mut *object(guard);
        lock(&o.lock);
        let woken = give_up(o, guard & 1 == 0);
        unlock(&o.lock);
        woken.into_iter().for_each(notify);
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
        lock(&o.lock);
        let woken = give_up(o, true);
        o.changes.push(Arc::as_ptr(&wait));
        unlock(&o.lock);
        woken.into_iter().for_each(notify);
        if wait.block() == CANCELLED {
            lock(&o.lock);
            o.changes.retain(|&w| w != Arc::as_ptr(&wait));
            unlock(&o.lock);
            return 0;
        }
        tondo_rt_sync_lock(object, 1, std::ptr::null())
    }
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
        let o = Box::from_raw(object);
        if let Some(release) = o.release {
            release(o.value);
        }
        std::alloc::dealloc(o.value, std::alloc::Layout::from_size_align(o.size.max(1), o.align).unwrap());
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
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
        unsafe { tondo_rt_panic(1, std::ptr::null(), location) };
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(SemaphoreObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        permits: permits as usize,
        waiting: VecDeque::new(),
    }))
}

fn new_permit(semaphore: *mut SemaphoreObject) -> *mut PermitObject {
    unsafe { (*semaphore).rc.fetch_add(1, Ordering::Relaxed) };
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(PermitObject { rc: AtomicUsize::new(1), semaphore }))
}

/// `acquire()`, or `tryAcquire()` with `try` 1: a permit, or null when
/// none is free for `try` or the task is cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_semaphore_acquire(
    semaphore: *mut SemaphoreObject,
    try_only: u8,
) -> *mut PermitObject {
    if cancelled() {
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
        if wait.block() == CANCELLED {
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
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
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
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
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
        unsafe { tondo_rt_panic(1, std::ptr::null(), location) };
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
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
    if cancelled() {
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
        if wait.block() == CANCELLED {
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
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}
