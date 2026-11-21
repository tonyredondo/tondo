//! Calls of C: what a worker in C keeps for the monitor, which starts another
//! worker in the place of one that stays there, and a call of C back into
//! Tondo (docs/refactor.md, phase 2).

use super::*;

/// A worker's calls of C: how deeply it is inside them, and a count of
/// entries and exits that changes whenever a call starts or ends; and, for
/// stack traces, where the program made each call.
pub(super) struct InC {
    pub(super) depth: AtomicUsize,
    pub(super) calls: AtomicUsize,
    /// For each call deep, the frame of the code that called C, and where
    /// it called from.
    pub(super) frames: [AtomicUsize; 8],
    pub(super) returns: [AtomicUsize; 8],
    /// When each call started, while a tool traces the program.
    pub(super) started: [AtomicU64; 8],
    /// The task that made the calls.
    pub(super) task: std::sync::atomic::AtomicPtr<Task>,
    /// Whether another worker took this one's place during the current call.
    pub(super) replaced: AtomicBool,
    /// Whether the thread is one of C's, not a worker, whose calls of C hold
    /// no worker: the monitor starts none in its place (`Task::native`).
    pub(super) native: AtomicBool,
}

/// Every worker that has called C, which the monitor watches.
pub(super) static CALLERS: Mutex<Vec<&'static InC>> = Mutex::new(Vec::new());
/// Those of threads that ended, for the next: a list of them taken before
/// may still read one.
static SPARE_CALLERS: Mutex<Vec<&'static InC>> = Mutex::new(Vec::new());
/// The calls of C made by threads that ended.
pub(super) static ENDED_C_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(super) static MONITOR: ThreadSlot = ThreadSlot::new();
pub(super) static MONITOR_ASLEEP: AtomicBool = AtomicBool::new(false);
/// The monitor's looks, counted as it makes them: what the checker sees of
/// whether it looks (16.14), rather than what its flag says.
pub(super) static MONITOR_LOOKS: AtomicU64 = AtomicU64::new(0);
/// How often the monitor looks: a call still in C after one look and the
/// next is long, and blocks its worker.
pub(super) const MONITOR_PERIOD: Duration = Duration::from_millis(1);

/// This worker's calls of C, if it made any.
pub(super) fn in_c_here() -> *const InC {
    worker(|w| w.in_c.get())
}

pub(super) fn in_c() -> &'static InC {
    let here = in_c_here();
    if !here.is_null() {
        return unsafe { &*here };
    }
    let spare = SPARE_CALLERS.lock().unwrap_or_else(|e| e.into_inner()).pop();
    let new: &'static InC = spare.unwrap_or_else(|| {
        Box::leak(Box::new(InC {
            depth: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            frames: Default::default(),
            returns: Default::default(),
            started: Default::default(),
            task: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            replaced: AtomicBool::new(false),
            native: AtomicBool::new(false),
        }))
    });
    new.native.store(unsafe { current().as_ref() }.is_some_and(|task| task.native), Ordering::SeqCst);
    CALLERS.lock().unwrap_or_else(|e| e.into_inner()).push(new);
    worker(|w| w.in_c.set(new));
    if let Some(next) = own_next() {
        next.in_c.store(new as *const InC as *mut InC, Ordering::Release);
    }
    kept().in_c.set(new);
    new
}

/// As a thread that called C ends: its calls leave the list and its count,
/// and serve the next thread that calls C.
pub(super) fn in_c_ended(kept: &Kept) {
    let Some(state) = (unsafe { kept.in_c.replace(std::ptr::null()).as_ref() }) else { return };
    CALLERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|&other| !std::ptr::eq(other, state));
    // `calls` counts entries and exits, all made.
    ENDED_C_CALLS.fetch_add(state.calls.swap(0, Ordering::SeqCst) / 2, Ordering::SeqCst);
    state.task.store(std::ptr::null_mut(), Ordering::SeqCst);
    state.replaced.store(false, Ordering::SeqCst);
    SPARE_CALLERS.lock().unwrap_or_else(|e| e.into_inner()).push(state);
}

