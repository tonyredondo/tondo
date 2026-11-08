//! What the tools hear of the scheduler (docs/refactor.md, phase 2): the
//! debugger, `tondo trace`'s recorder, the events a dump keeps, the profile
//! of blocking and the detector of lost updates. A tool that listens sets
//! its bit of `LISTENING`, and the scheduler tells the tools what happens
//! through the functions here, which look at that one word first: a program
//! no tool watches pays a load where a task runs, starts, waits or hands a
//! value over.
//!
//! A recorded run's replay, the exploration of orders and a live build's
//! reloads do not listen: they change how workers wait, which task runs
//! next and when no task can run, where the scheduler does those.

use super::*;

static LISTENING: AtomicU32 = AtomicU32::new(0);

/// The detector of lost updates of `tondo test` (`TONDO_RACE`), which notes
/// what tasks do for the runner that explores their orders.
pub(crate) const RACES: u32 = 1;
/// `tondo trace`'s recorder.
pub(crate) const RECORDER: u32 = 1 << 1;
/// The events a dump keeps.
pub(crate) const EVENTS: u32 = 1 << 2;
/// A debugger attached.
pub(crate) const DEBUGGER: u32 = 1 << 3;
/// A debugger's scan, which follows where the values tasks receive come from.
pub(crate) const FOLLOWING: u32 = 1 << 4;
/// `profile.startBlocking`.
pub(crate) const BLOCKING: u32 = 1 << 5;

/// Whether `tool` listens.
#[inline]
pub(crate) fn listens(tool: u32) -> bool {
    LISTENING.load(Ordering::Relaxed) & tool != 0
}

/// `tool` starts or stops listening.
pub(crate) fn listen(tool: u32, on: bool) {
    match on {
        true => LISTENING.fetch_or(tool, Ordering::SeqCst),
        false => LISTENING.fetch_and(!tool, Ordering::SeqCst),
    };
}

/// A task runs on this worker; true when the recorder heard it, to hear it
/// stop as well.
#[inline]
pub(super) fn running(task: *mut Task) -> bool {
    LISTENING.load(Ordering::Relaxed) != 0 && told_running(task)
}

#[cold]
#[inline(never)]
fn told_running(task: *mut Task) -> bool {
    note(|| format!("s {}\n", unsafe { (*task).name }));
    let traced = listens(RECORDER);
    if traced {
        recorder::record(recorder::Event::Run { task: unsafe { (*task).id } });
    }
    traced
}

/// The task `running` was told of went back to its worker: it waits,
/// yields or ended.
#[inline]
pub(super) fn stopped(task: *mut Task, traced: bool) {
    if traced || LISTENING.load(Ordering::Relaxed) != 0 {
        told_stopped(task, traced);
    }
}

#[cold]
#[inline(never)]
fn told_stopped(task: *mut Task, traced: bool) {
    let (id, finished) = unsafe { ((*task).id, (*task).finished) };
    if traced {
        // Nothing runs the task until it is parked or scheduled again.
        let why = match finished {
            true => recorder::Stop::Done,
            false if unsafe { (*task).state.load(Ordering::Acquire) } == PARKING => {
                recorder::Stop::Waits(recorder::frames(unsafe { parked_walk(task) }))
            }
            false => recorder::Stop::Yields,
        };
        recorder::record(recorder::Event::Stop { task: id, why });
    }
    if finished {
        if listens(EVENTS) {
            crate::events::add_of(id, crate::events::What::Ended);
        }
        if listens(FOLLOWING) {
            crate::debugger::task_ended(id);
        }
    }
}

/// The running task started `task`, which runs `code`, from `spawned`.
#[inline]
pub(super) fn spawned(task: *mut Task, code: *const u8, spawned: &Origin) {
    if LISTENING.load(Ordering::Relaxed) != 0 {
        told_spawned(task, code, spawned);
    }
}

