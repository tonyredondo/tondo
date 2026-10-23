//! Tasks (spec 11.11 to 11.14, 16.14): stackful coroutines that worker
//! threads run from one queue. A task that waits saves its registers on its
//! own stack and switches back to its worker, which runs another task; the
//! code it runs is ordinary compiled code that knows nothing of this.
//!
//! The process's main thread is the first worker and runs `main` as the
//! first task. More workers start, one per core, when the program first
//! spawns a task.
//!
//! Every wait goes through a `Wait`: the task registers it with the objects
//! it waits on (a join, a group, channels, a timer) and parks; whoever
//! completes one of those operations first claims the wait, under that
//! object's lock, and then notifies the task.

use super::cycles::{self, Kind, Taken, Visit, Visitor};
use super::trace::Origin;
use super::{Glue, LIVE_OBJECTS, Str, flush_stdout, runtime_fault};
use std::cell::Cell;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Switching stacks
// ---------------------------------------------------------------------------

/// Saves the callee-saved registers on the current stack, stores its pointer
/// in `*from`, and resumes the stack `to` saved the same way.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "sub sp, sp, #176",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "stp d8, d9, [sp, #96]",
        "stp d10, d11, [sp, #112]",
        "stp d12, d13, [sp, #128]",
        "stp d14, d15, [sp, #144]",
        "mov x9, sp",
        "str x9, [x0]",
        "mov sp, x1",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "ldp d8, d9, [sp, #96]",
        "ldp d10, d11, [sp, #112]",
        "ldp d12, d13, [sp, #128]",
        "ldp d14, d15, [sp, #144]",
        "add sp, sp, #176",
        "ret",
    )
}

/// The stack pointer `switch` resumes to start a task in `trampoline`.
unsafe fn initial_stack(top: *mut u8) -> *mut u8 {
    let top = (top as usize & !15) as *mut usize;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            // `ret` pops the trampoline's address and leaves the stack as a
            // call would: 8 bytes past a 16-byte boundary.
            let ret = top.sub(2);
            ret.write(trampoline as *const () as usize);
            let saved = ret.sub(6);
            for i in 0..6 {
                saved.add(i).write(0);
            }
            saved as *mut u8
        }
        #[cfg(target_arch = "aarch64")]
        {
            let saved = top.sub(22);
            for i in 0..22 {
                saved.add(i).write(0);
            }
            // x30, the link register, sits after x29 at offset 88.
            saved.add(11).write(trampoline as *const () as usize);
            saved as *mut u8
        }
    }
}

// ---------------------------------------------------------------------------
// Stacks
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ_WRITE: i32 = 3;
#[cfg(target_os = "linux")]
const MAP_FLAGS: i32 = 0x02 | 0x20 | 0x4000; // PRIVATE | ANONYMOUS | NORESERVE
#[cfg(not(target_os = "linux"))]
const MAP_FLAGS: i32 = 0x02 | 0x1000; // PRIVATE | ANON
/// Covers the largest page size of the supported targets.
pub(crate) const GUARD: usize = 64 * 1024;
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
const MAIN_STACK: usize = 64 * 1024 * 1024;
const TASK_STACK: usize = 8 * 1024 * 1024;

/// A stack mapped on demand, with an inaccessible guard region below it so
/// that an overflow faults instead of corrupting memory.
struct Stack {
    base: *mut u8,
    size: usize,
}

impl Stack {
    fn new(size: usize) -> Stack {
        unsafe {
            let base = mmap(std::ptr::null_mut(), size, PROT_READ_WRITE, MAP_FLAGS, -1, 0);
            if base as isize == -1 {
                runtime_fault("cannot map a task stack");
            }
            mprotect(base, GUARD, PROT_NONE);
            Stack { base, size }
        }
    }

    fn top(&self) -> *mut u8 {
        unsafe { self.base.add(self.size) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        unsafe { munmap(self.base, self.size) };
    }
}

unsafe impl Send for Stack {}

/// Task stacks kept for reuse: mapping and unmapping one per task costs
/// more than the task itself.
static SPARE_STACKS: Mutex<Vec<Stack>> = Mutex::new(Vec::new());
const SPARE_LIMIT: usize = 256;

fn task_stack() -> Stack {
    let spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner()).pop();
    spare.unwrap_or_else(|| Stack::new(TASK_STACK))
}

fn recycle(stack: Stack) {
    let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
    if stack.size == TASK_STACK && spare.len() < SPARE_LIMIT {
        spare.push(stack);
    }
}

// ---------------------------------------------------------------------------
// Tasks and the scheduler
// ---------------------------------------------------------------------------

/// A registered `defer` cleanup: its code and the closure it receives.
pub type Cleanup = unsafe extern "C" fn(*mut u8);

/// How the body of a task returns its result, which the compiler knows from
/// the result's representation.
#[derive(Copy, Clone)]
#[repr(u8)]
pub enum ResultKind {
    Nothing = 0,
    Integer = 1,
    Float64 = 2,
    Float32 = 3,
    Memory = 4,
}

/// The states of a task: it runs, it is switching out to wait, it waits,
/// or it was notified before it could wait; or it waits on its thread,
/// since C frames on its stack keep it there.
const RUNNING: u8 = 0;
const PARKING: u8 = 1;
const PARKED: u8 = 2;
const NOTIFIED: u8 = 3;
const PINNED: u8 = 4;

/// Where tasks that wait on their thread are woken.
static PINNED_LOCK: Mutex<()> = Mutex::new(());
static PINNED_WAKE: Condvar = Condvar::new();

pub(crate) struct Task {
    /// The stack pointer saved when the task is not running.
    sp: *mut u8,
    stack: Option<Stack>,
    /// The closure the task runs: a `fn` object, released afterwards.
    body: *mut u8,
    code: *const u8,
    kind: ResultKind,
    join: *mut JoinObject,
    /// Its `defer` cleanups, innermost last (13.7).
    cleanups: Vec<(Cleanup, usize)>,
    state: AtomicU8,
    /// Set by `cancel` (11.15): the task's waits end at once, and compiled
    /// code unwinds after them.
    cancelled: AtomicBool,
    /// The wait the task is blocked in, which a cancellation claims;
    /// guarded by `waiting_lock`.
    waiting: *const Wait,
    waiting_lock: AtomicBool,
    /// The `std.sync` locks it holds, whether exclusively, and where it
    /// took them.
    pub(crate) held: Vec<(usize, bool, *const Str)>,
    finished: bool,
    main: bool,
    /// For a task that runs a callback for a thread of C, the call.
    foreign: *const ForeignCall,
    /// Whether its clock is a test's virtual clock (testing 7.9).
    pub(crate) in_domain: bool,
    /// Where it was started, for stack traces.
    origin: Option<Arc<Origin>>,
}

/// A callback a thread of C makes, which a task runs while the thread
/// waits for it.
struct ForeignCall {
    inner: unsafe extern "C" fn(*mut u8),
    frame: *mut u8,
    done: Mutex<bool>,
    finished: Condvar,
}

/// Notifies `wait` with operation `index` at `at`, unless another operation
/// claimed it first.
struct Timer {
    at: Instant,
    sequence: u64,
    wait: Arc<Wait>,
    index: usize,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Timer) -> bool {
        (self.at, self.sequence) == (other.at, other.sequence)
    }
}

impl Eq for Timer {}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Timer) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The earliest timer is the greatest, for `BinaryHeap`.
impl Ord for Timer {
    fn cmp(&self, other: &Timer) -> std::cmp::Ordering {
        (other.at, other.sequence).cmp(&(self.at, self.sequence))
    }
}

struct Queue {
    runnable: VecDeque<*mut Task>,
    timers: BinaryHeap<Timer>,
    /// Timers whose wait ended another way; purged when they are most.
    stale: usize,
    sequence: u64,
    /// Workers started, and how many wait for a task.
    workers: usize,
    idle: usize,
    /// A worker waits for the network's events instead (`poll`).
    polling: bool,
    /// Waits for the network's events, which the watcher counts.
    polls: u64,
    /// Tasks taken from the queue, to look at the network now and then.
    picks: u32,
    /// Workers started for others that were long in C, which retire once
    /// those return.
    surplus: usize,
    /// The tasks that wait inside a function C called, each on its worker.
    pinned: Vec<*mut Task>,
}