/// Compiled code calls this before a C function: while the call lasts the
/// worker runs nothing else, so the monitor may start another in its place.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_foreign_enter() {
    REACHED_OUTSIDE.store(true, Ordering::Relaxed);
    touch(OUTSIDE);
    // C follows no references of Tondo values.
    leave_tondo();
    let state = in_c();
    state.calls.fetch_add(1, Ordering::SeqCst);
    if crate::diagnostics::enabled() {
        state.task.store(current(), Ordering::Relaxed);
    }
    let depth = state.depth.fetch_add(1, Ordering::SeqCst);
    if let (Some(frame), Some(returns)) = (state.frames.get(depth), state.returns.get(depth)) {
        // The caller's frame lives while C runs; this function's own is gone
        // once it returns, and C may write over it.
        let fp = crate::trace::frame_pointer();
        let (caller, address) = unsafe { (*(fp as *const usize), *((fp + 8) as *const usize)) };
        frame.store(caller, Ordering::Relaxed);
        returns.store(address, Ordering::Relaxed);
        state.started[depth].store(if recorder::on() { recorder::now() } else { 0 }, Ordering::Relaxed);
    }
    // One worker, exploring orders, starts no other in its place.
    if exploring().is_some() {
        crate::replay::calls_c();
        return;
    }
    MONITOR.start_once(|| spawn_thread(Some(64 * 1024), monitor));
    if MONITOR_ASLEEP.load(Ordering::SeqCst) && MONITOR_ASLEEP.swap(false, Ordering::SeqCst) {
        MONITOR.unpark();
    }
}

/// Where a fault inside C goes back to: the frame of the code that made the
/// running worker's innermost call of C, and the address it called from.
pub(crate) fn c_frame() -> Option<(usize, usize)> {
    let state = unsafe { in_c_here().as_ref()? };
    let depth = state.depth.load(Ordering::Relaxed).checked_sub(1)?;
    let frame = state.frames.get(depth)?.load(Ordering::Relaxed);
    let address = state.returns.get(depth)?.load(Ordering::Relaxed);
    (frame != 0).then_some((frame, address))
}

/// A call of C a trace shows: one of a millisecond or more, in nanoseconds.
pub(super) const LONG_CALL: u64 = 1_000_000;

/// After a C function returns: a worker that was replaced meanwhile makes
/// one worker retire.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_foreign_leave() {
    crate::replay::waited();
    enter_tondo();
    let state = in_c();
    let depth = state.depth.load(Ordering::Relaxed).wrapping_sub(1);
    if let Some(started) = state.started.get(depth).map(|s| s.load(Ordering::Relaxed))
        && started != 0
        && recorder::on()
    {
        let took = recorder::now().saturating_sub(started);
        if took >= LONG_CALL {
            let (frame, from) =
                (state.frames[depth].load(Ordering::Relaxed), state.returns[depth].load(Ordering::Relaxed));
            let walk = crate::trace::walk(frame, stack_bounds().unwrap_or((0, 0)));
            let at = recorder::frames(std::iter::once(from).chain(walk));
            let task = unsafe { current().as_ref().map_or(0, |task| task.id) };
            recorder::record(recorder::Event::Call { task, took, at });
        }
    }
    state.calls.fetch_add(1, Ordering::SeqCst);
    if state.depth.fetch_sub(1, Ordering::SeqCst) == 1 && state.replaced.swap(false, Ordering::SeqCst) {
        let mut queue = queue();
        queue.surplus += 1;
        wake_one(queue);
    }
}

/// A task waits on its worker for long, held by a tool: another worker runs
/// the others, as for a long call of C, until `step_back`. One worker,
/// exploring orders, starts no other.
pub(crate) fn step_aside() {
    if exploring().is_none() {
        ASIDE.fetch_add(1, Ordering::SeqCst);
        queue().workers += 1;
        start_worker(false, None);
    }
}

/// The tasks a tool holds on their worker, for each of which another
/// worker stands in (`step_aside`).
static ASIDE: AtomicUsize = AtomicUsize::new(0);

/// How many workers stand in for one in a call of C or one whose task a
/// tool holds: none of them counts against twice the workers (16.14).
pub(super) fn standing_in() -> usize {
    let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let in_c = callers
        .iter()
        .filter(|state| state.replaced.load(Ordering::SeqCst) && state.depth.load(Ordering::SeqCst) > 0)
        .count();
    in_c + ASIDE.load(Ordering::SeqCst)
}

/// The task goes on: one worker retires.
pub(crate) fn step_back() {
    if exploring().is_none() {
        ASIDE.fetch_sub(1, Ordering::SeqCst);
        let mut queue = queue();
        queue.surplus += 1;
        wake_one(queue);
    }
}

