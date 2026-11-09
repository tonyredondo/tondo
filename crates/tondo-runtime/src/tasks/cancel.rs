//! Cancellation: whether a task is cancelled or past its deadline, `within`
//! and `defer`, the checks compiled code makes, and cancelling a task by its id
//! (docs/refactor.md, phase 2).

use super::*;

/// Whether the current task is cancelled.
pub(crate) fn cancelled() -> bool {
    unsafe { current().as_ref() }.is_some_and(halted)
}

/// Whether a task's waits end at once: it is cancelled, but for those of a
/// `within` its cleanup entered, or a deadline of its passed (11.15).
pub(super) fn halted(task: &Task) -> bool {
    let cancelled = task.cancelled.load(Ordering::Acquire) || task.alarm.load(Ordering::Acquire);
    (cancelled && task.grace.is_none()) || task.expired
}

/// A `within` starts at `level` among the task's: in a `defer` of a
/// cancelled task, its waits go on until its deadline.
pub(crate) fn within_starts(task: &mut Task, level: usize) {
    let cancelled = task.cancelled.load(Ordering::Acquire) || task.alarm.load(Ordering::Acquire);
    if cancelled && task.deferring > 0 && task.grace.is_none() {
        task.grace = Some(level);
        let until = task.withins[level].0.saturating_duration_since(*EPOCH);
        GRACED.fetch_max(until.as_nanos().min(u64::MAX as u128) as u64, Ordering::SeqCst);
    }
}

/// The `within` at `level` ended, and the grace it gave with it.
pub(crate) fn within_ends(task: &mut Task, level: usize) {
    if task.grace.is_some_and(|graced| graced >= level) {
        task.grace = None;
    }
}

/// Whether an operation that may wait ends at once without being done: in
/// a cancelled task, but not in its `defer`s, which do what needs no wait,
/// and whose waits `Wait::block` ends at once (11.15).
pub(crate) fn stops() -> bool {
    cancelled() && unsafe { current().as_ref() }.is_some_and(|task| task.deferring == 0)
}

/// A `defer` starts to run.
pub(crate) fn defer_starts() {
    if let Some(task) = unsafe { current().as_mut() } {
        task.deferring += 1;
        task.cut = false;
    }
}

/// The `defer` ended, however it did: the code that ran it goes on.
pub(crate) fn defer_ends() {
    if let Some(task) = unsafe { current().as_mut() } {
        task.deferring = task.deferring.saturating_sub(1);
        task.cut = false;
    }
}

/// A function that unwinds because its task is cancelled returns: inside
/// a `defer`, its caller unwinds too, up to the `defer`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cancel_return() {
    if let Some(task) = unsafe { current().as_mut() }
        && task.deferring > 0
    {
        task.cut = true;
    }
}

/// Checked by compiled code after every call that may wait: a cancelled
/// task unwinds from there. While exploring orders, the task gives way
/// there too.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cancelled() -> u8 {
    safe_point();
    if exploring().is_some() {
        give_way();
    }
    let Some(task) = (unsafe { current().as_ref() }) else { return 0 };
    // In a `defer`, only after a wait that ended at once.
    if task.cancelled.load(Ordering::Acquire) && task.grace.is_none() && (task.deferring == 0 || task.cut) {
        return 1;
    }
    // A panic unwinds to a `recover` (language 15.9).
    if task.unwinding.is_some() {
        return 3;
    }
    // A live build's reload ended the wait: the statement starts again.
    // Read first: a swap at every call would cost more than the call.
    match task.again.load(Ordering::Relaxed) && task.again.swap(false, Ordering::AcqRel) {
        true => {
            attend(false);
            2
        }
        false => 0,
    }
}

/// Cancels the task numbered `id` if it is still alive, as a client that
/// went away cancels the task that answers it. Its number is never reused,
/// and while the set of tasks is locked none of them is freed.
pub(crate) fn cancel_by_id(id: u64) {
    let alive = tasks();
    for &task in alive.iter() {
        let task = task as *mut Task;
        if unsafe { (*task).id == id && !(*task).finished } {
            unsafe { cancel_task(task) };
            return;
        }
    }
}

/// Cancels a running task: marks it and ends the wait it is blocked in,
/// unless a `within` its cleanup entered gives that wait until its deadline
/// (11.15), as when the end of `main` cancels a detached task again.
pub(super) unsafe fn cancel_task(task: *mut Task) {
    unsafe {
        let t = &mut *task;
        // Attention first: a task that sees itself cancelled, as a lock
        // that returns at once without its guard, unwinds only if compiled
        // code then calls `tondo_rt_cancelled`.
        attend(true);
        if t.cancelled.swap(true, Ordering::SeqCst) {
            attend(false);
        }
        // Every step of a task is one that this could come before.
        touch(t.name as usize);
        lock_unseen(&t.waiting_lock);
        let woken = !t.waiting.is_null() && t.grace.is_none() && (*t.waiting).claim(CANCELLED);
        unlock(&t.waiting_lock);
        if woken {
            notify(task);
        }
    }
}

/// Stops `task`, which runs a `recover` a task of which ended with a panic
/// (language 15.9), as soon as it can: its wait ends as a cancellation's
/// would, and it unwinds from there. Called while it runs the `recover`.
pub(crate) unsafe fn alarm(task: *mut Task) {
    unsafe {
        let t = &mut *task;
        if !t.alarm.swap(true, Ordering::SeqCst) {
            attend(true);
        }
        lock_unseen(&t.waiting_lock);
        let woken = !t.waiting.is_null() && t.grace.is_none() && (*t.waiting).claim(CANCELLED);
        unlock(&t.waiting_lock);
        if woken {
            notify(task);
        }
    }
}