unsafe impl Send for Queue {}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    runnable: VecDeque::new(),
    timers: BinaryHeap::new(),
    stale: 0,
    sequence: 0,
    workers: 1,
    idle: 0,
    polling: false,
    polls: 0,
    picks: 0,
    surplus: 0,
    pinned: Vec::new(),
});
static READY: Condvar = Condvar::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static MAIN_STATUS: AtomicUsize = AtomicUsize::new(0);
static MAIN_DONE: AtomicBool = AtomicBool::new(false);
/// Host operations in progress for parked tasks: while any runs, idle
/// workers are not a deadlock.
pub(crate) static HOST_WORK: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static CURRENT: Cell<*mut Task> = const { Cell::new(std::ptr::null_mut()) };
    /// Where the worker's own stack pointer is saved while a task runs.
    static HOME: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// Cleanups registered outside any task, as in unit tests.
    static LOOSE_CLEANUPS: std::cell::RefCell<Vec<(Cleanup, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

// A task may resume on another thread, so these never inline: a caller
// must not keep a thread-local address across a switch.
#[inline(never)]
pub(crate) fn current() -> *mut Task {
    CURRENT.with(|c| c.get())
}

/// The bounds of the running task's stack, above its guard.
pub(crate) fn stack_bounds() -> Option<(usize, usize)> {
    let task = current();
    let stack = unsafe { task.as_ref()?.stack.as_ref()? };
    Some((stack.base as usize + GUARD, stack.top() as usize))
}

/// Where the running task was started.
pub(crate) fn origin() -> Option<Arc<Origin>> {
    unsafe { origin_of(current()) }
}

/// Where a task was started.
pub(crate) unsafe fn origin_of(task: *mut Task) -> Option<Arc<Origin>> {
    unsafe { task.as_ref()?.origin.clone() }
}

pub(crate) unsafe fn is_main(task: *mut Task) -> bool {
    unsafe { (*task).main }
}

#[inline(never)]
fn home() -> *mut *mut u8 {
    HOME.with(|h| h.as_ptr())
}

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes a new or notified task runnable.
fn schedule(task: *mut Task) {
    let mut queue = queue();
    queue.runnable.push_back(task);
    if LOOKS_NEXT.with(|next| next.replace(false)) {
        return;
    }
    wake_one(queue);
}

thread_local! {
    /// This worker looks at the queue next and finds nothing before the
    /// first task it schedules, which wakes no other worker.
    static LOOKS_NEXT: Cell<bool> = const { Cell::new(false) };
}

/// Wakes a worker to look at the queue: one that sleeps, or the one that
/// waits for the network's events if none sleeps.
fn wake_one(queue: MutexGuard<'static, Queue>) {
    let (sleeping, polling) = (queue.idle > 0, queue.polling);
    drop(queue);
    if sleeping {
        READY.notify_one();
    } else if polling && let Some(poller) = super::poll::poller() {
        poller.wake();
    }
}

/// The wait for the network's events, unless no socket needs it or another
/// worker has it.
fn claim(queue: &mut Queue) -> Option<(&'static super::poll::Poller, MutexGuard<'static, super::poll::Polling>)> {
    let poller = super::poll::poller().filter(|_| !queue.polling)?;
    let poll = poller.claim()?;
    queue.polling = true;
    queue.polls += 1;
    WATCHER.get_or_init(|| spawn_thread(Some(64 * 1024), watch).thread().clone());
    Some((poller, poll))
}

/// Waits for the network's events until `deadline` and makes runnable the
/// tasks they wake. With nothing else to run, this worker runs the first
/// one without waking another, as Go's scheduler does.
fn poll(
    poller: &'static super::poll::Poller,
    poll: MutexGuard<'static, super::poll::Polling>,
    deadline: Option<Instant>,
) -> MutexGuard<'static, Queue> {
    let events = poller.wait(poll, deadline);
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
    LOOKS_NEXT.with(|next| next.set(alone));
    events.wake();
    LOOKS_NEXT.with(|next| next.set(false));
    self::queue()
}

static WATCHER: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
static WATCHER_ASLEEP: AtomicBool = AtomicBool::new(false);
/// How long the network may go without a worker waiting for its events.
const WATCH_PERIOD: Duration = Duration::from_millis(10);

/// Watches the network while workers sleep: when no worker waited for its
/// events in a period, as when the one that did runs a long task since,
/// one that sleeps wakes to wait for them (as Go's `sysmon` polls). Sleeps
/// while a worker waits for them.
fn watch() {
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
        drop(queue);
        if unattended {
            READY.notify_one();
        }
    }
}

/// Wakes a task for its current wait, or makes its next `park` return at
/// once if it has not parked yet; `run` makes it runnable.
fn notify_with(task: *mut Task, run: impl FnOnce(*mut Task)) {
    let state = unsafe { &(*task).state };
    loop {
        let current = state.load(Ordering::Acquire);
        let next = if current == PARKED || current == PINNED { RUNNING } else { NOTIFIED };
        if current == NOTIFIED {
            return;
        }
        if state.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if current == PARKED {
                super::clock::woke(task);
                run(task);
            } else if current == PINNED {
                let _lock = PINNED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                PINNED_WAKE.notify_all();
            }
            return;
        }
    }
}

pub(crate) fn notify(task: *mut Task) {
    notify_with(task, schedule);
}

/// Switches the current task out until it is notified; returns at once if
/// it already was.
fn park() {
    let task = current();
    if task.is_null() {
        runtime_fault("waiting outside a task");
    }
    unsafe {
        let state = &(*task).state;
        // With C frames on its stack the task cannot resume on another
        // thread, so it waits on this one; the monitor may start another
        // worker meanwhile.
        let in_c = IN_C.with(|c| c.get());
        if !in_c.is_null() && (*in_c).depth.load(Ordering::SeqCst) > 0 {
            if state.compare_exchange(RUNNING, PINNED, Ordering::AcqRel, Ordering::Acquire).is_err() {
                state.store(RUNNING, Ordering::Release);
                return;
            }
            wait_pinned(task);
            return;
        }
        if state.compare_exchange(RUNNING, PARKING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            state.store(RUNNING, Ordering::Release);
            return;
        }
        switch(&mut (*task).sp, *home());
    }
}

/// Waits on this thread until the task is notified, as one more worker
/// that waits for the deadlock check, which runs again now and then while
/// only C could wake the program.
fn wait_pinned(task: *mut Task) {
    let state = unsafe { &(*task).state };
    let check = |queue: &Queue| {
        if matches!(stalled(queue), Stalled::Deadlock) {
            deadlock();
        }
    };
    {
        let mut queue = queue();
        queue.pinned.push(task);
        check(&queue);
    }
    // Waiting, the thread runs no Tondo code that a collection could meet.
    leave_tondo();
    loop {
        let lock = PINNED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if state.load(Ordering::Acquire) == RUNNING {
            break;
        }
        let (lock, _) = PINNED_WAKE.wait_timeout(lock, RECHECK).unwrap_or_else(|e| e.into_inner());
        if state.load(Ordering::Acquire) == RUNNING {
            break;
        }
        drop(lock);
        check(&queue());
    }
    enter_tondo();
    queue().pinned.retain(|&t| t != task);
}

// ---------------------------------------------------------------------------
// Stopping the world
// ---------------------------------------------------------------------------

/// Set while the cycle collector needs every thread out of Tondo code.
static STOPPING: AtomicBool = AtomicBool::new(false);
static RESUMED: Condvar = Condvar::new();
static RESUMED_LOCK: Mutex<()> = Mutex::new(());
/// Whether each thread that ever ran Tondo code runs it now.
static IN_TONDO: Mutex<Vec<&'static AtomicBool>> = Mutex::new(Vec::new());

thread_local! {
    static IN_TONDO_HERE: Cell<Option<&'static AtomicBool>> = const { Cell::new(None) };
}

/// This thread's flag, registered the first time it runs Tondo code.
fn running_here() -> &'static AtomicBool {
    IN_TONDO_HERE.with(|here| {
        if let Some(running) = here.get() {
            return running;
        }
        let running: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).push(running);
        here.set(Some(running));
        running
    })
}

/// From now on this thread runs Tondo code, once no collection runs.
pub(crate) fn enter_tondo() {
    let running = running_here();
    loop {
        running.store(true, Ordering::SeqCst);
        if !STOPPING.load(Ordering::SeqCst) {
            return;
        }
        running.store(false, Ordering::SeqCst);
        let mut resumed = RESUMED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        while STOPPING.load(Ordering::SeqCst) {
            resumed = RESUMED.wait(resumed).unwrap_or_else(|e| e.into_inner());
        }
    }
}

fn leave_tondo() {
    running_here().store(false, Ordering::SeqCst);
}

/// Takes this thread out of Tondo code where it is, as when it waits for
/// memory inside an allocation; whether it was in, to `enter_tondo` again.
/// Allocates nothing.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn pause_here() -> bool {
    IN_TONDO_HERE.with(|here| here.get()).is_some_and(|running| running.swap(false, Ordering::SeqCst))
}

/// Where a running task lets a collection run: between two steps.
fn safe_point() {
    if STOPPING.load(Ordering::Relaxed) {
        leave_tondo();
        enter_tondo();
    }
    if cycles::owed() {
        leave_tondo();
        cycles::assist();
        enter_tondo();
    }
}

/// Stops every thread at its next safe point, for the cycle collector;
/// gives up after `timeout`, when a task runs long without one.
pub(crate) fn stop_world(timeout: Duration) -> bool {
    STOPPING.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + timeout;
    let threads = IN_TONDO.lock().unwrap_or_else(|e| e.into_inner()).clone();
    loop {
        if threads.iter().all(|running| !running.load(Ordering::SeqCst)) {
            return true;
        }
        if Instant::now() >= deadline {
            resume_world();
            return false;
        }
        std::thread::yield_now();
    }
}

pub(crate) fn resume_world() {
    STOPPING.store(false, Ordering::SeqCst);
    let _resumed = RESUMED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    RESUMED.notify_all();
}

/// What it means that no task can run: nothing yet, a deadlock (`P0013`),
/// or a wait that a thread of C may end by calling back.
enum Stalled {
    No,
    Deadlock,
    MaybeC,
}

/// How often idle workers look again while only C could wake the program.
const RECHECK: Duration = Duration::from_millis(100);

/// Every worker waits, for a task or inside a function C called, no timer
/// is due and no host work runs. That is a deadlock unless a thread the
/// runtime did not start exists, which C could use to call back.
fn stalled(queue: &Queue) -> Stalled {
    // A notified task about to wake on its thread is running.
    let pinned = queue.pinned.iter().filter(|&&task| unsafe { (*task).state.load(Ordering::Acquire) } == PINNED);
    if !queue.runnable.is_empty()
        || queue.timers.peek().is_some()
        || queue.idle + pinned.count() + usize::from(queue.polling) < queue.workers
        || HOST_WORK.load(Ordering::SeqCst) != 0
        || MAIN_DONE.load(Ordering::SeqCst)
    {
        return Stalled::No;
    }
    // Threads of C that wait for a callback's task cannot call back.
    let known = RUNTIME_THREADS.load(Ordering::SeqCst) + FOREIGN_WAITING.load(Ordering::SeqCst);
    if os_threads().is_some_and(|count| count > known) { Stalled::MaybeC } else { Stalled::Deadlock }
}

/// Threads the runtime started that still run, the main thread included.
pub(crate) static RUNTIME_THREADS: AtomicUsize = AtomicUsize::new(1);
/// Threads of C waiting in `tondo_rt_callback` for the task they started.
static FOREIGN_WAITING: AtomicUsize = AtomicUsize::new(0);

