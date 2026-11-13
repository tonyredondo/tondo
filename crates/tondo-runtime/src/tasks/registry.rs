//! Every task created and not yet freed, in shards, which a panic cancels, a
//! test checks for leaks and the tools count (docs/refactor.md, phase 2).

use super::*;

/// Every task created and not yet freed: a panic cancels them, and a test
/// runner counts them before and after a test.
/// The tasks alive, in shards that a task's address picks, so that the
/// threads that start and end tasks seldom wait for each other.
pub(super) static TASKS: [Mutex<Tasks>; SHARDS] = [const { Mutex::new(Tasks::new()) }; SHARDS];
pub(super) const SHARDS: usize = 16;

/// A shard's tasks, each in a slot it knows, so that a task comes and goes
/// without a search; a slot let go of, 0, serves the next.
pub(super) struct Tasks {
    pub(super) slots: Vec<usize>,
    pub(super) free: Vec<usize>,
}

impl Tasks {
    const fn new() -> Tasks {
        Tasks { slots: Vec::new(), free: Vec::new() }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &usize> {
        self.slots.iter().filter(|&&task| task != 0)
    }

    pub(super) fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }
}

pub(super) fn shard(task: usize) -> std::sync::MutexGuard<'static, Tasks> {
    TASKS[(task >> 6) % SHARDS].lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn register(task: *mut Task) {
    let mut shard = shard(task as usize);
    let slot = match shard.free.pop() {
        Some(slot) => {
            shard.slots[slot] = task as usize;
            slot
        }
        None => {
            shard.slots.push(task as usize);
            shard.slots.len() - 1
        }
    };
    unsafe { (*task).slot = slot };
}

pub(super) fn unregister(task: *mut Task) {
    let mut shard = shard(task as usize);
    let slot = unsafe { (*task).slot };
    shard.slots[slot] = 0;
    shard.free.push(slot);
}

/// Every task alive, with every shard locked, in order: while they are, no
/// task is freed.
pub(super) struct All(Vec<std::sync::MutexGuard<'static, Tasks>>);

impl All {
    pub(super) fn iter(&self) -> impl Iterator<Item = &usize> {
        self.0.iter().flat_map(|shard| shard.iter())
    }

    /// The tasks in the order they started, which the shards, picked by
    /// addresses that change from run to run, do not keep: what goes
    /// through them all to wake them goes so, the same in a replay as in
    /// its recording.
    pub(super) fn in_start_order(&self) -> Vec<*mut Task> {
        let mut all: Vec<*mut Task> = self.iter().map(|&task| task as *mut Task).collect();
        all.sort_by_key(|&task| unsafe { (*task).id });
        all
    }

    pub(super) fn len(&self) -> usize {
        self.0.iter().map(|shard| shard.len()).sum()
    }
}

pub(super) fn tasks() -> All {
    All(TASKS.iter().map(|shard| shard.lock().unwrap_or_else(|e| e.into_inner())).collect())
}

/// What the scheduler holds now, for `tondo counters`.
pub(crate) struct Scheduler {
    pub live: usize,
    pub ready: usize,
    pub workers: usize,
    pub idle: usize,
    pub timers: usize,
    /// Workers inside a call of C, and the calls of C made so far.
    pub in_c: usize,
    pub c_calls: usize,
}

/// How many times workers looked for a task: what moves while anything
/// does.
pub(crate) fn turns() -> u64 {
    queue().picks as u64
}

/// The tasks ready to run, that wait for a worker.
pub(crate) fn ready_tasks() -> usize {
    scheduler().ready
}

pub(crate) fn scheduler() -> Scheduler {
    let live = tasks().len();
    let queue = queue();
    let waiting = waiting_next().len() + waiting_local().len();
    let (ready, workers, idle) = (queue.runnable.len() + waiting, queue.workers, queue.idle);
    let timers = queue.timers.len().saturating_sub(queue.stale);
    drop(queue);
    let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner());
    let depths = callers.iter().map(|c| (c.depth.load(Ordering::Relaxed), c.calls.load(Ordering::Relaxed)));
    // `calls` counts entries and exits.
    let (in_c, c_calls) = depths
        .fold((0, 0), |(inside, made), (depth, calls)| (inside + usize::from(depth > 0), made + (calls + depth) / 2));
    Scheduler { live, ready, workers, idle, timers, in_c, c_calls }
}

/// How many tasks are alive.
pub(crate) fn live_tasks() -> usize {
    tasks().len()
}

/// The tasks alive now.
pub(crate) fn alive() -> std::collections::HashSet<usize> {
    tasks().iter().copied().filter(|&task| !serves(task)).collect()
}

/// Whether a task serves the standard library itself, which a test leaves
/// running without fault.
fn serves(task: usize) -> bool {
    // While the set is locked no task in it is freed.
    unsafe { (*(task as *mut Task)).service }
}

/// The running task serves the standard library itself, as the server of
/// `App.client` does, and so do the tasks it starts: a test that ends
/// while they run left none of its own.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_task_service() {
    if let Some(task) = unsafe { current().as_mut() } {
        task.service = true;
    }
}

/// The tasks alive now that were not in `before`: where each waits, or
/// that it still runs, and where it was started.
pub(crate) fn leaked(before: &std::collections::HashSet<usize>) -> Vec<String> {
    // While the set is locked no task in it is freed.
    let alive = tasks();
    let mut found: Vec<usize> = alive.iter().copied().filter(|&t| !before.contains(&t) && !serves(t)).collect();
    found.sort_unstable();
    found
        .into_iter()
        .map(|task| unsafe {
            let task = task as *mut Task;
            let origin = || crate::trace::stack(crate::trace::walk(0, (0, 0)), (*task).origin, "TITLE");
            match (*task).state.load(Ordering::Acquire) {
                PARKED => waiting_at(task).replacen("TITLE", "a task waits at", 1),
                PINNED => format!("\na task waits in a call to C{}", origin()),
                _ => format!("\na task still runs{}", origin()),
            }
        })
        .collect()
}
