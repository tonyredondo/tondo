//! How the program ends: every task cancelled, with a time for their cleanups
//! when a panic ends it, and the status the program ends with (docs/refactor.md,
//! phase 2).

use super::*;

/// How long a panic waits for the other tasks to run their cleanups.
pub(super) const PANIC_GRACE: Duration = Duration::from_secs(5);

/// The furthest deadline of a `within` that a cleanup of a cancelled task
/// entered, which has until then (11.15), in nanoseconds after `EPOCH`: a
/// program that ends waits for it beyond `PANIC_GRACE`.
pub(super) static GRACED: AtomicU64 = AtomicU64::new(0);
pub(super) static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// When a program that ends may stop waiting for its cleanups: `PANIC_GRACE`
/// after `from`, or the deadline of a `within` one of them entered.
pub(super) fn cleaned_by(from: Instant) -> Instant {
    (from + PANIC_GRACE).max(*EPOCH + Duration::from_nanos(GRACED.load(Ordering::SeqCst)))
}

/// The exit status of a panic or a test's skip that ends the program.
pub(super) static ENDING: AtomicUsize = AtomicUsize::new(usize::MAX);

/// After a panic, or a test's skip, ran its task's cleanups (11.11): cancels
/// every other task, which runs its own `defer`s at its next wait, and
/// finishes this one. The process then ends with `status` once the main
/// task has finished and the other tasks with it, or after `PANIC_GRACE`.
pub(crate) fn end_all(status: i32) -> ! {
    abandon(status);
    let me = current();
    if me.is_null() {
        flush_stdout();
        crate::process::cancel_live();
        std::process::exit(status)
    }
    unsafe {
        let join = (*me).join;
        if (*me).main {
            MAIN_STATUS.store(status as usize, Ordering::SeqCst);
        } else if !(*me).foreign.is_null() {
            // The thread of C that waits for the callback goes on.
            let call = &*(*me).foreign;
            let mut done = call.done.lock().unwrap_or_else(|e| e.into_inner());
            *done = true;
            call.finished.notify_one();
            drop(done);
            // Its thread may be working for it, and sleep.
            drop(queue());
            READY.notify_all();
        } else if !join.is_null() {
            // Its join completes without a result, so whoever waits for
            // it goes on and nothing releases what it never made.
            if !(*join).result.is_null() {
                std::ptr::write_bytes((*join).result, 0, (*join).size);
            }
            (*join).taken = true;
            if complete(join) {
                DETACHED.lock().unwrap_or_else(|e| e.into_inner()).retain(|&j| j != join as usize);
                tondo_rt_join_release(join);
            }
        }
        (*me).finished = true;
        switch(&mut (*me).sp, *home());
    }
    runtime_fault("a finished task resumed")
}

/// Ends the program with `status` once its tasks have run their cleanups:
/// cancels every task but the running one, and ends the process anyway
/// after `PANIC_GRACE`.
pub(crate) fn abandon(status: i32) {
    let _ = ENDING.compare_exchange(usize::MAX, status as usize, Ordering::SeqCst, Ordering::SeqCst);
    // Read here, a call of the program's, and not first by the thread below,
    // which a replay does not have.
    std::sync::LazyLock::force(&EPOCH);
    // The program ends after a while, unless it replays a recording, which
    // ends where the recording does.
    static WATCHDOG: std::sync::Once = std::sync::Once::new();
    WATCHDOG.call_once(|| {
        if crate::replay::replaying() {
            return;
        }
        let _outside = crate::replay::Outside::enter();
        let from = Instant::now();
        spawn_thread(Some(64 * 1024), move || {
            while Instant::now() < cleaned_by(from) {
                std::thread::sleep(cleaned_by(from).saturating_duration_since(Instant::now()));
            }
            flush_stdout();
            crate::process::cancel_live();
            std::process::exit(ENDING.load(Ordering::SeqCst) as i32)
        });
    });
    let me = current();
    for &task in tasks().iter() {
        if task as *mut Task != me {
            unsafe { cancel_task(task as *mut Task) };
        }
    }
}

/// Ends the program as a signal nothing listens for does, from a task:
/// cancels every task, this one too, which unwinds at its next wait.
pub(crate) fn abandon_all(status: i32) {
    abandon(status);
    let me = current();
    if !me.is_null() {
        unsafe { cancel_task(me) };
    }
}

/// When the main task has finished after a panic: waits for the other
/// tasks to run their cleanups, and gives the panic's exit status.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn ending() -> Option<i32> {
    let status = ENDING.load(Ordering::SeqCst);
    if status == usize::MAX {
        return None;
    }
    // How long this takes is no part of a recording.
    let _outside = crate::replay::Outside::enter();
    let from = Instant::now();
    while live_tasks() > 0 && Instant::now() < cleaned_by(from) {
        std::thread::sleep(Duration::from_millis(1));
    }
    Some(status as i32)
}
