//! What a worker does: looks for a task, runs it until it waits or ends,
//! sleeps when there is none, and says when no task can run again, a deadlock;
//! and the workers it starts, one per core (docs/refactor.md, phase 2).

use super::*;

/// What it means that no task can run: nothing yet, a deadlock (`P0013`),
/// or a wait that a thread of C may end by calling back.
pub(super) enum Stalled {
    No,
    Deadlock,
    MaybeC,
}

/// How often idle workers look again while only C could wake the program.
pub(super) const RECHECK: Duration = Duration::from_millis(100);

/// Every worker waits, for a task or inside a function C called, no timer
/// is due and no host work runs. That is a deadlock unless a thread the
/// runtime did not start exists, which C could use to call back.
pub(super) fn stalled(queue: &Queue) -> Stalled {
    // A notified task about to wake on its thread is running.
    let pinned = queue.pinned.iter().filter(|&&task| unsafe { (*task).state.load(Ordering::Acquire) } == PINNED);
    if !queue.runnable.is_empty()
        || queue.timers.peek().is_some()
        || queue.idle + pinned.count() + usize::from(queue.polling) < queue.workers
    {
        return Stalled::No;
    }
    // Work on the host, which may be about to end, having made its task
    // runnable: the last to end looks again (`host_work_ended`).
    if HOST_WORK.load(Ordering::SeqCst) != 0 {
        HOST_WATCHED.store(true, Ordering::SeqCst);
        if HOST_WORK.load(Ordering::SeqCst) != 0 {
            return Stalled::No;
        }
    }
    if MAIN_DONE.load(Ordering::SeqCst)
        // A reload's barrier, which the tool that holds it lets go.
        || crate::live::barrier()
        // A signal the program waits for.
        || crate::signals::listening()
        // A program being ended: its tasks wait for the cancellation to
        // reach them, and that is no deadlock.
        || ENDING.load(Ordering::SeqCst) != usize::MAX
        || nexts().any(|next| !next.task.load(Ordering::Acquire).is_null() || !next.tasks().0.is_empty())
    {
        return Stalled::No;
    }
    // Threads of C that wait for a callback's task cannot call back, nor
    // can those there were before the program, as qemu's own.
    let known =
        RUNTIME_THREADS.load(Ordering::SeqCst) + FOREIGN_WAITING.load(Ordering::SeqCst) + BEFORE.load(Ordering::SeqCst);
    if os_threads().is_some_and(|count| count > known) { Stalled::MaybeC } else { Stalled::Deadlock }
}

/// Runs tasks; on the main thread, until the main task finishes, wherever
/// it ran, and then returns its exit status.
pub(super) fn work(main: bool) -> i32 {
    work_until(main, None)
}

/// The main thread has nothing left to do: the program ended. Exploring
/// orders this thread is the only worker: after a panic it runs the other
/// tasks' cleanups itself, and the tasks left when the program ends where
/// leaks are checked, which other workers would have run by then.
pub(super) fn program_ended(queue: &Queue) -> bool {
    let done = MAIN_DONE.load(Ordering::SeqCst);
    let ending = ENDING.load(Ordering::SeqCst) != usize::MAX || (done && crate::config::config().check_leaks.is_some());
    done && !(exploring().is_some() && ending && !queue.runnable.is_empty())
}

