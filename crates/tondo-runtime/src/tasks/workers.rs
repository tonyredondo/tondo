//! The workers' queues: the shared one, each worker's next task and its own
//! tasks, which others take after a moment, and waking a worker for them, or
//! the one that waits for the network's events (docs/refactor.md, phase 2).

use super::*;

/// What a worker runs next: a task the task it runs made runnable, as Go's
/// `runnext`, so that two tasks that hand each other values run one after
/// the other on one thread, and when it was put there. On lines of its own,
/// which its worker writes all the time.
#[repr(align(128))]
pub(super) struct Next {
    pub(super) task: AtomicPtr<Task>,
    pub(super) since: AtomicU64,
    /// And the tasks the tasks it runs started, which idle workers take
    /// half of at a time.
    pub(super) tasks: Mutex<Local>,
    /// How many there are, which a worker that would take some reads
    /// without the lock.
    pub(super) count: AtomicUsize,
    /// Since when they wait: since the first was started, or since others
    /// last took some.
    pub(super) waiting: AtomicU64,
    /// The domain of cores its worker last looked for tasks in (`domains`).
    pub(super) domain: AtomicUsize,
}

pub(super) struct Local(pub(super) VecDeque<*mut Task>);

unsafe impl Send for Local {}

impl Next {
    const fn new() -> Next {
        Next {
            task: AtomicPtr::new(std::ptr::null_mut()),
            since: AtomicU64::new(0),
            tasks: Mutex::new(Local(VecDeque::new())),
            count: AtomicUsize::new(0),
            waiting: AtomicU64::new(0),
            domain: AtomicUsize::new(0),
        }
    }

    pub(super) fn tasks(&self) -> MutexGuard<'_, Local> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Half of the tasks another worker started and has not run, the first to
/// run here and the rest to this worker's own, and whether more wait; or
/// whether some wait for less than they must, to look at again in a
/// moment. Tasks wait `NEXT_WAIT` before another worker of their domain
/// takes them, as a task next does: those that end sooner end on the worker
/// that started them, in the caches where they were made, and the others
/// are shared out as soon; and `CROSS_WAIT` before one of another domain
/// does, as crossing to it costs their caches.
pub(super) fn steal() -> Result<(*mut Task, bool), bool> {
    let Some(own) = own_next() else { return Err(false) };
    // The clock and the domain are read once a worker has tasks to take,
    // which is seldom when this one looks for them.
    let mut clock = None;
    let mut young = false;
    for other in nexts().filter(|other| !std::ptr::eq(*other, own)) {
        if other.count.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let (now, here) = *clock.get_or_insert_with(|| {
            let here = super::domains::here();
            own.domain.store(here, Ordering::Relaxed);
            (nanos(), here)
        });
        let since = other.waiting.load(Ordering::Relaxed);
        if now.saturating_sub(since) < wait_from(here, other) {
            young = true;
            continue;
        }
        let Ok(mut theirs) = other.tasks.try_lock() else { continue };
        let count = theirs.0.len().div_ceil(2);
        if count == 0 {
            continue;
        }
        let more = theirs.0.len() > 1;
        let mut taken: VecDeque<*mut Task> = theirs.0.drain(..count).collect();
        other.count.fetch_sub(count, Ordering::Relaxed);
        other.waiting.store(now, Ordering::Relaxed);
        drop(theirs);
        let Some(first) = taken.pop_front() else { continue };
        if !taken.is_empty() {
            // They waited already: another worker may take some at once.
            let mut mine = own.tasks();
            if mine.0.is_empty() {
                own.waiting.store(since, Ordering::Relaxed);
            }
            mine.0.extend(taken.iter().copied());
            own.count.fetch_add(taken.len(), Ordering::Relaxed);
        }
        return Ok((first, more));
    }
    Err(young)
}

/// Whether tasks wait in a worker's own queue.
pub(super) fn waiting_somewhere() -> bool {
    nexts().any(|next| next.count.load(Ordering::SeqCst) > 0)
}

/// The first of the tasks this worker's tasks started.
pub(super) fn own_started() -> Option<*mut Task> {
    let own = own_next()?;
    if own.count.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let task = own.tasks().0.pop_front()?;
    own.count.fetch_sub(1, Ordering::Relaxed);
    Some(task)
}

/// The tasks waiting in workers' own queues.
pub(super) fn waiting_local() -> Vec<*mut Task> {
    nexts().flat_map(|next| next.tasks().0.iter().copied().collect::<Vec<_>>()).collect()
}

/// Every worker's `Next`, which an idle worker takes a task from when the
/// worker that has it runs another for long: read without a lock, as every
/// idle worker reads them, and as a worker does before it waits for the
/// network. One after another: each made at the start of its thread's
/// first block put them all in one set of the caches, which a worker that
/// read more than eight missed every time. A worker past the last has none.
pub(super) static NEXTS: [Next; 1024] = [const { Next::new() }; 1024];
pub(super) static NEXT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// This worker's next, read by tasks as they wake others: through
/// `worker`, or a task that resumes on another thread writes the next of
/// the worker it ran on before, which may sleep.
pub(super) fn own_next() -> Option<&'static Next> {
    worker(|w| w.own_next.get())
}