/// Waits, up to a second, for the calls from threads of C still running:
/// what they hold is not leaked when the program ends before them.
pub fn wait_foreign_calls() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while FOREIGN_WAITING.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Starts a runtime thread, counted while it runs.
pub(crate) fn spawn_thread<T: Send + 'static>(
    stack: Option<usize>,
    body: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
        }
    }
    RUNTIME_THREADS.fetch_add(1, Ordering::SeqCst);
    let mut builder = std::thread::Builder::new();
    if let Some(stack) = stack {
        builder = builder.stack_size(stack);
    }
    builder
        .spawn(move || {
            let _running = Running;
            body()
        })
        .unwrap_or_else(|_| runtime_fault("cannot start a thread"))
}

/// How many threads the process has, where the system tells.
#[cfg(target_os = "linux")]
fn os_threads() -> Option<usize> {
    std::fs::read_dir("/proc/self/task").ok().map(|entries| entries.count())
}

#[cfg(target_os = "macos")]
fn os_threads() -> Option<usize> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, port: u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    }
    unsafe {
        let task = mach_task_self_;
        let (mut threads, mut count) = (std::ptr::null_mut(), 0u32);
        if task_threads(task, &mut threads, &mut count) != 0 {
            return None;
        }
        for index in 0..count as usize {
            mach_port_deallocate(task, *threads.add(index));
        }
        vm_deallocate(task, threads as usize, count as usize * 4);
        Some(count as usize)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn os_threads() -> Option<usize> {
    None
}

/// Fires the timers that are due; their tasks join `runnable`.
fn fire_timers(queue: &mut Queue) {
    let now = Instant::now();
    while queue.timers.peek().is_some_and(|t| t.at <= now) {
        let timer = queue.timers.pop().unwrap();
        if timer.wait.claim(timer.index) {
            notify_with(timer.wait.task, |task| queue.runnable.push_back(task));
        } else {
            queue.stale = queue.stale.saturating_sub(1);
        }
    }
}

/// Notes timers whose wait ended another way, and drops them once they
/// are most of the timers.
fn timers_ended(count: usize) {
    let mut queue = queue();
    queue.stale += count;
    if queue.stale > 64 && queue.stale * 2 > queue.timers.len() {
        queue.timers.retain(|t| t.wait.fired.load(Ordering::Acquire) == NONE);
        queue.stale = 0;
    }
}

fn add_timer(delay: Duration, wait: Arc<Wait>, index: usize) {
    if super::clock::add_timer(delay.as_nanos() as u64, &wait, index) {
        return;
    }
    let mut queue = queue();
    queue.sequence += 1;
    let sequence = queue.sequence;
    queue.timers.push(Timer { at: Instant::now() + delay, sequence, wait, index });
    // An idle worker recomputes how long to sleep.
    wake_one(queue);
}

/// Runs tasks; on the main thread, until the main task finishes, wherever
/// it ran, and then returns its exit status.
fn work(main: bool) -> i32 {
    super::trace::guard_thread();
    loop {
        let task = {
            let mut queue = queue();
            loop {
                // Exploring orders this thread is the only worker: after a
                // panic it runs the other tasks' cleanups itself.
                let draining =
                    exploring().is_some() && ENDING.load(Ordering::SeqCst) != usize::MAX && !queue.runnable.is_empty();
                if main && MAIN_DONE.load(Ordering::SeqCst) && !draining {
                    return MAIN_STATUS.load(Ordering::SeqCst) as i32;
                }
                if !main && queue.surplus > 0 {
                    queue.surplus -= 1;
                    queue.workers -= 1;
                    return 0;
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
                    queue = poll(poller, polled, Some(Instant::now()));
                }
                let next = match exploring() {
                    Some(seed) if !queue.runnable.is_empty() => {
                        let at = (seeded(seed) % queue.runnable.len() as u64) as usize;
                        queue.runnable.remove(at)
                    }
                    _ => queue.runnable.pop_front(),
                };
                if let Some(task) = next {
                    break task;
                }
                // With sockets to poll, an idle worker waits for their
                // events, unless another one does, and runs what they wake
                // on its own thread.
                let polling = claim(&mut queue);
                if polling.is_none() {
                    queue.idle += 1;
                }
                let mut next = queue.timers.peek().map(|t| t.at);
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
                    queue = poll(poller, polled, next);
                    continue;
                }
                queue = match next {
                    Some(at) => {
                        let delay = at.saturating_duration_since(Instant::now());
                        READY.wait_timeout(queue, delay).unwrap_or_else(|e| e.into_inner()).0
                    }
                    None => READY.wait(queue).unwrap_or_else(|e| e.into_inner()),
                };
                queue.idle -= 1;
            }
        };
        CURRENT.with(|c| c.set(task));
        enter_tondo();
        unsafe { switch(home(), (*task).sp) };
        leave_tondo();
        CURRENT.with(|c| c.set(std::ptr::null_mut()));
        unsafe {
            let in_domain = (*task).in_domain;
            if (*task).finished {
                let mut task = Box::from_raw(task);
                let was_main = task.main;
                if let Some(stack) = task.stack.take() {
                    recycle(stack);
                }
                tasks().remove(&(&*task as *const Task as usize));
                drop(task);
                super::clock::stopped(in_domain);
                if was_main {
                    MAIN_DONE.store(true, Ordering::SeqCst);
                    // Take the queue's lock so the main thread cannot miss
                    // the notification between its check and its wait.
                    drop(queue());
                    READY.notify_all();
                    if let Some(poller) = super::poll::poller() {
                        poller.wake();
                    }
                }
            } else if (*task).state.compare_exchange(PARKING, PARKED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                super::clock::stopped(in_domain);
            } else {
                // Notified while switching out.
                (*task).state.store(RUNNING, Ordering::Release);
                schedule(task);
            }
        }
    }
}

fn deadlock() -> ! {
    // Several idle workers may see it at once; one reports it.
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if REPORTED.swap(true, Ordering::SeqCst) {
        loop {
            std::thread::park();
        }
    }
    flush_stdout();
    let mut report = String::from("panic[P0013]: deadlock: every task waits and none of them can wake the others");
    // Where each task waits, the same place once, with the main task first.
    let mut waits: Vec<(String, usize)> = Vec::new();
    let mut all: Vec<*mut Task> = tasks().iter().map(|&t| t as *mut Task).collect();
    all.sort_by_key(|&t| unsafe { (!(*t).main, t as usize) });
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
    let _ = writeln!(std::io::stderr(), "{report}");
    std::process::exit(super::PANIC_EXIT_CODE)
}

/// The trace of a task that waits, under a `TITLE` to fill in: from the
/// frame pointer it saved when it switched out, or from here for the
/// running one.
unsafe fn waiting_at(task: *mut Task) -> String {
    unsafe {
        let Some(stack) = (*task).stack.as_ref() else { return String::new() };
        let bounds = (stack.base as usize + GUARD, stack.top() as usize);
        let fp = if task == current() {
            super::trace::frame_pointer()
        } else if (*task).state.load(Ordering::Acquire) == PARKED {
            let sp = (*task).sp as *const usize;
            #[cfg(target_arch = "x86_64")]
            let fp = *sp.add(5);
            #[cfg(target_arch = "aarch64")]
            let fp = *sp.add(10);
            fp
        } else {
            return "\nTITLE: a call to C".into();
        };
        super::trace::stack(super::trace::walk(fp, bounds), (*task).origin.as_deref(), "TITLE")
    }
}

/// Starts the other workers, one per core, the first time a task spawns.
fn start_workers() {
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

fn start_worker() {
    spawn_thread(Some(256 * 1024), || {
        work(false);
    });
}

// ---------------------------------------------------------------------------
// Calls of C
// ---------------------------------------------------------------------------

/// A worker's calls of C: how deeply it is inside them, and a count of
/// entries and exits that changes whenever a call starts or ends; and, for
/// stack traces, where the program made each call.
struct InC {
    depth: AtomicUsize,
    calls: AtomicUsize,
    /// For each call deep, the frame of the code that called C, and where
    /// it called from.
    frames: [AtomicUsize; 8],
    returns: [AtomicUsize; 8],
    /// Whether another worker took this one's place during the current call.
    replaced: AtomicBool,
}

thread_local! {
    static IN_C: Cell<*const InC> = const { Cell::new(std::ptr::null()) };
}

/// Every worker that has called C, which the monitor watches.
static CALLERS: Mutex<Vec<&'static InC>> = Mutex::new(Vec::new());
static MONITOR: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
static MONITOR_ASLEEP: AtomicBool = AtomicBool::new(false);
/// How often the monitor looks: a call still in C after one look and the
/// next is long, and blocks its worker.
const MONITOR_PERIOD: Duration = Duration::from_millis(1);

#[inline(never)]
fn in_c() -> &'static InC {
    let current = IN_C.with(|c| c.get());
    if !current.is_null() {
        return unsafe { &*current };
    }
    let new: &'static InC = Box::leak(Box::new(InC {
        depth: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        frames: Default::default(),
        returns: Default::default(),
        replaced: AtomicBool::new(false),
    }));
    CALLERS.lock().unwrap_or_else(|e| e.into_inner()).push(new);
    IN_C.with(|c| c.set(new));
    new
}

/// Compiled code calls this before a C function: while the call lasts the
/// worker runs nothing else, so the monitor may start another in its place.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_foreign_enter() {
    // C follows no references of Tondo values.
    leave_tondo();
    let state = in_c();
    state.calls.fetch_add(1, Ordering::SeqCst);
    let depth = state.depth.fetch_add(1, Ordering::SeqCst);
    if let (Some(frame), Some(returns)) = (state.frames.get(depth), state.returns.get(depth)) {
        // The caller's frame lives while C runs; this function's own is gone
        // once it returns, and C may write over it.
        let fp = super::trace::frame_pointer();
        let (caller, address) = unsafe { (*(fp as *const usize), *((fp + 8) as *const usize)) };
        frame.store(caller, Ordering::Relaxed);
        returns.store(address, Ordering::Relaxed);
    }
    let monitor = MONITOR.get_or_init(|| spawn_thread(Some(64 * 1024), monitor).thread().clone());
    if MONITOR_ASLEEP.load(Ordering::SeqCst) && MONITOR_ASLEEP.swap(false, Ordering::SeqCst) {
        monitor.unpark();
    }
}

