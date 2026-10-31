//! What `tondo trace` records (toolchain 7.5): while a tool traces the
//! program, the runtime's events, each kept by the thread it happens on,
//! and every millisecond of processor time a sample of where a thread is,
//! for a profile. Recording nothing costs a load where each event would be.

use std::cell::{Cell, UnsafeCell};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

static ON: AtomicBool = AtomicBool::new(false);
/// Whether threads are sampled: while a tool traces the program, or while
/// a test build profiles its tests.
static SAMPLING: AtomicBool = AtomicBool::new(false);

/// Whether a tool traces the program now.
#[inline]
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Nanoseconds of the monotonic clock, which a signal handler may read.
pub(crate) fn now() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// The frames of a stack a trace keeps: return addresses, innermost first.
#[derive(Clone, Copy)]
pub(crate) struct Stack {
    frames: [usize; DEPTH],
    count: usize,
}

const DEPTH: usize = 64;

impl Stack {
    pub(crate) const EMPTY: Stack = Stack { frames: [0; DEPTH], count: 0 };

    pub(crate) fn of(addresses: impl Iterator<Item = usize>) -> Stack {
        let mut stack = Stack::EMPTY;
        for address in addresses.take(DEPTH) {
            stack.frames[stack.count] = address;
            stack.count += 1;
        }
        stack
    }

    pub(crate) fn frames(&self) -> &[usize] {
        &self.frames[..self.count]
    }
}

/// The frames an event keeps, as many as it has.
pub(crate) fn frames(addresses: impl Iterator<Item = usize>) -> Box<[usize]> {
    addresses.take(DEPTH).collect()
}

pub(crate) enum Event {
    /// A task started to run on this thread.
    Run {
        task: u64,
    },
    /// It stopped: it finished, waits where the stack says, or let others
    /// run.
    Stop {
        task: u64,
        why: Stop,
    },
    /// A task started another, whose code is at `code`, from `at`.
    Spawn {
        task: u64,
        parent: u64,
        code: usize,
        at: Box<[usize]>,
    },
    /// The collector stopped the workers for `took` nanoseconds.
    Pause {
        took: u64,
    },
    /// A call of C that took `took` nanoseconds, made from `at`.
    Call {
        task: u64,
        took: u64,
        at: Box<[usize]>,
    },
    Panic {
        task: u64,
        message: String,
    },
}

pub(crate) enum Stop {
    Done,
    Waits(Box<[usize]>),
    Yields,
}

/// A thread's events since the tool last took them.
struct Buffer {
    thread: u32,
    events: Mutex<Vec<(u64, Event)>>,
}

static BUFFERS: Mutex<Vec<&'static Buffer>> = Mutex::new(Vec::new());
static THREADS: AtomicU32 = AtomicU32::new(0);
/// Events a thread keeps between two reads of the tool; more are dropped,
/// and counted.
const KEPT: usize = 1 << 20;
static DROPPED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static BUFFER: Cell<Option<&'static Buffer>> = const { Cell::new(None) };
    static THREAD: Cell<u32> = const { Cell::new(0) };
}

/// This thread's number in a trace, from 1, which the main thread takes
/// first.
pub(crate) fn thread() -> u32 {
    THREAD.with(|thread| {
        if thread.get() == 0 {
            thread.set(THREADS.fetch_add(1, Ordering::Relaxed) + 1);
        }
        thread.get()
    })
}

