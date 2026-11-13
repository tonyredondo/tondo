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
}

/// Every worker that has called C, which the monitor watches.
pub(super) static CALLERS: Mutex<Vec<&'static InC>> = Mutex::new(Vec::new());
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
    let current = in_c_here();
    if !current.is_null() {
        return unsafe { &*current };
    }
    let new: &'static InC = Box::leak(Box::new(InC {
        depth: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        frames: Default::default(),
        returns: Default::default(),
        started: Default::default(),
        task: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        replaced: AtomicBool::new(false),
    }));
    CALLERS.lock().unwrap_or_else(|e| e.into_inner()).push(new);
    worker(|w| w.in_c.set(new));
    new
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

/// Watches the workers in C: one still in the same call after a period
/// blocks its tasks, so when tasks wait to run and no worker is idle,
/// another worker starts in its place (as Go's runtime does). Sleeps while
/// no worker is in C.
pub(super) fn monitor() {
    let mut last: HashMap<usize, usize> = HashMap::new();
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
            if state.depth.load(Ordering::SeqCst) == 0 {
                last.remove(&key);
                continue;
            }
            busy = true;
            if last.insert(key, calls) == Some(calls) {
                let mut queue = queue();
                let due = queue.timers.peek().is_some_and(|timer| timer.at <= clock_now());
                if !due && queue.runnable.is_empty() && waiting_next().is_empty() && waiting_local().is_empty() {
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
                queue.workers += 1;
                // It looks for tasks for a domain where none looks.
                let domain = (0..super::domains::count()).find(|&domain| !SEEKING[domain].load(Ordering::SeqCst));
                if let Some(domain) = domain {
                    SEEKING[domain].store(true, Ordering::SeqCst);
                    queue.starting = Some(domain);
                }
                crate::diagnostics::WORKERS_REPLACED.add(1);
                drop(queue);
                start_worker();
            }
        }
        if !busy {
            MONITOR_ASLEEP.store(true, Ordering::SeqCst);
            let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if callers.iter().all(|state| state.depth.load(Ordering::SeqCst) == 0) && !stopping() {
                MONITOR.park();
            }
            MONITOR_ASLEEP.store(false, Ordering::SeqCst);
        }
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
    crate::trace::guard_thread(false);
    // Where it is in the program, the same in another run.
    crate::replay::entered((inner as usize).wrapping_sub(tondo_rt_callback as *const () as usize));
    let call = ForeignCall { inner, frame, done: Mutex::new(false), finished: Condvar::new() };
    let task = new_task(
        Some(task_stack()),
        std::ptr::null_mut(),
        std::ptr::null(),
        ResultKind::Nothing,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
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
    if !*call.done.lock().unwrap_or_else(|e| e.into_inner()) {
        start_workers();
        FOREIGN_WAITING.fetch_add(1, Ordering::SeqCst);
        // The one worker may have ended: this thread runs what it waits for.
        if exploring().is_some() {
            queue().workers += 1;
            work_until(false, Some(&|| *call.done.lock().unwrap_or_else(|e| e.into_inner())));
            queue().workers -= 1;
        }
        let mut done = call.done.lock().unwrap_or_else(|e| e.into_inner());
        while !*done {
            done = call.finished.wait(done).unwrap_or_else(|e| e.into_inner());
        }
        drop(done);
        FOREIGN_WAITING.fetch_sub(1, Ordering::SeqCst);
    }
    crate::replay::left();
    FOREIGN_CALLS.fetch_sub(1, Ordering::SeqCst);
    // What a library wrote comes before what its caller writes next.
    if library.is_some() {
        crate::flush_stdout();
    }
}