/// Where a fault inside C goes back to: the frame of the code that made the
/// running worker's innermost call of C, and the address it called from.
pub(crate) fn c_frame() -> Option<(usize, usize)> {
    let state = unsafe { IN_C.with(|c| c.get()).as_ref()? };
    let depth = state.depth.load(Ordering::Relaxed).checked_sub(1)?;
    let frame = state.frames.get(depth)?.load(Ordering::Relaxed);
    let address = state.returns.get(depth)?.load(Ordering::Relaxed);
    (frame != 0).then_some((frame, address))
}

/// After a C function returns: a worker that was replaced meanwhile makes
/// one worker retire.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_foreign_leave() {
    enter_tondo();
    let state = in_c();
    state.calls.fetch_add(1, Ordering::SeqCst);
    if state.depth.fetch_sub(1, Ordering::SeqCst) == 1 && state.replaced.swap(false, Ordering::SeqCst) {
        let mut queue = queue();
        queue.surplus += 1;
        wake_one(queue);
    }
}

/// Watches the workers in C: one still in the same call after a period
/// blocks its tasks, so when tasks wait to run and no worker is idle,
/// another worker starts in its place (as Go's runtime does). Sleeps while
/// no worker is in C.
fn monitor() {
    let mut last: HashMap<usize, usize> = HashMap::new();
    loop {
        std::thread::sleep(MONITOR_PERIOD);
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
            if last.insert(key, calls) == Some(calls) && !state.replaced.load(Ordering::SeqCst) {
                let mut queue = queue();
                if !queue.runnable.is_empty() && queue.idle == 0 {
                    state.replaced.store(true, Ordering::SeqCst);
                    queue.workers += 1;
                    drop(queue);
                    start_worker();
                }
            }
        }
        if !busy {
            MONITOR_ASLEEP.store(true, Ordering::SeqCst);
            let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if callers.iter().all(|state| state.depth.load(Ordering::SeqCst) == 0) {
                std::thread::park();
            }
            MONITOR_ASLEEP.store(false, Ordering::SeqCst);
        }
    }
}

/// Where every task starts: runs its body, completes its `Join` and
/// switches back to its worker for good.
extern "C" fn trampoline() -> ! {
    unsafe {
        let task = current();
        let (body, code, kind, join) = ((*task).body, (*task).code, (*task).kind, (*task).join);
        if !(*task).foreign.is_null() {
            let call = &*(*task).foreign;
            (call.inner)(call.frame);
            *call.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
            call.finished.notify_one();
        } else if (*task).main {
            let main: unsafe extern "C" fn() -> i32 = std::mem::transmute(code);
            MAIN_STATUS.store(main() as usize, Ordering::SeqCst);
            end_detached();
        } else {
            let captures = body.add(super::FN_HEADER);
            let out = (*join).result;
            match kind {
                ResultKind::Nothing => {
                    let f: unsafe extern "C" fn(*mut u8) = std::mem::transmute(code);
                    f(captures);
                }
                ResultKind::Integer => {
                    let f: unsafe extern "C" fn(*mut u8) -> u64 = std::mem::transmute(code);
                    let value = f(captures).to_le_bytes();
                    std::ptr::copy_nonoverlapping(value.as_ptr(), out, (*join).size.min(8));
                }
                ResultKind::Float64 => {
                    let f: unsafe extern "C" fn(*mut u8) -> f64 = std::mem::transmute(code);
                    (out as *mut f64).write_unaligned(f(captures));
                }
                ResultKind::Float32 => {
                    let f: unsafe extern "C" fn(*mut u8) -> f32 = std::mem::transmute(code);
                    (out as *mut f32).write_unaligned(f(captures));
                }
                ResultKind::Memory => {
                    let f: unsafe extern "C" fn(*mut u8, *mut u8) = std::mem::transmute(code);
                    f(out, captures);
                }
            }
            super::tondo_rt_fn_release(body as *mut super::FnObject);
            if complete(join) {
                let mut detached = DETACHED.lock().unwrap_or_else(|e| e.into_inner());
                detached.retain(|&j| j != join as usize);
                drop(detached);
                tondo_rt_join_release(join);
            }
        }
        (*task).finished = true;
        switch(&mut (*task).sp, *home());
    }
    runtime_fault("a finished task resumed")
}

/// Every task created and not yet freed: a panic cancels them, and a test
/// runner counts them before and after a test.
static TASKS: std::sync::LazyLock<Mutex<std::collections::HashSet<usize>>> = std::sync::LazyLock::new(Default::default);

fn tasks() -> std::sync::MutexGuard<'static, std::collections::HashSet<usize>> {
    TASKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// How many tasks are alive.
pub(crate) fn live_tasks() -> usize {
    tasks().len()
}

/// The tasks alive now.
pub(crate) fn alive() -> std::collections::HashSet<usize> {
    tasks().clone()
}

/// The tasks alive now that were not in `before`: where each waits, or
/// that it still runs, and where it was started.
pub(crate) fn leaked(before: &std::collections::HashSet<usize>) -> Vec<String> {
    // While the set is locked no task in it is freed.
    let alive = tasks();
    let mut found: Vec<usize> = alive.iter().copied().filter(|t| !before.contains(t)).collect();
    found.sort_unstable();
    found
        .into_iter()
        .map(|task| unsafe {
            let task = task as *mut Task;
            let origin = || super::trace::stack(super::trace::walk(0, (0, 0)), (*task).origin.as_deref(), "TITLE");
            match (*task).state.load(Ordering::Acquire) {
                PARKED => waiting_at(task).replacen("TITLE", "a task waits at", 1),
                PINNED => format!("\na task waits in a call to C{}", origin()),
                _ => format!("\na task still runs{}", origin()),
            }
        })
        .collect()
}

/// How long a panic waits for the other tasks to run their cleanups.
const PANIC_GRACE: Duration = Duration::from_secs(5);

/// The exit status of a panic or a test's skip that ends the program.
static ENDING: AtomicUsize = AtomicUsize::new(usize::MAX);

/// After a panic, or a test's skip, ran its task's cleanups (11.11): cancels
/// every other task, which runs its own `defer`s at its next wait, and
/// finishes this one. The process then ends with `status` once the main
/// task has finished and the other tasks with it, or after `PANIC_GRACE`.
pub(crate) fn end_all(status: i32) -> ! {
    abandon(status);
    let me = current();
    if me.is_null() {
        flush_stdout();
        super::process::cancel_live();
        std::process::exit(status)
    }
    unsafe {
        let join = (*me).join;
        if (*me).main {
            MAIN_STATUS.store(status as usize, Ordering::SeqCst);
        } else if !(*me).foreign.is_null() {
            // The thread of C that waits for the callback goes on.
            let call = &*(*me).foreign;
            *call.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
            call.finished.notify_one();
        } else if !join.is_null() {
            // Its join completes without a result, so whoever waits for
            // it goes on and nothing releases what it never made.
            if !(*join).result.is_null() {
                std::ptr::write_bytes((*join).result, 0, (*join).size);
            }
            (*join).taken = true;
            if complete(join) {
                DETACHED.lock().unwrap_or_else(|e| e.into_inner()).retain(|&j| j != join as usize);
                tondo_rt_join_release(join);
            }
        }
        (*me).finished = true;
        switch(&mut (*me).sp, *home());
    }
    runtime_fault("a finished task resumed")
}

/// Ends the program with `status` once its tasks have run their cleanups:
/// cancels every task but the running one, and ends the process anyway
/// after `PANIC_GRACE`.
pub(crate) fn abandon(status: i32) {
    let _ = ENDING.compare_exchange(usize::MAX, status as usize, Ordering::SeqCst, Ordering::SeqCst);
    static WATCHDOG: std::sync::Once = std::sync::Once::new();
    WATCHDOG.call_once(|| {
        spawn_thread(Some(64 * 1024), || {
            std::thread::sleep(PANIC_GRACE);
            flush_stdout();
            super::process::cancel_live();
            std::process::exit(ENDING.load(Ordering::SeqCst) as i32)
        });
    });
    let me = current();
    for &task in tasks().iter() {
        if task as *mut Task != me {
            unsafe { cancel_task(task as *mut Task) };
        }
    }
}

/// When the main task has finished after a panic: waits for the other
/// tasks to run their cleanups, and gives the panic's exit status.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn ending() -> Option<i32> {
    let status = ENDING.load(Ordering::SeqCst);
    if status == usize::MAX {
        return None;
    }
    let deadline = Instant::now() + PANIC_GRACE;
    while live_tasks() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    Some(status as i32)
}

fn new_task(stack: Stack, body: *mut u8, code: *const u8, kind: ResultKind, join: *mut JoinObject) -> *mut Task {
    let sp = unsafe { initial_stack(stack.top()) };
    let task = Box::into_raw(Box::new(Task {
        sp,
        stack: Some(stack),
        body,
        code,
        kind,
        join,
        cleanups: Vec::new(),
        state: AtomicU8::new(RUNNING),
        cancelled: AtomicBool::new(false),
        waiting: std::ptr::null(),
        waiting_lock: AtomicBool::new(false),
        held: Vec::new(),
        finished: false,
        main: join.is_null(),
        foreign: std::ptr::null(),
        // A task started on a virtual clock keeps it.
        in_domain: !current().is_null() && unsafe { (*current()).in_domain },
        origin: None,
    }));
    tasks().insert(task as usize);
    super::clock::woke(task);
    task
}

