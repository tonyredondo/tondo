//! Recording a run and replaying it (toolchain 7.6): with
//! `TONDO_RT_RECORD=<file>` the runtime writes what each call of the C
//! library the program's thread makes gave it, and with
//! `TONDO_RT_REPLAY=<file>` gives those results back instead of making the
//! calls, so that the run happens again, the same, without touching the
//! world. The tasks run on that one thread in an order a seed gives
//! (`tasks::exploring`), and `tondo` links these wrappers in place of the C
//! library's functions (`link::elf`, `tondo_rr_<name>` for `<name>`); each
//! calls the real one as `tondo_real_<name>`.
//!
//! A call another thread makes is the world's, as the debugger's own. The
//! threads of C the program starts run again in a replay, for real, and
//! what they do besides calling the program is not recorded. When one calls
//! the program it takes the baton (`Baton`), and a replay holds it back
//! until the recording says.

#![cfg_attr(not(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))), allow(dead_code))]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU8, AtomicU64, Ordering};

const OFF: u8 = 0;
const RECORD: u8 = 1;
const REPLAY: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(OFF);
/// The linker gave the program the wrappers: `tondo_rr_getpid` ran.
static WRAPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The probes the program's thread passed, which place each call.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// A replay shows what the program writes from this probe on: the debugger
/// that goes back runs it again from the start, and has shown the rest.
static SHOWN_FROM: AtomicU64 = AtomicU64::new(0);
/// The debugger that started the replay: its checkpoints end when it does.
static DEBUGGER: AtomicI32 = AtomicI32::new(0);

thread_local! {
    /// This is the program's thread, whose calls are recorded.
    static PROGRAM: Cell<bool> = const { Cell::new(false) };
    /// A wrapper runs here: the calls the real function makes are its own.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

const MAGIC: &[u8; 8] = b"TONDORR2";

/// What a recording holds, as it is written: the calls, each with its
/// function, the probe it came after, its result, `errno` and the bytes it
/// wrote where its arguments point. The file has them in blocks, each after
/// how long it is and how long it is stored, compressed unless these are
/// the same.
struct Log {
    file: Option<std::fs::File>,
    buffer: Vec<u8>,
    /// While replaying, the recording and where the next call is.
    recorded: Vec<u8>,
    at: usize,
}

static LOG: Mutex<Log> = Mutex::new(Log { file: None, buffer: Vec::new(), recorded: Vec::new(), at: 0 });

fn log() -> std::sync::MutexGuard<'static, Log> {
    LOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// One thread at a time runs the program's code, as one worker does in a
/// recorded run, and a thread of C that calls the program waits for its
/// turn: the baton says whose it is. The thread that has it lets go where
/// the program waits, in C or for its tasks, and at a turn of the worker
/// when another waits; each time it changes hands the recording says so,
/// and a replay moves it at the same places, whenever the threads of C get
/// there. Tickets keep the order the threads asked in.
struct Baton {
    /// The thread that had it last, by the order they first took it.
    owner: u32,
    held: bool,
    known: u32,
    tickets: u64,
    serving: u64,
    /// How the last thread let go, which a move of the baton keeps: at one
    /// place of the program a call of C and the end of a callback may come
    /// one after the other.
    how: u8,
}

/// In a call of C, at a turn, in a wait, at the end of a call from C; and,
/// in a replay, where a thread reads the recording and any move is due.
const IN_C: u8 = 0;
const AT_TURN: u8 = 1;
const WAITS: u8 = 2;
const CALL_ENDS: u8 = 3;
const ANY: u8 = 4;

static BATON: Mutex<Baton> = Mutex::new(Baton { owner: 1, held: true, known: 1, tickets: 1, serving: 0, how: WAITS });
static MOVED: std::sync::Condvar = std::sync::Condvar::new();
/// The calls of C the program made, which with the probes it passed place
/// a move of the baton.
static C_CALLS: AtomicU64 = AtomicU64::new(0);
/// Threads of C run in this replay, which a fork would lose.
static THREADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How long a replay waits with no thread running the program and nothing
/// of the recording used: then it says so and ends, rather than hang.
const STUCK: std::time::Duration = std::time::Duration::from_secs(30);
/// A move of the baton in a recording, among the calls: the program's calls
/// of C in the place of a result, how it was let go in that of `errno`, and
/// the thread it went to and the function that thread called.
const HAND: u32 = 0x4841_4e44;

thread_local! {
    /// This thread, as `Baton::owner` says; the first one is the program's.
    static NUMBER: Cell<u32> = const { Cell::new(0) };
    static HOLDS: Cell<bool> = const { Cell::new(false) };
    /// What tells a thread of C from the others in a replay, where they get
    /// to the program in another order: which `pthread_create` of the
    /// program made it, or what it calls for one the C library started.
    static CALLED: Cell<u64> = const { Cell::new(0) };
    static MADE: Cell<u64> = const { Cell::new(0) };
}

fn baton() -> std::sync::MutexGuard<'static, Baton> {
    BATON.lock().unwrap_or_else(|e| e.into_inner())
}

