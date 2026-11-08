//! Timers: the waits that end at a time, which the workers fire when due, and
//! those of waits that ended another way, dropped once they are most of them
//! (docs/refactor.md, phase 2).

use super::*;

/// Fires the timers that are due; their tasks join `runnable`.
pub(super) fn fire_timers(queue: &mut Queue) {
    if queue.timers.is_empty() {
        return;
    }
    let now = clock_now();
    while queue.timers.peek().is_some_and(|t| t.at <= now) {
        let timer = queue.timers.pop().unwrap();
        if timer.wait.claim(timer.index) {
            notify_with(timer.wait.task, |task| queue.runnable.push_back(task));
        } else {
            queue.stale = queue.stale.saturating_sub(1);
        }
    }
}

/// Notes timers whose wait ended another way, and drops them once they
/// are most of the timers.
pub(super) fn timers_ended(count: usize) {
    let mut queue = queue();
    queue.stale += count;
    if queue.stale > 64 && queue.stale * 2 > queue.timers.len() {
        queue.timers.retain(|t| t.wait.fired.load(Ordering::Acquire) == NONE);
        queue.stale = 0;
    }
}

pub(super) fn add_timer(delay: Duration, wait: Arc<Wait>, index: usize) {
    if crate::clock::add_timer(delay.as_nanos() as u64, &wait, index) {
        return;
    }
    let mut queue = queue();
    queue.sequence += 1;
    let sequence = queue.sequence;
    queue.timers.push(Timer { at: clock_now() + delay, sequence, wait, index });
    // An idle worker recomputes how long to sleep.
    wake_one(queue);
}