/// Runs the body of a callback C made: on this worker when C called back
/// inside a task, or as a new task while a thread of C waits for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_callback(inner: unsafe extern "C" fn(*mut u8), frame: *mut u8) {
    if !current().is_null() {
        // A call back from C on a worker: Tondo code again until it returns.
        enter_tondo();
        unsafe { inner(frame) };
        leave_tondo();
        return;
    }
    let call = ForeignCall { inner, frame, done: Mutex::new(false), finished: Condvar::new() };
    let task =
        new_task(task_stack(), std::ptr::null_mut(), std::ptr::null(), ResultKind::Nothing, std::ptr::null_mut());
    unsafe {
        (*task).main = false;
        (*task).foreign = &call;
        (*task).origin = Some(Arc::new(Origin::Foreign));
    }
    start_workers();
    FOREIGN_WAITING.fetch_add(1, Ordering::SeqCst);
    schedule(task);
    let mut done = call.done.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        done = call.finished.wait(done).unwrap_or_else(|e| e.into_inner());
    }
    FOREIGN_WAITING.fetch_sub(1, Ordering::SeqCst);
}

/// Runs `tondo_main` as the first task on the calling thread, which becomes
/// a worker; returns the program's exit status.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub fn run_main(code: unsafe extern "C" fn() -> i32) -> i32 {
    let task = new_task(
        Stack::new(MAIN_STACK),
        std::ptr::null_mut(),
        code as *const u8,
        ResultKind::Integer,
        std::ptr::null_mut(),
    );
    schedule(task);
    work(true)
}

// ---------------------------------------------------------------------------
// Cleanups
// ---------------------------------------------------------------------------

fn with_cleanups<R>(f: impl FnOnce(&mut Vec<(Cleanup, usize)>) -> R) -> R {
    let task = current();
    if task.is_null() { LOOSE_CLEANUPS.with(|c| f(&mut c.borrow_mut())) } else { f(unsafe { &mut (*task).cleanups }) }
}

pub fn push_cleanup(code: Cleanup, closure: *mut u8) {
    with_cleanups(|c| c.push((code, closure as usize)));
}

pub fn pop_cleanup() {
    with_cleanups(|c| c.pop());
}

/// Runs the current task's cleanups in LIFO order, each removed first so
/// that a panic inside one never runs it twice.
pub fn run_cleanups() {
    while let Some((code, closure)) = with_cleanups(|c| c.pop()) {
        unsafe { code(closure as *mut u8) };
    }
}

// ---------------------------------------------------------------------------
// Waits and locks
// ---------------------------------------------------------------------------

const NONE: usize = usize::MAX;
/// The index a cancellation claims a wait with.
pub(crate) const CANCELLED: usize = usize::MAX - 1;

/// A task's wait for the first of one or more operations. The objects it
/// waits on point to it until the task removes itself, and timers hold it,
/// since they outlive it.
pub(crate) struct Wait {
    pub(crate) task: *mut Task,
    fired: AtomicUsize,
}

// Other threads only claim the wait and notify its task.
unsafe impl Send for Wait {}
unsafe impl Sync for Wait {}

impl Wait {
    pub(crate) fn new() -> Arc<Wait> {
        Arc::new(Wait { task: current(), fired: AtomicUsize::new(NONE) })
    }

    /// Claims the wait for operation `index`; false when another operation
    /// did first. Whoever claims it notifies the task afterwards.
    pub(crate) fn claim(&self, index: usize) -> bool {
        self.fired.compare_exchange(NONE, index, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// Parks until the operation that claimed the wait notifies the task;
    /// returns its index, or `CANCELLED` if the task is cancelled first.
    pub(crate) fn block(&self) -> usize {
        let task = unsafe { &mut *self.task };
        lock(&task.waiting_lock);
        if task.cancelled.load(Ordering::Acquire) && self.claim(CANCELLED) {
            unlock(&task.waiting_lock);
            return CANCELLED;
        }
        task.waiting = self;
        unlock(&task.waiting_lock);
        let fired = self.block_fully();
        lock(&task.waiting_lock);
        task.waiting = std::ptr::null();
        unlock(&task.waiting_lock);
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

/// Whether the current task is cancelled.
pub(crate) fn cancelled() -> bool {
    let task = current();
    !task.is_null() && unsafe { (*task).cancelled.load(Ordering::Acquire) }
}

/// Checked by compiled code after every call that may wait: a cancelled
/// task unwinds from there. While exploring orders, the task gives way
/// there too.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cancelled() -> u8 {
    safe_point();
    if exploring().is_some() {
        give_way();
    }
    cancelled() as u8
}

/// `TONDO_RT_SCHEDULE`, a seed in hex (`tondo test --schedules`): the
/// tasks run on one worker, the next one is the seed's choice among those
/// that can run, and each gives way after every call that may wait, so
/// each seed meets the program's tasks in another order, and the same seed
/// in the same one.
fn exploring() -> Option<&'static AtomicU64> {
    static SEED: std::sync::OnceLock<Option<AtomicU64>> = std::sync::OnceLock::new();
    SEED.get_or_init(|| {
        let text = std::env::var("TONDO_RT_SCHEDULE").ok()?;
        u64::from_str_radix(&text, 16).ok().map(AtomicU64::new)
    })
    .as_ref()
}

/// The next number of the seed's sequence (SplitMix64).
fn seeded(state: &AtomicU64) -> u64 {
    let mut z = state.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Puts the running task back among those that can run, and lets the
/// worker choose again; not with C frames on its stack, which stay on
/// their thread.
fn give_way() {
    let task = current();
    if task.is_null() {
        return;
    }
    let in_c = IN_C.with(|c| c.get());
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

/// Cancels a running task: marks it and ends the wait it is blocked in.
unsafe fn cancel_task(task: *mut Task) {
    unsafe {
        let t = &mut *task;
        t.cancelled.store(true, Ordering::SeqCst);
        lock(&t.waiting_lock);
        let woken = !t.waiting.is_null() && (*t.waiting).claim(CANCELLED);
        unlock(&t.waiting_lock);
        if woken {
            notify(task);
        }
    }
}

/// A short spin lock over one object's state.
pub(crate) fn lock(flag: &AtomicBool) {
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
    if nanos <= 0 || cancelled() {
        return;
    }
    let wait = Wait::new();
    add_timer(Duration::from_nanos(nanos as u64), wait.clone(), 0);
    wait.block();
}

// ---------------------------------------------------------------------------
// Join
// ---------------------------------------------------------------------------

/// The handle `spawn` returns (11.11). `rc` counts the program's references;
/// the task itself only touches the object until it marks it done, and the
/// last reference waits for that before freeing it. The result lives here
/// until `wait` moves it out.
#[repr(C)]
pub struct JoinObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    done: bool,
    taken: bool,
    waiters: Vec<(*const Wait, usize)>,
    result: *mut u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    /// The group that observes the task's end, and its index there.
    group: *mut GroupObject,
    index: usize,
    /// The task, alive while it is not done.
    task: *mut Task,
    /// Let go with `detach()`: the task holds a reference until it ends.
    detached: bool,
    /// Visits the result for the cycle collector, when it may refer back
    /// to the join, which is then registered with it.
    visit: Option<Visit>,
}

/// Starts a task that calls the closure `body`, a `fn` object it takes,
/// and returns its `Join`. `kind`, `size` and `align` describe the result;
/// `release` frees one that is never taken.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_spawn(
    body: *mut u8,
    kind: u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    visit: Option<Visit>,
) -> *mut JoinObject {
    start_workers();
    let align = align.max(8);
    let result = if size == 0 {
        std::ptr::null_mut()
    } else {
        unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, align).unwrap()) }
    };
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let join = Box::into_raw(Box::new(JoinObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        done: false,
        taken: false,
        waiters: Vec::new(),
        result,
        size,
        align,
        release,
        group: std::ptr::null_mut(),
        index: 0,
        task: std::ptr::null_mut(),
        detached: false,
        visit,
    }));
    if visit.is_some() {
        cycles::remember(join as *const u8, Kind::Join);
    }
    let code = unsafe { *(body.add(16) as *const *const u8) };
    let kind = match kind {
        0 => ResultKind::Nothing,
        1 => ResultKind::Integer,
        2 => ResultKind::Float64,
        3 => ResultKind::Float32,
        _ => ResultKind::Memory,
    };
    let task = new_task(task_stack(), body, code, kind, join);
    unsafe {
        (*join).task = task;
        (*task).origin = Some(super::trace::spawned(origin()));
    }
    schedule(task);
    join
}

/// Marks the task done and notifies whoever waits for it. The group hears
/// of it under the join's lock, so nobody sees the task done, and frees the
/// group, before that. True when the task was detached.
unsafe fn complete(join: *mut JoinObject) -> bool {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        j.done = true;
        let detached = j.detached;
        let mut woken: Vec<*mut Task> = Vec::new();
        for (wait, index) in std::mem::take(&mut j.waiters) {
            if (*wait).claim(index) {
                woken.push((*wait).task);
            }
        }
        if !j.group.is_null() {
            woken.extend(group_completed(j.group, j.index));
        }
        unlock(&j.lock);
        woken.into_iter().for_each(notify);
        detached
    }
}

/// The joins of detached tasks that are still running.
static DETACHED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `job.detach()`: the task runs on without anyone waiting for it, and
/// holds its join until it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_detach(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        let mut detached = DETACHED.lock().unwrap_or_else(|e| e.into_inner());
        lock(&j.lock);
        if !j.done {
            j.detached = true;
            j.rc.fetch_add(1, Ordering::Relaxed);
            detached.push(join as usize);
        }
        unlock(&j.lock);
    }
}

/// When `main` returns: cancels the detached tasks still running and waits
/// for their cleanup (11.11), including tasks they detach meanwhile.
fn end_detached() {
    loop {
        let joins: Vec<usize> = {
            let detached = DETACHED.lock().unwrap_or_else(|e| e.into_inner());
            for &join in detached.iter() {
                unsafe { (*(join as *mut JoinObject)).rc.fetch_add(1, Ordering::Relaxed) };
            }
            detached.clone()
        };
        if joins.is_empty() {
            return;
        }
        for join in joins {
            unsafe {
                cancel_join(join as *mut JoinObject);
                tondo_rt_join_release(join as *mut JoinObject);
            }
        }
    }
}

