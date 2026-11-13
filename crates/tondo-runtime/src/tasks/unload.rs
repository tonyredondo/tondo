//! The end of a library's runtime, before the library is unloaded
//! (toolchain 7.7): its tasks stop and run their `defer`s, and every thread
//! it started ends. A later call starts it again, as a first call does.

use super::*;

/// Set while a library's runtime ends: the threads it started end too.
static STOPPING: AtomicBool = AtomicBool::new(false);

pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::Acquire)
}

/// Ends the tasks and the threads of the runtime, once the calls of the
/// library in progress on other threads have returned.
pub(crate) fn stop_runtime() {
    while FOREIGN_CALLS.load(Ordering::SeqCst) > 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    // Each task stops at its next wait, as a cancelled one does.
    let all = tasks();
    for task in all.in_start_order() {
        unsafe { cancel_task(task) };
    }
    drop(all);
    // And the processes they started end, as when a program ends.
    crate::process::cancel_live();
    while live_tasks() > 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    STOPPING.store(true, Ordering::Release);
    // Every worker leaves as a surplus one does.
    queue().surplus = usize::MAX / 2;
    READY.iter().for_each(Condvar::notify_all);
    if let Some(poller) = crate::poll::poller() {
        poller.wake();
    }
    MONITOR.unpark();
    WATCHER.unpark();
    crate::cycles::stop();
    crate::io::stop();
    crate::diagnostics::stop();
    crate::recorder::give_back();
    crate::http::stop();
    join_threads();
    give_back_kept();
    // As before the first call.
    MONITOR.clear();
    WATCHER.clear();
    MONITOR_ASLEEP.store(false, Ordering::SeqCst);
    WATCHER_ASLEEP.store(false, Ordering::SeqCst);
    {
        let mut queue = queue();
        queue.surplus = 0;
        queue.workers = 1;
        queue.idle = 0;
        queue.idle_in = [0; domains::MOST];
        queue.waking = [false; domains::MOST];
        queue.polling = false;
        queue.starting = None;
        queue.timers.clear();
        queue.stale = 0;
    }
    IDLE.store(0, Ordering::SeqCst);
    SEEKING.iter().for_each(|seeking| seeking.store(false, Ordering::SeqCst));
    NEXT_COUNT.store(0, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    crate::cycles::restart();
    crate::profiling::restart();
    STOPPING.store(false, Ordering::Release);
}
