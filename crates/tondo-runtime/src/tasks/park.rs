//! Parking and waking: a task that waits parks, another thread notifies it,
//! and a task pinned to its thread waits there (docs/refactor.md, phase 2).

use super::*;

/// Wakes a task for its current wait, or makes its next `park` return at
/// once if it has not parked yet; `run` makes it runnable. The state is
/// written even when it says notified already, as the task takes a
/// notification with a write too: a notifier that only read it could see
/// one the task has just taken, while the task, which then reads its wait,
/// may not see what this notifier claimed, and both would let it sleep.
pub(super) fn notify_with(task: *mut Task, run: impl FnOnce(*mut Task)) {
    let state = unsafe { &(*task).state };
    loop {
        let current = state.load(Ordering::Acquire);
        let next = if current == PARKED || current == PINNED { RUNNING } else { NOTIFIED };
        if state.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if current == PARKED {
                crate::clock::woke(task);
                run(task);
            } else if current == PINNED {
                let _lock = PINNED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                PINNED_WAKE.notify_all();
                // The thread that stands in for its worker may wait in the
                // poller, or in the runtime a recorded run drives, which
                // nothing else tells that the task goes on.
                if exploring().is_some()
                    && let Some(poller) = crate::poll::poller()
                {
                    poller.wake();
                }
            }
            return;
        }
    }
}

pub(crate) fn notify(task: *mut Task) {
    notify_with(task, schedule);
}

/// Switches the current task out until it is notified; returns at once if
/// it already was.
pub(super) fn park() {
    let task = current();
    if task.is_null() {
        runtime_fault("waiting outside a task");
    }
    unsafe {
        let state = &(*task).state;
        // With C frames on its stack the task cannot resume on another
        // thread, so it waits on this one; the monitor may start another
        // worker meanwhile.
        let in_c = in_c_here();
        if !in_c.is_null() && (*in_c).depth.load(Ordering::SeqCst) > 0 {
            if state.compare_exchange(RUNNING, PINNED, Ordering::AcqRel, Ordering::Acquire).is_err() {
                state.swap(RUNNING, Ordering::AcqRel);
                return;
            }
            wait_pinned(task);
            return;
        }
        // Notified already: the notification is taken with a write, which
        // `notify_with` says why.
        if state.compare_exchange(RUNNING, PARKING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            state.swap(RUNNING, Ordering::AcqRel);
            return;
        }
        switch(&mut (*task).sp, *home());
    }
}

/// Waits on this thread until the task is notified, as one more worker
/// that waits for the deadlock check, which runs again now and then while
/// only C could wake the program.
pub(super) fn wait_pinned(task: *mut Task) {
    let state = unsafe { &(*task).state };
    // The queue is let go first: a debugger reads it while it shows the
    // deadlock.
    let check = |queue: MutexGuard<'static, Queue>| {
        if matches!(stalled(&queue), Stalled::Deadlock) {
            drop(queue);
            deadlock();
        }
    };
    {
        let mut queue = queue();
        queue.pinned.push(task);
        check(queue);
    }
    // One worker has no other to run what it waits for: a thread stands in
    // for it meanwhile.
    let standing_in = exploring().is_some().then(|| stand_in(state));
    // Waiting, the thread runs no Tondo code that a collection could meet.
    leave_tondo();
    // The wait's own reading of the clock is not the program's.
    let outside = crate::replay::Outside::enter();
    loop {
        let lock = PINNED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if state.load(Ordering::Acquire) == RUNNING {
            break;
        }
        let (lock, _) = PINNED_WAKE.wait_timeout(lock, RECHECK).unwrap_or_else(|e| e.into_inner());
        if state.load(Ordering::Acquire) == RUNNING {
            break;
        }
        drop(lock);
        check(queue());
    }
    if let Some(thread) = standing_in {
        // It reads the task's state, which is this thread's again.
        thread.join();
        crate::replay::waited();
    }
    drop(outside);
    enter_tondo();
    queue().pinned.retain(|&t| t != task);
}

/// Starts a thread that runs the program, with the baton, for a worker
/// that waits with C's frames on its task's stack, until the task's `state`
/// says it goes on, which comes at the same place of the program in a replay.
pub(super) fn stand_in(state: &AtomicU8) -> RuntimeThread {
    let state = state as *const AtomicU8 as usize;
    let outside = crate::replay::Outside::enter();
    let thread = spawn_thread(Some(256 * 1024), move || {
        let state = unsafe { &*(state as *const AtomicU8) };
        crate::replay::entered(0);
        queue().workers += 1;
        work_until(false, Some(&|| state.load(Ordering::Acquire) == RUNNING));
        queue().workers -= 1;
        crate::replay::left();
    });
    drop(outside);
    crate::replay::waits();
    thread
}

// ---------------------------------------------------------------------------