/// Waits until the task is done; false if the waiting task is cancelled
/// first, unless `fully`.
unsafe fn finish(join: *mut JoinObject, fully: bool) -> bool {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        if j.done {
            unlock(&j.lock);
            return true;
        }
        let wait = Wait::new();
        j.waiters.push((Arc::as_ptr(&wait), 0));
        unlock(&j.lock);
        if fully {
            wait.block_fully();
            return true;
        }
        if wait.block() != CANCELLED {
            return true;
        }
        lock(&j.lock);
        j.waiters.retain(|&(w, _)| w != Arc::as_ptr(&wait));
        unlock(&j.lock);
        false
    }
}

/// Cancels the task of a join and waits until it is done (11.15).
unsafe fn cancel_join(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        if !j.done {
            cancel_task(j.task);
        }
        unlock(&j.lock);
        finish(join, true);
    }
}

/// Moves the result out of a finished task into `out`.
unsafe fn take(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        if (*join).size > 0 {
            std::ptr::copy_nonoverlapping((*join).result, out, (*join).size);
        }
        (*join).taken = true;
    }
}

/// `job.wait()`: waits for the task and moves its result into `out`. The
/// handle is released as usual by its owner. A cancelled waiter gets zeros.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_wait(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        if !cancelled() && finish(join, false) {
            take(join, out);
        } else {
            std::ptr::write_bytes(out, 0, (*join).size);
        }
    }
}

/// `job.wait(timeout: delay)`: the result into `out` and 1, or 0 after
/// cancelling the task when it does not finish within `nanos`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_wait_timeout(join: *mut JoinObject, out: *mut u8, nanos: i64) -> u8 {
    let mut cases = [
        Case { kind: WAIT, found: 0, handle: join as *mut u8, value: out, nanos: 0 },
        Case { kind: SLEEP, found: 0, handle: std::ptr::null_mut(), value: std::ptr::null_mut(), nanos },
    ];
    match unsafe { select(&mut cases, true, std::ptr::null()) } {
        0 => 1,
        1 => {
            unsafe { cancel_join(join) };
            0
        }
        _ => 0,
    }
}

/// `job.cancel()`: cancels the task and waits until it is done.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_cancel(join: *mut JoinObject) {
    unsafe { cancel_join(join) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_handle_retain(handle: *mut u8) {
    unsafe { (*(handle as *const AtomicUsize)).fetch_add(1, Ordering::Relaxed) };
}

/// Releases a `Join`: the last reference waits for a task nobody waited for
/// (11.11) and frees its result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_release(join: *mut JoinObject) {
    if join.is_null() {
        return;
    }
    unsafe {
        if (*join).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        free_join(join);
    }
}

/// Visits a finished task's result for the cycle collector; a task still
/// running holds its join.
pub(crate) unsafe fn visit_join(visitor: &mut Visitor, join: *mut u8) {
    let j = unsafe { &*(join as *const JoinObject) };
    if !j.done {
        visitor.hold();
    } else if !j.taken {
        unsafe { visitor.visit(j.visit, j.result) };
    }
}

/// Takes the result out of a garbage join.
pub(crate) unsafe fn clear_join(join: *mut u8) -> Option<Taken> {
    let j = unsafe { &mut *(join as *mut JoinObject) };
    if !j.done || j.taken {
        return None;
    }
    let release = j.release?;
    j.taken = true;
    let mut result = vec![0u64; j.size.div_ceil(8).max(1)];
    unsafe { std::ptr::copy_nonoverlapping(j.result, result.as_mut_ptr() as *mut u8, j.size) };
    Some(Box::new(move || unsafe { release(result.as_mut_ptr() as *mut u8) }))
}

/// Visits a group's joins for the cycle collector; tasks still running
/// hold their group.
pub(crate) unsafe fn visit_group(visitor: &mut Visitor, group: *mut u8) {
    let g = unsafe { &*(group as *const GroupObject) };
    if g.running > 0 {
        visitor.hold();
    }
    for &join in &g.joins {
        visitor.edge(join as *const u8, Kind::Join, 0, 0);
    }
}

unsafe fn free_join(join: *mut JoinObject) {
    unsafe {
        if (*join).visit.is_some() {
            cycles::forget(join as *const u8);
        }
        // A cancelled task cancels the tasks it still owns (11.15).
        if cancelled() {
            cancel_join(join);
        }
        finish(join, true);
        if !(*join).taken
            && let Some(release) = (*join).release
        {
            release((*join).result);
        }
        if !(*join).result.is_null() {
            let layout = std::alloc::Layout::from_size_align((*join).size, (*join).align).unwrap();
            std::alloc::dealloc((*join).result, layout);
        }
        drop(Box::from_raw(join));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

/// `Group[T]` (11.12): the joins added, in order, which tasks finished and
/// `next` has not returned yet, and, when `T` is a result, the first task
/// that finished with an error.
#[repr(C)]
pub struct GroupObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    fallible: bool,
    joins: Vec<*mut JoinObject>,
    taken: Vec<bool>,
    finished: VecDeque<usize>,
    running: usize,
    failed: Option<usize>,
    waiter: Option<(*const Wait, usize)>,
}

/// `Group.new[T]()`; `fallible` when `T` is `R ! E`, whose results start
/// with their tag, `1` for `err`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_group_new(fallible: u8) -> *mut GroupObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(GroupObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        fallible: fallible != 0,
        joins: Vec::new(),
        taken: Vec::new(),
        finished: VecDeque::new(),
        running: 0,
        failed: None,
        waiter: None,
    }))
}

/// `group.add(job)`: the group takes over the join's reference, which the
/// caller retained for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_add(group: *mut GroupObject, join: *mut JoinObject) {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        let index = g.joins.len();
        g.joins.push(join);
        g.taken.push(false);
        g.running += 1;
        unlock(&g.lock);
        let j = &mut *join;
        lock(&j.lock);
        let woken = if j.done {
            group_completed(group, index)
        } else {
            j.group = group;
            j.index = index;
            None
        };
        unlock(&j.lock);
        woken.into_iter().for_each(notify);
    }
}

/// Records that the task at `index` finished; called under its join's
/// lock. Returns the task to notify, if one waits.
unsafe fn group_completed(group: *mut GroupObject, index: usize) -> Option<*mut Task> {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        g.finished.push_back(index);
        g.running -= 1;
        let join = g.joins[index];
        if g.fallible && g.failed.is_none() && *(*join).result != 0 {
            g.failed = Some(index);
        }
        let woken = g.waiter.take().filter(|&(wait, i)| (*wait).claim(i)).map(|(wait, _)| (*wait).task);
        unlock(&g.lock);
        woken
    }
}

/// Waits, with the group locked, until a task finishes; returns locked,
/// and false if the waiting task is cancelled.
unsafe fn group_block(g: &mut GroupObject) -> bool {
    let wait = Wait::new();
    g.waiter = Some((Arc::as_ptr(&wait), 0));
    unlock(&g.lock);
    let fired = wait.block();
    lock(&g.lock);
    if g.waiter.is_some_and(|(w, _)| w == Arc::as_ptr(&wait)) {
        g.waiter = None;
    }
    fired != CANCELLED
}

/// `group.length()`: the tasks whose results the group still holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_length(group: *mut GroupObject) -> i64 {
    unsafe {
        let g = &*group;
        lock(&g.lock);
        let count = g.taken.iter().filter(|t| !**t).count();
        unlock(&g.lock);
        count as i64
    }
}

/// Takes the next finished result into `out`, with the group locked:
/// `Some(true)` when there was one, `Some(false)` when no task is left, and
/// `None` when it must wait.
unsafe fn group_try_next(g: &mut GroupObject, out: *mut u8) -> Option<bool> {
    if g.taken.iter().all(|t| *t) {
        return Some(false);
    }
    while let Some(index) = g.finished.pop_front() {
        if !g.taken[index] {
            g.taken[index] = true;
            unsafe { take(g.joins[index], out) };
            return Some(true);
        }
    }
    None
}

/// `group.next()`: moves the result of the next task to finish into `out`
/// and returns 1, or returns 0 when no task is left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_next(group: *mut GroupObject, out: *mut u8) -> u8 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        loop {
            if let Some(found) = group_try_next(g, out).filter(|_| !cancelled()) {
                unlock(&g.lock);
                return found as u8;
            }
            if cancelled() || !group_block(g) {
                unlock(&g.lock);
                return 0;
            }
        }
    }
}

/// The first part of `group.wait()`: waits until every task finished or,
/// for results, one finished with an error, which cancels the others;
/// returns the index of that task, -1, or -2 if the waiting task is
/// cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_settle(group: *mut GroupObject) -> i64 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        loop {
            if cancelled() {
                unlock(&g.lock);
                return -2;
            }
            if let Some(index) = g.failed.filter(|&i| !g.taken[i]) {
                unlock(&g.lock);
                tondo_rt_group_cancel(group);
                return index as i64;
            }
            if g.running == 0 {
                unlock(&g.lock);
                return -1;
            }
            if !group_block(g) {
                unlock(&g.lock);
                return -2;
            }
        }
    }
}

/// `group.cancel()`: cancels the tasks still running and waits for them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_cancel(group: *mut GroupObject) {
    unsafe {
        for index in 0..(*group).joins.len() {
            cancel_join((&(*group).joins)[index]);
        }
    }
}

/// Moves the result of the finished task at `index`, or of the first one
/// not taken yet when `index` is -1, into `out` and returns 1; returns 0
/// when every result was taken.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_take(group: *mut GroupObject, index: i64, out: *mut u8) -> u8 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        let found = if index >= 0 { Some(index as usize) } else { g.taken.iter().position(|t| !*t) };
        let Some(index) = found else {
            unlock(&g.lock);
            return 0;
        };
        g.taken[index] = true;
        let join = g.joins[index];
        unlock(&g.lock);
        finish(join, true);
        take(join, out);
        1
    }
}

