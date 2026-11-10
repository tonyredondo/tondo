//! Waits: a task's wait for the first of one or more operations, which other
//! threads claim and notify it of, its deadline, and the short spin locks over
//! one object's state (docs/refactor.md, phase 2).

use super::*;

pub(super) const NONE: usize = usize::MAX;
/// The index a cancellation claims a wait with.
pub(crate) const CANCELLED: usize = usize::MAX - 1;
/// What a wait gives when its task's deadline passed (11.17).
pub(super) const DEADLINE: usize = usize::MAX - 2;

/// The deadline of the task passed: it unwinds to its `within` at its next
/// check (11.17).
pub(super) fn expire(task: &mut Task) {
    if !task.expired {
        task.expired = true;
        attend(true);
    }
}

/// A task's wait for the first of one or more operations. The objects it
/// waits on point to it until the task removes itself, and timers hold it,
/// since they outlive it.
///
/// What makes one wait serve a task's next: the task goes on only once the
/// operation that claimed its wait notified it, and first leaves every
/// queue that points to the wait, with that queue's lock; whoever claims
/// it through such a pointer does so with the lock held, and reads the
/// task before notifying it. So when nothing but the task holds the wait,
/// as a timer or the waker of a future does until it is dropped, no one
/// can claim it any more, and it is the task's to wait with again, or,
/// once the task ends, another's on its thread.
pub(crate) struct Wait {
    pub(crate) task: *mut Task,
    pub(super) fired: AtomicUsize,
}

// Other threads only claim the wait and notify its task.
unsafe impl Send for Wait {}
unsafe impl Sync for Wait {}

impl Wait {
    /// A wait for the running task: the one it waited with last, when
    /// nothing else holds it, or one a task that ended on this thread left,
    /// or a new one, which it keeps for its next (docs/speed.md, C).
    /// Outside a task, always a new one.
    pub(crate) fn new() -> Arc<Wait> {
        let task = current();
        let Some(running) = (unsafe { task.as_mut() }) else {
            return Arc::new(Wait { task, fired: AtomicUsize::new(NONE) });
        };
        if let Some(kept) = &running.kept_wait
            && Arc::strong_count(kept) == 1
        {
            kept.fired.store(NONE, Ordering::Relaxed);
            return kept.clone();
        }
        let mut wait = spare_waits().pop().unwrap_or_else(|| Arc::new(Wait { task, fired: AtomicUsize::new(NONE) }));
        match Arc::get_mut(&mut wait) {
            Some(only) => {
                only.task = task;
                *only.fired.get_mut() = NONE;
            }
            None => wait = Arc::new(Wait { task, fired: AtomicUsize::new(NONE) }),
        }
        running.kept_wait = Some(wait.clone());
        wait
    }

    /// What a task that ended leaves: the wait it kept, for the next task
    /// on this thread, when nothing else holds it.
    pub(super) fn left(task: &mut Task) {
        let Some(wait) = task.kept_wait.take() else { return };
        let spare = spare_waits();
        if Arc::strong_count(&wait) == 1 && spare.len() < SPARE_WAITS {
            spare.push(wait);
        }
    }

    /// Claims the wait for operation `index`; false when another operation
    /// did first. Whoever claims it notifies the task afterwards.
    pub(crate) fn claim(&self, index: usize) -> bool {
        self.fired.compare_exchange(NONE, index, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// `block`, for a wait on another task, as for a channel, a join or a
    /// lock, which `profile.startBlocking` samples.
    pub(crate) fn block_on_task(&self) -> usize {
        let since = tools::blocking();
        let fired = self.block();
        crate::profiling::blocked(since);
        fired
    }

    /// Parks until the operation that claimed the wait notifies the task;
    /// returns its index, or `CANCELLED` if the task is cancelled first. A
    /// wait that counts goes on the timeline of the task's request.
    pub(crate) fn block(&self) -> usize {
        let task = unsafe { &*self.task };
        if task.flight == 0 {
            return self.parked();
        }
        let on = match task.blocked_kind.load(Ordering::Acquire) {
            LOCK => "a lock",
            RECEIVING => "a value of a channel",
            SENDING => "room in a channel",
            JOIN => "a task",
            _ => "time or input",
        };
        let since = Instant::now();
        let fired = self.parked();
        let took = since.elapsed().as_millis();
        if took >= 1 {
            flight_step(|| crate::diagnostics::Did::Waited(on, took));
        }
        fired
    }

    fn parked(&self) -> usize {
        let task = unsafe { &mut *self.task };
        tools::note(|| "b\n".to_string());
        // The deadline of a `within` ends the wait when it passes, as a
        // cancellation would, and the task unwinds to it (11.17).
        if let Some(&(at, _)) = task.withins.last() {
            let now = clock_now();
            match at.checked_duration_since(now).filter(|left| !left.is_zero()) {
                Some(left) => {
                    // Every wait is made in an `Arc` by `Wait::new`.
                    let wait = unsafe {
                        Arc::increment_strong_count(self as *const Wait);
                        Arc::from_raw(self as *const Wait)
                    };
                    add_timer(left, wait, DEADLINE);
                }
                None => expire(task),
            }
        }
        lock_unseen(&task.waiting_lock);
        let stopped = halted(task);
        let fired = if stopped && self.claim(CANCELLED) {
            unlock(&task.waiting_lock);
            CANCELLED
        } else {
            task.waiting = self;
            unlock(&task.waiting_lock);
            let fired = self.block_fully();
            lock_unseen(&task.waiting_lock);
            task.waiting = std::ptr::null();
            unlock(&task.waiting_lock);
            if fired == DEADLINE {
                expire(task);
                CANCELLED
            } else {
                fired
            }
        };
        // In a `defer`, the code that waited unwinds to it.
        if fired == CANCELLED && task.deferring > 0 {
            task.cut = true;
        }
        fired
    }

    /// Parks until the operation that claimed the wait notifies the task,
    /// even if it is cancelled: how a release waits for a task.
    pub(crate) fn block_fully(&self) -> usize {
        loop {
            park();
            let fired = self.fired.load(Ordering::Acquire);
            if fired != NONE {
                return fired;
            }
        }
    }
}

/// How many waits of tasks that ended a worker keeps.
const SPARE_WAITS: usize = 16;

fn spare_waits() -> &'static mut Vec<Arc<Wait>> {
    let waits = worker(|w| {
        if w.waits.get().is_null() {
            w.waits.set(Box::into_raw(Box::default()));
        }
        w.waits.get()
    });
    unsafe { &mut *waits }
}

/// The waker of a future the task waits on: its wake claims the wait as
/// operation 0.
impl std::task::Wake for Wait {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if self.claim(0) {
            notify(self.task);
        }
    }
}

/// A short spin lock over one object's state.
#[inline(always)]
pub(crate) fn lock(flag: &AtomicBool) {
    touch(flag as *const AtomicBool as usize);
    lock_unseen(flag);
}

/// A lock whose taking changes nothing another task could tell.
#[inline(always)]
pub(super) fn lock_unseen(flag: &AtomicBool) {
    while flag.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        std::hint::spin_loop();
    }
}

pub(crate) fn unlock(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

/// `time.sleep(delay)` in nanoseconds; no wait when it is not positive.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sleep(nanos: i64) {
    if nanos <= 0 {
        return;
    }
    if cancelled() {
        // A sleep always waits: in a `defer` too, it ends at once.
        if let Some(task) = unsafe { current().as_mut() } {
            task.cut |= task.deferring > 0;
        }
        return;
    }
    let wait = Wait::new();
    add_timer(Duration::from_nanos(nanos as u64), wait.clone(), 0);
    wait.block();
}