/// How long a task waits next to a worker that runs another before an idle
/// worker takes it, so that tasks that could run in parallel still do.
pub(super) const NEXT_WAIT: Duration = Duration::from_micros(20);
/// How long they wait before an idle worker of another domain of cores
/// takes them (`domains`).
pub(super) const CROSS_WAIT: Duration = Duration::from_micros(200);

/// How long the tasks of `other` wait before a worker in domain `here`
/// takes them.
fn wait_from(here: usize, other: &Next) -> u64 {
    match other.domain.load(Ordering::Relaxed) == here {
        true => NEXT_WAIT.as_nanos() as u64,
        false => CROSS_WAIT.as_nanos() as u64,
    }
}
/// When a worker was last woken for a task waiting next.
pub(super) static NEXT_WOKE: AtomicU64 = AtomicU64::new(0);
/// How many workers sleep, as the queue counts them, read without its lock.
pub(super) static IDLE: AtomicUsize = AtomicUsize::new(0);
/// A worker of the domain was woken for tasks, or looks for them since: no
/// other of the domain is woken until it finds one or sleeps again, and then
/// it wakes the next if more wait, as Go's scheduler wakes a thread only
/// when none looks for work. Workers that look at once would read what the
/// busy ones write all the time, and slow them. Set under the queue's lock.
pub(super) static SEEKING: [AtomicBool; super::domains::MOST] =
    [const { AtomicBool::new(false) }; super::domains::MOST];

/// Whether a worker looks for tasks in any domain.
pub(super) fn seeking_anywhere() -> bool {
    SEEKING[..super::domains::count()].iter().any(|seeking| seeking.load(Ordering::SeqCst))
}

/// The domain to wake a worker in for tasks made in `here`: `here`, when one
/// sleeps there and none of it looks; or, when none sleeps there, another
/// where one sleeps and none looks (the model of `testing/models/tests/
/// wake_domains.to`).
fn domain_to_wake(queue: &Queue, here: usize) -> Option<usize> {
    if queue.idle_in[here] > 0 {
        return (!SEEKING[here].load(Ordering::SeqCst)).then_some(here);
    }
    (0..super::domains::count()).find(|&domain| queue.idle_in[domain] > 0 && !SEEKING[domain].load(Ordering::SeqCst))
}

/// Wakes a worker that sleeps in `domain` to look for tasks there.
fn wake_in(mut queue: MutexGuard<'static, Queue>, domain: usize) {
    queue.waking[domain] = true;
    SEEKING[domain].store(true, Ordering::SeqCst);
    drop(queue);
    READY[domain].notify_one();
}

/// A domain where a worker sleeps, `here` first.
fn domain_asleep(queue: &Queue, here: usize) -> Option<usize> {
    if queue.idle_in[here] > 0 {
        return Some(here);
    }
    (0..super::domains::count()).find(|&domain| queue.idle_in[domain] > 0)
}

pub(super) fn nanos() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

pub(super) fn nexts() -> impl Iterator<Item = &'static Next> {
    NEXTS[..NEXT_COUNT.load(Ordering::Acquire).min(NEXTS.len())].iter()
}