/// Releases a `Group`: the last reference waits for its remaining tasks
/// and frees their results.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_release(group: *mut GroupObject) {
    if group.is_null() {
        return;
    }
    unsafe {
        if (*group).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        // Each join finishes before it is freed, and with it the last use
        // of the group by its task; tasks still running read `joins`.
        for index in 0..(*group).joins.len() {
            free_join((&(*group).joins)[index]);
        }
        drop(Box::from_raw(group));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// A task waiting on a channel: the value it sends, or where the value it
/// receives goes, and how the wait ended.
struct Waiter {
    wait: *const Wait,
    index: usize,
    value: *mut u8,
    state: u8,
}

const MOVED: u8 = 1;
const CLOSED: u8 = 2;

/// A channel (11.13). `rc` counts the `Receiver` handles and the sender
/// objects; `senders` counts the latter, and the channel closes when it
/// reaches zero. A receiver only waits while the buffer is empty, and a
/// sender only while it is full.
#[repr(C)]
pub struct ChannelObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    closed: bool,
    senders: usize,
    /// `usize::MAX` when unbounded; 0 for a rendezvous.
    capacity: usize,
    size: usize,
    align: usize,
    release: Option<Glue>,
    /// Visits a value for the cycle collector, when it may refer back to
    /// the channel, which is then registered with it.
    visit: Option<Visit>,
    /// The buffered values' bytes, `count` values of `size` bytes each.
    buffer: VecDeque<u8>,
    count: usize,
    receiving: VecDeque<*mut Waiter>,
    sending: VecDeque<*mut Waiter>,
}

/// A `Sender` handle: one per `channel` or `copy()`, shared by the
/// program's references to it.
#[repr(C)]
pub struct SenderObject {
    rc: AtomicUsize,
    channel: *mut ChannelObject,
}

/// `channel[T](capacity)` without its sender: the `Receiver`. A negative
/// capacity panics with `P0001`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_new(
    capacity: i64,
    bounded: u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    visit: Option<Visit>,
    location: *const Str,
) -> *mut ChannelObject {
    if bounded != 0 && capacity < 0 {
        super::panic_with(
            1,
            &format!("a channel cannot hold {capacity} values: its capacity must not be negative"),
            location,
        );
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let channel = Box::into_raw(Box::new(ChannelObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        closed: false,
        senders: 0,
        capacity: if bounded != 0 { capacity as usize } else { usize::MAX },
        size,
        align: align.max(1),
        release,
        visit,
        buffer: VecDeque::new(),
        count: 0,
        receiving: VecDeque::new(),
        sending: VecDeque::new(),
    }));
    if visit.is_some() {
        cycles::remember(channel as *const u8, Kind::Channel);
    }
    channel
}

/// Visits the buffered values for the cycle collector.
pub(crate) unsafe fn visit_channel(visitor: &mut Visitor, channel: *mut u8) {
    let c = unsafe { &*(channel as *const ChannelObject) };
    let mut value = vec![0u64; c.size.div_ceil(8).max(1)];
    for index in 0..c.count {
        let bytes = c.buffer.range(index * c.size..(index + 1) * c.size);
        for (at, &byte) in bytes.enumerate() {
            unsafe { *(value.as_mut_ptr() as *mut u8).add(at) = byte };
        }
        unsafe { visitor.visit(c.visit, value.as_mut_ptr() as *mut u8) };
    }
}

/// The channel a sender sends to, for the cycle collector.
pub(crate) unsafe fn visit_sender(visitor: &mut Visitor, sender: *mut u8) {
    let channel = unsafe { (*(sender as *const SenderObject)).channel };
    visitor.edge(channel as *const u8, Kind::Channel, 0, 0);
}

/// Takes the buffered values out of a garbage channel.
pub(crate) unsafe fn clear_channel(channel: *mut u8) -> Option<Taken> {
    let c = unsafe { &mut *(channel as *mut ChannelObject) };
    let release = c.release?;
    let (mut buffer, count, size) = (std::mem::take(&mut c.buffer), std::mem::take(&mut c.count), c.size);
    Some(Box::new(move || {
        let mut value = vec![0u64; size.div_ceil(8).max(1)];
        for _ in 0..count {
            pop_bytes(&mut buffer, value.as_mut_ptr() as *mut u8, size);
            unsafe { release(value.as_mut_ptr() as *mut u8) };
        }
    }))
}

/// A new `Sender` of the channel: the one `channel` returns, or `copy()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_new(channel: *mut ChannelObject) -> *mut SenderObject {
    unsafe {
        let c = &mut *channel;
        lock(&c.lock);
        c.senders += 1;
        unlock(&c.lock);
        c.rc.fetch_add(1, Ordering::Relaxed);
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(SenderObject { rc: AtomicUsize::new(1), channel }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_copy(sender: *mut SenderObject) -> *mut SenderObject {
    unsafe { tondo_rt_sender_new((*sender).channel) }
}

/// Moves the value at the front of the buffer, `size` bytes, into `out`.
fn pop_bytes(buffer: &mut VecDeque<u8>, out: *mut u8, size: usize) {
    let (front, back) = buffer.as_slices();
    let first = front.len().min(size);
    unsafe {
        std::ptr::copy_nonoverlapping(front.as_ptr(), out, first);
        std::ptr::copy_nonoverlapping(back.as_ptr(), out.add(first), size - first);
    }
    buffer.drain(..size);
}

/// The first waiter of `queue` whose wait it can still claim, removed;
/// waiters another operation claimed first are dropped.
unsafe fn claim_first(queue: &mut VecDeque<*mut Waiter>) -> Option<*mut Waiter> {
    while let Some(waiter) = queue.pop_front() {
        if unsafe { (*(*waiter).wait).claim((*waiter).index) } {
            return Some(waiter);
        }
    }
    None
}

/// Ends the waits of a channel that just closed; returns who to notify.
unsafe fn close_waits(c: &mut ChannelObject) -> Vec<*mut Task> {
    c.closed = true;
    let mut woken = Vec::new();
    for waiter in c.receiving.drain(..).chain(c.sending.drain(..)) {
        unsafe {
            if (*(*waiter).wait).claim((*waiter).index) {
                (*waiter).state = CLOSED;
                woken.push((*(*waiter).wait).task);
            }
        }
    }
    woken
}

fn closed_panic(location: *const Str) -> ! {
    super::panic_with(12, "cannot send on this channel: it was closed", location)
}

/// Moves the value at `value` into the channel if it can without waiting,
/// with the channel locked: `Some(task)` with a receiver to notify, or
/// `None` when it must wait.
unsafe fn try_send(c: &mut ChannelObject, value: *mut u8) -> Option<Option<*mut Task>> {
    unsafe {
        if let Some(receiver) = claim_first(&mut c.receiving) {
            std::ptr::copy_nonoverlapping(value, (*receiver).value, c.size);
            (*receiver).state = MOVED;
            return Some(Some((*(*receiver).wait).task));
        }
        if c.count < c.capacity {
            c.buffer.extend(std::slice::from_raw_parts(value, c.size));
            c.count += 1;
            return Some(None);
        }
        None
    }
}

/// Takes the next value into `out` if it can without waiting, with the
/// channel locked: `Some((found, task))`, where `found` is false once the
/// channel is closed and empty and `task` is a sender to notify.
unsafe fn try_receive(c: &mut ChannelObject, out: *mut u8) -> Option<(bool, Option<*mut Task>)> {
    unsafe {
        if c.count > 0 {
            pop_bytes(&mut c.buffer, out, c.size);
            c.count -= 1;
            // A sender waiting for room moves its value in.
            let woken = claim_first(&mut c.sending).map(|sender| {
                c.buffer.extend(std::slice::from_raw_parts((*sender).value, c.size));
                c.count += 1;
                (*sender).state = MOVED;
                (*(*sender).wait).task
            });
            return Some((true, woken));
        }
        // A rendezvous: take the value from a waiting sender.
        if let Some(sender) = claim_first(&mut c.sending) {
            std::ptr::copy_nonoverlapping((*sender).value, out, c.size);
            (*sender).state = MOVED;
            return Some((true, Some((*(*sender).wait).task)));
        }
        if c.closed {
            return Some((false, None));
        }
        None
    }
}

/// `sender.send(value)`: moves the value at `value` into the channel,
/// waiting while it is full; panics with `P0012` if it is closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_send(sender: *mut SenderObject, value: *mut u8, location: *const Str) {
    unsafe {
        let c = &mut *(*sender).channel;
        if cancelled() {
            drop_unsent(c, value);
            return;
        }
        lock(&c.lock);
        if c.closed {
            unlock(&c.lock);
            closed_panic(location);
        }
        if let Some(woken) = try_send(c, value) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return;
        }
        let wait = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&wait), index: 0, value, state: 0 };
        c.sending.push_back(&mut waiter);
        unlock(&c.lock);
        if wait.block() == CANCELLED {
            lock(&c.lock);
            c.sending.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            drop_unsent(c, value);
            return;
        }
        if std::ptr::read_volatile(&waiter.state) == CLOSED {
            closed_panic(location);
        }
    }
}

/// `std.internal.channel`'s `offer` (`wait` 1) and `tryOffer`: sends the
/// value, waiting for room only when `wait` is set, and gives 0 when sent,
/// 1 when the channel is full and it did not wait, 2 once the channel is
/// closed or the task cancelled. A value not sent is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_offer(sender: *mut SenderObject, value: *mut u8, wait: u8) -> i64 {
    const SENT: i64 = 0;
    const FULL: i64 = 1;
    const SHUT: i64 = 2;
    unsafe {
        let c = &mut *(*sender).channel;
        if cancelled() {
            drop_unsent(c, value);
            return SHUT;
        }
        lock(&c.lock);
        if c.closed {
            unlock(&c.lock);
            drop_unsent(c, value);
            return SHUT;
        }
        if let Some(woken) = try_send(c, value) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return SENT;
        }
        if wait == 0 {
            unlock(&c.lock);
            drop_unsent(c, value);
            return FULL;
        }
        let blocked = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&blocked), index: 0, value, state: 0 };
        c.sending.push_back(&mut waiter);
        unlock(&c.lock);
        if blocked.block() == CANCELLED {
            lock(&c.lock);
            c.sending.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            drop_unsent(c, value);
            return SHUT;
        }
        if std::ptr::read_volatile(&waiter.state) == CLOSED {
            drop_unsent(c, value);
            return SHUT;
        }
        SENT
    }
}

