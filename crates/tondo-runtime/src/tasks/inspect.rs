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

/// Sends `signal` to every thread that has run Tondo code.
pub(crate) fn signal_workers(signal: i32) {
    let runners: Vec<&Runner> = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for runner in runners {
        unsafe { libc::pthread_kill(runner.thread, signal) };
    }
}

/// Asks the threads that still run Tondo code where they are, a while.
pub(super) fn ask_where(runners: &[&Runner]) -> Vec<(*mut Task, Vec<usize>)> {
    crate::recorder::install();
    let asked: Vec<&&Runner> = runners.iter().filter(|r| r.running.load(Ordering::SeqCst)).collect();
    for runner in &asked {
        runner.answered.store(0, Ordering::Relaxed);
        runner.asked.store(true, Ordering::Release);
        unsafe { libc::pthread_kill(runner.thread, libc::SIGPROF) };
    }
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
            let paused = runners.iter().find(|r| r.task.load(Ordering::SeqCst) == task);
            let in_c =
                callers.iter().find(|c| c.depth.load(Ordering::SeqCst) > 0 && c.task.load(Ordering::SeqCst) == task);
            let answered = answers.iter().find(|(t, _)| *t == task);
            let (state, stack): (&str, Vec<usize>) = if task == me {
                ("runs", here.take().unwrap_or_default())
            } else if let Some((_, stack)) = answered {
                ("runs", stack.clone())
            } else if let Some(caller) = in_c {
                let depth = (caller.depth.load(Ordering::SeqCst) - 1).min(caller.frames.len() - 1);
                let (frame, from) =
                    (caller.frames[depth].load(Ordering::Relaxed), caller.returns[depth].load(Ordering::Relaxed));
                ("in C", std::iter::once(from).chain(crate::trace::walk(frame, bounds)).collect())
            } else if let Some(runner) = paused {
                ("runs", crate::trace::walk(runner.frame.load(Ordering::SeqCst), bounds).collect())
            } else if shown.state.load(Ordering::Acquire) == PARKED {
                ("waits", parked_walk(task).collect())
            } else if ready.contains(&(task as usize)) {
                ("ready", parked_walk(task).collect())
            } else {
                ("runs", Vec::new())
            };
            let locks = shown
                .held
                .iter()
                .map(|&(_, _, at)| String::from_utf8_lossy(crate::str_bytes(at)).into_owned())
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
            }
        })
        .collect()
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

/// Blocks the running task as `block` does, on `object` of the kind
/// `kind`, which the debugger shows while it waits.
pub(crate) fn blocked_in(kind: u8, object: usize, block: impl FnOnce() -> usize) -> usize {
    let task = current();
    let since = Instant::now();
    if let Some(task) = unsafe { task.as_ref() } {
        let from = *WAITS_FROM.get_or_init(|| since);
        task.blocked_since.store(since.duration_since(from).as_millis() as u64, Ordering::Relaxed);
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

/// What `blocked_since` counts from.
pub(super) static WAITS_FROM: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// How long the task `id` has been blocked, in milliseconds.
pub(crate) fn blocked_for(id: u64) -> u64 {
    let Some(task) = (unsafe { by_id(id).and_then(|task| task.as_ref()) }) else { return 0 };
    let now = WAITS_FROM.get().map_or(0, |from| from.elapsed().as_millis() as u64);
    now.saturating_sub(task.blocked_since.load(Ordering::Relaxed))
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
        for &(held, exclusive, at) in task.held.iter().filter(|(held, ..)| *held == object) {
            let _ = held;
            out.push((task.id, exclusive, String::from_utf8_lossy(unsafe { crate::str_bytes(at) }).into_owned()));
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
