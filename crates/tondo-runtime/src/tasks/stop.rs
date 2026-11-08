//! Stopping the world: the threads that run Tondo code, each stopped at a safe
//! point while the cycle collector, a dump or a logpoint needs them out, one
//! stop at a time, and the count of reasons compiled code checks after a call
//! that may wait (docs/refactor.md, phase 2).

use super::*;

/// Set while the cycle collector needs every thread out of Tondo code.
pub(super) static STOPPING: AtomicBool = AtomicBool::new(false);
/// Set while someone has the world stopped, or is stopping it.
pub(super) static STOPPER: AtomicBool = AtomicBool::new(false);

/// How many reasons there are for compiled code to call
/// `tondo_rt_cancelled` after a call that may wait: a stop of the world, a
/// thread that owes the collector, an exploration of orders, a task
/// cancelled or one whose statement starts again. Compiled code reads it,
/// and at zero, nearly always, goes on without the call.
#[unsafe(export_name = "tondo_rt_attention")]
pub static ATTENTION: AtomicU32 = AtomicU32::new(0);

/// One more reason, or one less, to call `tondo_rt_cancelled`.
pub(crate) fn attend(more: bool) {
    match more {
        true => ATTENTION.fetch_add(1, Ordering::SeqCst),
        false => ATTENTION.fetch_sub(1, Ordering::SeqCst),
    };
}
pub(super) static RESUMED: Condvar = Condvar::new();
pub(super) static RESUMED_LOCK: Mutex<()> = Mutex::new(());
/// A thread that ever ran Tondo code: whether it runs it now, and while
/// it stands still for a stop, its task and the frame it stopped in, for a
/// dump.
pub(super) struct Runner {
    pub(super) running: AtomicBool,
    pub(super) task: std::sync::atomic::AtomicPtr<Task>,
    pub(super) frame: AtomicUsize,
    /// For a dump, a thread that runs on is asked where it is with a
    /// signal, and answers here.
    pub(super) thread: libc::pthread_t,
    pub(super) asked: AtomicBool,
    pub(super) answer: [AtomicUsize; ANSWER],
    pub(super) answered: AtomicUsize,
}

pub(super) const ANSWER: usize = 64;

// The thread's handle, a pointer with musl and on macOS, only goes to
// `pthread_kill`.
unsafe impl Send for Runner {}
unsafe impl Sync for Runner {}

pub(super) static IN_TONDO: Mutex<Vec<&'static Runner>> = Mutex::new(Vec::new());

/// This thread's state, registered the first time it runs Tondo code.
pub(super) fn running_here() -> &'static Runner {
    if let Some(runner) = worker(|w| w.runner.get()) {
        return runner;
    }
    let runner: &'static Runner = Box::leak(Box::new(Runner {
        running: AtomicBool::new(false),
        task: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        frame: AtomicUsize::new(0),
        thread: unsafe { libc::pthread_self() },
        asked: AtomicBool::new(false),
        answer: [const { AtomicUsize::new(0) }; ANSWER],
        answered: AtomicUsize::new(0),
    }));
    IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).push(runner);
    worker(|w| w.runner.set(Some(runner)));
    runner
}

/// From now on this thread runs Tondo code, once no collection runs.
pub(crate) fn enter_tondo() {
    let runner = running_here();
    loop {
        runner.running.store(true, Ordering::SeqCst);
        if !STOPPING.load(Ordering::SeqCst) {
            return;
        }
        runner.task.store(current(), Ordering::SeqCst);
        runner.frame.store(crate::trace::frame_pointer(), Ordering::SeqCst);
        runner.running.store(false, Ordering::SeqCst);
        let mut resumed = RESUMED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        while STOPPING.load(Ordering::SeqCst) {
            resumed = RESUMED.wait(resumed).unwrap_or_else(|e| e.into_inner());
        }
        drop(resumed);
        runner.task.store(std::ptr::null_mut(), Ordering::SeqCst);
    }
}

pub(super) fn leave_tondo() {
    running_here().running.store(false, Ordering::SeqCst);
}

/// Takes this thread out of Tondo code where it is, as when it waits for
/// memory inside an allocation; whether it was in, to `enter_tondo` again.
/// Allocates nothing.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn pause_here() -> bool {
    worker(|w| w.runner.get()).is_some_and(|runner| runner.running.swap(false, Ordering::SeqCst))
}

/// Where a running task lets a collection run: between two steps.
pub(super) fn safe_point() {
    if STOPPING.load(Ordering::Relaxed) {
        leave_tondo();
        enter_tondo();
    }
    if cycles::owed() {
        leave_tondo();
        cycles::assist();
        enter_tondo();
    }
}

/// Stops every thread at its next safe point, for the cycle collector;
/// gives up after `timeout`, when a task runs long without one.
pub(crate) fn stop_world(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    // One at a time: the first to end would let the world go on under another.
    while STOPPER.swap(true, Ordering::SeqCst) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
    STOPPING.store(true, Ordering::SeqCst);
    attend(true);
    let threads = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).clone();
    loop {
        if threads.iter().all(|runner| !runner.running.load(Ordering::SeqCst)) {
            return true;
        }
        if Instant::now() >= deadline {
            resume_world();
            return false;
        }
        std::thread::yield_now();
    }
}

/// A lock if it comes soon: another thread may be in what it guards, and
/// a fork does not wait for long.
pub(super) fn soon<T>(lock: &'static Mutex<T>) -> Option<MutexGuard<'static, T>> {
    for _ in 0..50 {
        match lock.try_lock() {
            Ok(held) => return Some(held),
            Err(std::sync::TryLockError::Poisoned(held)) => return Some(held.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    None
}

/// Forks a copy of the process that goes on running, on this thread alone.
/// What another thread is in the middle of stays so in the copy, with no
/// one to end it: the scheduler's locks are held across the fork, so that the
/// copy has them free, and a stop of the world ends there.
pub(crate) fn fork_copy() -> i32 {
    let held = (soon(&QUEUE), soon(&IN_TONDO), soon(crate::cycles::wanted()));
    let pid = unsafe { libc::fork() };
    drop(held);
    if pid == 0 {
        if STOPPING.swap(false, Ordering::SeqCst) {
            attend(false);
        }
        STOPPER.store(false, Ordering::SeqCst);
        crate::cycles::forked();
    }
    pid
}

pub(crate) fn resume_world() {
    STOPPING.store(false, Ordering::SeqCst);
    attend(false);
    let resumed = RESUMED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    RESUMED.notify_all();
    drop(resumed);
    STOPPER.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// The cycle collector, a dump and a logpoint each stop the world, and
    /// the first to end must not let it go on under another
    /// (testing/models/Stop.tla).
    #[test]
    fn one_stops_the_world_at_a_time() {
        static INSIDE: AtomicUsize = AtomicUsize::new(0);
        let stoppers: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..2000 {
                        if super::stop_world(Duration::from_millis(50)) {
                            assert_eq!(INSIDE.fetch_add(1, Ordering::SeqCst), 0, "two stopped the world");
                            std::thread::yield_now();
                            INSIDE.fetch_sub(1, Ordering::SeqCst);
                            super::resume_world();
                        }
                    }
                })
            })
            .collect();
        stoppers.into_iter().for_each(|stopper| stopper.join().unwrap());
    }
}
