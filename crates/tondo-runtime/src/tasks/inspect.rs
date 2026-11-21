//! What the tools ask of the tasks: where each stands, for a dump, a trace and
//! the debugger, its stack and the registers a switch left, and what it waits in
//! and who holds what it waits for (docs/refactor.md, phase 2).

use super::*;

/// A task as a dump shows it.
pub(crate) struct Shot {
    pub id: u64,
    pub main: bool,
    /// `waits`, `ready`, `runs` or `in C`.
    pub state: &'static str,
    /// Where it is, innermost first: empty for one that runs and did not
    /// stop.
    pub stack: Vec<usize>,
    /// Where it was spawned.
    pub spawned: Vec<usize>,
    /// Where it took each `std.sync` lock it holds.
    pub locks: Vec<String>,
    pub cancelled: bool,
    /// The request it works for.
    pub request: Option<String>,
    /// What it waits for, in words; the tasks that can end that wait; the
    /// object, which tasks that wait for the same one share; and for how
    /// many milliseconds.
    pub wait: Option<(String, Vec<u64>, usize, u64)>,
    /// For a task parked in a wait, whether an operation claimed it to wake
    /// the task: one that waits so has lost its wake.
    pub woken: Option<bool>,
    /// Its last waits and wakes, under `TONDO_WAKES=1` (`wakes.rs`).
    pub wakes: Vec<(u64, super::wakes::Did)>,
}

/// In the handler of `SIGPROF`: answers a dump that asked this thread where
/// it is, from the instruction and frame the signal interrupted.
pub(crate) fn answer_where(pc: usize, fp: usize) {
    let Some(runner) = worker(|w| w.runner.get()) else { return };
    if !runner.asked.load(Ordering::Acquire) {
        return;
    }
    runner.task.store(current(), Ordering::Relaxed);
    let bounds = stack_bounds().unwrap_or((0, 0));
    let mut count = 0;
    // The place it runs at is kept one past it, as a return address is.
    for (slot, address) in runner.answer.iter().zip(std::iter::once(pc + 1).chain(crate::trace::walk(fp, bounds))) {
        slot.store(address, Ordering::Relaxed);
        count += 1;
    }
    runner.answered.store(count, Ordering::Relaxed);
    runner.asked.store(false, Ordering::Release);
}

/// Whether a runner is a thread of C's (`Task::native`), which macOS does
/// not signal: one may get the signal just after its call, as it ends, when
/// macOS freed its thread-locals already, and the handler that reads them
/// then allocates inside the signal, as an Intel Mac of CI showed. Its calls
/// are not sampled there, nor asked where they are by a dump.
fn of_c(runner: &Runner) -> bool {
    runner.native.load(Ordering::SeqCst)
}

/// Sends `signal` to every thread that has run Tondo code, but the threads
/// of C (`of_c`).
pub(crate) fn signal_workers(signal: i32) {
    // Under the lock, which a thread that ends takes to leave the list.
    let runners = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner());
    for runner in runners.iter().filter(|runner| !of_c(runner)) {
        unsafe { libc::pthread_kill(runner.thread.load(Ordering::SeqCst) as libc::pthread_t, signal) };
    }
}

/// Asks the threads that still run Tondo code where they are, a while.
pub(super) fn ask_where(runners: &[&Runner]) -> Vec<(*mut Task, Vec<usize>)> {
    crate::recorder::install();
    // Only threads still in the list, under its lock, which a thread that
    // ends takes to leave it.
    let present = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner());
    let asked: Vec<&&Runner> = runners
        .iter()
        .filter(|&&r| {
            r.running.load(Ordering::SeqCst)
                && present.iter().any(|&p| std::ptr::eq(p, r))
                && !(cfg!(target_os = "macos") && of_c(r))
        })
        .collect();
    for runner in &asked {
        runner.answered.store(0, Ordering::Relaxed);
        runner.asked.store(true, Ordering::Release);
        unsafe { libc::pthread_kill(runner.thread.load(Ordering::SeqCst) as libc::pthread_t, libc::SIGPROF) };
    }
    drop(present);
    let deadline = Instant::now() + Duration::from_millis(100);
    while asked.iter().any(|r| r.asked.load(Ordering::Acquire)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut out = Vec::new();
    for runner in asked {
        if runner.asked.swap(false, Ordering::AcqRel) {
            continue;
        }
        let count = runner.answered.load(Ordering::Relaxed);
        let stack = runner.answer[..count].iter().map(|a| a.load(Ordering::Relaxed)).collect();
        out.push((runner.task.swap(std::ptr::null_mut(), Ordering::Relaxed), stack));
    }
    out
}