#[cold]
#[inline(never)]
fn told_spawned(task: *mut Task, code: *const u8, spawned: &Origin) {
    let task = unsafe { (*task).id };
    if listens(RECORDER) {
        let parent = current_id();
        let at = recorder::frames(spawned.frames().iter().copied());
        recorder::record(recorder::Event::Spawn { task, parent, code: code as usize, at });
    }
    if listens(EVENTS) {
        let at = spawned.frames().first().copied().unwrap_or(0);
        crate::events::add(crate::events::What::Spawned(task, at));
    }
}

/// The task `task` was made, to be started by another: a debugger that
/// stops tasks as they start stops it.
#[inline]
pub(super) fn began(task: u64) {
    if listens(DEBUGGER) {
        crate::debugger::began(task);
    }
}

/// A line for the runner that explores orders, while the detector of lost
/// updates listens.
#[inline]
pub(super) fn note(line: impl FnOnce() -> String) {
    if listens(RACES) && noting() {
        noted(line());
    }
}

/// The running task waits for another, as for a channel, a join or a lock:
/// when, if the profile of blocking samples it.
#[inline]
pub(super) fn blocking() -> Option<Instant> {
    listens(BLOCKING).then(Instant::now)
}

/// When the running task starts to wait on an object, for the tools that
/// time its waits.
#[inline]
pub(super) fn waiting() -> Option<Instant> {
    if LISTENING.load(Ordering::Relaxed) != 0 { started() } else { None }
}

#[cold]
#[inline(never)]
fn started() -> Option<Instant> {
    let _outside = crate::replay::Outside::enter();
    Some(Instant::now())
}

/// The running task waited on an object since `since`.
#[inline]
pub(super) fn waited(task: u64, since: Option<Instant>) {
    if LISTENING.load(Ordering::Relaxed) != 0 {
        told_waited(task, since);
    }
}

#[cold]
#[inline(never)]
fn told_waited(task: u64, since: Option<Instant>) {
    // The tools' reading of the clock, which a recording may not have; a
    // wait that began before they listened counts from when the task
    // blocked, to the system's tick.
    let took = {
        let _outside = crate::replay::Outside::enter();
        since.map_or_else(|| Duration::from_millis(super::blocked_for(task)), |since| since.elapsed())
    };
    // A debugger that stops long waits stops this one, as it ends.
    if listens(DEBUGGER) {
        crate::debugger::waited(task, took);
    }
    if listens(EVENTS) && took.as_millis() >= LONG_WAIT {
        crate::events::add(crate::events::What::Waited(took.as_millis() as u64));
    }
}

/// The running task sends on a channel (`SENDING`), or receives from one.
#[inline]
pub(super) fn channel_used(kind: u8, channel: usize) {
    if listens(DEBUGGER) {
        crate::debugger::channel_used(kind, channel);
    }
}

/// A value went from task `from` to task `to` with no buffer between.
#[inline]
pub(super) fn handed(from: u64, to: u64) {
    if listens(FOLLOWING) {
        crate::debugger::handed(from, to);
    }
}

/// Task `from` put a value into `channel`, which held `held` before.
#[inline]
pub(super) fn sent(channel: usize, held: usize, from: u64) {
    if listens(FOLLOWING) {
        crate::debugger::sent(channel, held, from);
    }
}

/// Task `by` took the oldest value of `channel`, which held `held`.
#[inline]
pub(super) fn taken(channel: usize, held: usize, by: u64) {
    if listens(FOLLOWING) {
        crate::debugger::taken(channel, held, by);
    }
}

/// The task of `join` ended, its result there.
#[inline]
pub(super) fn ended(join: usize) {
    if listens(FOLLOWING) {
        crate::debugger::ended(join);
    }
}

/// The running task took the result of `join`.
#[inline]
pub(super) fn joined(join: usize) {
    if listens(FOLLOWING) {
        crate::debugger::joined(join);
    }
}