/// A move of the baton: to which thread, for which function, how it was
/// let go, and how long its entry is.
struct Hand {
    to: u32,
    called: u64,
    how: u8,
    size: usize,
}

/// The move of the baton that comes next in the recording, if the program
/// got to it: until then the thread that has the baton may let go, in C,
/// and takes it again.
fn next_hand(log: &Log) -> Option<Hand> {
    let head = log.recorded.get(log.at..log.at + 25 + 4 + 12)?;
    let tick = u64::from_le_bytes(head[4..12].try_into().expect("8 bytes"));
    let calls = u64::from_le_bytes(head[12..20].try_into().expect("8 bytes"));
    let here = tick == TICKS.load(Ordering::Relaxed) && calls == C_CALLS.load(Ordering::Relaxed);
    if u32::from_le_bytes(head[0..4].try_into().expect("4 bytes")) != HAND || !here {
        return None;
    }
    Some(Hand {
        to: u32::from_le_bytes(head[29..33].try_into().expect("4 bytes")),
        called: u64::from_le_bytes(head[33..41].try_into().expect("8 bytes")),
        how: head[20],
        size: head.len(),
    })
}

/// This thread takes the baton, when it is its turn.
fn take() {
    if HOLDS.get() {
        return;
    }
    // Its waits read the clock, which is not the program's.
    let _outside = Outside::enter();
    let mut baton = baton();
    if MODE.load(Ordering::Relaxed) == RECORD {
        let ticket = baton.tickets;
        baton.tickets += 1;
        while baton.serving != ticket {
            baton = MOVED.wait(baton).unwrap_or_else(|e| e.into_inner());
        }
        if NUMBER.get() == 0 {
            baton.known += 1;
            NUMBER.set(baton.known);
        }
        if baton.owner != NUMBER.get() {
            baton.owner = NUMBER.get();
            let mut log = log();
            let buffer = &mut log.buffer;
            buffer.extend_from_slice(&HAND.to_le_bytes());
            buffer.extend_from_slice(&TICKS.load(Ordering::Relaxed).to_le_bytes());
            buffer.extend_from_slice(&C_CALLS.load(Ordering::Relaxed).to_le_bytes());
            buffer.extend_from_slice(&i32::from(baton.how).to_le_bytes());
            buffer.push(1);
            buffer.extend_from_slice(&12u32.to_le_bytes());
            buffer.extend_from_slice(&baton.owner.to_le_bytes());
            buffer.extend_from_slice(&CALLED.get().to_le_bytes());
        }
    } else {
        let (mut seen, mut since) = (None, std::time::Instant::now());
        loop {
            if !baton.held {
                let mut log = log();
                let now = Some((log.at, TICKS.load(Ordering::Relaxed)));
                if now != seen {
                    (seen, since) = (now, std::time::Instant::now());
                }
                let next = next_hand(&log);
                let number = NUMBER.get();
                let how = baton.how;
                match next.filter(|hand| hand.how == how || how == ANY) {
                    Some(hand) if hand.to == number => {
                        log.at += hand.size;
                        baton.owner = number;
                        break;
                    }
                    Some(hand) if number == 0 && hand.to > baton.known && hand.called == CALLED.get() => {
                        log.at += hand.size;
                        NUMBER.set(hand.to);
                        baton.known = hand.to;
                        baton.owner = hand.to;
                        break;
                    }
                    None if baton.owner == number => break,
                    _ => {}
                }
                if since.elapsed() > STUCK {
                    drop(log);
                    drop(baton);
                    INSIDE.set(true);
                    crate::debugger::stop_for_good(
                        "the replay ends here: the threads of C and the program wait for each other, \
                         in an order the recording does not have: a thread of C may have called the \
                         program at another place, or not at all"
                            .into(),
                    );
                }
            } else {
                since = std::time::Instant::now();
            }
            let waited = MOVED.wait_timeout(baton, std::time::Duration::from_millis(10));
            baton = waited.unwrap_or_else(|e| e.into_inner()).0;
        }
    }
    baton.held = true;
    HOLDS.set(true);
    MOVED.notify_all();
}

/// This thread lets go of the baton, if it has it.
fn let_go(how: u8) {
    if HOLDS.replace(false) {
        let mut baton = baton();
        baton.held = false;
        baton.serving += 1;
        baton.how = how;
        MOVED.notify_all();
    }
}

