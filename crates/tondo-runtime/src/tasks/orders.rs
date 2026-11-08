//! The orders a test tries (testing 8.10): a seed's order of the tasks on one
//! worker, or the choices a run is given and what it writes for the runner,
//! with the virtual clock of those runs (docs/refactor.md, phase 2).

use super::*;

/// `TONDO_RT_SCHEDULE`, a seed in hex (`tondo test --schedules`): the
/// tasks run on one worker, the next one is the seed's choice among those
/// that can run, and each gives way after every call that may wait, so
/// each seed meets the program's tasks in another order, and the same seed
/// in the same one.
pub(super) fn exploring() -> Option<&'static AtomicU64> {
    static SEED: std::sync::OnceLock<Option<AtomicU64>> = std::sync::OnceLock::new();
    SEED.get_or_init(|| {
        // A recorded run, and its replay, run in one order, and so does one
        // whose choices are given.
        let given = crate::replay::on() || script().is_some();
        let text = std::env::var("TONDO_RT_SCHEDULE").ok().or_else(|| given.then(|| "0".into()))?;
        let seed = u64::from_str_radix(&text, 16).ok()?;
        // Every call that may wait gives way.
        attend(true);
        Some(AtomicU64::new(seed))
    })
    .as_ref()
}

/// `TONDO_RT_ORDER`, the choices to make, as `0.2.1`, for `tondo test
/// --schedules all`, and after a `/` the tasks to run next, as `5.7`: the
/// rest are the first of each. A task goes by its name, which unlike its
/// number does not depend on the order. `TONDO_RT_ORDER_OUT`
/// names where what the run did goes, so that the runner knows what else it
/// could have done and which of that would differ: a line for each choice,
/// `d <choice> <count> <tasks>`, with the tasks it was among, for a choice of
/// a task; `s <task>` as a task runs; `t <object>` for what it touches;
/// `h <object>` for what it only comes after; `b` as it waits; `n <task>`
/// for one it starts.
pub(super) struct Script {
    pub(super) given: Vec<usize>,
    pub(super) made: usize,
    pub(super) next: VecDeque<u64>,
    pub(super) following: bool,
    pub(super) out: Option<std::fs::File>,
    /// The task chosen last when nothing said which, none for an arm of a
    /// `select`, and how many times in a row: a task that waits by spinning
    /// gives way after `FAIR` of them, and an arm to the next.
    pub(super) last: Option<u64>,
    pub(super) streak: usize,
}

/// How many times in a row a run given its choices takes the same task, or
/// the first arm of a `select`, by itself while another can go: no scheduler
/// leaves a task spinning for ever, nor a `select` taking one arm, so
/// neither does an order (testing 8.10).
pub(super) const FAIR: usize = 1000;

pub(super) fn script() -> Option<&'static Mutex<Script>> {
    static SCRIPT: std::sync::OnceLock<Option<Mutex<Script>>> = std::sync::OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let text = std::env::var("TONDO_RT_ORDER").ok()?;
            let (given, next) = text.split_once('/').unwrap_or((&text, ""));
            // `2*5` is five choices of 2.
            let given = given
                .split('.')
                .flat_map(|choice| {
                    let (choice, times) = choice.split_once('*').unwrap_or((choice, "1"));
                    let times = times.parse().unwrap_or(1);
                    choice.parse().ok().map(|choice| std::iter::repeat_n(choice, times)).into_iter().flatten()
                })
                .collect();
            let next = next.split('.').filter_map(|task| task.parse().ok()).collect();
            let out = std::env::var_os("TONDO_RT_ORDER_OUT").and_then(|path| std::fs::File::create(path).ok());
            Some(Mutex::new(Script { given, made: 0, next, following: false, out, last: None, streak: 0 }))
        })
        .as_ref()
}

/// Whether what the run does goes to the runner.
#[cold]
#[inline(never)]
pub(super) fn noting() -> bool {
    script().is_some()
}

pub(super) fn noted(line: String) {
    use std::io::Write;
    let Some(script) = script() else { return };
    if let Some(out) = &mut script.lock().unwrap_or_else(|e| e.into_inner()).out {
        let _ = out.write_all(line.as_bytes());
    }
}

/// The running task touched something other tasks may touch, for the runner
/// to try the orders in which that comes the other way around.
#[inline(always)]
pub(crate) fn touch(object: usize) {
    if crate::race::on() {
        touched(object);
    }
}

#[cold]
#[inline(never)]
pub(super) fn touched(object: usize) {
    if noting() {
        noted(format!("t {object}\n"));
    }
}

/// A task uses a collection of `std.sync`: another may go first, as before
/// the write of an `Atomic`.
#[inline]
pub(crate) fn shared_used(object: usize) {
    if crate::race::on() {
        may_switch();
        touch(object);
    }
}

/// What every task reaches outside the program, as files and sockets, in the
/// orders of a test: what the program cannot see the identity of.
pub(super) const OUTSIDE: usize = 0;

/// Whether the running test reached outside the program, which a run of it
/// in another order would change again (testing 8.10).
pub(crate) static REACHED_OUTSIDE: AtomicBool = AtomicBool::new(false);

/// A test calls something that reaches outside the program.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_outside() {
    REACHED_OUTSIDE.store(true, Ordering::Relaxed);
    touch(OUTSIDE);
}

/// How far a run given its choices is ahead of the clock.
pub(super) static AHEAD: AtomicU64 = AtomicU64::new(0);

/// What the clock says, which a run given its choices moves itself.
pub(crate) fn clock_now() -> Instant {
    Instant::now() + Duration::from_nanos(AHEAD.load(Ordering::Relaxed))
}

