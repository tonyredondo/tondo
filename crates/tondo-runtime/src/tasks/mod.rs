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
use super::recorder;
use super::trace::Origin;
use super::{Glue, LIVE_OBJECTS, Str, flush_stdout, runtime_fault};
use std::cell::Cell;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

mod stop;
pub(crate) use stop::*;

mod foreign;
pub(crate) use foreign::*;

mod orders;
pub(crate) use orders::*;

mod cancel;
pub(crate) use cancel::*;

mod reload;
pub(crate) use reload::*;

mod waits;
pub(crate) use waits::*;

mod join;
pub(crate) use join::*;

mod group;
pub(crate) use group::*;

mod channels;
pub(crate) use channels::*;

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
            // A task uses a few pages of its stack: a huge page for the top
            // would hold 2 MiB.
            #[cfg(target_os = "linux")]
            libc::madvise(base.cast(), size, libc::MADV_NOHUGEPAGE);
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
/// more than the task itself. Each thread keeps a few, and trades them
/// with the shared ones a batch at a time, since a task often ends on
/// another thread than the one that started it.
static SPARE_STACKS: Mutex<Vec<Stack>> = Mutex::new(Vec::new());
const SPARE_LIMIT: usize = 256;
const BATCH: usize = 16;

/// This worker's spare stacks, made when it first keeps one. They are not a
/// field that drops with the thread, which would make every read of `Worker`
/// check whether the thread ends: a thread-local of their own gives them
/// back then.
fn own_stacks() -> &'static mut Vec<Stack> {
    struct GiveBack(Cell<*mut Vec<Stack>>);
    impl Drop for GiveBack {
        fn drop(&mut self) {
            let stacks = self.0.get();
            if !stacks.is_null() {
                let stacks = unsafe { Box::from_raw(stacks) };
                let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
                let room = SPARE_LIMIT.saturating_sub(spare.len());
                spare.extend(stacks.into_iter().take(room));
            }
        }
    }
    thread_local! {
        static GIVE_BACK: GiveBack = const { GiveBack(Cell::new(std::ptr::null_mut())) };
    }
    let mut stacks = worker(|w| w.stacks.get());
    if stacks.is_null() {
        stacks = Box::into_raw(Box::new(Vec::new()));
        GIVE_BACK.with(|give_back| give_back.0.set(stacks));
        worker(|w| w.stacks.set(stacks));
    }
    // Neither caller is entered again while it holds them.
    unsafe { &mut *stacks }
}

fn task_stack() -> Stack {
    let own = own_stacks();
    if own.is_empty() {
        let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
        let at = spare.len().saturating_sub(BATCH);
        own.extend(spare.drain(at..));
    }
    own.pop().unwrap_or_else(|| Stack::new(TASK_STACK))
}