/// Every task and where it is, for a dump: best while the world stands
/// still, when the running ones are stopped at a known frame; one that
/// runs on is asked with a signal. The running task of this thread, if
/// any, is where `here` says.
pub(crate) fn snapshot(here: Option<Vec<usize>>) -> Vec<Shot> {
    // A task a worker took off the queue and has not yet run, or one that
    // is parking, is where no runner, queue or wait says: it would show no
    // stack, and a profile of the tasks would leave it out. A worker is out
    // of that in microseconds, or stops where it is when the world stands
    // still, unless a busy machine leaves it unrun a while; so the snapshot
    // is taken again until no other task is so, a thousand times at most,
    // about as long as a stop of the world waits for its workers. Counted in
    // tries, not by the clock: a process the system stops meanwhile loses no
    // try, where it would lose the time.
    let me = unsafe { current().as_ref() }.map(|task| task.id);
    let mut tries = 0;
    loop {
        let shots = snapshot_once(here.clone());
        let between = shots.iter().any(|shot| shot.state == "runs" && shot.stack.is_empty() && Some(shot.id) != me);
        tries += 1;
        if !between || tries >= 1000 {
            return shots;
        }
        std::thread::sleep(std::time::Duration::from_micros(100));
    }
}

/// Every task and where it is, once.
fn snapshot_once(here: Option<Vec<usize>>) -> Vec<Shot> {
    let runners: Vec<&Runner> = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let me_runner = worker(|w| w.runner.get());
    let others: Vec<&Runner> = runners
        .iter()
        .copied()
        .filter(|r| Some(*r).map(|r| r as *const Runner) != me_runner.map(|r| r as *const Runner))
        .collect();
    let answers = ask_where(&others);
    let callers: Vec<&InC> = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let ready: std::collections::HashSet<usize> =
        queue().runnable.iter().copied().chain(waiting_next()).chain(waiting_local()).map(|t| t as usize).collect();
    let mut all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    all.sort_by_key(|&t| unsafe { (*t).id });
    let me = current();
    let mut here = here;
    all.into_iter()
        .map(|task| unsafe {
            let shown = &*task;
            let bounds = shown.stack.as_ref().map_or((0, 0), |s| (s.base as usize + GUARD, s.top() as usize));
            // A worker the stop holds, which knows the frame it stopped at:
            // what it answers a signal with is where it waits for the stop's
            // end, in the system's code.
            let paused =
                runners.iter().find(|r| r.task.load(Ordering::SeqCst) == task && !r.running.load(Ordering::SeqCst));
            let in_c =
                callers.iter().find(|c| c.depth.load(Ordering::SeqCst) > 0 && c.task.load(Ordering::SeqCst) == task);
            let answered = answers.iter().find(|(t, _)| *t == task);
            let (state, stack): (&str, Vec<usize>) = if task == me {
                ("runs", here.take().unwrap_or_default())
            } else if let Some(runner) = paused {
                // A worker that stopped as it was about to run the task, still
                // on its own stack: the task is where it switched out.
                let stack: Vec<usize> = crate::trace::walk(runner.frame.load(Ordering::SeqCst), bounds).collect();
                ("runs", if stack.is_empty() { parked_walk(task).collect() } else { stack })
            } else if let Some((_, stack)) = answered {
                ("runs", stack.clone())
            } else if let Some(caller) = in_c {
                let depth = (caller.depth.load(Ordering::SeqCst) - 1).min(caller.frames.len() - 1);
                let (frame, from) =
                    (caller.frames[depth].load(Ordering::Relaxed), caller.returns[depth].load(Ordering::Relaxed));
                ("in C", std::iter::once(from).chain(crate::trace::walk(frame, bounds)).collect())
            } else if shown.state.load(Ordering::Acquire) == PARKED {
                ("waits", parked_walk(task).collect())
            } else if ready.contains(&(task as usize)) {
                ("ready", parked_walk(task).collect())
            } else {
                ("runs", Vec::new())
            };
            let locks = crate::sync::locks_of(shown)
                .iter()
                .map(|&(_, _, at)| String::from_utf8_lossy(crate::str_bytes(&at)).into_owned())
                .collect();
            Shot {
                id: shown.id,
                main: shown.main,
                state,
                stack,
                spawned: shown.origin.map(|o| o.frames().to_vec()).unwrap_or_default(),
                locks,
                cancelled: shown.cancelled.load(Ordering::Relaxed),
                request: crate::diagnostics::flight_text(shown.flight),
                wait: crate::debugger::waits_for(shown.id)
                    .map(|(text, on)| (text, on, shown.blocked.load(Ordering::Relaxed), blocked_for(shown.id))),
                woken: woken(shown),
                wakes: if tools::listens(tools::WAKES) { super::wakes::of(shown.id) } else { Vec::new() },
            }
        })
        .collect()
}