/// How far the clock of a run given its choices is ahead, in nanoseconds.
pub(crate) fn ahead() -> u64 {
    AHEAD.load(Ordering::Relaxed)
}

/// In a run given its choices nothing waits for the clock: time goes to the
/// next timer once no task can run, and sooner is one more choice, so that a
/// timeout that comes before an answer is an order like any other.
pub(super) fn time_passes(queue: &mut Queue) {
    let Some(at) = queue.timers.peek().map(|timer| timer.at) else { return };
    if queue.runnable.is_empty() || scripted(2, &[]) == Some(1) {
        let left = at.saturating_duration_since(clock_now());
        AHEAD.fetch_add(left.as_nanos() as u64, Ordering::Relaxed);
    }
}

/// A choice among `count` things, when the run is given its choices: which
/// are `tasks`, if they are.
pub(super) fn scripted(count: usize, tasks: &[u64]) -> Option<usize> {
    let mut script = script()?.lock().unwrap_or_else(|e| e.into_inner());
    let script = &mut *script;
    let given = script.given.get(script.made).copied();
    // They start at a choice: before it, a task that can run alone ran.
    let wanted = given.is_none() && (count > 1 || script.following);
    let next = script.next.front().filter(|_| wanted).and_then(|name| tasks.iter().position(|task| task == name));
    if next.is_some() {
        script.next.pop_front();
        script.following = true;
    }
    if count < 2 {
        return Some(0);
    }
    let choice = given.or(next).unwrap_or_else(|| {
        // By itself the first, unless that one has gone too long.
        let first = tasks.first().copied();
        script.streak = if first == script.last { script.streak + 1 } else { 1 };
        script.last = first;
        if script.streak <= FAIR {
            return 0;
        }
        script.streak = 0;
        tasks.iter().position(|&task| Some(task) != first).unwrap_or(1)
    }) % count;
    script.made += 1;
    if let Some(out) = &mut script.out {
        use std::io::Write;
        let tasks = tasks.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let _ = out.write_all(format!("d {choice} {count} {tasks}\n").as_bytes());
    }
    Some(choice)
}

/// After a task reads a cell of `std.sync`, while exploring orders: by the
/// seed, it drops below the others and they run, so that a write of theirs
/// comes before its own and a lost update does not depend on chance.
pub(crate) fn may_lose_update() {
    let Some(seed) = exploring() else { return };
    // The choice of the task to run next covers it, given the choices.
    if script().is_some() || seeded(seed).is_multiple_of(2) {
        if let Some(task) = unsafe { current().as_mut() } {
            task.priority = 1;
        }
        give_way();
    }
}

/// Where the order of two tasks can change without a wait, while exploring
/// orders: a lock let go, a write of an `Atomic`, a task spawned.
pub(crate) fn may_switch() {
    if exploring().is_some() {
        give_way();
    }
}

/// Where the task to run next is among those that can, while exploring
/// orders (testing 8.10), by PCT or, with one seed in four, at random: each
/// task has a priority the seed gives
/// as it starts, the highest runs, and at two of its steps, which the seed
/// chooses within a scale it chooses too, the task that would run drops
/// below all, so that an order that needs a task to stop at a few given
/// steps comes with a probability that does not fall with the program's
/// length.
pub(super) fn prioritized(seed: &AtomicU64, runnable: &VecDeque<*mut Task>) -> usize {
    if script().is_some() {
        let tasks: Vec<u64> = runnable.iter().map(|&task| unsafe { (*task).name }).collect();
        return scripted(runnable.len(), &tasks).unwrap_or(0);
    }
    static STEPS: AtomicU64 = AtomicU64::new(0);
    static CHANGES: std::sync::OnceLock<[u64; 2]> = std::sync::OnceLock::new();
    let changes = CHANGES.get_or_init(|| {
        // From a few steps, as a short test has, to a long one's.
        let scale = 1 << (2 + seeded(seed) % 18);
        [1 + seeded(seed) % scale, 1 + seeded(seed) % scale]
    });
    // One seed in four chooses at random at every step instead: what comes
    // of a change in the right step comes far more often in a short test.
    static WALKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *WALKS.get_or_init(|| seeded(seed).is_multiple_of(4)) {
        return seeded(seed) as usize % runnable.len();
    }
    let step = STEPS.fetch_add(1, Ordering::Relaxed) + 1;
    let priority = |at: usize| unsafe { (*runnable[at]).priority };
    let highest = (0..runnable.len()).max_by_key(|&at| priority(at)).unwrap_or(0);
    // A change: below every priority a task starts with, and below the
    // earlier changes'.
    if let Some(change) = changes.iter().position(|&c| c == step) {
        let task = runnable[highest];
        unsafe { (*task).priority = (changes.len() - change) as u64 };
        return (0..runnable.len()).max_by_key(|&at| priority(at)).unwrap_or(0);
    }
    highest
}

/// Whether the tasks run on one thread in an order a seed gives, where
/// nothing that runs meanwhile on another thread may change what they do.
pub(crate) fn deterministic() -> bool {
    exploring().is_some()
}

/// The next number of the seed's sequence (SplitMix64).
pub(super) fn seeded(state: &AtomicU64) -> u64 {
    let mut z = state.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Puts the running task back among those that can run, and lets the
/// worker choose again; not with C frames on its stack, which stay on
/// their thread.
pub(super) fn give_way() {
    let task = current();
    if task.is_null() {
        return;
    }
    let in_c = in_c_here();
    if !in_c.is_null() && unsafe { (*in_c).depth.load(Ordering::SeqCst) } > 0 {
        return;
    }
    unsafe {
        // The worker sees a task notified while switching out, and makes
        // it runnable again.
        (*task).state.store(NOTIFIED, Ordering::Release);
        switch(&mut (*task).sp, *home());
    }
}