/// `work` in a recorded run, on a thread other than the worker's until
/// `until` says: the worker may be in C, and only the baton's runs anyway.
pub(super) fn work_until(main: bool, until: Option<&dyn Fn() -> bool>) -> i32 {
    crate::trace::guard_thread(true);
    if exploring().is_none()
        && own_next().is_none()
        && let Some(next) = NEXTS.get(NEXT_COUNT.fetch_add(1, Ordering::AcqRel))
    {
        next.domain.store(super::domains::here(), Ordering::Relaxed);
        worker(|w| w.own_next.set(Some(next)));
    }
    // A worker just woken looks once more, a moment later, for a task
    // waiting next to a worker that runs another, before it sleeps again.
    // One that starts to take another's place was woken for that.
    let starts = queue().starting.take();
    let (mut woken, mut waited) = (starts.is_some(), false);
    // The domain this worker looks for tasks for, as `SEEKING` says.
    let mut seeker: Option<usize> = starts;
    loop {
        turn_host();
        // The task made runnable by the one that ran here, but now and then
        // the queue first, so that two tasks that wake each other do not
        // keep the others waiting.
        if let Some(next) = own_next() {
            let runs = worker(|w| w.next_turns.replace(w.next_turns.get().wrapping_add(1)));
            let task = next.task.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !task.is_null() {
                if !runs.is_multiple_of(61) {
                    run(task);
                    continue;
                }
                queue().runnable.push_back(task);
            }
            // Then the tasks its tasks started.
            if let Some(task) = own_started() {
                if !runs.is_multiple_of(61) {
                    run(task);
                    continue;
                }
                queue().runnable.push_back(task);
            }
        }
        let (task, more) = {
            let mut queue = queue();
            let (mut looked, mut stealing) = (false, false);
            loop {
                if main && program_ended(&queue) {
                    return MAIN_STATUS.load(Ordering::SeqCst) as i32;
                }
                if until.is_some_and(|until| until()) {
                    return 0;
                }
                if !main && queue.surplus > 0 {
                    if let Some(domain) = seeker {
                        SEEKING[domain].store(false, Ordering::SeqCst);
                    }
                    queue.surplus -= 1;
                    queue.workers -= 1;
                    if let Some(next) = own_next() {
                        let task = next.task.swap(std::ptr::null_mut(), Ordering::AcqRel);
                        queue.runnable.extend(Some(task).filter(|task| !task.is_null()));
                        queue.runnable.extend(next.tasks().0.drain(..));
                        next.count.store(0, Ordering::Relaxed);
                    }
                    // What it leaves is another's, which may sleep trusting a
                    // worker that looks and leaves too (testing/models/Wake.tla);
                    // and without it the workers left may all wait, which one
                    // that looks sees is a deadlock, where the last to sleep
                    // counted this one as busy and slept for good.
                    wake_one(queue);
                    return 0;
                }
                // The signals of a recorded run come at the worker's turns,
                // and it does the work that blocks its runtime meanwhile.
                if crate::replay::on() {
                    drop(queue);
                    crate::signals::at_turn();
                    crate::http::deferred();
                    queue = self::queue();
                    // The program may have ended meanwhile, on a thread of
                    // C that ran its task, with nobody asleep to hear it.
                    if main && program_ended(&queue) {
                        continue;
                    }
                }
                if script().is_some() {
                    time_passes(&mut queue);
                }
                fire_timers(&mut queue);
                // Now and then a worker with tasks to run looks at the
                // network first, so that what it woke does not wait behind
                // a queue that never empties.
                queue.picks = queue.picks.wrapping_add(1);
                if queue.picks.is_multiple_of(61)
                    && !queue.runnable.is_empty()
                    && let Some((poller, polled)) = claim(&mut queue)
                {
                    drop(queue);
                    // And a recorded run's worker at the runtime it drives.
                    if let Some(runtime) = crate::http::driven() {
                        crate::http::drive(runtime, false, None);
                    }
                    queue = poll(poller, polled, Some(Instant::now()));
                }
                let next = match exploring() {
                    Some(seed) if !queue.runnable.is_empty() => {
                        let at = prioritized(seed, &queue.runnable);
                        queue.runnable.remove(at)
                    }
                    _ => queue.runnable.pop_front(),
                };
                if let Some(task) = next {
                    break (task, !queue.runnable.is_empty());
                }
                // Tasks another worker started, looked for without the
                // queue's lock, which the workers that look would otherwise
                // take in turns; then the queue once more, which may have
                // changed meanwhile.
                if !looked {
                    looked = true;
                    drop(queue);
                    // Its own first: one of them went to the queue, for
                    // fairness, and another worker may have run it.
                    if let Some(task) = own_started() {
                        break (task, false);
                    }
                    match steal() {
                        Ok(found) => break found,
                        Err(young) => stealing = young,
                    }
                    queue = self::queue();
                    continue;
                }
                // Or one next to a worker that runs another for long, one
                // there for less looked at again in a moment. Only a worker
                // just woken looks at the others' next, which they write
                // all the time.
                let (taken, young) = if woken || waited { take_next() } else { (None, false) };
                if let Some(task) = taken {
                    break (task, false);
                }
                // The last worker to stop: with nothing to do, what the
                // program wrote goes out, as it may not write again.
                if queue.idle + 1 >= queue.workers && crate::stdout_waits() {
                    drop(queue);
                    flush_stdout();
                    queue = self::queue();
                    continue;
                }
                // What the host has ready to poll comes first.
                if HOST_READY_COUNT.load(Ordering::SeqCst) > 0 {
                    drop(queue);
                    turn_host();
                    queue = self::queue();
                    continue;
                }
                // With sockets to poll, an idle worker waits for their
                // events, unless another one does, and runs what they wake
                // on its own thread.
                let polling = claim(&mut queue);
                if polling.is_none() {
                    queue.idle += 1;
                    IDLE.fetch_add(1, Ordering::SeqCst);
                }
                // The domain of the core it runs on now, which the tasks it
                // starts or wakes next are of (`steal`).
                let here = super::domains::here();
                if let Some(own) = own_next() {
                    own.domain.store(here, Ordering::Relaxed);
                }
                let mut next = queue.timers.peek().map(|t| t.at);
                // A worker that looks for tasks looks again in a moment at
                // those that wait for less, and goes on looking.
                // One that looks keeps looking while tasks wait that are
                // not yet its to take, as those of another domain wait
                // longer for it than the moment it looks again in.
                waited = (young && (woken || seeker.is_some())) || (stealing && seeker.is_some());
                if !waited {
                    // Before it sleeps, one that looked for tasks stops, and
                    // then, counted idle, looks at the workers' queues once
                    // more: tasks started meanwhile, or too young to take,
                    // whose worker may not come back to them soon, as one
                    // that waits in a call of C. Unless another worker looks
                    // for them, this one does again in a moment.
                    if let Some(domain) = seeker.take() {
                        SEEKING[domain].store(false, Ordering::SeqCst);
                    }
                    if waiting_somewhere() && !SEEKING[here].swap(true, Ordering::SeqCst) {
                        (seeker, waited) = (Some(here), true);
                    }
                }
                // It sleeps in the domain it looks for tasks for, or in the
                // one of the core it runs on, where a task made there wakes it.
                let sleeps_in = seeker.unwrap_or(here);
                if polling.is_none() {
                    queue.idle_in[sleeps_in] += 1;
                }
                if waited {
                    let soon = Instant::now() + NEXT_WAIT;
                    next = Some(next.map_or(soon, |at| at.min(soon)));
                }
                match stalled(&queue) {
                    Stalled::Deadlock => {
                        drop(queue);
                        deadlock();
                    }
                    // Look again later: the threads of C may end.
                    Stalled::MaybeC => next = Some(Instant::now() + RECHECK),
                    Stalled::No => {}
                }
                if let Some((poller, polled)) = polling {
                    drop(queue);
                    crate::replay::waits();
                    queue = match crate::http::driven() {
                        // A recorded run's worker waits in the runtime it
                        // drives, whose sources have the poller, unless the
                        // poller has events already, which the runtime
                        // would not hear of again.
                        Some(runtime) => {
                            let mut ready = libc::pollfd { fd: poller.fd(), events: libc::POLLIN, revents: 0 };
                            if unsafe { libc::poll(&mut ready, 1, 0) } == 0 {
                                crate::http::drive(runtime, true, next);
                            }
                            poll(poller, polled, Some(Instant::now()))
                        }
                        None => poll(poller, polled, next),
                    };
                    if crate::replay::on() {
                        drop(queue);
                        crate::replay::waited();
                        queue = self::queue();
                    }
                    looked = false;
                    continue;
                }
                let delay = next.map(|at| at.saturating_duration_since(Instant::now()));
                queue = if crate::replay::replaying() {
                    // A replay waits for nothing: what woke the recorded
                    // run comes next in its recording.
                    drop(queue);
                    crate::replay::waiting();
                    self::queue()
                } else {
                    crate::replay::waits();
                    // The wait's own reading of the clock is not the
                    // program's.
                    let outside = crate::replay::Outside::enter();
                    let woken = match (delay, until) {
                        (Some(delay), _) => {
                            READY[sleeps_in].wait_timeout(queue, delay).unwrap_or_else(|e| e.into_inner()).0
                        }
                        (None, None) => READY[sleeps_in].wait(queue).unwrap_or_else(|e| e.into_inner()),
                        // What it waits for may come without a wake, and
                        // looking again is no turn of the worker's.
                        (None, Some(until)) => loop {
                            let (woken, waited) =
                                READY[sleeps_in].wait_timeout(queue, RECHECK).unwrap_or_else(|e| e.into_inner());
                            if !waited.timed_out() || until() {
                                break woken;
                            }
                            queue = woken;
                        },
                    };
                    drop(outside);
                    match crate::replay::on() {
                        true => {
                            drop(woken);
                            crate::replay::waited();
                            self::queue()
                        }
                        false => woken,
                    }
                };
                queue.idle -= 1;
                queue.idle_in[sleeps_in] -= 1;
                IDLE.fetch_sub(1, Ordering::SeqCst);
                // Woken for a task: the first to look after a wake looks for
                // what it was for, and one that waited a moment for a task
                // looks again as `waited` says. A wake that found nothing
                // must not make the next one count for nothing.
                woken = queue.waking[sleeps_in];
                if std::mem::take(&mut queue.waking[sleeps_in]) {
                    seeker = Some(sleeps_in);
                }
                looked = false;
            }
        };
        // Found: the next worker looks, if more wait anywhere.
        let mut more = more;
        if let Some(domain) = seeker.take() {
            SEEKING[domain].store(false, Ordering::SeqCst);
            more = more || waiting_somewhere() || !queue().runnable.is_empty();
        }
        if more {
            wake_idle();
        }
        (woken, waited) = (false, false);
        run(task);
    }
}