pub(crate) fn record(event: Event) {
    let at = now();
    let buffer = BUFFER.with(|cell| {
        if let Some(buffer) = cell.get() {
            return buffer;
        }
        let buffer: &'static Buffer = Box::leak(Box::new(Buffer { thread: thread(), events: Mutex::new(Vec::new()) }));
        BUFFERS.lock().unwrap_or_else(|e| e.into_inner()).push(buffer);
        cell.set(Some(buffer));
        buffer
    });
    let mut events = buffer.events.lock().unwrap_or_else(|e| e.into_inner());
    if events.len() < KEPT {
        events.push((at, event));
    } else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The events recorded since the last time, each with its time and
/// thread, and how many were dropped.
pub(crate) fn take_events() -> (Vec<(u64, u32, Event)>, u64) {
    let mut out = Vec::new();
    for buffer in BUFFERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let events = std::mem::take(&mut *buffer.events.lock().unwrap_or_else(|e| e.into_inner()));
        out.extend(events.into_iter().map(|(at, event)| (at, buffer.thread, event)));
    }
    out.sort_by_key(|(at, ..)| *at);
    (out, DROPPED.swap(0, Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Samples
// ---------------------------------------------------------------------------

/// A sample: where a thread was, when. The signal handler that writes it
/// may not allocate or lock, so samples go in a ring made in advance, each
/// slot marked with the sequence number of the sample it holds.
struct Slot {
    written: AtomicU64,
    thread: UnsafeCell<u32>,
    at: UnsafeCell<u64>,
    stack: UnsafeCell<Stack>,
    /// The labels of the task that ran (`labels`).
    labels: UnsafeCell<u32>,
}

unsafe impl Sync for Slot {}

const SLOTS: usize = 8192;
/// 4 MiB, made when the first trace starts.
static RING: std::sync::atomic::AtomicPtr<Slot> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

fn ring() -> &'static [Slot] {
    let ring = RING.load(Ordering::Acquire);
    match ring.is_null() {
        true => &[],
        false => unsafe { std::slice::from_raw_parts(ring, SLOTS) },
    }
}
/// The sequence number of the next sample, and of the next one to read.
static NEXT: AtomicU64 = AtomicU64::new(0);
static READ: Mutex<u64> = Mutex::new(0);
const BUSY: u64 = u64::MAX;

/// The handler of `SIGPROF`, which the processor time of the process
/// raises every millisecond on the thread that runs.
extern "C" fn sample(_: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // A handler leaves `errno` as it found it.
    #[cfg(target_os = "linux")]
    let errno = unsafe { libc::__errno_location() };
    #[cfg(target_os = "macos")]
    let errno = unsafe { libc::__error() };
    let saved = unsafe { *errno };
    let (_, pc, fp, _) = unsafe { crate::trace::faulted(info, context) };
    // A dump asks a thread that runs on where it is with this signal too.
    crate::tasks::answer_where(pc, fp);
    // On macOS a worker is signalled whether it runs or waits: only one
    // that runs a task makes a sample.
    if !SAMPLING.load(Ordering::Relaxed) || (cfg!(target_os = "macos") && crate::tasks::current().is_null()) {
        unsafe { *errno = saved };
        return;
    }
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    // In C, which may keep no frame pointers, the trace goes on from where
    // the program called it.
    let stack = match crate::tasks::c_frame() {
        Some((called, from)) if !crate::trace::compiled(pc) => {
            Stack::of([pc, from].into_iter().chain(crate::trace::walk(called, bounds)))
        }
        _ => Stack::of(std::iter::once(pc).chain(crate::trace::walk(fp, bounds))),
    };
    let ring = ring();
    if ring.is_empty() {
        unsafe { *errno = saved };
        return;
    }
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let slot = &ring[sequence as usize % SLOTS];
    slot.written.store(BUSY, Ordering::Relaxed);
    std::sync::atomic::fence(Ordering::Release);
    unsafe {
        *slot.thread.get() = thread();
        *slot.at.get() = now();
        *slot.stack.get() = stack;
        *slot.labels.get() = crate::tasks::labels();
    }
    slot.written.store(sequence + 1, Ordering::Release);
    unsafe { *errno = saved };
}

/// A sample: when, on which thread, where, and the labels of its task.
pub(crate) type Sample = (u64, u32, Stack, u32);

/// The samples taken since the last time, and how many were lost because
/// the ring went round before the tool took them.
pub(crate) fn take_samples() -> (Vec<Sample>, u64) {
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    let next = NEXT.load(Ordering::Acquire);
    let mut lost = 0;
    if next - *read > SLOTS as u64 {
        lost = next - *read - SLOTS as u64;
        *read = next - SLOTS as u64;
    }
    let mut out = Vec::new();
    let ring = ring();
    while *read < next && !ring.is_empty() {
        let slot = &ring[*read as usize % SLOTS];
        let before = slot.written.load(Ordering::Acquire);
        if before != *read + 1 {
            // Still being written: the next time.
            if before == BUSY || before <= *read {
                break;
            }
            lost += 1;
            *read += 1;
            continue;
        }
        let copied = unsafe { (*slot.at.get(), *slot.thread.get(), *slot.stack.get(), *slot.labels.get()) };
        std::sync::atomic::fence(Ordering::Acquire);
        if slot.written.load(Ordering::Relaxed) == before {
            out.push(copied);
        } else {
            lost += 1;
        }
        *read += 1;
    }
    (out, lost)
}

// ---------------------------------------------------------------------------
// Starting and stopping
// ---------------------------------------------------------------------------

/// Installs the handler of `SIGPROF`, once.
pub(crate) fn install() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = sample as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART | libc::SA_ONSTACK;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGPROF, &action, std::ptr::null_mut());
    });
}

pub(crate) fn start() {
    ON.store(true, Ordering::SeqCst);
    sample_threads();
}

pub(crate) fn stop() {
    stop_sampling();
    ON.store(false, Ordering::SeqCst);
}

/// Microseconds of processor time between two samples of the threads,
/// which `profile.startCpu(every:)` sets before they start.
pub(crate) static CPU_EVERY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1000);

/// Samples the threads every `CPU_EVERY` of processor time, without
/// recording events.
pub(crate) fn sample_threads() {
    install();
    if RING.load(Ordering::Acquire).is_null() {
        RING.store(crate::zeroed(SLOTS), Ordering::Release);
    }
    if SAMPLING.swap(true, Ordering::SeqCst) {
        return;
    }
    // macOS sends the signal of the process's processor time to whichever
    // thread it likes, mostly the main one, however idle; a thread of the
    // runtime signals each worker every millisecond instead.
    if cfg!(target_os = "macos") {
        let _ = crate::tasks::try_spawn_thread(Some(64 * 1024), || {
            while SAMPLING.load(Ordering::Relaxed) {
                crate::tasks::signal_workers(libc::SIGPROF);
                std::thread::sleep(std::time::Duration::from_micros(CPU_EVERY.load(Ordering::Relaxed) as u64));
            }
        });
    } else {
        timer(CPU_EVERY.load(Ordering::Relaxed));
    }
}

pub(crate) fn stop_sampling() {
    timer(0);
    SAMPLING.store(false, Ordering::SeqCst);
}

/// Raises `SIGPROF` every `micros` of the process's processor time, or
/// never with 0.
fn timer(micros: i64) {
    let every = libc::timeval { tv_sec: (micros / 1_000_000) as _, tv_usec: (micros % 1_000_000) as _ };
    let timer = libc::itimerval { it_interval: every, it_value: every };
    unsafe { libc::setitimer(libc::ITIMER_PROF, &timer, std::ptr::null_mut()) };
}
