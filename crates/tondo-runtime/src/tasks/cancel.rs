//! Cancellation: whether a task is cancelled or past its deadline, `within`
//! and `defer`, the checks compiled code makes, and cancelling a task by its id
//! (docs/refactor.md, phase 2).

use super::*;

/// Whether the current task is cancelled.
pub(crate) fn cancelled() -> bool {
    unsafe { current().as_ref() }.is_some_and(halted)
}

/// Whether the current task itself is cancelled, not only past a deadline
/// or alarmed by a `recover`.
pub(crate) fn cancelled_itself() -> bool {
    unsafe { current().as_ref() }.is_some_and(|task| task.cancelled.load(Ordering::Acquire))
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
    // Nearly always nothing asks for attention, and the running task, which
    // costs a call to find, is not read.
    attended() && stops_task(current())
}

/// `stops`, for the running task already read.
pub(crate) fn stops_task(task: *mut Task) -> bool {
    attended() && unsafe { task.as_ref() }.is_some_and(|task| halted(task) && task.deferring == 0)
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

/// What a checkpoint must see to, which compiled code reads beside the
/// count of reasons to ask: until a bit is set, a checkpoint asks the
/// runtime only when something else does.
#[unsafe(export_name = "tondo_rt_checkpoint_asks")]
pub static CHECKPOINT_ASKS: AtomicU32 = AtomicU32::new(0);
/// The program entered a `within` with a deadline, which a checkpoint must
/// compare with the clock. Set once, never cleared.
const DEADLINES: u32 = 1;
/// A worker is held by one task while tasks wait for one: the first task
/// to ask gives its worker away. Set by the monitor, swapped off by it.
pub(super) const GIVE_WAY: u32 = 2;

/// A `within` with a deadline starts: a load each time, a write the first.
pub(crate) fn deadline_entered() {
    if CHECKPOINT_ASKS.load(Ordering::Relaxed) & DEADLINES == 0 {
        CHECKPOINT_ASKS.fetch_or(DEADLINES, Ordering::Relaxed);
    }
}

/// Nanoseconds of the system's monotonic clock, exactly or as of its last
/// tick: the coarse reading costs a fourth of the exact one and is never
/// ahead of it.
pub(super) fn monotonic(coarse: bool) -> u64 {
    #[cfg(target_os = "macos")]
    let clock = if coarse { libc::CLOCK_UPTIME_RAW_APPROX } else { libc::CLOCK_UPTIME_RAW };
    #[cfg(not(target_os = "macos"))]
    let clock = if coarse { libc::CLOCK_MONOTONIC_COARSE } else { libc::CLOCK_MONOTONIC };
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(clock, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// Whether a deadline of the block the task runs has passed. The first
/// time a `within` is asked, its deadline is read in the system's clock,
/// whose coarse reading answers after that: a deadline is seen at most a
/// tick of that clock late, never early, a tick being 1 to 10 ms on Linux
/// by how its kernel was built, and a few at most on macOS. A run given its
/// choices moves its clock itself, which only the exact reading follows.
fn past_deadline(task: &mut Task) -> bool {
    if task.expired {
        return true;
    }
    let Some(entry) = task.withins.last_mut() else { return false };
    if ahead() != 0 {
        return clock_now() >= entry.0;
    }
    if entry.2 == 0 {
        let now = Instant::now();
        // Asked first past it: past, without the coarse clock, which may
        // lag the exact one by a tick and so see it a tick after the ask.
        if entry.0 <= now {
            return true;
        }
        entry.2 = monotonic(false) + entry.0.saturating_duration_since(now).as_nanos() as u64;
    }
    monotonic(true) >= entry.2
}

/// `tasks.checkpoint()`, in a program that entered a deadline or when the
/// runtime is asked something (11.15): a deadline of the running task that
/// passed while it computed unwinds it to its `within`, at the check for a
/// panic that follows, as a wait would have seen it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_tasks_checkpoint() {
    let task = current();
    if task.is_null() {
        return;
    }
    // A worker held while tasks wait: this task gives its own away, once
    // for each time the monitor asks, unless frames of C on its stack keep
    // it on its thread, or one worker runs the tasks in the order given.
    if CHECKPOINT_ASKS.load(Ordering::Relaxed) & GIVE_WAY != 0
        && exploring().is_none()
        && !stays(task)
        && CHECKPOINT_ASKS.fetch_and(!GIVE_WAY, Ordering::SeqCst) & GIVE_WAY != 0
    {
        give_way_to_waiting(task);
    }
    let task = unsafe { &mut *task };
    if !task.expired && past_deadline(task) {
        expire(task);
    }
}

/// Switches the running task out to its worker, as a wait does, to run
/// again behind the tasks that wait for a worker (`run`).
#[inline(never)]
fn give_way_to_waiting(task: *mut Task) {
    crate::diagnostics::TASKS_YIELDS.add(1);
    worker(|w| w.yielded.set(true));
    unsafe { switch(&mut (*task).sp, *home()) };
}

/// `tasks.cancelled()`: whether the running task was cancelled, or a
/// deadline of the block it runs passed; not a panic its `recover` passes
/// on, which is no cancel (11.15).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_tasks_cancelled() -> u8 {
    let Some(task) = (unsafe { current().as_mut() }) else { return 0 };
    u8::from(task.cancelled.load(Ordering::Acquire) || past_deadline(task))
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
