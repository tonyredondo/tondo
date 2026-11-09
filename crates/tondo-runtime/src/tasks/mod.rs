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

mod workers;
pub(crate) use workers::*;

mod work;
use work::*;

mod park;
pub(crate) use park::*;

mod stacks;
pub(crate) use stacks::*;

mod registry;
pub(crate) use registry::*;

mod ending;
pub(crate) use ending::*;

mod threads;
pub(crate) use threads::*;

mod timers;
use timers::*;

mod stop;
pub(crate) use stop::*;

mod foreign;
pub(crate) use foreign::*;

mod inspect;
pub(crate) use inspect::*;

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

pub(crate) mod tools;

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
    /// The wait it waited with last, which its next takes again when
    /// nothing else holds it (`Wait::new`).
    kept_wait: Option<Arc<Wait>>,
    /// Under `tondo test`, what it goes by in the orders of a test, from
    /// who started it, and how many it started.
    name: u64,
    started: u64,
    /// The `std.sync` locks it holds, whether exclusively, and where it
    /// took them.
    pub(crate) held: Vec<(usize, bool, *const Str)>,
    /// When it blocked there, in milliseconds of `inspect::millis`.
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
    let name = match unsafe { current().as_mut() }.filter(|_| tools::listens(tools::RACES) && noting()) {
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
        tools::began(id);
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
        kept_wait: None,
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
        priority: exploring().map_or(0, first_priority),
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
    // After a panic, the other tasks' cleanups, which no other worker runs
    // when this is the only one: exploring orders, recording or on one core.
    if ENDING.load(Ordering::SeqCst) != usize::MAX && queue().workers == 1 {
        work_until(false, Some(&|| live_tasks() == 0));
    }
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
