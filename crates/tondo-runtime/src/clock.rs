//! The virtual clock of `testing.withVirtualTime` (testing 7.9).
//!
//! While a test runs the closure, the clock of the task that calls it and
//! of the tasks it starts is a domain's: `time.now()` reads the domain's
//! instant and timers wait on it. When every task of the domain waits on
//! something only the domain can bring, the domain moves on: it finishes a
//! `settle`, or goes to the next deadline (no further than an `advance`'s
//! target), or, with nothing left that could happen, is the panic `P2003`.
//! A task waiting on the host keeps it where it is.
//!
//! A worker runs one test at a time, so there is at most one domain.

use super::tasks::{CANCELLED, Task, Wait, current, notify};
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};

struct Timer {
    at: u64,
    sequence: u64,
    wait: Arc<Wait>,
    index: usize,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.sequence) == (other.at, other.sequence)
    }
}

impl Eq for Timer {}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timer {
    // The earliest first, and among them the first set.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.at, other.sequence).cmp(&(self.at, self.sequence))
    }
}

struct Domain {
    now: u64,
    timers: BinaryHeap<Timer>,
    sequence: u64,
    /// Its tasks that run or can run.
    active: usize,
    /// Waits of its tasks on the host, which the domain cannot bring.
    external: usize,
    /// The closure's task waiting in `settle`, or in `advance` with its
    /// target.
    waiter: Option<(Arc<Wait>, Option<u64>)>,
    /// Set while one thread moves the domain on.
    driving: bool,
    automatic: u64,
    explicit: u64,
    settles: u64,
}

static DOMAIN: Mutex<Option<Domain>> = Mutex::new(None);

fn domain() -> std::sync::MutexGuard<'static, Option<Domain>> {
    DOMAIN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the running task's clock is the domain's.
fn virtual_task() -> bool {
    let task = current();
    !task.is_null() && unsafe { (*task).in_domain }
}

/// The domain's instant, for `time.now()` in one of its tasks.
pub(crate) fn now() -> Option<i64> {
    if !virtual_task() {
        return None;
    }
    domain().as_ref().map(|d| d.now as i64)
}

/// Sets a timer on the domain's clock; false when the task's clock is the
/// host's.
pub(crate) fn add_timer(nanos: u64, wait: &Arc<Wait>, index: usize) -> bool {
    if !virtual_task() {
        return false;
    }
    let mut guard = domain();
    let Some(d) = guard.as_mut() else { return false };
    d.sequence += 1;
    let at = d.now.saturating_add(nanos);
    d.timers.push(Timer { at, sequence: d.sequence, wait: wait.clone(), index });
    true
}

/// A task of the domain started, or woke.
pub(crate) fn woke(task: *mut Task) {
    if unsafe { (*task).in_domain }
        && let Some(d) = domain().as_mut()
    {
        d.active += 1;
    }
}

/// A task of the domain parked or ended: the domain may move on.
pub(crate) fn stopped(in_domain: bool) {
    if !in_domain {
        return;
    }
    if let Some(d) = domain().as_mut() {
        d.active -= 1;
    }
    drive();
}

/// A task of the domain waits on the host (`true`), or the host is done.
pub(crate) fn external(start: bool) {
    if !virtual_task() && start {
        return;
    }
    if let Some(d) = domain().as_mut() {
        if start {
            d.external += 1;
        } else {
            d.external = d.external.saturating_sub(1);
        }
    }
    if !start {
        drive();
    }
}

/// Whether the running task waits on the host for the domain.
pub(crate) fn in_domain() -> bool {
    virtual_task()
}

