//! `std.sync` (standard library 14.4.4): shared state behind locks that
//! park tasks instead of blocking threads, semaphores and barriers.

use super::tasks::{CANCELLED, Task, Wait, cancelled, current, lock, notify, unlock};
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
    /// Guards `waiting` and `changes`.
    queue: AtomicBool,
    /// Tasks waiting to take it, and whether to change it.
    waiting: VecDeque<(*const Wait, bool)>,
    /// Tasks in `updateWhen` waiting for the value to change.
    changes: Vec<*const Wait>,
    value: *mut u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
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
/// `tondo_rt_sync_value`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sync_new(size: usize, align: usize, release: Option<Glue>) -> *mut SyncObject {
    let align = align.max(8);
    let layout = std::alloc::Layout::from_size_align(size.max(1), align).unwrap();
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(SyncObject {
        rc: AtomicUsize::new(1),
        state: AtomicUsize::new(0),
        queue: AtomicBool::new(false),
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

/// The locks the current task holds.
fn held() -> &'static mut Vec<(usize, bool, *const Str)> {
    unsafe { &mut (*current()).held }
}

/// Takes the lock to read (`mode` 0), to change (1), or to change if it is
/// free (2). Returns the guard, or 0 when `try` finds it taken or the task
/// is cancelled. Taking it again from the task that holds it, in a way
/// that would wait forever, panics with `P0014`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_lock(object: *mut SyncObject, mode: u8, location: *const Str) -> usize {
    if cancelled() {
        return 0;
    }
    let exclusive = mode != SHARED;
    let address = object as usize;
    let holds = held();
    if let Some(&(_, alone, taken)) = holds.iter().find(|&&(o, alone, _)| o == address && (exclusive || alone)) {
        let how = |exclusive: bool| if exclusive { "to change its value" } else { "to read it" };
        let mut message = format!(
            "this task already holds this lock {}, so taking it again {} would wait for itself forever",
            how(alone),
            how(exclusive)
        );
        if !taken.is_null() {
            message
                .push_str(&format!(": it took it at {}", String::from_utf8_lossy(unsafe { super::str_bytes(taken) })));
        }
        super::panic_with(14, &message, location);
    }
    let guard = address | (!exclusive) as usize;
    let o = unsafe { &mut *object };
    let mut spins = 0;
    let mut since = None;
    let mut woken = false;
    loop {
        if try_take(o, exclusive) {
            held().push((address, exclusive, location));
            return guard;
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
            held().push((address, exclusive, location));
            return guard;
        }
        // A task woken before goes first, and starves the lock once it
        // has lost for too long.
        let since = *since.get_or_insert_with(std::time::Instant::now);
        if woken {
            o.waiting.push_front((Arc::as_ptr(&wait), exclusive));
            if since.elapsed() > STARVATION {
                o.state.fetch_or(STARVING, Ordering::AcqRel);
            }
        } else {
            o.waiting.push_back((Arc::as_ptr(&wait), exclusive));
        }
        unlock(&o.queue);
        match wait.block() {
            CANCELLED => {
                lock(&o.queue);
                o.waiting.retain(|&(w, _)| w != Arc::as_ptr(&wait));
                unlock(&o.queue);
                return 0;
            }
            HANDED => {
                held().push((address, exclusive, location));
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
/// reader in a row at the head of the queue. Returns the tasks.
unsafe fn wake_lockers(o: &mut SyncObject) -> Vec<*mut Task> {
    let mut woken = Vec::new();
    unsafe {
        if o.state.load(Ordering::Acquire) & STARVING != 0 {
            while let Some(&(wait, exclusive)) = o.waiting.front() {
                if !grant(o, exclusive) {
                    break;
                }
                o.waiting.pop_front();
                if (*wait).claim(HANDED) {
                    woken.push((*wait).task);
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
            while let Some((wait, _)) = o.waiting.pop_front() {
                if (*wait).claim(0) {
                    woken.push((*wait).task);
                    break;
                }
            }
        }
    }
    if o.waiting.is_empty() && o.changes.is_empty() {
        o.state.fetch_and(!PARKED, Ordering::AcqRel);
    }
    woken
}

/// Releases the lock a guard holds; nothing for 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sync_unlock(guard: usize) {
    if guard == 0 {
        return;
    }
    let exclusive = guard & 1 == 0;
    let address = object(guard) as usize;
    let holds = held();
    if let Some(index) = holds.iter().position(|&(o, _, _)| o == address) {
        holds.swap_remove(index);
    }
    unsafe {
        let o = &mut *object(guard);
        let before = if exclusive {
            o.state.fetch_and(!WRITER, Ordering::Release)
        } else {
            o.state.fetch_sub(READER, Ordering::Release)
        };
        if before & PARKED != 0 {
            lock(&o.queue);
            let mut woken = if exclusive { wake_changes(o) } else { Vec::new() };
            woken.extend(wake_lockers(o));
            unlock(&o.queue);
            woken.into_iter().for_each(notify);
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
        o.state.fetch_and(!WRITER, Ordering::Release);
        woken.extend(wake_lockers(o));
        unlock(&o.queue);
        let holds = held();
        let mut location = std::ptr::null();
        if let Some(index) = holds.iter().position(|&(h, _, _)| h == object as usize) {
            location = holds.swap_remove(index).2;
        }
        woken.into_iter().for_each(notify);
        if wait.block() == CANCELLED {
            lock(&o.queue);
            o.changes.retain(|&w| w != Arc::as_ptr(&wait));
            unlock(&o.queue);
            return 0;
        }
        tondo_rt_sync_lock(object, 1, location)
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
        super::panic_with(1, &format!("a semaphore needs at least 1 permit, and this one has {permits}"), location);
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
        super::panic_with(1, &format!("a barrier needs at least 1 party, and this one has {parties}"), location);
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
    word!(object, width, |a| a.load(Ordering::SeqCst))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_store(object: *mut SyncObject, width: u8, value: u64) {
    word!(object, width, |a| {
        a.store(value as _, Ordering::SeqCst);
        0u8
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_atomic_swap(object: *mut SyncObject, width: u8, value: u64) -> u64 {
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
            super::panic_with(5, &message, location);
        }
        let next = next as u64 & (u64::MAX >> (64 - bits));
        if unsafe { tondo_rt_atomic_exchange(object, width, current, next) } == current {
            return next;
        }
    }
}