/// Whether an operation claimed the wait `task` is parked in, when it is.
fn woken(task: &Task) -> Option<bool> {
    // A dump of a crash may come while this thread holds the lock: it then
    // says nothing of the wait, rather than wait for itself.
    let mut tries = 0;
    while task.waiting_lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        tries += 1;
        if tries == 10_000 {
            return None;
        }
        std::hint::spin_loop();
    }
    // The task holds its wait until it clears this, under the same lock.
    let woken = unsafe { task.waiting.as_ref() }.map(|wait| wait.fired.load(Ordering::Acquire) != NONE);
    unlock(&task.waiting_lock);
    woken
}

/// Each task's number and the part of its stack in use, for a dump that
/// looks for the objects it points to: from where a task that switched
/// out saved its registers, or all of it for one that runs.
pub(crate) fn stacks() -> Vec<(u64, usize, usize)> {
    let all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    let mut out = Vec::new();
    for task in all {
        unsafe {
            let Some(stack) = (*task).stack.as_ref() else { continue };
            let (low, high) = (stack.base as usize + GUARD, stack.top() as usize);
            let parked = (*task).state.load(Ordering::Acquire) == PARKED;
            let low = if parked { ((*task).sp as usize).clamp(low, high) } else { low };
            out.push(((*task).id, low, high));
        }
    }
    out
}

/// The stack of a task that switched out, from the frame pointer it saved.
pub(super) unsafe fn parked_walk(task: *mut Task) -> crate::trace::Walk {
    match unsafe { switched_out(task) } {
        Some((fp, bounds)) => crate::trace::walk(fp, bounds),
        None => crate::trace::walk(0, (0, 0)),
    }
}

/// The frame pointer a task that switched out saved, and its stack's
/// bounds.
pub(super) unsafe fn switched_out(task: *mut Task) -> Option<(usize, (usize, usize))> {
    unsafe {
        let stack = (*task).stack.as_ref()?;
        let bounds = (stack.base as usize + GUARD, stack.top() as usize);
        let sp = (*task).sp as *const usize;
        #[cfg(target_arch = "x86_64")]
        let fp = *sp.add(5);
        #[cfg(target_arch = "aarch64")]
        let fp = *sp.add(10);
        Some((fp, bounds))
    }
}