fn recycle(stack: Stack) {
    if stack.size != TASK_STACK {
        return;
    }
    let own = own_stacks();
    own.push(stack);
    if own.len() > 2 * BATCH {
        let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
        let room = SPARE_LIMIT.saturating_sub(spare.len()).min(BATCH);
        let at = own.len() - BATCH;
        spare.extend(own.drain(at..).take(room));
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
const TASK_MARK: u64 = u64::from_le_bytes(*b"TONDOTSK");
const RUNNING: u8 = 0;
const PARKING: u8 = 1;
const PARKED: u8 = 2;
const NOTIFIED: u8 = 3;
const PINNED: u8 = 4;

/// Where tasks that wait on their thread are woken.
static PINNED_LOCK: Mutex<()> = Mutex::new(());
static PINNED_WAKE: Condvar = Condvar::new();

#[repr(C)]
pub(crate) struct Task {
    /// `TASK_MARK` while the task is alive: what finds it in a core, which
    /// knows the fields up to `blocked` (toolchain 7.5).
    mark: u64,
    /// The stack pointer saved when the task is not running.
    sp: *mut u8,
    /// Its number in a trace, from 1 for the main task.
    pub(crate) id: u64,
    state: AtomicU8,
    main: bool,
    /// Set by `cancel` (11.15): the task's waits end at once, and compiled
    /// code unwinds after them.
    cancelled: AtomicBool,
    /// What it is blocked on, for a debugger and a deadlock: the object,
    /// and whether a lock, a channel to receive from or send to, or a task
    /// to finish (`LOCK`…), 0 when it runs.
    blocked_kind: AtomicU8,
    blocked: AtomicUsize,
    /// Where the set of tasks alive keeps it.
    slot: usize,
    /// None before a spawned task first runs: one started and not run yet
    /// holds no stack, as when a loop starts thousands at once.
    stack: Option<Stack>,
    /// The closure the task runs: a `fn` object, released afterwards.
    body: *mut u8,
    code: *const u8,
    kind: ResultKind,
    join: *mut JoinObject,
    /// Its `defer` cleanups, innermost last (13.7).
    cleanups: Vec<(Cleanup, usize)>,
    /// The `defer`s it runs, nested: in them a cancelled task still does
    /// what needs no wait (11.15).
    deferring: u32,
    /// In those `defer`s, a wait ended at once because the task is
    /// cancelled: the code unwinds to the `defer` that runs.
    cut: bool,
    /// It shows its contexts for a log: a `Display` of theirs that logs too
    /// shows none. The task's, not the thread's, as one may wait meanwhile.
    pub(crate) showing: bool,
    /// Set by a live build's reload: the wait ends, and the statement it is
    /// in starts again (toolchain 7.10).
    pub(crate) again: AtomicBool,
    /// The wait the task is blocked in, which a cancellation claims;
    /// guarded by `waiting_lock`.
    waiting: *const Wait,
    waiting_lock: AtomicBool,
    /// Under `tondo test`, what it goes by in the orders of a test, from
    /// who started it, and how many it started.
    name: u64,
    started: u64,
    /// The `std.sync` locks it holds, whether exclusively, and where it
    /// took them.
    pub(crate) held: Vec<(usize, bool, *const Str)>,
    /// When it blocked there, in milliseconds of `WAITS_FROM`.
    blocked_since: AtomicU64,
    finished: bool,
    /// For a task that runs a callback for a thread of C, the call.
    foreign: *const ForeignCall,
    /// Whether its clock is a test's virtual clock (testing 7.9).
    pub(crate) in_domain: bool,
    /// Where it was started, for stack traces.
    origin: Option<&'static Origin>,
    /// The set of labels of `profile.withLabels` it carries, `0` for none,
    /// which its samples record and the tasks it starts inherit.
    labels: std::sync::atomic::AtomicU32,
    /// Those of them `profile.withLabels` gives, without its contexts'.
    own_labels: std::sync::atomic::AtomicU32,
    /// Its contexts (language 11.16), shared with the task that spawned it
    /// until a `with` changes them.
    pub(crate) context: super::contexts::Held,
    /// The deadlines of the `within` blocks it runs (language 11.17),
    /// innermost last: the one that holds, the nearest, and the index of the
    /// block that set it.
    pub(crate) withins: Vec<(Instant, usize)>,
    /// The `within` entered in a `defer` of the cancelled task, whose waits
    /// go on until its deadline (11.15): its level among `withins`.
    pub(crate) grace: Option<usize>,
    /// Its deadline passed in a wait: it unwinds to the block of it.
    pub(crate) expired: bool,
    /// The deadline of the block that spawned it, which it reads and does
    /// not keep (`contexts::deadline_nanos`).
    pub(crate) inherited: AtomicU64,
    /// The `recover` blocks it is running, innermost last (language 15.9),
    /// with the tasks of each once it starts one.
    pub(crate) recovers: Vec<Option<Arc<super::panics::Owner>>>,
    /// The `recover` it belongs to: started without `detached` inside one,
    /// or by a task that belongs to one.
    pub(crate) owner: Option<Arc<super::panics::Owner>>,
    /// Detached since: it belongs to no `recover` but its own.
    pub(crate) free: AtomicBool,
    /// The next task it spawns starts detached (11.11).
    detaching: bool,
    /// A task of one of its `recover`s ended with a panic, which stops it
    /// as a cancellation would, without cancelling it.
    pub(crate) alarm: AtomicBool,
    /// The panic it unwinds from: compiled code returns until a `recover`
    /// stops it.
    pub(crate) unwinding: Option<Box<super::panics::Record>>,
    /// Panics whose unwinding waits while the cleanups of a frame run,
    /// innermost last.
    pub(crate) paused: Vec<Option<Box<super::panics::Record>>>,
    /// The panic the last `recover` stopped, which the standard library
    /// reads into a `Panic`.
    pub(crate) caught: Option<Box<super::panics::Record>>,
    /// While exploring orders, its priority: the highest that can run runs.
    priority: u64,
    /// The request it works for, by its number in a timeline, as the task
    /// that started it; 0 for none.
    flight: u64,
    /// A panic that held it goes up to the frame that calls the function
    /// again.
    pub(crate) retrying: bool,
    /// Where its live frames were last, while panics wait for a fix: each
    /// frame with its site and the addresses of its bindings.
    probes: Vec<Probe>,
}

/// A frame at a probe: its pointer, its site, and where its bindings are.
#[derive(Clone, Copy)]
pub(crate) struct Probe {
    pub frame: usize,
    pub site: usize,
    pub addresses: usize,
    pub alive: usize,
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
    /// A worker was woken for the tasks scheduled and has not looked at
    /// the queue yet: the first that looks takes over `SEEKING`.
    waking: bool,
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
    waking: false,
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

/// What a thread that runs tasks keeps for itself (docs/refactor.md, phase
/// 1). A task may resume on another thread, so this is reached through
/// `worker`, which never inlines, and asked for again after anything
/// a task may switch at: no caller keeps its address, or that of a field.
/// Nothing in it drops, so that reading it costs a load, as the old
/// `CURRENT` did.
struct Worker {
    /// The task it runs.
    current: Cell<*mut Task>,
    /// Where its own stack pointer is saved while a task runs.
    home: Cell<*mut u8>,
    /// The tasks it started, which wait next to it; and how many turns it
    /// gave them since it last looked at the shared queue.
    own_next: Cell<Option<&'static Next>>,
    next_turns: Cell<u32>,
    /// It looks at the queue next and finds nothing before the first task
    /// it schedules, which wakes no other worker.
    looks_next: Cell<bool>,
    /// Task stacks kept for reuse, made when first needed (`own_stacks`).
    stacks: Cell<*mut Vec<Stack>>,
    /// Its state for stopping the world, once it ran Tondo code.
    runner: Cell<Option<&'static Runner>>,
    /// Its calls of C, once it made one.
    in_c: Cell<*const InC>,
}

thread_local! {
    static WORKER: Worker = const {
        Worker {
            current: Cell::new(std::ptr::null_mut()),
            home: Cell::new(std::ptr::null_mut()),
            own_next: Cell::new(None),
            next_turns: Cell::new(0),
            looks_next: Cell::new(false),
            stacks: Cell::new(std::ptr::null_mut()),
            runner: Cell::new(None),
            in_c: Cell::new(std::ptr::null()),
        }
    };
}

/// This thread's `Worker`, lent to `f`, which reads or writes it. Never
/// inlined, so that the compiler cannot keep the thread-local's address
/// across a switch of a task between threads; and nothing hands that address
/// out, or a function that only computed it would read no memory, which the
/// compiler takes for one whose result never changes, merging its calls
/// across a switch.
#[inline(never)]
fn worker<R>(f: impl FnOnce(&Worker) -> R) -> R {
    WORKER.with(f)
}

pub(crate) fn current() -> *mut Task {
    worker(|w| w.current.get())
}

/// The labels of the running task, which a signal handler may read.
pub(crate) fn labels() -> u32 {
    unsafe { current().as_ref() }.map_or(0, |task| task.labels.load(Ordering::Relaxed))
}

/// Gives the running task the labels `set`; the ones it had.
pub(crate) fn set_labels(set: u32) -> u32 {
    unsafe { current().as_ref() }.map_or(0, |task| task.labels.swap(set, Ordering::Relaxed))
}

/// The labels `profile.withLabels` gives the running task, without those of
/// its contexts.
pub(crate) fn own_labels() -> u32 {
    unsafe { current().as_ref() }.map_or(0, |task| task.own_labels.load(Ordering::Relaxed))
}

pub(crate) fn set_own_labels(set: u32) {
    if let Some(task) = unsafe { current().as_ref() } {
        task.own_labels.store(set, Ordering::Relaxed);
    }
}

/// The bounds of the running task's stack, above its guard.
pub(crate) fn stack_bounds() -> Option<(usize, usize)> {
    let task = current();
    let stack = unsafe { task.as_ref()?.stack.as_ref()? };
    Some((stack.base as usize + GUARD, stack.top() as usize))
}

/// Where the running task was started.
pub(crate) fn origin() -> Option<&'static Origin> {
    unsafe { origin_of(current()) }
}

/// Where a task was started.
pub(crate) unsafe fn origin_of(task: *mut Task) -> Option<&'static Origin> {
    unsafe { task.as_ref()?.origin }
}

/// The request the task `id` works for, in words.
pub(crate) fn request_of(id: u64) -> Option<String> {
    let task = unsafe { by_id(id)?.as_ref()? };
    super::diagnostics::flight_text(task.flight)
}

/// A frame of the running task is at a probe: those below it returned.
pub(crate) fn note_probe(probe: Probe) {
    if let Some(task) = unsafe { current().as_mut() } {
        while task.probes.last().is_some_and(|last| last.frame < probe.frame) {
            task.probes.pop();
        }
        match task.probes.last_mut().filter(|last| last.frame == probe.frame) {
            Some(last) => *last = probe,
            None => task.probes.push(probe),
        }
    }
}

/// Where a frame of the running task was last.
pub(crate) fn probe_of(frame: usize) -> Option<Probe> {
    let task = unsafe { current().as_ref()? };
    task.probes.iter().rev().find(|probe| probe.frame == frame).copied()
}

/// The running task works for a request, from now on.
pub(crate) fn join_flight(flight: u64) {
    if let Some(task) = unsafe { current().as_mut() } {
        task.flight = flight;
    }
}

/// Adds what the running task did to the timeline of its request, if it has
/// one.
pub(crate) fn flight_step(did: impl FnOnce() -> super::diagnostics::Did) {
    if let Some(task) = unsafe { current().as_ref() }.filter(|task| task.flight != 0) {
        let here = super::trace::walk(super::trace::frame_pointer(), stack_bounds().unwrap_or((0, 0)));
        super::diagnostics::step(task.flight, task.id, did(), here.take(FLIGHT_FRAMES).collect());
    }
}

/// How many callers of a step its timeline keeps, to find the program's code.
const FLIGHT_FRAMES: usize = 16;

/// The running task's number in a trace, or 0 outside any task.
pub(crate) fn current_id() -> u64 {
    unsafe { current().as_ref().map_or(0, |task| task.id) }
}

pub(crate) unsafe fn is_main(task: *mut Task) -> bool {
    unsafe { (*task).main }
}

fn home() -> *mut *mut u8 {
    // An address alone reads no memory: opaque, as `worker` says.
    worker(|w| std::hint::black_box(w.home.as_ptr()))
}

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a worker runs next: a task the task it runs made runnable, as Go's
/// `runnext`, so that two tasks that hand each other values run one after
/// the other on one thread, and when it was put there.
struct Next {
    task: AtomicPtr<Task>,
    since: AtomicU64,
    /// And the tasks the tasks it runs started, which idle workers take
    /// half of at a time.
    tasks: Mutex<Local>,
    /// How many there are, which a worker that would take some reads
    /// without the lock.
    count: AtomicUsize,
    /// Since when they wait: since the first was started, or since others
    /// last took some.
    waiting: AtomicU64,
}

struct Local(VecDeque<*mut Task>);

unsafe impl Send for Local {}

impl Next {
    fn tasks(&self) -> MutexGuard<'_, Local> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Half of the tasks another worker started and has not run, the first to
/// run here and the rest to this worker's own, and whether more wait; or
/// whether some wait for less than `NEXT_WAIT`, to look at again in a
/// moment. Tasks wait that long before another worker takes them, as a task
/// next does: those that end sooner end on the worker that started them,
/// in the caches where they were made, and the others are shared out as
/// soon.
fn steal() -> Result<(*mut Task, bool), bool> {
    let Some(own) = own_next() else { return Err(false) };
    let now = nanos();
    let mut young = false;
    for other in nexts().filter(|other| !std::ptr::eq(*other, own)) {
        if other.count.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let since = other.waiting.load(Ordering::Relaxed);
        if now.saturating_sub(since) < NEXT_WAIT.as_nanos() as u64 {
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
fn waiting_somewhere() -> bool {
    nexts().any(|next| next.count.load(Ordering::SeqCst) > 0)
}

/// The first of the tasks this worker's tasks started.
fn own_started() -> Option<*mut Task> {
    let own = own_next()?;
    if own.count.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let task = own.tasks().0.pop_front()?;
    own.count.fetch_sub(1, Ordering::Relaxed);
    Some(task)
}

/// The tasks waiting in workers' own queues.
fn waiting_local() -> Vec<*mut Task> {
    nexts().flat_map(|next| next.tasks().0.iter().copied().collect::<Vec<_>>()).collect()
}

/// Every worker's `Next`, which an idle worker takes a task from when the
/// worker that has it runs another for long: read without a lock, as every
/// idle worker reads them. A worker past the last has none.
static NEXTS: [AtomicPtr<Next>; 1024] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 1024];
static NEXT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// This worker's next, read by tasks as they wake others: through
/// `worker`, or a task that resumes on another thread writes the next of
/// the worker it ran on before, which may sleep.
fn own_next() -> Option<&'static Next> {
    worker(|w| w.own_next.get())
}

/// How long a task waits next to a worker that runs another before an idle
/// worker takes it, so that tasks that could run in parallel still do.
const NEXT_WAIT: Duration = Duration::from_micros(20);
/// When a worker was last woken for a task waiting next.
static NEXT_WOKE: AtomicU64 = AtomicU64::new(0);
/// How many workers sleep, as the queue counts them, read without its lock.
static IDLE: AtomicUsize = AtomicUsize::new(0);
/// A worker was woken for tasks, or looks for them since: no other is woken
/// until it finds one or sleeps again, and then it wakes the next if more
/// wait, as Go's scheduler wakes a thread only when none looks for work.
/// Workers that look at once would read what the busy ones write all the
/// time, and slow them. Set under the queue's lock.
static SEEKING: AtomicBool = AtomicBool::new(false);

fn nanos() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn nexts() -> impl Iterator<Item = &'static Next> {
    let count = NEXT_COUNT.load(Ordering::Acquire).min(NEXTS.len());
    NEXTS[..count].iter().filter_map(|next| unsafe { next.load(Ordering::Acquire).as_ref() })
}

/// The tasks waiting next to their workers.
fn waiting_next() -> Vec<*mut Task> {
    nexts().map(|next| next.task.load(Ordering::Acquire)).filter(|task| !task.is_null()).collect()
}

/// A task another worker has had next for longer than `NEXT_WAIT`, taken
/// from it; and whether one waits there for less.
fn take_next() -> (Option<*mut Task>, bool) {
    let mine = own_next().map(|next| next as *const Next);
    let now = nanos();
    let mut young = false;
    for next in nexts().filter(|next| Some(*next as *const Next) != mine) {
        let task = next.task.load(Ordering::Acquire);
        if task.is_null() {
            continue;
        }
        if now.saturating_sub(next.since.load(Ordering::Relaxed)) < NEXT_WAIT.as_nanos() as u64 {
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
fn schedule(task: *mut Task) {
    if !current().is_null()
        && exploring().is_none()
        && let Some(next) = own_next()
    {
        next.since.store(nanos(), Ordering::Relaxed);
        let replaced = next.task.swap(task, Ordering::AcqRel);
        if !replaced.is_null() {
            enqueue(replaced);
            return;
        }
        // An idle worker takes it if this one runs on for long, as Go's
        // scheduler steals `runnext` from a processor busy for a while: one
        // woken now and then, not at every task two tasks hand each other.
        let now = nanos();
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
fn enqueue(task: *mut Task) {
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
    if looks_next() || SEEKING.load(Ordering::SeqCst) {
        return;
    }
    // One worker woken at a time, not one for each task: the one woken
    // wakes the next if it finds more.
    if queue.idle > 0 {
        queue.waking = true;
        SEEKING.store(true, Ordering::SeqCst);
    }
    wake_one(queue);
}

/// Wakes a worker that sleeps to look for tasks, unless one looks already.
fn wake_idle() {
    if IDLE.load(Ordering::SeqCst) == 0 || SEEKING.load(Ordering::SeqCst) {
        return;
    }
    let mut queue = queue();
    if queue.idle > 0 && !SEEKING.load(Ordering::SeqCst) {
        queue.waking = true;
        SEEKING.store(true, Ordering::SeqCst);
        drop(queue);
        READY.notify_one();
    }
}

/// Whether this worker looks at the queue next, once.
fn looks_next() -> bool {
    worker(|w| w.looks_next.replace(false))
}

/// Wakes a worker to look at the queue, as for a signal that came.
pub(crate) fn wake_worker() {
    wake_one(queue());
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
    worker(|w| w.looks_next.set(alone));
    events.wake();
    worker(|w| w.looks_next.set(false));
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
        let in_c = in_c_here();
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
    // The queue is let go first: a debugger reads it while it shows the
    // deadlock.
    let check = |queue: MutexGuard<'static, Queue>| {
        if matches!(stalled(&queue), Stalled::Deadlock) {
            drop(queue);
            deadlock();
        }
    };
    {
        let mut queue = queue();
        queue.pinned.push(task);
        check(queue);
    }
    // One worker has no other to run what it waits for: a thread stands in
    // for it meanwhile.
    let standing_in = exploring().is_some().then(|| stand_in(state));
    // Waiting, the thread runs no Tondo code that a collection could meet.
    leave_tondo();
    // The wait's own reading of the clock is not the program's.
    let outside = super::replay::Outside::enter();
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
        check(queue());
    }
    if let Some(thread) = standing_in {
        // It reads the task's state, which is this thread's again.
        let _ = thread.join();
        super::replay::waited();
    }
    drop(outside);
    enter_tondo();
    queue().pinned.retain(|&t| t != task);
}

/// Starts a thread that runs the program, with the baton, for a worker
/// that waits with C's frames on its task's stack, until the task's `state`
/// says it goes on, which comes at the same place of the program in a replay.
fn stand_in(state: &AtomicU8) -> std::thread::JoinHandle<()> {
    let state = state as *const AtomicU8 as usize;
    let outside = super::replay::Outside::enter();
    let thread = spawn_thread(Some(256 * 1024), move || {
        let state = unsafe { &*(state as *const AtomicU8) };
        super::replay::entered(0);
        queue().workers += 1;
        work_until(false, Some(&|| state.load(Ordering::Acquire) == RUNNING));
        queue().workers -= 1;
        super::replay::left();
    });
    drop(outside);
    super::replay::waits();
    thread
}

// ---------------------------------------------------------------------------
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

/// Threads the runtime started that still run, the main thread included.
pub(crate) static RUNTIME_THREADS: AtomicUsize = AtomicUsize::new(1);
/// Threads of C waiting in `tondo_rt_callback` for the task they started,
/// which went on on a worker; while it runs on theirs, they can call back.
static FOREIGN_WAITING: AtomicUsize = AtomicUsize::new(0);
/// Calls from threads of C that have not returned.
static FOREIGN_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Threads an executable's process had before its program started, which
/// are not the program's: an emulator's, as qemu's. A library's host has
/// its own, which may call it.
static BEFORE: AtomicUsize = AtomicUsize::new(0);

/// Waits, up to a second, for the calls from threads of C still running:
/// what they hold is not leaked when the program ends before them.
pub fn wait_foreign_calls() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while FOREIGN_CALLS.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Waits, up to a second, until every task is freed: a detached task that
/// `main`'s end cancelled is done before its thread lets go of what it held.
pub fn wait_tasks_freed() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while live_tasks() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Starts a runtime thread, counted while it runs.
pub(crate) fn spawn_thread<T: Send + 'static>(
    stack: Option<usize>,
    body: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    try_spawn_thread(stack, body).unwrap_or_else(|_| runtime_fault("cannot start a thread"))
}

/// Starts a runtime thread, or says why the system would not.
pub(crate) fn try_spawn_thread<T: Send + 'static>(
    stack: Option<usize>,
    body: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            super::diagnostics::retire_here();
            RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
        }
    }
    RUNTIME_THREADS.fetch_add(1, Ordering::SeqCst);
    let mut builder = std::thread::Builder::new();
    if let Some(stack) = stack {
        builder = builder.stack_size(stack);
    }
    let started = builder.spawn(move || {
        let _running = Running;
        body()
    });
    if started.is_err() {
        RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
    }
    started
}

/// How many threads the process has, where the system tells.
#[cfg(target_os = "linux")]
pub(crate) fn os_threads() -> Option<usize> {
    std::fs::read_dir("/proc/self/task").ok().map(|entries| entries.count())
}

#[cfg(target_os = "macos")]
pub(crate) fn os_threads() -> Option<usize> {
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
pub(crate) fn os_threads() -> Option<usize> {
    None
}

/// Fires the timers that are due; their tasks join `runnable`.
fn fire_timers(queue: &mut Queue) {
    if queue.timers.is_empty() {
        return;
    }
    let now = clock_now();
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
    queue.timers.push(Timer { at: clock_now() + delay, sequence, wait, index });
    // An idle worker recomputes how long to sleep.
    wake_one(queue);
}

/// Runs tasks; on the main thread, until the main task finishes, wherever
/// it ran, and then returns its exit status.
fn work(main: bool) -> i32 {
    work_until(main, None)
}

/// The main thread has nothing left to do: the program ended. Exploring
/// orders this thread is the only worker: after a panic it runs the other
/// tasks' cleanups itself, and the tasks left when the program ends where
/// leaks are checked, which other workers would have run by then.
fn program_ended(queue: &Queue) -> bool {
    let done = MAIN_DONE.load(Ordering::SeqCst);
    let ending =
        ENDING.load(Ordering::SeqCst) != usize::MAX || (done && std::env::var_os("TONDO_RT_CHECK_LEAKS").is_some());
    done && !(exploring().is_some() && ending && !queue.runnable.is_empty())
}

/// `work` in a recorded run, on a thread other than the worker's until
/// `until` says: the worker may be in C, and only the baton's runs anyway.
fn work_until(main: bool, until: Option<&dyn Fn() -> bool>) -> i32 {
    super::trace::guard_thread(true);
    if exploring().is_none() && own_next().is_none() {
        let index = NEXT_COUNT.load(Ordering::Acquire);
        if index < NEXTS.len() {
            let next: &'static Next = Box::leak(Box::new(Next {
                task: AtomicPtr::new(std::ptr::null_mut()),
                since: AtomicU64::new(0),
                tasks: Mutex::new(Local(VecDeque::new())),
                count: AtomicUsize::new(0),
                waiting: AtomicU64::new(0),
            }));
            let index = NEXT_COUNT.fetch_add(1, Ordering::AcqRel);
            if let Some(slot) = NEXTS.get(index) {
                slot.store(next as *const Next as *mut Next, Ordering::Release);
                worker(|w| w.own_next.set(Some(next)));
            }
        }
    }
    // A worker just woken looks once more, a moment later, for a task
    // waiting next to a worker that runs another, before it sleeps again.
    // One that starts to take another's place was woken for that.
    let starts = std::mem::take(&mut queue().waking);
    let (mut woken, mut waited) = (starts, false);
    // This worker looks for tasks for `SEEKING`.
    let mut seeker = starts;
    loop {
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
                    if seeker {
                        SEEKING.store(false, Ordering::SeqCst);
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
                    // worker that looks and leaves too (testing/models/Wake.tla).
                    if !queue.runnable.is_empty() {
                        wake_one(queue);
                    }
                    return 0;
                }
                // The signals of a recorded run come at the worker's turns,
                // and it does the work that blocks its runtime meanwhile.
                if super::replay::on() {
                    drop(queue);
                    super::signals::at_turn();
                    super::http::deferred();
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
                    if let Some(runtime) = super::http::driven() {
                        super::http::drive(runtime, false, None);
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
                if queue.idle + 1 >= queue.workers && super::stdout_waits() {
                    drop(queue);
                    flush_stdout();
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
                let mut next = queue.timers.peek().map(|t| t.at);
                // A worker that looks for tasks looks again in a moment at
                // those that wait for less, and goes on looking.
                waited = (young && woken) || (stealing && seeker);
                if !waited {
                    // Before it sleeps, one that looked for tasks stops, and
                    // then, counted idle, looks at the workers' queues once
                    // more: tasks started meanwhile, or too young to take,
                    // whose worker may not come back to them soon, as one
                    // that waits in a call of C. Unless another worker looks
                    // for them, this one does again in a moment.
                    if seeker {
                        seeker = false;
                        SEEKING.store(false, Ordering::SeqCst);
                    }
                    if waiting_somewhere() && !SEEKING.swap(true, Ordering::SeqCst) {
                        (seeker, waited) = (true, true);
                    }
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
                    super::replay::waits();
                    queue = match super::http::driven() {
                        // A recorded run's worker waits in the runtime it
                        // drives, whose sources have the poller, unless the
                        // poller has events already, which the runtime
                        // would not hear of again.
                        Some(runtime) => {
                            let mut ready = libc::pollfd { fd: poller.fd(), events: libc::POLLIN, revents: 0 };
                            if unsafe { libc::poll(&mut ready, 1, 0) } == 0 {
                                super::http::drive(runtime, true, next);
                            }
                            poll(poller, polled, Some(Instant::now()))
                        }
                        None => poll(poller, polled, next),
                    };
                    if super::replay::on() {
                        drop(queue);
                        super::replay::waited();
                        queue = self::queue();
                    }
                    looked = false;
                    continue;
                }
                let delay = next.map(|at| at.saturating_duration_since(Instant::now()));
                queue = if super::replay::replaying() {
                    // A replay waits for nothing: what woke the recorded
                    // run comes next in its recording.
                    drop(queue);
                    super::replay::waiting();
                    self::queue()
                } else {
                    super::replay::waits();
                    // The wait's own reading of the clock is not the
                    // program's.
                    let outside = super::replay::Outside::enter();
                    let woken = match (delay, until) {
                        (Some(delay), _) => READY.wait_timeout(queue, delay).unwrap_or_else(|e| e.into_inner()).0,
                        (None, None) => READY.wait(queue).unwrap_or_else(|e| e.into_inner()),
                        // What it waits for may come without a wake, and
                        // looking again is no turn of the worker's.
                        (None, Some(until)) => loop {
                            let (woken, waited) = READY.wait_timeout(queue, RECHECK).unwrap_or_else(|e| e.into_inner());
                            if !waited.timed_out() || until() {
                                break woken;
                            }
                            queue = woken;
                        },
                    };
                    drop(outside);
                    match super::replay::on() {
                        true => {
                            drop(woken);
                            super::replay::waited();
                            self::queue()
                        }
                        false => woken,
                    }
                };
                queue.idle -= 1;
                IDLE.fetch_sub(1, Ordering::SeqCst);
                // Woken for a task, or after waiting a moment for one. The
                // first to look after a wake looks for what it was for.
                woken = queue.waking && !woken;
                seeker |= std::mem::take(&mut queue.waking);
                looked = false;
            }
        };
        // Found: the next worker looks, if more wait anywhere.
        let mut more = more;
        if seeker {
            seeker = false;
            SEEKING.store(false, Ordering::SeqCst);
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
fn run(task: *mut Task) {
    worker(|w| w.current.set(task));
    if super::race::on() && noting() {
        noted(format!("s {}\n", unsafe { (*task).name }));
    }
    let traced = recorder::on();
    if traced {
        recorder::record(recorder::Event::Run { task: unsafe { (*task).id } });
    }
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
    if traced {
        // Nothing runs the task until it is parked or scheduled again.
        let why = unsafe {
            match (*task).finished {
                true => recorder::Stop::Done,
                false if (*task).state.load(Ordering::Acquire) == PARKING => {
                    recorder::Stop::Waits(recorder::frames(parked_walk(task)))
                }
                false => recorder::Stop::Yields,
            }
        };
        recorder::record(recorder::Event::Stop { task: unsafe { (*task).id }, why });
    }
    unsafe {
        let in_domain = (*task).in_domain;
        if (*task).finished && super::events::on() {
            super::events::add_of((*task).id, super::events::What::Ended);
        }
        if (*task).finished {
            super::debugger::task_ended((*task).id);
            let (was_main, join) = ((*task).main, (*task).join);
            for asked in [&(*task).cancelled, &(*task).again] {
                if asked.load(Ordering::SeqCst) {
                    attend(false);
                }
            }
            if let Some(stack) = (*task).stack.take() {
                recycle(stack);
            }
            unregister(task);
            (*task).mark = 0;
            // A spawned task lives in its join's block.
            if join.is_null() {
                drop(Box::from_raw(task));
            } else {
                std::ptr::drop_in_place(task);
                let_go(join);
            }
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

fn deadlock() -> ! {
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
    super::debugger::deadlocked(headline);
    let mut report = String::from(headline);
    // The tasks that wait for each other, when locks or joins tie them.
    let all: Vec<u64> = tasks().iter().map(|&t| unsafe { (*(t as *mut Task)).id }).collect();
    if let Some(cycle) = super::debugger::cycle(&all) {
        report.push_str(&format!("\n{cycle}"));
    }
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
    super::diagnostics::crash_dump(
        "deadlock",
        "every task waits and none of them can wake the others",
        Vec::new(),
        true,
    );
    std::process::exit(super::PANIC_EXIT_CODE)
}

/// The trace of a task that waits, under a `TITLE` to fill in: from the
/// frame pointer it saved when it switched out, or from here for the
/// running one.
unsafe fn waiting_at(task: *mut Task) -> String {
    unsafe {
        let walk = if task == current() {
            let Some(stack) = (*task).stack.as_ref() else { return String::new() };
            let bounds = (stack.base as usize + GUARD, stack.top() as usize);
            super::trace::walk(super::trace::frame_pointer(), bounds)
        } else if (*task).state.load(Ordering::Acquire) == PARKED {
            parked_walk(task)
        } else {
            return "\nTITLE: a call to C".into();
        };
        super::trace::stack(walk, (*task).origin, "TITLE")
    }
}

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
    for (slot, address) in runner.answer.iter().zip(std::iter::once(pc + 1).chain(super::trace::walk(fp, bounds))) {
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
fn ask_where(runners: &[&Runner]) -> Vec<(*mut Task, Vec<usize>)> {
    super::recorder::install();
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
                ("in C", std::iter::once(from).chain(super::trace::walk(frame, bounds)).collect())
            } else if let Some(runner) = paused {
                ("runs", super::trace::walk(runner.frame.load(Ordering::SeqCst), bounds).collect())
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
                .map(|&(_, _, at)| String::from_utf8_lossy(super::str_bytes(at)).into_owned())
                .collect();
            Shot {
                id: shown.id,
                main: shown.main,
                state,
                stack,
                spawned: shown.origin.map(|o| o.frames().to_vec()).unwrap_or_default(),
                locks,
                cancelled: shown.cancelled.load(Ordering::Relaxed),
                request: super::diagnostics::flight_text(shown.flight),
                wait: super::debugger::waits_for(shown.id)
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
unsafe fn parked_walk(task: *mut Task) -> super::trace::Walk {
    match unsafe { switched_out(task) } {
        Some((fp, bounds)) => super::trace::walk(fp, bounds),
        None => super::trace::walk(0, (0, 0)),
    }
}

/// The frame pointer a task that switched out saved, and its stack's
/// bounds.
unsafe fn switched_out(task: *mut Task) -> Option<(usize, (usize, usize))> {
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
unsafe fn switched_registers(task: *mut Task) -> super::unwinding::Switched {
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
pub(crate) type Standing = (u64, usize, (usize, usize), Option<super::unwinding::Switched>);

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
        // A debugger that stops long waits stops this one, as it ends.
        super::debugger::waited(task.id, since.elapsed());
        if super::events::on() && since.elapsed().as_millis() >= LONG_WAIT {
            super::events::add(super::events::What::Waited(since.elapsed().as_millis() as u64));
        }
        task.blocked_kind.store(0, Ordering::Release);
    }
    fired
}

/// The milliseconds from which a dump's events keep a wait.
const LONG_WAIT: u128 = 100;

/// What `blocked_since` counts from.
static WAITS_FROM: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

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
            out.push((task.id, exclusive, String::from_utf8_lossy(unsafe { super::str_bytes(at) }).into_owned()));
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

/// Starts the other workers, one per core, the first time a task spawns or
/// a call from C waits.
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
/// Where every task starts: runs its body, completes its `Join` and
/// switches back to its worker for good.
extern "C" fn trampoline() -> ! {
    unsafe {
        let task = current();
        let (body, code, kind, join) = ((*task).body, (*task).code, (*task).kind, (*task).join);
        if !(*task).foreign.is_null() {
            let call = &*(*task).foreign;
            (call.inner)(call.frame);
            // Under the lock: the thread of C may free the call once it
            // sees it done.
            let mut done = call.done.lock().unwrap_or_else(|e| e.into_inner());
            *done = true;
            call.finished.notify_one();
        } else if (*task).main {
            let main: unsafe extern "C" fn() -> i32 = std::mem::transmute(code);
            MAIN_STATUS.store(main() as usize, Ordering::SeqCst);
            super::panics::unheld(&mut *task);
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
            super::panics::unheld(&mut *task);
            // Before whoever waits for it hears: the request may end then.
            if (*task).flight != 0 {
                super::diagnostics::step((*task).flight, (*task).id, super::diagnostics::Did::Ended, Vec::new());
            }
            // A panic it ended with goes to its `recover`, or to whoever
            // waits for it.
            if let Some(panic) = (*task).unwinding.take() {
                attend(false);
                (*join).panic = super::panics::deliver(&*task, panic);
            }
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
/// The tasks alive, in shards that a task's address picks, so that the
/// threads that start and end tasks seldom wait for each other.
static TASKS: [Mutex<Tasks>; SHARDS] = [const { Mutex::new(Tasks::new()) }; SHARDS];
const SHARDS: usize = 16;

/// A shard's tasks, each in a slot it knows, so that a task comes and goes
/// without a search; a slot let go of, 0, serves the next.
struct Tasks {
    slots: Vec<usize>,
    free: Vec<usize>,
}

impl Tasks {
    const fn new() -> Tasks {
        Tasks { slots: Vec::new(), free: Vec::new() }
    }

    fn iter(&self) -> impl Iterator<Item = &usize> {
        self.slots.iter().filter(|&&task| task != 0)
    }

    fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }
}

fn shard(task: usize) -> std::sync::MutexGuard<'static, Tasks> {
    TASKS[(task >> 6) % SHARDS].lock().unwrap_or_else(|e| e.into_inner())
}

fn register(task: *mut Task) {
    let mut shard = shard(task as usize);
    let slot = match shard.free.pop() {
        Some(slot) => {
            shard.slots[slot] = task as usize;
            slot
        }
        None => {
            shard.slots.push(task as usize);
            shard.slots.len() - 1
        }
    };
    unsafe { (*task).slot = slot };
}

fn unregister(task: *mut Task) {
    let mut shard = shard(task as usize);
    let slot = unsafe { (*task).slot };
    shard.slots[slot] = 0;
    shard.free.push(slot);
}

/// Every task alive, with every shard locked, in order: while they are, no
/// task is freed.
struct All(Vec<std::sync::MutexGuard<'static, Tasks>>);

impl All {
    fn iter(&self) -> impl Iterator<Item = &usize> {
        self.0.iter().flat_map(|shard| shard.iter())
    }

    fn len(&self) -> usize {
        self.0.iter().map(|shard| shard.len()).sum()
    }
}

fn tasks() -> All {
    All(TASKS.iter().map(|shard| shard.lock().unwrap_or_else(|e| e.into_inner())).collect())
}

/// What the scheduler holds now, for `tondo counters`.
pub(crate) struct Scheduler {
    pub live: usize,
    pub ready: usize,
    pub workers: usize,
    pub idle: usize,
    pub timers: usize,
    /// Workers inside a call of C, and the calls of C made so far.
    pub in_c: usize,
    pub c_calls: usize,
}

/// How many times workers looked for a task: what moves while anything
/// does.
pub(crate) fn turns() -> u64 {
    queue().picks as u64
}

/// The tasks ready to run, that wait for a worker.
pub(crate) fn ready_tasks() -> usize {
    scheduler().ready
}

pub(crate) fn scheduler() -> Scheduler {
    let live = tasks().len();
    let queue = queue();
    let waiting = waiting_next().len() + waiting_local().len();
    let (ready, workers, idle) = (queue.runnable.len() + waiting, queue.workers, queue.idle);
    let timers = queue.timers.len().saturating_sub(queue.stale);
    drop(queue);
    let callers = CALLERS.lock().unwrap_or_else(|e| e.into_inner());
    let depths = callers.iter().map(|c| (c.depth.load(Ordering::Relaxed), c.calls.load(Ordering::Relaxed)));
    // `calls` counts entries and exits.
    let (in_c, c_calls) = depths
        .fold((0, 0), |(inside, made), (depth, calls)| (inside + usize::from(depth > 0), made + (calls + depth) / 2));
    Scheduler { live, ready, workers, idle, timers, in_c, c_calls }
}

/// How many tasks are alive.
pub(crate) fn live_tasks() -> usize {
    tasks().len()
}

/// The tasks alive now.
pub(crate) fn alive() -> std::collections::HashSet<usize> {
    tasks().iter().copied().collect()
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
            let origin = || super::trace::stack(super::trace::walk(0, (0, 0)), (*task).origin, "TITLE");
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

/// The furthest deadline of a `within` that a cleanup of a cancelled task
/// entered, which has until then (11.15), in nanoseconds after `EPOCH`: a
/// program that ends waits for it beyond `PANIC_GRACE`.
static GRACED: AtomicU64 = AtomicU64::new(0);
static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// When a program that ends may stop waiting for its cleanups: `PANIC_GRACE`
/// after `from`, or the deadline of a `within` one of them entered.
fn cleaned_by(from: Instant) -> Instant {
    (from + PANIC_GRACE).max(*EPOCH + Duration::from_nanos(GRACED.load(Ordering::SeqCst)))
}

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
            let mut done = call.done.lock().unwrap_or_else(|e| e.into_inner());
            *done = true;
            call.finished.notify_one();
            drop(done);
            // Its thread may be working for it, and sleep.
            drop(queue());
            READY.notify_all();
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
    // Read here, a call of the program's, and not first by the thread below,
    // which a replay does not have.
    std::sync::LazyLock::force(&EPOCH);
    // The program ends after a while, unless it replays a recording, which
    // ends where the recording does.
    static WATCHDOG: std::sync::Once = std::sync::Once::new();
    WATCHDOG.call_once(|| {
        if super::replay::replaying() {
            return;
        }
        let _outside = super::replay::Outside::enter();
        let from = Instant::now();
        spawn_thread(Some(64 * 1024), move || {
            while Instant::now() < cleaned_by(from) {
                std::thread::sleep(cleaned_by(from).saturating_duration_since(Instant::now()));
            }
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

/// Ends the program as a signal nothing listens for does, from a task:
/// cancels every task, this one too, which unwinds at its next wait.
pub(crate) fn abandon_all(status: i32) {
    abandon(status);
    let me = current();
    if !me.is_null() {
        unsafe { cancel_task(me) };
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
    // How long this takes is no part of a recording.
    let _outside = super::replay::Outside::enter();
    let from = Instant::now();
    while live_tasks() > 0 && Instant::now() < cleaned_by(from) {
        std::thread::sleep(Duration::from_millis(1));
    }
    Some(status as i32)
}

/// A new task, in `place` or in a block of its own when that is null.
fn new_task(
    stack: Option<Stack>,
    body: *mut u8,
    code: *const u8,
    kind: ResultKind,
    join: *mut JoinObject,
    place: *mut Task,
) -> *mut Task {
    super::diagnostics::TASKS_SPAWNED.add(1);
    static IDS: AtomicU64 = AtomicU64::new(1);
    let sp = stack.as_ref().map_or(std::ptr::null_mut(), |stack| unsafe { initial_stack(stack.top()) });
    // `spawn detached` said so just before (11.11).
    let detached = unsafe { current().as_mut() }.is_some_and(|parent| std::mem::take(&mut parent.detaching));
    let id = IDS.fetch_add(1, Ordering::Relaxed);
    let name = match unsafe { current().as_mut() }.filter(|_| super::race::on() && noting()) {
        Some(parent) => {
            parent.started += 1;
            let name = parent.name.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(parent.started);
            noted(format!("n {name}\n"));
            name
        }
        None => id,
    };
    // A debugger that stops tasks as they start stops this one, but the
    // main task.
    if !join.is_null() {
        super::debugger::began(id);
    }
    let task = Task {
        mark: TASK_MARK,
        sp,
        id,
        slot: 0,
        stack,
        body,
        code,
        kind,
        join,
        cleanups: Vec::new(),
        state: AtomicU8::new(RUNNING),
        cancelled: AtomicBool::new(false),
        deferring: 0,
        cut: false,
        showing: false,
        again: AtomicBool::new(false),
        waiting: std::ptr::null(),
        waiting_lock: AtomicBool::new(false),
        name,
        started: 0,
        held: Vec::new(),
        blocked: AtomicUsize::new(0),
        blocked_kind: AtomicU8::new(0),
        blocked_since: AtomicU64::new(0),
        flight: 0,
        retrying: false,
        probes: Vec::new(),
        finished: false,
        main: join.is_null(),
        foreign: std::ptr::null(),
        // A task started on a virtual clock keeps it, and its labels.
        in_domain: !current().is_null() && unsafe { (*current()).in_domain },
        origin: None,
        labels: std::sync::atomic::AtomicU32::new(labels()),
        own_labels: std::sync::atomic::AtomicU32::new(own_labels()),
        context: super::contexts::inherited(),
        withins: Vec::new(),
        grace: None,
        expired: false,
        inherited: AtomicU64::new(if detached { u64::MAX } else { super::contexts::deadline_nanos() }),
        recovers: Vec::new(),
        // A task started inside a `recover` belongs to it, until detached.
        owner: if detached { None } else { super::panics::owner_of_new() },
        free: AtomicBool::new(detached),
        detaching: false,
        alarm: AtomicBool::new(false),
        unwinding: None,
        paused: Vec::new(),
        caught: None,
        priority: exploring().map_or(0, |seed| seeded(seed) | 1 << 63),
    };
    let task = match place.is_null() {
        true => Box::into_raw(Box::new(task)),
        false => unsafe {
            place.write(task);
            place
        },
    };
    register(task);
    super::clock::woke(task);
    task
}

/// Runs `tondo_main` as the first task on the calling thread, which becomes
/// a worker; returns the program's exit status.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub fn run_main(code: unsafe extern "C" fn() -> i32) -> i32 {
    let others = os_threads().unwrap_or(0).saturating_sub(RUNTIME_THREADS.load(Ordering::SeqCst));
    BEFORE.store(others, Ordering::SeqCst);
    // Before any compiled code reads `ATTENTION`.
    exploring();
    let task = new_task(
        Some(Stack::new(MAIN_STACK)),
        std::ptr::null_mut(),
        code as *const u8,
        ResultKind::Integer,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    schedule(task);
    let status = work(true);
    // A thread of C that waits for the program finishes its call, as the
    // end of the program waits for it.
    super::replay::waits();
    status
}

// ---------------------------------------------------------------------------
// Cleanups
// ---------------------------------------------------------------------------

fn with_cleanups<R>(f: impl FnOnce(&mut Vec<(Cleanup, usize)>) -> R) -> R {
    let task = current();
    thread_local! {
        /// Cleanups registered outside any task, as in unit tests.
        static LOOSE_CLEANUPS: std::cell::RefCell<Vec<(Cleanup, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
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
/// The task of an id, while it lives.
pub(crate) fn by_id(id: u64) -> Option<*mut Task> {
    tasks().iter().find(|&&t| unsafe { (*(t as *mut Task)).id } == id).map(|&t| t as *mut Task)
}
