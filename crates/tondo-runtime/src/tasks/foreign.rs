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
        queue().workers += 1;
        start_worker();
    }
}

/// The task goes on: one worker retires.
pub(crate) fn step_back() {
    if exploring().is_none() {
        let mut queue = queue();
        queue.surplus += 1;
        wake_one(queue);
    }
}

/// Looks of the monitor a worker may stay on one task while tasks wait for
/// one and none is idle before the first task to ask gives its worker away
/// (language 11.15).
const GIVE_WAY_LOOKS: u32 = 10;
/// And before another worker starts beside it (language 16.14): a tenth of
/// a second, as a worker started and retired costs the processor some 100 µs
/// and buys a saturated machine nothing, so tasks that compute for less
/// never get one, while the task that waits is still served soon enough for
/// a person or a health check (docs/decisions.md).
const ADD_LOOKS: u32 = 100;

/// Watches the workers. One still in the same call of C after a period
/// blocks its tasks, so when tasks wait to run and no worker is idle,
/// another worker starts in its place (as Go's runtime does). One that stays
/// on the same task while tasks wait and none is idle has the first task to
/// ask give its worker away, and then another worker beside it
/// (testing/models/tests/watcher.to). Sleeps while no worker is in C and
/// none of that can happen.
pub(super) fn monitor() {
    let mut last: HashMap<usize, usize> = HashMap::new();
    let mut held: Vec<(u64, u32)> = Vec::new();
    loop {
        std::thread::sleep(MONITOR_PERIOD);
        if stopping() {
            return;
        }
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
                add_worker(queue);
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

/// Another worker, in the place of one in C or beside one a task holds.
fn add_worker(mut queue: MutexGuard<'static, Queue>) {
    queue.workers += 1;
    // It looks for tasks for a domain where none looks.
    let domain = (0..super::domains::count()).find(|&domain| !SEEKING[domain].load(Ordering::SeqCst));
    if let Some(domain) = domain {
        SEEKING[domain].store(true, Ordering::SeqCst);
        queue.starting = Some(domain);
    }
    drop(queue);
    start_worker();
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
/// wait, one whose count of started tasks has not moved for `GIVE_WAY_LOOKS`
/// looks has the first task to ask give its worker away, and one still on
/// it `ADD_LOOKS` looks on gets another worker beside it, once.
fn watch_held(held: &mut Vec<(u64, u32)>) -> Watched {
    if exploring().is_some() {
        held.clear();
        return Watched::Idle;
    }
    let shared = queue();
    if shared.idle > 0 {
        held.clear();
        return Watched::Idle;
    }
    if !tasks_wait(&shared) {
        held.clear();
        return Watched::Until(shared.timers.peek().map(|timer| timer.at));
    }
    drop(shared);
    let count = NEXT_COUNT.load(Ordering::Acquire).min(NEXTS.len());
    held.resize(count, (u64::MAX, 0));
    for (next, (seen, looks)) in NEXTS[..count].iter().zip(held.iter_mut()) {
        // A worker in a call of C is the watch above's.
        let in_c = next.in_c.load(Ordering::Acquire);
        if !in_c.is_null() && unsafe { (*in_c).depth.load(Ordering::SeqCst) } > 0 {
            (*seen, *looks) = (u64::MAX, 0);
            continue;
        }
        let turns = next.turns.load(Ordering::Relaxed);
        if *seen != turns {
            (*seen, *looks) = (turns, 0);
            continue;
        }
        *looks = looks.saturating_add(1);
        if *looks >= GIVE_WAY_LOOKS && CHECKPOINT_ASKS.load(Ordering::Relaxed) & GIVE_WAY == 0 {
            CHECKPOINT_ASKS.fetch_or(GIVE_WAY, Ordering::SeqCst);
        }
        if *looks == ADD_LOOKS && next.held.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            crate::diagnostics::WORKERS_ADDED.add(1);
            add_worker(queue());
        }
    }
    Watched::Looking
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