/// How long a worker may stay on one task while tasks wait for one and none
/// is idle before the first task to ask gives its worker away (language
/// 11.15). By the clock, not by the monitor's looks, whose millisecond of
/// sleep lasts longer on some systems (macOS's coalesced timers).
const GIVE_WAY_AFTER: Duration = Duration::from_millis(10);
/// And before another worker starts beside it (language 16.14): a tenth of
/// a second, as a worker started and retired costs the processor some 100 µs
/// and buys a saturated machine nothing, so tasks that compute for less
/// never get one, while the task that waits is still served soon enough for
/// a person or a health check (docs/decisions.md).
const ADD_AFTER: Duration = Duration::from_millis(100);

/// Watches the workers. One still in the same call of C after a period
/// blocks its tasks, so when tasks wait to run and no worker is idle,
/// another worker starts in its place (as Go's runtime does). One that stays
/// on the same task while tasks wait and none is idle has the first task to
/// ask give its worker away, and then another worker beside it
/// (testing/models/tests/watcher.to). Sleeps while no worker is in C and
/// none of that can happen.
pub(super) fn monitor() {
    let mut last: HashMap<usize, usize> = HashMap::new();
    let mut held: Vec<(u64, Instant)> = Vec::new();
    loop {
        std::thread::sleep(MONITOR_PERIOD);
        if stopping() {
            return;
        }
        MONITOR_LOOKS.store(MONITOR_LOOKS.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
        let callers: Vec<&'static InC> = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut busy = false;
        for state in callers {
            let key = state as *const InC as usize;
            let calls = state.calls.load(Ordering::SeqCst);
            if state.depth.load(Ordering::SeqCst) == 0 || state.native.load(Ordering::SeqCst) {
                last.remove(&key);
                continue;
            }
            busy = true;
            if last.insert(key, calls) == Some(calls) {
                let queue = queue();
                if !tasks_wait(&queue) {
                    continue;
                }
                // What waits for this worker has another: one that sleeps,
                // told to look, as often as it takes, or a new one, once.
                if queue.idle > 0 {
                    drop(queue);
                    wake_idle();
                    continue;
                }
                if state.replaced.load(Ordering::SeqCst) {
                    continue;
                }
                state.replaced.store(true, Ordering::SeqCst);
                crate::diagnostics::WORKERS_REPLACED.add(1);
                add_worker(queue, false);
            }
        }
        let watched = watch_held(&mut held);
        if !busy && watched != Watched::Looking {
            MONITOR_ASLEEP.store(true, Ordering::SeqCst);
            let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let quiet = callers
                .iter()
                .all(|state| state.depth.load(Ordering::SeqCst) == 0 || state.native.load(Ordering::SeqCst));
            if quiet && !stopping() {
                // Workers all busy with nothing waiting: the next timer may
                // make something wait.
                match watch_held(&mut held) {
                    Watched::Looking => {}
                    Watched::Until(at) => MONITOR.park_until(at),
                    Watched::Idle => MONITOR.park(),
                }
            }
            MONITOR_ASLEEP.store(false, Ordering::SeqCst);
        }
    }
}

/// Whether tasks wait for a worker: in the queues, or as timers due.
fn tasks_wait(queue: &Queue) -> bool {
    let due = queue.timers.peek().is_some_and(|timer| timer.at <= clock_now());
    due || !queue.runnable.is_empty() || !waiting_next().is_empty() || !waiting_local().is_empty()
}

/// Another worker, in the place of one in C or `beside` one a task holds.
fn add_worker(mut queue: MutexGuard<'static, Queue>, beside: bool) {
    queue.workers += 1;
    // It looks for tasks for a domain where none looks, which it is given
    // as it starts: through a slot of the queue, two added in one look took
    // one domain, and the other's `SEEKING` stayed set with nobody seeking,
    // so that nothing woke a worker for a task made ready from outside.
    let domain = (0..super::domains::count()).find(|&domain| !SEEKING[domain].load(Ordering::SeqCst));
    if let Some(domain) = domain {
        SEEKING[domain].store(true, Ordering::SeqCst);
    }
    drop(queue);
    start_worker(beside, domain);
}

/// What the watch of held workers found: workers to look at again in a
/// moment, every worker busy with nothing waiting until a timer is due, or
/// a worker idle, which needs no watch.
#[derive(PartialEq)]
enum Watched {
    Looking,
    Until(Option<Instant>),
    Idle,
}

/// One look at the workers that run Tondo code: while none is idle and tasks
/// wait, one whose count of started tasks has not moved for `GIVE_WAY_AFTER`
/// has the first task to ask give its worker away, and one still on it after
/// `ADD_AFTER` gets another worker beside it, once.
fn watch_held(held: &mut Vec<(u64, Instant)>) -> Watched {
    if exploring().is_some() {
        held.clear();
        return Watched::Idle;
    }
    let shared = queue();
    if watches_nexts(shared.idle) {
        held.clear();
        drop(shared);
        return watch_nexts();
    }
    if !tasks_wait(&shared) {
        held.clear();
        return Watched::Until(shared.timers.peek().map(|timer| timer.at));
    }
    let workers = shared.workers;
    drop(shared);
    let given = crate::config::workers();
    let count = NEXT_COUNT.load(Ordering::Acquire).min(NEXTS.len());
    // The clock the runtime's timers keep.
    let now = clock_now();
    held.resize(count, (u64::MAX, now));
    for (next, (seen, since)) in NEXTS[..count].iter().zip(held.iter_mut()) {
        // A worker in a call of C is the watch above's.
        let in_c = next.in_c.load(Ordering::Acquire);
        if !in_c.is_null() && unsafe { (*in_c).depth.load(Ordering::SeqCst) } > 0 {
            (*seen, *since) = (u64::MAX, now);
            continue;
        }
        let mut look = (*seen, *since);
        let still = stayed(&mut look, next.turns.load(Ordering::Relaxed), now);
        (*seen, *since) = look;
        if asks_to_give_way(still, CHECKPOINT_ASKS.load(Ordering::Relaxed)) {
            CHECKPOINT_ASKS.fetch_or(GIVE_WAY, Ordering::SeqCst);
        }
        let (marked, beside) = (next.held.load(Ordering::Relaxed), next.beside.load(Ordering::Relaxed));
        if puts_beside(still, marked, beside, workers, given) {
            let queue = queue();
            // Again under the lock: another may have started meanwhile.
            if puts_beside(still, false, beside, queue.workers, given)
                && next.held.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
            {
                crate::diagnostics::WORKERS_ADDED.add(1);
                add_worker(queue, true);
            }
        }
    }
    Watched::Looking
}

/// While a worker is idle, the tasks woken next to workers that run others,
/// which may compute for long: one there for longer than NEXT_WAIT has an
/// idle worker told to look for it, as no other wake may come while its
/// waker computes (testing/models/tests/next.to). The monitor watches while
/// one waits there.
fn watch_nexts() -> Watched {
    let now = nanos();
    let mut waiting = false;
    let mut told = false;
    for next in nexts() {
        if next.task.load(Ordering::SeqCst).is_null() {
            continue;
        }
        waiting = true;
        if !told && now.saturating_sub(next.since.load(Ordering::Relaxed)) >= NEXT_WAIT.as_nanos() as u64 {
            told = true;
            wake_idle();
        }
    }
    if waiting { Watched::Looking } else { Watched::Idle }
}

/// A task was woken next to a worker, whose task may compute for long: the
/// monitor watches until it runs, asleep as it may be with every worker
/// busy and nothing waiting before. With a worker idle it tells that one
/// (`watch_nexts`); with none, it watches the worker held (`watch_held`).
pub(super) fn watch_next() {
    watch_busy();
}

/// Whether the watch is of the tasks woken next to workers, not of held
/// workers: while a worker is idle, which takes what waits, so that none is
/// put beside a held one then (16.14).
fn watches_nexts(idle: usize) -> bool {
    idle > 0
}

/// Whether the watcher asks the first task to ask with `tasks.checkpoint()`
/// to give its worker away, a worker having stayed `still` on one task, by
/// the word `asks` checkpoints read (11.15): once past the first interval,
/// and not while it asks already.
fn asks_to_give_way(still: Duration, asks: u32) -> bool {
    still >= GIVE_WAY_AFTER && asks & GIVE_WAY == 0
}

/// Whether the watcher puts a worker beside one that has stayed `still` on
/// one task (16.14): only past the second interval, never beside one that
/// has one already (`marked`) or was itself put beside another (`beside`),
/// and never once the program has twice the workers `given`. A promise about
/// one choice, held by this function's test, where the checker holds those
/// about what the scheduler does over time.
fn puts_beside(still: Duration, marked: bool, beside: bool, workers: usize, given: usize) -> bool {
    still >= ADD_AFTER && !marked && !beside && workers < 2 * given
}

/// How long a worker has stayed on one task, by a look at it: its count of
/// started tasks, `turns`, at `now`, against what the last look saw, `seen`,
/// which a count that moved starts again.
fn stayed(seen: &mut (u64, Instant), turns: u64, now: Instant) -> Duration {
    if seen.0 != turns {
        *seen = (turns, now);
        return Duration::ZERO;
    }
    now.saturating_duration_since(seen.1)
}

/// Every worker is busy, or a task was queued with none idle: the monitor
/// looks at what holds them, unless it does already.
pub(super) fn watch_busy() {
    if exploring().is_some() {
        return;
    }
    MONITOR.start_once(|| spawn_thread(Some(64 * 1024), monitor));
    if MONITOR_ASLEEP.load(Ordering::SeqCst) && MONITOR_ASLEEP.swap(false, Ordering::SeqCst) {
        MONITOR.unpark();
    }
}

/// Runs the body of a function C calls: on this worker when C called back
/// inside a task, or as a new task, which starts on the calling thread of
/// C and goes on on a worker if it waits, while that thread waits for it.
/// A library passes what starts it, for the first call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_callback(
    inner: unsafe extern "C" fn(*mut u8),
    frame: *mut u8,
    library: Option<unsafe extern "C" fn()>,
) {
    if !current().is_null() {
        // A call back from C on a worker: Tondo code again until it returns.
        let _held = crate::replay::held();
        enter_tondo();
        unsafe { inner(frame) };
        leave_tondo();
        return;
    }
    if let Some(start) = library {
        crate::start_library(start);
    }
    let kept = kept();
    crate::trace::guard_thread(false, kept);
    // Where it is in the program, the same in another run.
    crate::replay::entered((inner as usize).wrapping_sub(tondo_rt_callback as *const () as usize));
    // On this thread's own stack, as a task that stays here: no stack of its
    // own nor a switch to one, and if it waits, this thread waits for it. One
    // worker exploring orders runs every task, this one too; and a library's
    // tasks leave the threads of the program that loaded it, which keep
    // nothing of it, so that it unloads (`leave_host_thread`).
    if exploring().is_none() && !crate::library() {
        unsafe { run_here(inner, frame, kept) };
        crate::replay::left();
        if library.is_some() {
            crate::flush_stdout();
        }
        return;
    }
    let call =
        ForeignCall { inner, frame, state: AtomicU8::new(CALL_RUNS), lock: Mutex::new(()), finished: Condvar::new() };
    let task = new_task(
        std::ptr::null_mut(),
        Some(task_stack()),
        std::ptr::null_mut(),
        std::ptr::null(),
        ResultKind::Nothing,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        None,
    );
    unsafe {
        (*task).main = false;
        (*task).foreign = &call;
        (*task).origin = Some(&crate::trace::FOREIGN);
    }
    FOREIGN_CALLS.fetch_add(1, Ordering::SeqCst);
    run(task);
    // The workers start when a call first waits: one that returns at once
    // needs none.
    if !call.done() {
        start_workers();
        FOREIGN_WAITING.fetch_add(1, Ordering::SeqCst);
        // The one worker may have ended: this thread runs what it waits for.
        if exploring().is_some() {
            queue().workers += 1;
            work_until(false, Some(&|| call.done()));
            queue().workers -= 1;
        }
        // Waited for from now, unless it ended meanwhile; its end stores
        // that it is done under the lock, so this thread sees it there.
        let mut locked = call.lock.lock().unwrap_or_else(|e| e.into_inner());
        if call.state.compare_exchange(CALL_RUNS, CALL_WAITED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            while !call.done() {
                locked = call.finished.wait(locked).unwrap_or_else(|e| e.into_inner());
            }
        }
        drop(locked);
        FOREIGN_WAITING.fetch_sub(1, Ordering::SeqCst);
    }
    crate::replay::left();
    FOREIGN_CALLS.fetch_sub(1, Ordering::SeqCst);
    // What a library wrote comes before what its caller writes next.
    if library.is_some() {
        crate::flush_stdout();
    }
}