/// The tasks waiting next to their workers.
pub(super) fn waiting_next() -> Vec<*mut Task> {
    nexts().map(|next| next.task.load(Ordering::Acquire)).filter(|task| !task.is_null()).collect()
}

/// A task another worker has had next for longer than `NEXT_WAIT`, taken
/// from it; and whether one waits there for less.
pub(super) fn take_next() -> (Option<*mut Task>, bool) {
    let mine = own_next().map(|next| next as *const Next);
    let mut clock = None;
    let mut young = false;
    for next in nexts().filter(|next| Some(*next as *const Next) != mine) {
        let task = next.task.load(Ordering::Acquire);
        if task.is_null() {
            continue;
        }
        let (now, here) = *clock.get_or_insert_with(|| (nanos(), super::domains::here()));
        if now.saturating_sub(next.since.load(Ordering::Relaxed)) < wait_from(here, next) {
            young = true;
        } else if next.task.compare_exchange(task, std::ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return (Some(task), young);
        }
    }
    (None, young)
}

/// Makes a notified task runnable: one that the running task notifies runs
/// next on its thread, and the one it replaces there, if any, goes to the
/// queue.
pub(super) fn schedule(task: *mut Task) {
    if !current().is_null()
        && exploring().is_none()
        && let Some(next) = own_next()
    {
        let now = nanos();
        next.since.store(now, Ordering::Relaxed);
        let replaced = next.task.swap(task, Ordering::AcqRel);
        if !replaced.is_null() {
            enqueue(replaced);
            return;
        }
        // An idle worker takes it if this one runs on for long, as Go's
        // scheduler steals `runnext` from a processor busy for a while: one
        // woken now and then, not at every task two tasks hand each other.
        if now.saturating_sub(NEXT_WOKE.load(Ordering::Relaxed)) < 5 * NEXT_WAIT.as_nanos() as u64 {
            return;
        }
        NEXT_WOKE.store(now, Ordering::Relaxed);
        wake_idle();
        return;
    }
    enqueue(task);
}

/// Makes a task runnable: one a running task starts in its worker's own
/// queue, which idle workers take from; others in the queue every worker
/// takes from.
pub(super) fn enqueue(task: *mut Task) {
    if !current().is_null()
        && exploring().is_none()
        && let Some(own) = own_next()
    {
        let mut tasks = own.tasks();
        tasks.0.push_back(task);
        let first = tasks.0.len() == 1;
        drop(tasks);
        // Counted before the idle workers are read: a worker that goes to
        // sleep counts itself idle before it reads these (`work`), so one of
        // the two sees the other. The first that waits wakes a worker, which
        // takes some once they waited for a while, and wakes the next if
        // more wait; the next wait with it, so that short tasks end where
        // they started.
        own.count.fetch_add(1, Ordering::SeqCst);
        if first {
            own.waiting.store(nanos(), Ordering::Relaxed);
            wake_idle();
        }
        return;
    }
    let mut queue = queue();
    queue.runnable.push_back(task);
    // Any worker that looks takes from the queue.
    if looks_next() || seeking_anywhere() {
        return;
    }
    // One worker woken at a time, not one for each task: the one woken
    // wakes the next if it finds more.
    match domain_asleep(&queue, super::domains::here()) {
        Some(domain) => wake_in(queue, domain),
        None => wake_one(queue),
    }
}

/// Wakes a worker that sleeps to look for tasks, unless one looks already.
pub(super) fn wake_idle() {
    if IDLE.load(Ordering::SeqCst) == 0 {
        return;
    }
    let here = super::domains::here();
    if SEEKING[here].load(Ordering::SeqCst) {
        return;
    }
    let queue = queue();
    if let Some(domain) = domain_to_wake(&queue, here) {
        wake_in(queue, domain);
    }
}

/// Whether this worker looks at the queue next, once.
pub(super) fn looks_next() -> bool {
    worker(|w| w.looks_next.replace(false))
}

/// Wakes a worker to look at the queue, as for a signal that came.
pub(crate) fn wake_worker() {
    wake_one(queue());
}

