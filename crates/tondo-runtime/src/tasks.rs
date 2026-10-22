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

use super::{Glue, LIVE_OBJECTS, Str, flush_stdout, runtime_fault, tondo_rt_panic};
use std::cell::Cell;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
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
const GUARD: usize = 64 * 1024;
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
    /// The `std.sync` locks it holds, and whether exclusively.
    pub(crate) held: Vec<(usize, bool)>,
    finished: bool,
    main: bool,
    /// For a task that runs a callback for a thread of C, the call.
    foreign: *const ForeignCall,
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

#[inline(never)]
fn home() -> *mut *mut u8 {
    HOME.with(|h| h.as_ptr())
}

fn queue() -> std::sync::MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes a new or notified task runnable.
fn schedule(task: *mut Task) {
    queue().runnable.push_back(task);
    READY.notify_one();
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
    queue().pinned.retain(|&t| t != task);
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
        || queue.idle + pinned.count() < queue.workers
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
    let mut queue = queue();
    queue.sequence += 1;
    let sequence = queue.sequence;
    queue.timers.push(Timer { at: Instant::now() + delay, sequence, wait, index });
    drop(queue);
    // An idle worker recomputes how long to sleep.
    READY.notify_one();
}

/// Runs tasks; on the main thread, until the main task finishes, wherever
/// it ran, and then returns its exit status.
fn work(main: bool) -> i32 {
    loop {
        let task = {
            let mut queue = queue();
            loop {
                if main && MAIN_DONE.load(Ordering::SeqCst) {
                    return MAIN_STATUS.load(Ordering::SeqCst) as i32;
                }
                if !main && queue.surplus > 0 {
                    queue.surplus -= 1;
                    queue.workers -= 1;
                    return 0;
                }
                fire_timers(&mut queue);
                if let Some(task) = queue.runnable.pop_front() {
                    break task;
                }
                queue.idle += 1;
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
        unsafe { switch(home(), (*task).sp) };
        CURRENT.with(|c| c.set(std::ptr::null_mut()));
        unsafe {
            if (*task).finished {
                let mut task = Box::from_raw(task);
                let was_main = task.main;
                if let Some(stack) = task.stack.take() {
                    recycle(stack);
                }
                drop(task);
                if was_main {
                    MAIN_DONE.store(true, Ordering::SeqCst);
                    // Take the queue's lock so the main thread cannot miss
                    // the notification between its check and its wait.
                    drop(queue());
                    READY.notify_all();
                }
            } else if (*task).state.compare_exchange(PARKING, PARKED, Ordering::AcqRel, Ordering::Acquire).is_err() {
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
    let _ = writeln!(std::io::stderr(), "panic[P0013]: deadlock: every task is waiting");
    std::process::exit(super::PANIC_EXIT_CODE)
}

/// Starts the other workers, one per core, the first time a task spawns.
fn start_workers() {
    if STARTED.swap(true, Ordering::SeqCst) {
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
/// entries and exits that changes whenever a call starts or ends.
struct InC {
    depth: AtomicUsize,
    calls: AtomicUsize,
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
    let state = in_c();
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.depth.fetch_add(1, Ordering::SeqCst);
    let monitor = MONITOR.get_or_init(|| spawn_thread(Some(64 * 1024), monitor).thread().clone());
    if MONITOR_ASLEEP.load(Ordering::SeqCst) && MONITOR_ASLEEP.swap(false, Ordering::SeqCst) {
        monitor.unpark();
    }
}

/// After a C function returns: a worker that was replaced meanwhile makes
/// one worker retire.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_foreign_leave() {
    let state = in_c();
    state.calls.fetch_add(1, Ordering::SeqCst);
    if state.depth.fetch_sub(1, Ordering::SeqCst) == 1 && state.replaced.swap(false, Ordering::SeqCst) {
        queue().surplus += 1;
        READY.notify_one();
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

fn new_task(stack: Stack, body: *mut u8, code: *const u8, kind: ResultKind, join: *mut JoinObject) -> *mut Task {
    let sp = unsafe { initial_stack(stack.top()) };
    Box::into_raw(Box::new(Task {
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
    }))
}

/// Runs the body of a callback C made: on this worker when C called back
/// inside a task, or as a new task while a thread of C waits for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_callback(inner: unsafe extern "C" fn(*mut u8), frame: *mut u8) {
    if !current().is_null() {
        return unsafe { inner(frame) };
    }
    let call = ForeignCall { inner, frame, done: Mutex::new(false), finished: Condvar::new() };
    let task =
        new_task(task_stack(), std::ptr::null_mut(), std::ptr::null(), ResultKind::Nothing, std::ptr::null_mut());
    unsafe {
        (*task).main = false;
        (*task).foreign = &call;
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
/// task unwinds from there.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cancelled() -> u8 {
    cancelled() as u8
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
    }));
    let code = unsafe { *(body.add(16) as *const *const u8) };
    let kind = match kind {
        0 => ResultKind::Nothing,
        1 => ResultKind::Integer,
        2 => ResultKind::Float64,
        3 => ResultKind::Float32,
        _ => ResultKind::Memory,
    };
    let task = new_task(task_stack(), body, code, kind, join);
    unsafe { (*join).task = task };
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

unsafe fn free_join(join: *mut JoinObject) {
    unsafe {
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
    location: *const Str,
) -> *mut ChannelObject {
    if bounded != 0 && capacity < 0 {
        unsafe { tondo_rt_panic(1, std::ptr::null(), location) };
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(ChannelObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        closed: false,
        senders: 0,
        capacity: if bounded != 0 { capacity as usize } else { usize::MAX },
        size,
        align: align.max(1),
        release,
        buffer: VecDeque::new(),
        count: 0,
        receiving: VecDeque::new(),
        sending: VecDeque::new(),
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
    unsafe { tondo_rt_panic(12, std::ptr::null(), location) }
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

/// A small generator for the random choice among ready operations.
fn random(bound: usize) -> usize {
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