/// A value a cancelled task did not send: it was the channel's to release.
unsafe fn drop_unsent(c: &ChannelObject, value: *mut u8) {
    if let Some(release) = c.release {
        unsafe { release(value) };
    }
}

/// `receiver.receive()`: moves the next value into `out` and returns 1, or
/// returns 0 once the channel is closed and empty, or if the task is
/// cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_receive(channel: *mut ChannelObject, out: *mut u8) -> u8 {
    unsafe {
        let c = &mut *channel;
        if cancelled() {
            return 0;
        }
        lock(&c.lock);
        if let Some((found, woken)) = try_receive(c, out) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return found as u8;
        }
        let wait = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&wait), index: 0, value: out, state: 0 };
        c.receiving.push_back(&mut waiter);
        unlock(&c.lock);
        if wait.block() == CANCELLED {
            lock(&c.lock);
            c.receiving.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            return 0;
        }
        (std::ptr::read_volatile(&waiter.state) == MOVED) as u8
    }
}

/// `receiver.receive(timeout: delay)`: like `receive`, but gives 0 if
/// nothing arrives within `nanos`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_receive_timeout(channel: *mut ChannelObject, out: *mut u8, nanos: i64) -> u8 {
    let mut cases = [
        Case { kind: RECEIVE, found: 0, handle: channel as *mut u8, value: out, nanos: 0 },
        Case { kind: SLEEP, found: 0, handle: std::ptr::null_mut(), value: std::ptr::null_mut(), nanos },
    ];
    let won = unsafe { select(&mut cases, true, std::ptr::null()) };
    (won == 0 && cases[0].found != 0) as u8
}

/// `sender.close()`: later sends panic, and receivers get what is buffered
/// and then `none`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_close(sender: *mut SenderObject) {
    unsafe {
        let c = &mut *(*sender).channel;
        lock(&c.lock);
        let woken = if c.closed { Vec::new() } else { close_waits(c) };
        unlock(&c.lock);
        woken.into_iter().for_each(notify);
    }
}

/// `isClosed()` of a `Sender` (`sender` is 1) or a `Receiver`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_closed(handle: *mut u8, sender: u8) -> u8 {
    unsafe {
        let c = if sender != 0 { &*(*(handle as *mut SenderObject)).channel } else { &*(handle as *mut ChannelObject) };
        lock(&c.lock);
        let closed = c.closed;
        unlock(&c.lock);
        closed as u8
    }
}

/// Releases a `Sender`; the channel closes when its last one goes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_release(sender: *mut SenderObject) {
    if sender.is_null() {
        return;
    }
    unsafe {
        if (*sender).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let sender = Box::from_raw(sender);
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
        let c = &mut *sender.channel;
        lock(&c.lock);
        c.senders -= 1;
        let woken = if c.senders == 0 && !c.closed { close_waits(c) } else { Vec::new() };
        unlock(&c.lock);
        woken.into_iter().for_each(notify);
        tondo_rt_receiver_release(sender.channel);
    }
}

/// Releases a `Receiver`, or a sender's hold on its channel; the last one
/// frees the channel and the values still buffered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_receiver_release(channel: *mut ChannelObject) {
    if channel.is_null() {
        return;
    }
    unsafe {
        if (*channel).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        if (*channel).visit.is_some() {
            cycles::forget(channel as *const u8);
        }
        let mut c = Box::from_raw(channel);
        if let Some(release) = c.release
            && c.size > 0
        {
            let layout = std::alloc::Layout::from_size_align(c.size, c.align).unwrap();
            let value = std::alloc::alloc(layout);
            while c.count > 0 {
                pop_bytes(&mut c.buffer, value, c.size);
                c.count -= 1;
                release(value);
            }
            std::alloc::dealloc(value, layout);
        }
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// select
// ---------------------------------------------------------------------------

const RECEIVE: u8 = 0;
const SEND: u8 = 1;
const WAIT: u8 = 2;
const NEXT: u8 = 3;
const SLEEP: u8 = 4;

/// One operation of a `select` (11.14), as compiled code lays it out:
/// `handle` is the receiver's channel, the sender, the join or the group;
/// `value` the value sent, or where a received value or a result goes;
/// `found` tells whether `receive` or `next` produced one.
#[repr(C)]
pub struct Case {
    kind: u8,
    found: u8,
    handle: *mut u8,
    value: *mut u8,
    nanos: i64,
}

impl Case {
    /// The lock of the object the operation waits on.
    unsafe fn lock(&self) -> Option<*const AtomicBool> {
        unsafe {
            match self.kind {
                RECEIVE => Some(&raw const (*(self.handle as *mut ChannelObject)).lock),
                SEND => Some(&raw const (*(*(self.handle as *mut SenderObject)).channel).lock),
                WAIT => Some(&raw const (*(self.handle as *mut JoinObject)).lock),
                NEXT => Some(&raw const (*(self.handle as *mut GroupObject)).lock),
                _ => None,
            }
        }
    }
}

/// A small generator for the random choice among ready operations; the
/// seed's, while exploring orders.
fn random(bound: usize) -> usize {
    if let Some(seed) = exploring() {
        return (seeded(seed) % bound as u64) as usize;
    }
    static STATE: AtomicUsize = AtomicUsize::new(0x9e37_79b9_7f4a_7c15);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x % bound
}

/// `select` over `count` operations at `cases`: completes the first that
/// is ready, chosen at random among those ready, and returns its index;
/// with `blocking` 0 (an `else` arm) returns -1 when none is, and -2 when
/// the task is cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_select(cases: *mut Case, count: usize, blocking: u8, location: *const Str) -> i64 {
    let cases = unsafe { std::slice::from_raw_parts_mut(cases, count) };
    unsafe { select(cases, blocking != 0, location) }
}

unsafe fn select(cases: &mut [Case], blocking: bool, location: *const Str) -> i64 {
    if cancelled() {
        return -2;
    }
    unsafe {
        // Every object is locked, once and in address order, from checking
        // readiness until the task is registered everywhere.
        let mut locks: Vec<*const AtomicBool> = cases.iter().filter_map(|c| c.lock()).collect();
        locks.sort();
        locks.dedup();
        let lock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| lock(&**l));
        let unlock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| unlock(&**l));
        lock_all(&locks);
        let mut order: Vec<usize> = (0..cases.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, random(i + 1));
        }
        for &index in &order {
            let case = &mut cases[index];
            if case.kind == SEND && (*(*(case.handle as *mut SenderObject)).channel).closed {
                unlock_all(&locks);
                closed_panic(location);
            }
            if let Some(woken) = try_case(case) {
                unlock_all(&locks);
                woken.into_iter().for_each(notify);
                return index as i64;
            }
        }
        if !blocking {
            unlock_all(&locks);
            return -1;
        }
        let wait = Wait::new();
        let mut waiters: Vec<Waiter> = cases
            .iter()
            .enumerate()
            .map(|(index, case)| Waiter { wait: Arc::as_ptr(&wait), index, value: case.value, state: 0 })
            .collect();
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.push_back(waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.push_back(waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.push((Arc::as_ptr(&wait), index)),
                NEXT => (*(case.handle as *mut GroupObject)).waiter = Some((Arc::as_ptr(&wait), index)),
                _ => add_timer(Duration::from_nanos(case.nanos as u64), wait.clone(), index),
            }
        }
        unlock_all(&locks);
        let won = wait.block();
        // Leave every queue the wait is still in.
        lock_all(&locks);
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.retain(|w| *w != waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.retain(|w| *w != waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.retain(|&(w, _)| w != Arc::as_ptr(&wait)),
                NEXT => {
                    let group = &mut *(case.handle as *mut GroupObject);
                    if group.waiter.is_some_and(|(w, _)| w == Arc::as_ptr(&wait)) {
                        group.waiter = None;
                    }
                }
                _ => {}
            }
        }
        let sleeps = cases.iter().enumerate().filter(|&(i, c)| c.kind == SLEEP && i != won).count();
        if won == CANCELLED {
            unlock_all(&locks);
            timers_ended(sleeps);
            return -2;
        }
        // The operation that won is complete but for what this task does.
        let case = &mut cases[won];
        match case.kind {
            RECEIVE => case.found = (waiters[won].state == MOVED) as u8,
            SEND if waiters[won].state == CLOSED => {
                unlock_all(&locks);
                closed_panic(location);
            }
            WAIT => take(case.handle as *mut JoinObject, case.value),
            NEXT => {
                let group = &mut *(case.handle as *mut GroupObject);
                case.found = group_try_next(group, case.value).unwrap_or(false) as u8;
            }
            _ => {}
        }
        unlock_all(&locks);
        if sleeps > 0 {
            timers_ended(sleeps);
        }
        won as i64
    }
}

/// Completes one operation of a `select` if it is ready, with its object
/// locked; returns a task to notify, if any.
unsafe fn try_case(case: &mut Case) -> Option<Option<*mut Task>> {
    unsafe {
        match case.kind {
            RECEIVE => {
                let c = &mut *(case.handle as *mut ChannelObject);
                let (found, woken) = try_receive(c, case.value)?;
                case.found = found as u8;
                Some(woken)
            }
            SEND => try_send(&mut *(*(case.handle as *mut SenderObject)).channel, case.value),
            WAIT => {
                let join = case.handle as *mut JoinObject;
                if !(*join).done {
                    return None;
                }
                take(join, case.value);
                Some(None)
            }
            NEXT => {
                let group = &mut *(case.handle as *mut GroupObject);
                case.found = group_try_next(group, case.value)? as u8;
                Some(None)
            }
            _ => (case.nanos <= 0).then_some(None),
        }
    }
}
