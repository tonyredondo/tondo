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

/// The directory of the recording a run makes or replays.
static RECORDING: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The recording's directory, whole, as children that run elsewhere and
/// the debugger find it.
fn directory_of(calls: &Path) {
    let calls = std::path::absolute(calls).unwrap_or_else(|_| calls.to_path_buf());
    let _ = RECORDING.set(calls.parent().unwrap_or(Path::new("/")).to_path_buf());
}

/// Where a child of the program records its run, when it is a debug build
/// of Tondo: a directory of its own there, named by its process.
pub(crate) fn children() -> Option<PathBuf> {
    RECORDING.get().map(|dir| dir.join("children"))
}

/// In a replay, the recording of the child `pid` started, if it made one.
pub(crate) fn recorded_child(pid: u32) -> Option<PathBuf> {
    let _outside = Outside::enter();
    let dir = children()?.join(pid.to_string());
    (replaying() && dir.join("run.json").is_file()).then_some(dir)
}

/// The recording of this run, a child's in `children`: the program, and
/// what it ran with, as `tondo run --record` writes them; the file its
/// calls go to.
fn recording_of_child(children: &Path) -> Option<PathBuf> {
    let dir = children.join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::copy(std::env::current_exe().ok()?, dir.join("program")).ok()?;
    let quoted = |text: &std::ffi::OsStr| super::diagnostics::quoted(&text.to_string_lossy());
    let (invoked, arguments) = super::env::arguments().split_first()?;
    let arguments: Vec<String> = arguments.iter().map(|a| quoted(a)).collect();
    let environment: Vec<String> =
        std::env::vars_os().map(|(name, value)| format!("{}:{}", quoted(&name), quoted(&value))).collect();
    let seed = super::config::config().schedule.clone().unwrap_or_else(|| "0".into());
    let run = format!(
        "{{\"source\":{},\"arguments\":[{}],\"environment\":{{{}}},\"directory\":{},\"seed\":{},\"tondo\":{}}}",
        quoted(invoked),
        arguments.join(","),
        environment.join(","),
        quoted(std::env::current_dir().unwrap_or_default().as_os_str()),
        super::diagnostics::quoted(&seed),
        super::diagnostics::quoted(env!("CARGO_PKG_VERSION")),
    );
    std::fs::write(dir.join("run.json"), run).ok()?;
    Some(dir.join("calls"))
}

/// The debugger that started this replay is gone.
pub(crate) fn orphaned() -> bool {
    let debugger = DEBUGGER.load(Ordering::Relaxed);
    if debugger <= 1 {
        return false;
    }
    let _outside = Outside::enter();
    // Gone, or ended and not yet waited for.
    #[cfg(target_os = "linux")]
    return match std::fs::read_to_string(format!("/proc/{debugger}/stat")) {
        Ok(stat) => stat.rsplit_once(") ").is_some_and(|(_, state)| state.starts_with(['Z', 'X'])),
        Err(_) => true,
    };
    #[cfg(not(target_os = "linux"))]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let found = unsafe { libc::proc_pidinfo(debugger, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        found != size || info.pbi_status == libc::SZOMB
    }
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

/// How much of its end a recording keeps, in milliseconds, or all of it.
static LAST: AtomicU64 = AtomicU64::new(0);

/// How long a copy waits unused, in milliseconds, as a checkpoint does.
const HOUR: u64 = 3_600_000;
static HOLD_FOR: AtomicU64 = AtomicU64::new(HOUR);

pub(crate) fn hold_for() -> std::time::Duration {
    std::time::Duration::from_millis(HOLD_FOR.load(Ordering::Relaxed))
}

/// A copy of the program held at a probe, for `--last`: where the recording
/// goes on from, in the segment it started; the start of the run is one, in
/// no process.
struct Copied {
    tick: u64,
    pid: i32,
    segment: u32,
    at: std::time::Instant,
}

static COPIES: Mutex<Vec<Copied>> = Mutex::new(Vec::new());
/// The last time a copy was taken, which the next is due after.
static TAKEN: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The file of a segment of the recording: the first is `calls`.
fn segment_path(calls: &Path, segment: u32) -> PathBuf {
    match segment {
        0 => calls.to_path_buf(),
        _ => calls.with_extension(segment.to_string()),
    }
}

/// The calls of a recording from a segment on, to its last.
fn calls_from(calls: &Path, first: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for segment in first.. {
        let path = segment_path(calls, segment);
        let file = match std::fs::read(&path) {
            Ok(file) => file,
            Err(_) if segment > first => break,
            Err(e) => return Err(format!("cannot read the recording {}: {e}", path.display())),
        };
        let blocks = file.strip_prefix(MAGIC).and_then(unpacked);
        out.extend(blocks.ok_or("the file is not a recording of this tondo")?);
    }
    Ok(out)
}

/// Whether a copy of the program is due, as the end of the recording moves
/// on, looked at every so many probes. A copy would lose threads of C, so a
/// run that has them keeps it all.
#[inline]
pub(crate) fn copy_due() -> bool {
    static LOOK: AtomicU64 = AtomicU64::new(0);
    let last = LAST.load(Ordering::Relaxed);
    if last == 0 || !look_at(&LOOK, 1 << 12) || !PROGRAM.get() || threaded() {
        return false;
    }
    let _outside = Outside::enter();
    let now = std::time::Instant::now();
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    let at = *taken.get_or_insert(now);
    at != now && now.duration_since(at).as_millis() as u64 >= last / 2
}