/// While it lives this thread has the baton: what it records or replays
/// comes in its place among the program's.
pub(crate) struct Held(bool);

impl Drop for Held {
    fn drop(&mut self) {
        if self.0 {
            let_go(WAITS);
        }
    }
}

/// The baton for this thread. In a replay one that has it lets go first
/// where the recording moves it.
pub(crate) fn held() -> Held {
    if !on() {
        return Held(false);
    }
    let had = HOLDS.get();
    if had && replaying() && next_hand(&log()).is_some() {
        let_go(ANY);
    }
    take();
    Held(!had)
}

/// The program calls C, which may take long.
pub(crate) fn calls_c() {
    if on() {
        C_CALLS.fetch_add(1, Ordering::Relaxed);
        let_go(IN_C);
    }
}

/// The program waits for what comes: another thread may run it.
pub(crate) fn waits() {
    if on() {
        let_go(WAITS);
    }
}

/// The wait ended.
pub(crate) fn waited() {
    if on() {
        take();
    }
}

/// A turn of the worker: another thread that waits for the baton has it
/// now, or where the recording says.
pub(crate) fn turn() {
    if !on() {
        return;
    }
    let moves = match replaying() {
        true => next_hand(&log()).is_some_and(|hand| hand.how == AT_TURN),
        false => {
            let baton = baton();
            baton.tickets > baton.serving + 1
        }
    };
    if moves {
        let_go(AT_TURN);
        take();
    }
}

/// A thread of C calls `function` of the program, which it runs when it
/// has the baton, as a thread of the program, until `left`.
pub(crate) fn entered(function: usize) {
    if on() {
        THREADED.store(true, Ordering::Relaxed);
        CALLED.set(match MADE.get() {
            0 => function as u64,
            made => made | 1 << 63,
        });
        take();
        let first = !PROGRAM.replace(true);
        // The keys of its hash maps, as in `begin`.
        if first {
            let _ = std::collections::hash_map::RandomState::new();
        }
    }
}

pub(crate) fn left() {
    if on() {
        PROGRAM.set(false);
        let_go(CALL_ENDS);
    }
}

/// Threads of C run here: a replay cannot be forked and go on.
pub(crate) fn threaded() -> bool {
    THREADED.load(Ordering::Relaxed)
}

/// Whether the runtime records or replays the program.
#[inline]
pub(crate) fn on() -> bool {
    MODE.load(Ordering::Relaxed) != OFF
}

/// From the probe `tick` on, a replay shows what the program writes.
pub(crate) fn shown_from(tick: u64) {
    SHOWN_FROM.store(tick, Ordering::Relaxed);
}

/// Whether it replays a recording.
pub(crate) fn replaying() -> bool {
    MODE.load(Ordering::Relaxed) == REPLAY
}

/// A probe the program's thread passed.
#[inline]
pub(crate) fn tick() {
    if PROGRAM.get() {
        TICKS.fetch_add(1, Ordering::Relaxed);
    }
}

/// The probes the program's thread passed so far.
pub(crate) fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Whether `every` probes passed since this last said so, in `next`. A
/// loop's probes may never fall on a multiple of it.
#[inline]
pub(crate) fn look_at(next: &AtomicU64, every: u64) -> bool {
    let ticks = TICKS.load(Ordering::Relaxed);
    let due = ticks >= next.load(Ordering::Relaxed);
    if due {
        next.store(ticks + every, Ordering::Relaxed);
    }
    due
}

/// While it lives, the calls of the program's thread are not the
/// program's but the debugger's, or the runtime's own, which a replay makes
/// again.
pub(crate) struct Outside(bool);

impl Outside {
    pub(crate) fn enter() -> Outside {
        Outside(INSIDE.replace(true))
    }
}

impl Drop for Outside {
    fn drop(&mut self) {
        INSIDE.set(self.0);
    }
}