/// Runs a call of C's on the calling thread's stack as a task that stays
/// there (`Task::native`): it waits on this thread if it waits, and ends as
/// a task that switched back to its worker would.
unsafe fn run_here(inner: unsafe extern "C" fn(*mut u8), frame: *mut u8, kept: &Kept) {
    kept.find_stack();
    // In this thread's block, which the call before left: no allocation.
    let task = new_task(
        std::ptr::null_mut(),
        None,
        std::ptr::null_mut(),
        std::ptr::null(),
        ResultKind::Nothing,
        std::ptr::null_mut(),
        kept.call_block(),
        Some(kept.native()),
    );
    unsafe {
        (*task).main = false;
        (*task).origin = Some(&crate::trace::FOREIGN);
    }
    // Counted on this thread, as the others' calls are on theirs.
    kept.calling.store(true, Ordering::Release);
    worker(|w| w.current.set(task));
    let traced = tools::running(task);
    let first = kept.runner.get().is_none();
    enter_tondo();
    if first && let Some(runner) = kept.runner.get() {
        runner.native.store(true, Ordering::SeqCst);
    }
    unsafe { inner(frame) };
    leave_tondo();
    worker(|w| w.current.set(std::ptr::null_mut()));
    tools::stopped(task, traced);
    unsafe {
        let in_domain = (*task).in_domain;
        for asked in [&(*task).cancelled, &(*task).again] {
            if asked.load(Ordering::SeqCst) {
                attend(false);
            }
        }
        Wait::left(&mut *task);
        left_held(&mut *task);
        unregister(task);
        (*task).mark = 0;
        std::ptr::drop_in_place(task);
        crate::clock::stopped(in_domain);
    }
    kept.calling.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{ADD_AFTER, GIVE_WAY, GIVE_WAY_AFTER, asks_to_give_way, puts_beside, stayed, watches_nexts};
    use std::time::{Duration, Instant};

    /// A worker whose tasks change, as tasks of 2 ms each change it, never
    /// stays on one long enough to give way or to get a worker beside it,
    /// however long it is watched; one whose count stops moving stays from
    /// the look that last saw it move, a late look counting the time it
    /// missed. What the watch decides is a matter of counts and of the clock
    /// it is given, not of how the threads were run.
    #[test]
    fn a_worker_stays_only_while_its_count_does_not_move() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut seen = (u64::MAX, start);
        for millis in 0..1000 {
            let still = stayed(&mut seen, millis / 2, at(millis));
            assert!(still < GIVE_WAY_AFTER, "at {millis} ms it stayed {still:?}");
        }
        let mut seen = (u64::MAX, start);
        assert_eq!(stayed(&mut seen, 7, at(0)), Duration::ZERO);
        assert_eq!(stayed(&mut seen, 7, at(10)), GIVE_WAY_AFTER);
        assert_eq!(stayed(&mut seen, 7, at(100)), ADD_AFTER);
        assert_eq!(stayed(&mut seen, 8, at(150)), Duration::ZERO);
        assert_eq!(stayed(&mut seen, 8, at(400)), Duration::from_millis(250));
    }

    /// A worker is put beside one only once that one has stayed on its task
    /// past the second interval, never beside one that has one already or
    /// was itself put beside another, and never at twice the workers given.
    #[test]
    fn a_worker_is_put_beside_only_one_held_past_the_intervals() {
        let long = ADD_AFTER;
        assert!(puts_beside(long, false, false, 1, 1));
        assert!(!puts_beside(ADD_AFTER - Duration::from_nanos(1), false, false, 1, 1));
        assert!(!puts_beside(GIVE_WAY_AFTER, false, false, 1, 1));
        assert!(!puts_beside(long, true, false, 1, 1));
        assert!(!puts_beside(long, false, true, 1, 1));
        assert!(!puts_beside(long, false, false, 2, 1));
        assert!(puts_beside(long, false, false, 7, 4));
        assert!(!puts_beside(long, false, false, 8, 4));
    }

    /// The watcher asks a task to give its worker away once that worker has
    /// stayed on one task past the first interval, and not while it asks
    /// already; another bit of the word, as that of deadlines, changes
    /// nothing of it.
    #[test]
    fn a_worker_idle_turns_the_watch_to_the_nexts() {
        assert!(!watches_nexts(0));
        assert!(watches_nexts(1));
        assert!(watches_nexts(7));
    }

    #[test]
    fn a_task_is_asked_to_give_way_only_past_the_first_interval() {
        let deadlines = 1;
        assert!(asks_to_give_way(GIVE_WAY_AFTER, 0));
        assert!(asks_to_give_way(ADD_AFTER, deadlines));
        assert!(!asks_to_give_way(GIVE_WAY_AFTER - Duration::from_nanos(1), 0));
        assert!(!asks_to_give_way(ADD_AFTER, GIVE_WAY));
        assert!(!asks_to_give_way(ADD_AFTER, GIVE_WAY | deadlines));
    }
}