/// The callee-saved registers `switch` left on the stack of a task, and
/// where it will go on.
pub(super) unsafe fn switched_registers(task: *mut Task) -> crate::unwinding::Switched {
    let mut registers = [None; 32];
    unsafe {
        let sp = (*task).sp as *const usize;
        #[cfg(target_arch = "x86_64")]
        {
            for (slot, register) in [15, 14, 13, 12, 3].into_iter().enumerate() {
                registers[register] = Some(*sp.add(slot));
            }
            (registers, *sp.add(6))
        }
        #[cfg(target_arch = "aarch64")]
        {
            for slot in 0..10 {
                registers[19 + slot] = Some(*sp.add(slot));
            }
            (registers, *sp.add(11))
        }
    }
}

/// A task's id, its frame pointer, its stack, and what `switch` left.
pub(crate) type Standing = (u64, usize, (usize, usize), Option<crate::unwinding::Switched>);

/// The tasks that are not running on this thread, each where it stands:
/// its id, the frame pointer of the place it stopped in, its stack, and
/// for one switched out, its registers and where it goes on.
pub(crate) fn standing() -> Vec<Standing> {
    let runners: Vec<&Runner> = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let callers: Vec<&InC> = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    let me = current();
    let mut out = Vec::new();
    for task in all.into_iter().filter(|&task| task != me) {
        let shown = unsafe { &*task };
        let Some(stack) = shown.stack.as_ref() else { continue };
        let bounds = (stack.base as usize + GUARD, stack.top() as usize);
        let paused = runners.iter().find(|r| r.task.load(Ordering::SeqCst) == task);
        let in_c = callers.iter().find(|c| c.depth.load(Ordering::SeqCst) > 0 && c.task.load(Ordering::SeqCst) == task);
        let (frame, saved) = if let Some(caller) = in_c {
            let depth = (caller.depth.load(Ordering::SeqCst) - 1).min(caller.frames.len() - 1);
            (caller.frames[depth].load(Ordering::Relaxed), None)
        } else if let Some(runner) = paused {
            (runner.frame.load(Ordering::SeqCst), None)
        } else {
            match unsafe { switched_out(task) } {
                Some((frame, _)) => (frame, Some(unsafe { switched_registers(task) })),
                None => continue,
            }
        };
        out.push((shown.id, frame, bounds, saved));
    }
    out.sort_by_key(|&(id, ..)| id);
    out
}

/// Where the task `id` was spawned, the return addresses of each spawn,
/// innermost first, then where its spawner was, and on.
pub(crate) fn spawned_at(id: u64) -> Vec<usize> {
    let Some(task) = by_id(id) else { return Vec::new() };
    let mut out = Vec::new();
    let mut origin = unsafe { (*task).origin };
    while let Some(Origin::Spawned { parent, .. }) = origin {
        out.extend_from_slice(origin.map_or(&[][..], |o| o.frames()));
        origin = *parent;
    }
    out
}

/// What a task is blocked on (`Task::blocked_kind`).
pub(crate) const LOCK: u8 = 1;
pub(crate) const RECEIVING: u8 = 2;
pub(crate) const SENDING: u8 = 3;
pub(crate) const JOIN: u8 = 4;
pub(crate) const PLACE: u8 = 5;

/// Blocks the running task as `block` does, on `object` of the kind
/// `kind`, which the debugger shows while it waits.
pub(crate) fn blocked_in(kind: u8, object: usize, block: impl FnOnce() -> usize) -> usize {
    let task = current();
    let since = tools::waiting();
    if let Some(task) = unsafe { task.as_ref() } {
        task.blocked_since.store(millis(), Ordering::Relaxed);
        task.blocked.store(object, Ordering::Relaxed);
        task.blocked_kind.store(kind, Ordering::Release);
    }
    let fired = block();
    if let Some(task) = unsafe { task.as_ref() } {
        tools::waited(task.id, since);
        task.blocked_kind.store(0, Ordering::Release);
    }
    fired
}

/// The milliseconds from which a dump's events keep a wait.
pub(super) const LONG_WAIT: u128 = 100;

/// Milliseconds of a clock the system advances a tick at a time, which
/// costs less to read than the exact one: enough to say how long a task has
/// been blocked, read at every wait.
pub(super) fn millis() -> u64 {
    #[cfg(target_os = "macos")]
    const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW_APPROX;
    #[cfg(not(target_os = "macos"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC_COARSE;
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(CLOCK, &mut time) };
    time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000
}