/// Wakes a worker to look at the queue: one that sleeps, or the one that
/// waits for the network's events if none sleeps.
pub(super) fn wake_one(queue: MutexGuard<'static, Queue>) {
    let (sleeping, polling) = (domain_asleep(&queue, super::domains::here()), queue.polling);
    drop(queue);
    if let Some(domain) = sleeping {
        READY[domain].notify_one();
    } else if polling && let Some(poller) = crate::poll::poller() {
        poller.wake();
    }
}

/// The wait for the network's events, unless no socket needs it or another
/// worker has it.
pub(super) fn claim(
    queue: &mut Queue,
) -> Option<(&'static crate::poll::Poller, MutexGuard<'static, crate::poll::Polling>)> {
    let poller = crate::poll::poller().filter(|_| !queue.polling)?;
    let poll = poller.claim()?;
    queue.polling = true;
    queue.polls += 1;
    // One worker, exploring orders, waits for the events itself: nothing
    // else wakes it.
    if exploring().is_none() {
        WATCHER.get_or_init(|| spawn_thread(Some(64 * 1024), watch).thread().clone());
    }
    Some((poller, poll))
}

/// Waits for the network's events until `deadline` and makes runnable the
/// tasks they wake. With nothing else to run, this worker runs the first
/// one without waking another, as Go's scheduler does.
pub(super) fn poll(
    poller: &'static crate::poll::Poller,
    poll: MutexGuard<'static, crate::poll::Polling>,
    deadline: Option<Instant>,
) -> MutexGuard<'static, Queue> {
    let events = worker(|w| {
        if w.events.get().is_null() {
            w.events.set(Box::into_raw(Box::default()));
        }
        w.events.get()
    });
    // Waking queues what waits, and polls nothing that could wait here again.
    let events = unsafe { &mut *events };
    poller.wait(poll, deadline, events);
    let mut queue = queue();
    queue.polling = false;
    let alone = queue.runnable.is_empty();
    drop(queue);
    if WATCHER_ASLEEP.load(Ordering::Relaxed)
        && WATCHER_ASLEEP.swap(false, Ordering::SeqCst)
        && let Some(watcher) = WATCHER.get()
    {
        watcher.unpark();
    }
    worker(|w| w.looks_next.set(alone));
    events.wake();
    turn_host();
    worker(|w| w.looks_next.set(false));
    self::queue()
}

pub(super) static WATCHER: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
pub(super) static WATCHER_ASLEEP: AtomicBool = AtomicBool::new(false);
/// How long the network may go without a worker waiting for its events.
pub(super) const WATCH_PERIOD: Duration = Duration::from_millis(10);

/// Watches the network while workers sleep: when no worker waited for its
/// events in a period, as when the one that did runs a long task since,
/// one that sleeps wakes to wait for them (as Go's `sysmon` polls). Sleeps
/// while a worker waits for them.
pub(super) fn watch() {
    let mut seen = 0;
    loop {
        std::thread::sleep(WATCH_PERIOD);
        let queue = queue();
        let unattended = !queue.polling && queue.polls == seen && queue.idle > 0;
        seen = queue.polls;
        if queue.polling {
            WATCHER_ASLEEP.store(true, Ordering::SeqCst);
            drop(queue);
            std::thread::park();
            continue;
        }
        let asleep = domain_asleep(&queue, 0);
        drop(queue);
        if unattended && let Some(domain) = asleep {
            READY[domain].notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    /// A worker counted idle may wake before `wake_idle` takes the queue's
    /// lock: then none is woken and none is marked as looking for tasks, for
    /// a mark that no worker clears keeps every later task from waking one.
    #[test]
    fn a_worker_is_marked_looking_only_when_one_sleeps() {
        super::IDLE.store(1, Ordering::SeqCst);
        super::wake_idle();
        super::IDLE.store(0, Ordering::SeqCst);
        let waking = std::mem::take(&mut super::queue().waking).iter().any(|&waking| waking);
        let seeking = super::SEEKING.iter().any(|seeking| seeking.swap(false, Ordering::SeqCst));
        assert!(!waking && !seeking, "marked as looking with no worker asleep");
    }
}
