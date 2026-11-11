//! `TONDO_WAKES=1` (toolchain 7.5): each task's last waits and wakes, which
//! a dump and `tondo blocked` show of the tasks that wait: when it parked,
//! on what and where, and when it ran again. It hears them through the
//! tools' hooks (`tools.rs`), so that a program without it pays nothing
//! more: a wake lost, the most common reason a program hangs now and then,
//! shows as a task whose last wait was claimed but that never ran again, or
//! that nothing claimed.

use super::*;
use std::collections::{HashMap, VecDeque};

/// The events kept of each task, the last ones.
const KEPT: usize = 16;

/// What a task did: parked, waiting on `on` (a `blocked_kind`) where
/// `frames` say; ran, after what claimed its wait (`Wait::fired`), or
/// `NONE` when it was in none; or took a value task `from` handed it.
#[derive(Clone)]
pub(crate) enum Did {
    Parked { on: u8, frames: Box<[usize]> },
    Ran { by: usize },
    Handed { from: u64 },
}

/// What each task did, with when, in the milliseconds of `millis`.
type Log = HashMap<u64, VecDeque<(u64, Did)>>;

static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// Keeps the tasks' waits and wakes from now on.
pub(crate) fn start() {
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashMap::new());
    tools::listen(tools::WAKES, true);
}

/// How many frames of where a task parked are kept.
const FRAMES: usize = 32;

fn keep(task: u64, did: Did) {
    // The system's tick, which a recording does not hold.
    let at = millis();
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(log) = log.as_mut() else { return };
    let kept = log.entry(task).or_default();
    if kept.len() == KEPT {
        kept.pop_front();
    }
    kept.push_back((at, did));
}

/// The task parks, waiting.
pub(super) fn parked(task: *mut Task) {
    let (id, on) = unsafe { ((*task).id, (*task).blocked_kind.load(Ordering::Acquire)) };
    let frames = unsafe { parked_walk(task) }.take(FRAMES).collect();
    keep(id, Did::Parked { on, frames });
}

/// The task runs again, from the wait it is in, if any.
pub(super) fn ran(task: *mut Task) {
    let task = unsafe { &*task };
    lock_unseen(&task.waiting_lock);
    // The task holds its wait until it clears this, under the same lock.
    let by = unsafe { task.waiting.as_ref() }.map_or(NONE, |wait| wait.fired.load(Ordering::Acquire));
    unlock(&task.waiting_lock);
    keep(task.id, Did::Ran { by });
}

/// Task `from` handed task `to` a value, with no buffer between.
pub(super) fn handed(from: u64, to: u64) {
    keep(to, Did::Handed { from });
}

/// The task ended: nothing of it is kept.
pub(super) fn ended(task: u64) {
    if let Some(log) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        log.remove(&task);
    }
}

/// What is kept of a task, the oldest first, with how many milliseconds
/// ago.
pub(crate) fn of(task: u64) -> Vec<(u64, Did)> {
    let now = millis();
    // A dump of a crash may come while this thread holds the log: it then
    // goes without it, rather than wait for itself.
    let Some(log) = (0..1000).find_map(|_| match LOG.try_lock() {
        Ok(log) => Some(log),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            std::thread::yield_now();
            None
        }
    }) else {
        return Vec::new();
    };
    let Some(kept) = log.as_ref().and_then(|log| log.get(&task)) else { return Vec::new() };
    kept.iter().map(|(at, did)| (now.saturating_sub(*at), did.clone())).collect()
}

/// What ended a wait, by what claimed it, in words: none when the task was
/// in no wait, as one that starts or yields.
pub(crate) fn woken_by(by: usize) -> Option<&'static str> {
    match by {
        NONE => None,
        CANCELLED => Some("a cancellation"),
        DEADLINE => Some("its deadline"),
        _ => Some("a wake"),
    }
}

/// What a task waits for, by its `blocked_kind`, in words.
pub(crate) fn waiting_on(kind: u8) -> &'static str {
    match kind {
        LOCK => "a lock",
        RECEIVING => "a value of a channel",
        SENDING => "room in a channel",
        JOIN => "a task",
        _ => "time or input",
    }
}