/// Runs a task on this thread until it waits, yields or ends; a task that
/// ends is freed.
pub(super) fn run(task: *mut Task) {
    worker(|w| w.current.set(task));
    let traced = tools::running(task);
    unsafe {
        if (*task).stack.is_none() {
            let stack = task_stack();
            (*task).sp = initial_stack(stack.top());
            (*task).stack = Some(stack);
        }
    }
    enter_tondo();
    unsafe { switch(home(), (*task).sp) };
    leave_tondo();
    worker(|w| w.current.set(std::ptr::null_mut()));
    tools::stopped(task, traced);
    unsafe {
        let in_domain = (*task).in_domain;
        if (*task).finished {
            let (was_main, join) = ((*task).main, (*task).join);
            for asked in [&(*task).cancelled, &(*task).again] {
                if asked.load(Ordering::SeqCst) {
                    attend(false);
                }
            }
            if let Some(stack) = (*task).stack.take() {
                recycle(stack);
            }
            Wait::left(&mut *task);
            left_held(&mut *task);
            unregister(task);
            (*task).mark = 0;
            // A spawned task lives in its join's block.
            if join.is_null() {
                drop(Box::from_raw(task));
            } else {
                std::ptr::drop_in_place(task);
                let_go(join);
            }
            crate::clock::stopped(in_domain);
            if was_main {
                MAIN_DONE.store(true, Ordering::SeqCst);
                // Take the queue's lock so the main thread cannot miss
                // the notification between its check and its wait.
                drop(queue());
                READY.iter().for_each(Condvar::notify_all);
                if let Some(poller) = crate::poll::poller() {
                    poller.wake();
                }
            }
        } else if (*task).state.compare_exchange(PARKING, PARKED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            crate::clock::stopped(in_domain);
        } else {
            // Notified while switching out, taken with a write as `park`
            // takes it.
            (*task).state.swap(RUNNING, Ordering::AcqRel);
            schedule(task);
        }
    }
}