/// Reads the settings, before the program's first call: the file to record
/// to, or the recording to replay. Gives whether either is on.
pub(crate) fn start() -> bool {
    let config = super::config::config();
    let asked = config.record.is_some() || config.replay.is_some();
    if asked && !cfg!(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))) {
        super::runtime_fault("a run is recorded and replayed on Linux and macOS");
    }
    let able = unsafe { libc::getpid() } != 0 && WRAPPED.load(Ordering::Relaxed);
    if asked && !able {
        super::runtime_fault("this executable cannot record its runs: it is not a debug build that tondo linked");
    }
    // A child of a recorded run records its own, when it can, among its
    // parent's recording.
    let parent = config.record_children.clone();
    if parent.is_some() {
        unsafe { std::env::remove_var("TONDO_RT_RECORD_CHILDREN") };
    }
    let child = parent.filter(|_| !asked && able).and_then(|dir| recording_of_child(Path::new(&dir)));
    // What the Rust library learns of the system the first time it asks,
    // as whether `statx` exists, it learns here, the same way in a run and
    // in its replay, whose runtime does more before the program starts.
    if asked || child.is_some() {
        let _ = std::fs::metadata("/");
    }
    if let Some(path) = config.record.clone().map(PathBuf::from).or(child) {
        unsafe { std::env::remove_var("TONDO_RT_RECORD") };
        directory_of(&path);
        let cannot = |e: std::io::Error| -> ! {
            super::runtime_fault(&format!("cannot record to {}: {e}", std::path::Path::new(&path).display()))
        };
        let mut file = std::fs::File::create(&path).unwrap_or_else(|e| cannot(e));
        std::io::Write::write_all(&mut file, MAGIC).unwrap_or_else(|e| cannot(e));
        let mut log = log();
        log.file = Some(file);
        MODE.store(RECORD, Ordering::Relaxed);
        let last = config.record_last.as_deref().and_then(|ms| ms.parse().ok()).unwrap_or(0);
        unsafe { std::env::remove_var("TONDO_RT_RECORD_LAST") };
        LAST.store(last, Ordering::Relaxed);
        let hold = config.record_hold.as_deref().and_then(|ms| ms.parse().ok());
        unsafe { std::env::remove_var("TONDO_RT_RECORD_HOLD") };
        HOLD_FOR.store(hold.unwrap_or(HOUR), Ordering::Relaxed);
        unsafe extern "C" {
            fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
        }
        extern "C" fn flushed(_: *mut u8) {
            store(true);
        }
        unsafe { __cxa_atexit(flushed, std::ptr::null_mut(), std::ptr::null_mut()) };
    } else if let Some(path) = config.replay.clone().map(PathBuf::from) {
        unsafe { std::env::remove_var("TONDO_RT_REPLAY") };
        directory_of(&path);
        let recorded = calls_from(&path, 0).unwrap_or_else(|message| super::runtime_fault(&message));
        let mut log = log();
        log.recorded = recorded;
        log.at = 0;
        MODE.store(REPLAY, Ordering::Relaxed);
        let from = config.shown_from.as_deref().and_then(|t| t.parse().ok()).unwrap_or(0);
        SHOWN_FROM.store(from, Ordering::Relaxed);
        unsafe { std::env::remove_var("TONDO_RT_SHOWN_FROM") };
        DEBUGGER.store(unsafe { libc::getppid() }, Ordering::Relaxed);
    }
    on()
}

/// From here on, this thread's calls are the program's.
pub(crate) fn begin() {
    if on() {
        PROGRAM.set(true);
        NUMBER.set(1);
        HOLDS.set(true);
        // The keys of the thread's hash maps, which the Rust library draws
        // from the system the first time it makes one: here, a call the
        // recording has, and not where the program, or the debugger at its
        // probes, first makes one.
        let _ = std::collections::hash_map::RandomState::new();
    }
}

/// What the runtime gives the program of what a recording cannot follow,
/// as a profile's samples, which a signal and a thread of its own take:
/// a recorded run keeps the bytes `make` gives, and its replay gives them
/// back without making them.
pub(crate) fn kept(make: impl FnOnce() -> Vec<u8>) -> Vec<u8> {
    #[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
    return calls::kept(make);
    #[cfg(not(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))))]
    make()
}

/// The worker would wait for a task to run: a replay has nothing to wait
/// for once its recording ends.
pub(crate) fn waiting() {
    // Another thread comes first, which may have ended what this one waits
    // for: the worker looks again.
    if next_hand(&log()).is_some() {
        drop(held());
    } else {
        #[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
        calls::waiting();
    }
}

/// A signal the worker takes at its turn `turn`, in the recording.
pub(crate) fn signal_came(turn: u64, signal: i32) {
    #[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
    calls::signal_came(turn, signal);
    let _ = (turn, signal);
}

/// The signal the recording has at the worker's turn `turn`, if any.
pub(crate) fn signal_at(turn: u64) -> Option<i32> {
    #[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
    return calls::signal_at(turn);
    #[cfg(not(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = turn;
        None
    }
}

/// The process ends on a fault, its exit handlers not run: the recording
/// is written, unless the fault came while it was being added to.
pub(crate) fn ending() {
    if MODE.load(Ordering::Relaxed) == RECORD && LOG.try_lock().is_ok() {
        flush();
    }
}

/// The wrappers, in what `tondo` links on Linux, where the linker gives
/// them the C library's calls.
#[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
mod calls;
mod recording;
pub(crate) use recording::*;