/// Moves the domain on while none of its tasks can run.
fn drive() {
    loop {
        let mut wake: Vec<*mut Task> = Vec::new();
        {
            let mut guard = domain();
            let Some(d) = guard.as_mut() else { return };
            if d.active > 0 || d.external > 0 || d.driving {
                return;
            }
            d.driving = true;
            let next = d.timers.peek().map(|t| t.at);
            match d.waiter.take() {
                // `settle` ends where the clock is.
                Some((wait, None)) => {
                    d.settles += 1;
                    if wait.claim(0) {
                        wake.push(wait.task);
                    }
                }
                // `advance` visits each deadline up to its target.
                Some((wait, Some(target))) if next.is_none_or(|at| at > target) => {
                    d.now = target;
                    d.explicit += 1;
                    if wait.claim(0) {
                        wake.push(wait.task);
                    }
                }
                waiter => {
                    let Some(at) = next else {
                        drop(guard);
                        deadlock();
                        return;
                    };
                    if waiter.is_none() {
                        d.automatic += 1;
                    }
                    d.waiter = waiter;
                    d.now = at;
                    while d.timers.peek().is_some_and(|t| t.at == at) {
                        let timer = d.timers.pop().unwrap();
                        if timer.wait.claim(timer.index) {
                            wake.push(timer.wait.task);
                        }
                    }
                }
            }
        }
        for task in wake {
            notify(task);
        }
        let mut guard = domain();
        let Some(d) = guard.as_mut() else { return };
        d.driving = false;
        if d.active > 0 || d.external > 0 {
            return;
        }
    }
}

/// Every task of the domain waits and nothing can wake them (`P2003`).
fn deadlock() {
    let report = "panic[P2003]: test-virtual-time-deadlock: every task waits and no timer or other task of the virtual clock can wake them";
    if !super::testing::terminal(2003, report) {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), format!("{report}\n").as_bytes());
    }
    super::tasks::abandon(super::PANIC_EXIT_CODE);
}

fn fail(code: u32, message: &str, location: *const super::Str) -> ! {
    super::panic_with(code, message, location)
}

/// The value a `VirtualTime` holds: a count of its copies, since its
/// only state is the domain's.
#[repr(C)]
pub struct ClockObject {
    rc: std::sync::atomic::AtomicUsize,
}

/// Starts a domain for the running task (`P2004` if one is running).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_clock_begin(location: *const super::Str) -> *mut ClockObject {
    let task = current();
    {
        let mut guard = domain();
        if guard.is_some() {
            drop(guard);
            fail(2004, "a virtual clock is already running; one closure of `withVirtualTime` at a time", location);
        }
        *guard = Some(Domain {
            now: 0,
            timers: BinaryHeap::new(),
            sequence: 0,
            active: 1,
            external: 0,
            waiter: None,
            driving: false,
            automatic: 0,
            explicit: 0,
            settles: 0,
        });
    }
    if !task.is_null() {
        unsafe { (*task).in_domain = true };
    }
    super::LIVE_OBJECTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Box::into_raw(Box::new(ClockObject { rc: std::sync::atomic::AtomicUsize::new(1) }))
}

/// Ends the domain and reports what it did.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_clock_end() {
    let task = current();
    if !task.is_null() {
        unsafe { (*task).in_domain = false };
    }
    if let Some(d) = domain().take() {
        super::testing::virtual_time(d.now, d.automatic, d.explicit, d.settles);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_clock_release(clock: *mut ClockObject) {
    use std::sync::atomic::Ordering;
    if clock.is_null() || unsafe { (*clock).rc.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    std::sync::atomic::fence(Ordering::Acquire);
    drop(unsafe { Box::from_raw(clock) });
    super::LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
}

/// `clock.settle()`: the other tasks run until each ends or waits on the
/// clock, which does not move.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_clock_settle(_clock: *mut ClockObject) {
    wait_for(None);
}

/// `clock.advance(duration)`: the clock moves to `now + duration`, waking
/// each deadline on the way; `P2005` for a negative duration or one past
/// the clock's range.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_clock_advance(_clock: *mut ClockObject, nanos: i64, location: *const super::Str) {
    let now = domain().as_ref().map_or(0, |d| d.now);
    let Some(target) = u64::try_from(nanos).ok().and_then(|n| now.checked_add(n)).filter(|&t| t <= i64::MAX as u64)
    else {
        fail(2005, "the virtual clock cannot move back or past its range", location);
    };
    wait_for(Some(target));
}

fn wait_for(target: Option<u64>) {
    let wait = Wait::new();
    match domain().as_mut() {
        Some(d) => d.waiter = Some((wait.clone(), target)),
        None => return,
    }
    if wait.block() == CANCELLED
        && let Some(d) = domain().as_mut()
    {
        d.waiter = None;
    }
}