/// A future of the host that the workers poll at their turns, where the
/// code that woke it holds none of its locks (docs/speed.md, H2): a
/// connection of the HTTP server.
pub(crate) trait Turned: Send + Sync {
    fn turn(self: Arc<Self>);
}

static HOST_READY: Mutex<Vec<Arc<dyn Turned>>> = Mutex::new(Vec::new());
static HOST_READY_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Polls `future` at a worker's turn: on a worker, its next, once the task
/// it runs waits or ends, or after the events of the network it woke for,
/// with no other worker woken; elsewhere, one woken for it. A task that
/// keeps its worker without waiting keeps what it woke waiting too.
pub(crate) fn poll_soon(future: Arc<dyn Turned>) {
    HOST_READY.lock().unwrap_or_else(|e| e.into_inner()).push(future);
    HOST_READY_COUNT.fetch_add(1, Ordering::SeqCst);
    if own_next().is_none() {
        wake_one(queue());
    }
}

/// Polls what the host has ready.
pub(super) fn turn_host() {
    if HOST_READY_COUNT.load(Ordering::SeqCst) == 0 {
        return;
    }
    // Swapped with the worker's own list, which keeps its room for the next
    // wake: taken whole, the list left the next wake to allocate another.
    let turns = worker(|w| {
        if w.turns.get().is_null() {
            w.turns.set(Box::into_raw(Box::default()));
        }
        w.turns.get()
    });
    let mut ready = std::mem::take(unsafe { &mut *turns });
    std::mem::swap(&mut ready, &mut *HOST_READY.lock().unwrap_or_else(|e| e.into_inner()));
    HOST_READY_COUNT.fetch_sub(ready.len(), Ordering::SeqCst);
    for future in ready.drain(..) {
        future.turn();
    }
    unsafe { *turns = ready };
}