/// How long the task `id` has been blocked, in milliseconds.
pub(crate) fn blocked_for(id: u64) -> u64 {
    let Some(task) = (unsafe { by_id(id).and_then(|task| task.as_ref()) }) else { return 0 };
    millis().saturating_sub(task.blocked_since.load(Ordering::Relaxed))
}

/// What the task `id` is blocked on: its kind and the object.
pub(crate) fn blocked_of(id: u64) -> Option<(u8, usize)> {
    let task = unsafe { by_id(id)?.as_ref()? };
    let kind = task.blocked_kind.load(Ordering::Acquire);
    (kind != 0).then(|| (kind, task.blocked.load(Ordering::Relaxed)))
}

/// The tasks that hold the lock `object`, whether exclusively, and where
/// they took it.
pub(crate) fn holders(object: usize) -> Vec<(u64, bool, String)> {
    let all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    let mut out = Vec::new();
    for task in all {
        let task = unsafe { &*task };
        for (_, exclusive, at) in crate::sync::locks_of(task).into_iter().filter(|&(held, ..)| held == object) {
            out.push((task.id, exclusive, String::from_utf8_lossy(unsafe { crate::str_bytes(&at) }).into_owned()));
        }
    }
    out
}

/// The task whose end the join `object` waits for.
pub(crate) fn joined(object: usize) -> Option<u64> {
    let all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    all.into_iter().map(|t| unsafe { &*t }).find(|t| t.join as usize == object).map(|t| t.id)
}

/// How many values the channel `object` holds, and how many it takes,
/// `None` when unbounded.
pub(crate) fn channel_fill(object: usize) -> (usize, Option<usize>) {
    let channel = unsafe { &*(object as *const ChannelObject) };
    (channel.count, (channel.capacity != usize::MAX).then_some(channel.capacity))
}

/// The tasks inside a call of C, which run no line of Tondo until it
/// returns: each one's number, the frame of the code that called C, where
/// in it it called, and its stack's bounds, for the debugger to show where
/// each is.
pub(crate) fn calling_c() -> Vec<(u64, usize, usize, (usize, usize))> {
    let callers: Vec<&InC> = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let all = tasks();
    let mut out = Vec::new();
    for caller in callers {
        let depth = caller.depth.load(Ordering::SeqCst);
        let task = caller.task.load(Ordering::SeqCst);
        if depth == 0 || task.is_null() || !all.iter().any(|&t| t as *mut Task == task) {
            continue;
        }
        // While the set is locked no task in it is freed.
        let shown = unsafe { &*task };
        let Some(stack) = shown.stack.as_ref() else { continue };
        let deepest = (depth - 1).min(caller.frames.len() - 1);
        let (frame, called) =
            (caller.frames[deepest].load(Ordering::Relaxed), caller.returns[deepest].load(Ordering::Relaxed));
        out.push((shown.id, frame, called, (stack.base as usize + GUARD, stack.top() as usize)));
    }
    out.sort_by_key(|(id, ..)| *id);
    out
}

/// The tasks switched out that wait, or are ready to run again: each
/// one's number, which of the two, the frame pointer it saved and its
/// stack's bounds, for the debugger to show where each is.
pub(crate) fn waiting() -> Vec<(u64, &'static str, usize, (usize, usize))> {
    let ready: std::collections::HashSet<usize> =
        queue().runnable.iter().copied().chain(waiting_next()).chain(waiting_local()).map(|t| t as usize).collect();
    let all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    let mut out = Vec::new();
    for task in all {
        let state = match unsafe { (*task).state.load(Ordering::Acquire) } {
            PARKED => "waits",
            _ if ready.contains(&(task as usize)) => "ready",
            _ => continue,
        };
        if let Some((fp, bounds)) = unsafe { switched_out(task) } {
            out.push((unsafe { (*task).id }, state, fp, bounds));
        }
    }
    out.sort_by_key(|(id, ..)| *id);
    out
}