/// A copy of the program here, and a new segment after it: `copied` runs in
/// the copy, which only comes back to replay what follows, and takes the
/// segment it starts at. Copies and segments the end of the run no longer
/// needs go. The log is held across the fork, so that the copy has it free.
pub(crate) fn copy(copied: impl FnOnce(u32)) {
    let Some(calls) = RECORDING.get().map(|dir| dir.join("calls")) else { return };
    store(true);
    let mut held = COPIES.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    *TAKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    if held.is_empty() {
        held.push(Copied { tick: 0, pid: 0, segment: 0, at: now });
    }
    let segment = held.last().map_or(1, |last| last.segment + 1);
    let Ok(mut file) = std::fs::File::create(segment_path(&calls, segment)) else { return };
    if std::io::Write::write_all(&mut file, MAGIC).is_err() {
        return;
    }
    let mut log = log();
    let baton = baton();
    let pid = super::tasks::fork_copy();
    if pid == 0 {
        drop(baton);
        drop(file);
        log.file = None;
        drop(log);
        drop(held);
        unsafe { libc::setsid() };
        copied(segment);
        return;
    }
    drop(baton);
    if pid < 0 {
        return;
    }
    log.file = Some(file);
    drop(log);
    held.push(Copied { tick: TICKS.load(Ordering::Relaxed), pid, segment, at: now });
    // The oldest goes once the next is old enough to cover the window.
    let window = std::time::Duration::from_millis(LAST.load(Ordering::Relaxed));
    while held.len() > 1 && now.duration_since(held[1].at) >= window {
        let gone = held.remove(0);
        if gone.pid != 0 {
            unsafe {
                libc::kill(gone.pid, libc::SIGKILL);
                libc::waitpid(gone.pid, std::ptr::null_mut(), 0);
            }
            let _ = std::fs::remove_file(super::diagnostics::sockets().join(format!("{}.checkpoint", gone.pid)));
        }
        for segment in gone.segment..held[0].segment {
            let _ = std::fs::remove_file(segment_path(&calls, segment));
        }
    }
    prune_children(&calls, window);
    let text: String = held.iter().map(|h| format!("{} {}\n", h.tick, h.pid)).collect();
    let (list, written) = (calls.with_file_name("held"), calls.with_file_name("held.new"));
    if std::fs::write(&written, text).is_ok() {
        let _ = std::fs::rename(&written, list);
    }
}

/// The recordings of children that wrote nothing for as long as the end of
/// the run is kept go: they started before it, and no replay reaches them.
fn prune_children(calls: &Path, window: std::time::Duration) {
    let children = calls.with_file_name("children");
    let now = std::time::SystemTime::now();
    let old = |dir: &Path| {
        let written = std::fs::metadata(dir.join("calls")).and_then(|calls| calls.modified());
        written.is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age >= window))
    };
    for child in std::fs::read_dir(children).into_iter().flatten().flatten() {
        if old(&child.path()) {
            let _ = std::fs::remove_dir_all(child.path());
        }
    }
}

/// A copy goes on as a replay, of what was recorded since it was taken.
pub(crate) fn replay_from(segment: u32, debugger: i32) {
    let Some(calls) = RECORDING.get().map(|dir| dir.join("calls")) else { return };
    let recorded = calls_from(&calls, segment).unwrap_or_else(|message| super::runtime_fault(&message));
    let mut log = log();
    log.recorded = recorded;
    log.at = 0;
    LAST.store(0, Ordering::Relaxed);
    DEBUGGER.store(debugger, Ordering::Relaxed);
    MODE.store(REPLAY, Ordering::Relaxed);
}

/// Writes what the recording holds so far, where a crash may have left
/// anything held: as it is.
pub(crate) fn flush() {
    store(false);
}

/// Writes what the recording holds so far as a block, compressed if asked.
fn store(compressed: bool) {
    use std::io::Write;
    let mut log = log();
    let Log { file, buffer, .. } = &mut *log;
    if let Some(file) = file.as_mut().filter(|_| !buffer.is_empty()) {
        let _outside = Outside::enter();
        let packed = compressed.then(|| packed(buffer)).flatten().filter(|packed| packed.len() < buffer.len());
        let stored = packed.as_ref().unwrap_or(buffer);
        let _ = file.write_all(&(buffer.len() as u32).to_le_bytes());
        let _ = file.write_all(&(stored.len() as u32).to_le_bytes());
        let _ = file.write_all(stored);
        buffer.clear();
    }
}

fn packed(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

/// The calls of the blocks of a recording; none if it is not one, but for a
/// last block a crash cut short, which is lost as what it held would be.
fn unpacked(mut blocks: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    while let Some((head, rest)) = blocks.split_first_chunk::<8>() {
        let length = u32::from_le_bytes(head[..4].try_into().ok()?) as usize;
        let stored = u32::from_le_bytes(head[4..].try_into().ok()?) as usize;
        let Some((block, rest)) = rest.split_at_checked(stored) else { break };
        match stored == length {
            true => out.extend_from_slice(block),
            false => {
                flate2::read::DeflateDecoder::new(block).read_to_end(&mut out).ok()?;
            }
        }
        blocks = rest;
    }
    Some(out)
}

/// The wrappers, in what `tondo` links on Linux, where the linker gives
/// them the C library's calls.
#[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
mod calls;