pub(super) fn deadlock() -> ! {
    // Several idle workers may see it at once; one reports it.
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if REPORTED.swap(true, Ordering::SeqCst) {
        loop {
            std::thread::park();
        }
    }
    flush_stdout();
    let headline = "panic[P0013]: deadlock: every task waits and none of them can wake the others";
    // A debugger shows where every task waits first.
    crate::debugger::deadlocked(headline);
    let mut report = String::from(headline);
    // The tasks that wait for each other, when locks or joins tie them.
    let all: Vec<u64> = tasks().iter().map(|&t| unsafe { (*(t as *mut Task)).id }).collect();
    if let Some(cycle) = crate::debugger::cycle(&all) {
        report.push_str(&format!("\n{cycle}"));
    }
    // Where each task waits, the same place once, with the main task first.
    let mut waits: Vec<(String, usize)> = Vec::new();
    let mut all = tasks().in_start_order();
    all.sort_by_key(|&t| unsafe { !(*t).main });
    for task in all {
        let text = unsafe { waiting_at(task) };
        match waits.iter_mut().find(|(t, _)| *t == text) {
            Some((_, count)) => *count += 1,
            None => waits.push((text, unsafe { !(*task).main } as usize)),
        }
    }
    for (text, count) in waits {
        let title = match count {
            0 => "the main task waits at".to_string(),
            1 => "a task waits at".to_string(),
            _ => format!("{count} tasks wait at"),
        };
        report.push_str(&text.replacen("TITLE", &title, 1));
    }
    // What the others wait for may be what one of these would have given.
    for (task, spawned, result) in unread_failures() {
        report.push_str(&format!("\ntask {task} ended with an error nobody read: {result}"));
        crate::trace::write_stack(&mut report, "it was spawned at", || spawned.iter().copied(), None);
    }
    let _ = writeln!(std::io::stderr(), "{report}");
    crate::diagnostics::crash_dump(
        "deadlock",
        "every task waits and none of them can wake the others",
        Vec::new(),
        true,
    );
    std::process::exit(crate::PANIC_EXIT_CODE)
}

/// The trace of a task that waits, under a `TITLE` to fill in: from the
/// frame pointer it saved when it switched out, or from here for the
/// running one.
pub(super) unsafe fn waiting_at(task: *mut Task) -> String {
    unsafe {
        let walk = if task == current() {
            let Some(stack) = (*task).stack.as_ref() else { return String::new() };
            let bounds = (stack.base as usize + GUARD, stack.top() as usize);
            crate::trace::walk(crate::trace::frame_pointer(), bounds)
        } else if (*task).state.load(Ordering::Acquire) == PARKED {
            parked_walk(task)
        } else {
            return "\nTITLE: a call to C".into();
        };
        crate::trace::stack(walk, (*task).origin, "TITLE")
    }
}

/// Starts the other workers, one per core, the first time a task spawns or
/// a call from C waits.
pub(super) fn start_workers() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if exploring().is_some() {
        return;
    }
    let count = std::thread::available_parallelism().map_or(1, |n| n.get());
    queue().workers = count;
    for _ in 1..count {
        start_worker();
    }
}

pub(super) fn start_worker() {
    spawn_thread(Some(256 * 1024), || {
        work(false);
    });
}

// ---------------------------------------------------------------------------
