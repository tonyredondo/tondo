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

const MAGIC: &[u8; 8] = b"TONDORR1";

/// What a recording holds, as it is written: the calls, each with its
/// function, the probe it came after, its result, `errno` and the bytes it
/// wrote where its arguments point.
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
/// How long a replay waits for a call from a thread of C the recording has.
const CALL_FROM_C: std::time::Duration = std::time::Duration::from_secs(30);
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
        let mut since = None;
        loop {
            if !baton.held {
                let mut log = log();
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
                    Some(hand) if hand.to > baton.known => {
                        let waited = since.get_or_insert_with(std::time::Instant::now).elapsed();
                        if waited > CALL_FROM_C {
                            drop(log);
                            drop(baton);
                            INSIDE.set(true);
                            crate::debugger::stop_for_good(
                                "the replay ends here: the recording has a call from a thread of C that did not come"
                                    .into(),
                            );
                        }
                    }
                    Some(_) => {}
                    None if baton.owner == number => break,
                    None => {}
                }
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
    let asked = std::env::var_os("TONDO_RT_RECORD").is_some() || std::env::var_os("TONDO_RT_REPLAY").is_some();
    if asked && !cfg!(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))) {
        super::runtime_fault("a run is recorded and replayed on Linux and macOS");
    }
    let able = unsafe { libc::getpid() } != 0 && WRAPPED.load(Ordering::Relaxed);
    if asked && !able {
        super::runtime_fault("this executable cannot record its runs: it is not a debug build that tondo linked");
    }
    // A child of a recorded run records its own, when it can, among its
    // parent's recording.
    let parent = std::env::var_os("TONDO_RT_RECORD_CHILDREN");
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
    if let Some(path) = std::env::var_os("TONDO_RT_RECORD").map(PathBuf::from).or(child) {
        unsafe { std::env::remove_var("TONDO_RT_RECORD") };
        directory_of(&path);
        let file = std::fs::File::create(&path).unwrap_or_else(|e| {
            super::runtime_fault(&format!("cannot record to {}: {e}", std::path::Path::new(&path).display()))
        });
        let mut log = log();
        log.file = Some(file);
        log.buffer.extend_from_slice(MAGIC);
        MODE.store(RECORD, Ordering::Relaxed);
        unsafe extern "C" {
            fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
        }
        extern "C" fn flushed(_: *mut u8) {
            flush();
        }
        unsafe { __cxa_atexit(flushed, std::ptr::null_mut(), std::ptr::null_mut()) };
    } else if let Some(path) = std::env::var_os("TONDO_RT_REPLAY").map(PathBuf::from) {
        unsafe { std::env::remove_var("TONDO_RT_REPLAY") };
        directory_of(&path);
        let recorded = std::fs::read(&path).unwrap_or_else(|e| {
            super::runtime_fault(&format!("cannot read the recording {}: {e}", std::path::Path::new(&path).display()))
        });
        if !recorded.starts_with(MAGIC) {
            super::runtime_fault("the file is not a recording of this tondo");
        }
        let mut log = log();
        log.recorded = recorded;
        log.at = MAGIC.len();
        MODE.store(REPLAY, Ordering::Relaxed);
        let from = std::env::var("TONDO_RT_SHOWN_FROM").ok().and_then(|t| t.parse().ok()).unwrap_or(0);
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
    let seed = std::env::var("TONDO_RT_SCHEDULE").unwrap_or_else(|_| "0".into());
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

/// Writes what the recording holds so far.
pub(crate) fn flush() {
    use std::io::Write;
    let mut log = log();
    let Log { file, buffer, .. } = &mut *log;
    if let Some(file) = file.as_mut() {
        INSIDE.set(true);
        let _ = file.write_all(buffer);
        INSIDE.set(false);
        buffer.clear();
    }
}

/// The wrappers, in what `tondo` links on Linux, where the linker gives
/// them the C library's calls.
#[cfg(all(tondo_staticlib, any(target_os = "linux", target_os = "macos")))]
mod calls {
    use super::*;
    use libc::{c_char, c_int, c_long, c_uint, c_ulong, c_void, size_t, ssize_t};

    #[derive(PartialEq)]
    enum Here {
        Pass,
        Record,
        Replay,
    }

    #[inline]
    fn here() -> Here {
        match MODE.load(Ordering::Relaxed) {
            OFF => Here::Pass,
            _ if !PROGRAM.get() || INSIDE.get() => Here::Pass,
            RECORD => Here::Record,
            _ => Here::Replay,
        }
    }

    /// The functions a recording has calls of, to name them.
    const NAMES: &[&str] = &[
        "accept",
        "accept4",
        "access",
        "bind",
        "chdir",
        "chmod",
        "chown",
        "clock_gettime",
        "clock_nanosleep",
        "close",
        "closedir",
        "connect",
        "copy_file_range",
        "dirfd",
        "dup",
        "dup2",
        "epoll_create1",
        "epoll_ctl",
        "epoll_wait",
        "eventfd",
        "fchmod",
        "fcntl",
        "fcntl64",
        "fdatasync",
        "fdopendir",
        "fork",
        "fstat",
        "fstat64",
        "fstatat",
        "fstatat64",
        "fsync",
        "ftruncate",
        "ftruncate64",
        "futimens",
        "getaddrinfo",
        "getcwd",
        "geteuid",
        "gethostname",
        "getpeername",
        "getpid",
        "getppid",
        "getrandom",
        "getsockname",
        "getsockopt",
        "gettimeofday",
        "getuid",
        "inotify_add_watch",
        "inotify_init1",
        "inotify_rm_watch",
        "ioctl",
        "kill",
        "killpg",
        "linkat",
        "listen",
        "lseek",
        "lseek64",
        "lstat",
        "lstat64",
        "mkdir",
        "nanosleep",
        "open",
        "open64",
        "openat",
        "openat64",
        "opendir",
        "pipe",
        "pipe2",
        "poll",
        "posix_spawnp",
        "pread",
        "pread64",
        "preadv",
        "pthread_create",
        "pwrite",
        "pwrite64",
        "pwritev",
        "read",
        "readdir",
        "readdir64",
        "readlink",
        "readv",
        "realpath",
        "recv",
        "recvfrom",
        "recvmsg",
        "rename",
        "rmdir",
        "sched_getaffinity",
        "send",
        "sendfile",
        "sendfile64",
        "sendmsg",
        "sendto",
        "setpgid",
        "setsid",
        "setsockopt",
        "socket",
        "socketpair",
        "splice",
        "stat",
        "stat64",
        "statvfs",
        "statx",
        "symlink",
        "sysconf",
        "tcgetattr",
        "tcsetattr",
        "time",
        "uname",
        "unlink",
        "unlinkat",
        "utimensat",
        "utimes",
        "waitid",
        "waitpid",
        "write",
        "writev",
        "flock",
        "shutdown",
        "sleep",
        "pause",
        "isatty",
        "sysinfo",
        "getrusage",
        "getgid",
        "chroot",
        "fchown",
        "lchown",
        "mkfifo",
        "pathconf",
        "setuid",
        "setgid",
        "setgroups",
        "getpwuid_r",
        "signal",
        "kept",
        "readdir_r",
        "kqueue",
        "kevent",
        "CCRandomGenerateBytes",
        "getentropy",
        "sysctl",
        "sysctlbyname",
        "confstr",
        "statfs",
        "fstatfs",
        "futimes",
        "fsetattrlist",
        "fclonefileat",
        "fcopyfile",
        "copyfile_state_get",
        "proc_pidinfo",
        "gethostuuid",
    ];

    /// The number a function's calls have in a recording.
    const fn id(name: &str) -> u32 {
        let bytes = name.as_bytes();
        let mut hash: u32 = 0x811c_9dc5;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u32;
            hash = hash.wrapping_mul(0x0100_0193);
            i += 1;
        }
        hash
    }

    fn errno() -> *mut c_int {
        #[cfg(target_os = "linux")]
        return unsafe { libc::__errno_location() };
        #[cfg(target_os = "macos")]
        return unsafe { libc::__error() };
    }

    /// Declares the real functions weak, which a C library may not have.
    macro_rules! weak {
        ($($name:expr),* $(,)?) => {
            #[cfg(target_os = "linux")]
            std::arch::global_asm!($(concat!(".weak tondo_real_", $name)),*);
            #[cfg(target_os = "macos")]
            std::arch::global_asm!($(concat!(".weak_reference _tondo_real_", $name)),*);
        };
    }

    /// Runs the real function, its own calls not recorded.
    fn inside<T>(real: impl FnOnce() -> T) -> T {
        INSIDE.set(true);
        let result = real();
        INSIDE.set(false);
        result
    }

    /// Records a call: its result, `errno`, and the bytes at each of `outs`.
    fn record(id: u32, result: i64, outs: &[(*mut u8, usize)]) {
        let error = unsafe { *errno() };
        let _held = held();
        // What runs meanwhile, as an allocation the profiler watches, is
        // the recording's own.
        let _outside = Outside::enter();
        let mut log = log();
        let buffer = &mut log.buffer;
        buffer.extend_from_slice(&id.to_le_bytes());
        buffer.extend_from_slice(&TICKS.load(Ordering::Relaxed).to_le_bytes());
        buffer.extend_from_slice(&result.to_le_bytes());
        buffer.extend_from_slice(&error.to_le_bytes());
        buffer.push(outs.len() as u8);
        for &(at, length) in outs {
            let length = if at.is_null() { 0 } else { length };
            buffer.extend_from_slice(&(length as u32).to_le_bytes());
            if length > 0 {
                buffer.extend_from_slice(unsafe { std::slice::from_raw_parts(at, length) });
            }
        }
        let full = buffer.len() >= 1 << 20;
        drop(log);
        if full {
            flush();
        }
        unsafe { *errno() = error };
    }

    /// The next call of the recording, which must be one of `name`: its
    /// result, and the bytes it wrote copied to `outs`, which its result
    /// gives. Another call, or none, ends the replay.
    fn replay(id: u32, name: &str, outs: impl FnOnce(i64) -> Vec<(*mut u8, usize)>) -> i64 {
        let _held = held();
        let _outside = Outside::enter();
        let mut log = log();
        let Log { recorded, at, .. } = &mut *log;
        let word = |at: usize, size: usize| recorded.get(at..at + size);
        let Some(head) = word(*at, 25) else {
            drop(log);
            ended(&format!("the recording ends before the program calls `{name}`"));
        };
        let called = u32::from_le_bytes(head[0..4].try_into().expect("4 bytes"));
        if called != id {
            let tick = u64::from_le_bytes(head[4..12].try_into().expect("8 bytes"));
            drop(log);
            let recorded =
                NAMES.iter().find(|n| self::id(n) == called).map_or("another call".into(), |n| format!("`{n}`"));
            ended(&format!(
                "the program calls `{name}` where the recording has {recorded}, {tick} probes after the start"
            ));
        }
        let result = i64::from_le_bytes(head[12..20].try_into().expect("8 bytes"));
        let error = i32::from_le_bytes(head[20..24].try_into().expect("4 bytes"));
        let count = head[24] as usize;
        let mut next = *at + 25;
        let destinations = outs(result);
        for index in 0..count {
            let Some(length) = word(next, 4) else { break };
            let length = u32::from_le_bytes(length.try_into().expect("4 bytes")) as usize;
            next += 4;
            let bytes = &recorded[next..(next + length).min(recorded.len())];
            if let Some(&(to, room)) = destinations.get(index)
                && !to.is_null()
            {
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), to, bytes.len().min(room)) };
            }
            next += length;
        }
        *at = next;
        drop(log);
        unsafe { *errno() = error };
        result
    }

    /// The replay cannot go on: the program stops there for good, or ends
    /// when nothing debugs it.
    fn ended(why: &str) -> ! {
        INSIDE.set(true);
        crate::debugger::stop_for_good(format!("the replay ends here: {why}"))
    }

    pub(super) fn waiting() {
        let log = log();
        if log.at >= log.recorded.len() {
            drop(log);
            INSIDE.set(true);
            crate::debugger::stop_for_good("the replay ends here: the recording ends while the program waits".into())
        }
    }

    pub(super) fn kept(make: impl FnOnce() -> Vec<u8>) -> Vec<u8> {
        const ID: u32 = id("kept");
        match here() {
            Here::Pass => make(),
            Here::Record => {
                let mut bytes = inside(make);
                record(ID, bytes.len() as i64, &[(bytes.as_mut_ptr(), bytes.len())]);
                bytes
            }
            Here::Replay => {
                let mut bytes = Vec::new();
                replay(ID, "kept", |length| {
                    bytes.resize(length.max(0) as usize, 0);
                    vec![(bytes.as_mut_ptr(), bytes.len())]
                });
                bytes
            }
        }
    }

    /// A signal the worker takes at a turn, which a replay gives there.
    pub(super) fn signal_came(turn: u64, signal: i32) {
        let mut byte = signal as u8;
        record(id("signal"), turn as i64, &[(&mut byte, 1)]);
    }

    pub(super) fn signal_at(turn: u64) -> Option<i32> {
        let log = log();
        let head = log.recorded.get(log.at..log.at + 25)?;
        let called = u32::from_le_bytes(head[0..4].try_into().expect("4 bytes"));
        let at = i64::from_le_bytes(head[12..20].try_into().expect("8 bytes"));
        if called != id("signal") || at != turn as i64 {
            return None;
        }
        drop(log);
        let mut byte = 0u8;
        replay(id("signal"), "signal", |_| vec![(&mut byte, 1)]);
        Some(byte as i32)
    }

    /// The bytes of a buffer the result says a call filled: none on an error.
    fn filled(at: *mut c_void, result: i64) -> (*mut u8, usize) {
        (at.cast(), result.max(0) as usize)
    }

    /// The parts of `iov` that `result` bytes fill, in order.
    fn spread(iov: *const libc::iovec, count: c_int, result: i64) -> Vec<(*mut u8, usize)> {
        let mut left = result.max(0) as usize;
        let mut outs = Vec::new();
        for index in 0..count.max(0) as usize {
            let part = unsafe { *iov.add(index) };
            let length = part.iov_len.min(left);
            outs.push((part.iov_base.cast(), length));
            left -= length;
        }
        outs
    }

    fn of<T>(at: *mut T) -> (*mut u8, usize) {
        (at.cast(), std::mem::size_of::<T>())
    }

    /// A socket address and its length, which a call wrote.
    fn address(at: *mut libc::sockaddr, length: *mut libc::socklen_t) -> [(*mut u8, usize); 2] {
        let room = if length.is_null() { 0 } else { (unsafe { *length }) as usize };
        [(at.cast(), room), of(length)]
    }

    /// Defines `tondo_rr_<name>`, which passes the call to `tondo_real_<name>`
    /// on other threads, records it on the program's, or replays it; `outs`
    /// gives, from the result, what the call writes where.
    macro_rules! wrap {
        ($name:ident ($($arg:ident: $ty:ty),*) -> $ret:ty, |$result:ident| $outs:expr) => {
            wrap!($name ($($arg: $ty),*) -> $ret, |$result| $outs, |_r| {});
        };
        ($name:ident ($($arg:ident: $ty:ty),*) -> $ret:ty, |$result:ident| $outs:expr, |$replayed:ident| $after:expr) => {
            #[allow(non_snake_case, clippy::missing_safety_doc)]
            pub mod $name {
                use super::*;
                // Weak: a C library without it, as musl without the `*64`
                // functions, leaves it out.
                weak!(stringify!($name));
                unsafe extern "C" {
                    #[link_name = concat!("tondo_real_", stringify!($name))]
                    pub fn real($($arg: $ty),*) -> $ret;
                }
                #[unsafe(export_name = concat!("tondo_rr_", stringify!($name)))]
                pub unsafe extern "C" fn wrapper($($arg: $ty),*) -> $ret {
                    const ID: u32 = id(stringify!($name));
                    let outs = |$result: i64| -> Vec<(*mut u8, usize)> {
                        let _ = $result;
                        $outs.into_iter().collect()
                    };
                    match here() {
                        Here::Pass => unsafe { real($($arg),*) },
                        Here::Record => {
                            let result = inside(|| unsafe { real($($arg),*) });
                            let _outside = Outside::enter();
                            record(ID, result as i64, &outs(result as i64));
                            result
                        }
                        Here::Replay => {
                            let $replayed = replay(ID, stringify!($name), outs);
                            $after;
                            $replayed as $ret
                        }
                    }
                }
            }
        };
    }

    /// `wrap!` for a function of C whose last argument is variadic, as
    /// `open`'s mode and `fcntl`'s argument: the real one is called so. On
    /// Apple's arm64 such an argument is on the stack, where a function of
    /// Rust does not look: `tondo_rr_<name>` loads it into `$register`, the
    /// one after the fixed arguments, and goes on to the wrapper.
    macro_rules! wrap_variadic {
        ($register:literal, $name:ident ($($arg:ident: $ty:ty),*; $last:ident: $lty:ty) -> $ret:ty, |$result:ident| $outs:expr) => {
            #[allow(non_snake_case, clippy::missing_safety_doc)]
            pub mod $name {
                use super::*;
                weak!(stringify!($name));
                unsafe extern "C" {
                    #[link_name = concat!("tondo_real_", stringify!($name))]
                    pub fn real($($arg: $ty),*, ...) -> $ret;
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                std::arch::global_asm!(concat!(
                    ".globl _tondo_rr_", stringify!($name), "\n",
                    ".p2align 2\n",
                    "_tondo_rr_", stringify!($name), ":\n",
                    "ldr ", $register, ", [sp]\n",
                    "b _tondo_rr_", stringify!($name), "_fixed\n",
                ));
                #[cfg_attr(
                    all(target_os = "macos", target_arch = "aarch64"),
                    unsafe(export_name = concat!("tondo_rr_", stringify!($name), "_fixed"))
                )]
                #[cfg_attr(
                    not(all(target_os = "macos", target_arch = "aarch64")),
                    unsafe(export_name = concat!("tondo_rr_", stringify!($name)))
                )]
                pub unsafe extern "C" fn wrapper($($arg: $ty),*, $last: $lty) -> $ret {
                    const ID: u32 = id(stringify!($name));
                    let outs = |$result: i64| -> Vec<(*mut u8, usize)> {
                        let _ = $result;
                        $outs.into_iter().collect()
                    };
                    match here() {
                        Here::Pass => unsafe { real($($arg),*, $last) },
                        Here::Record => {
                            let result = inside(|| unsafe { real($($arg),*, $last) });
                            let _outside = Outside::enter();
                            record(ID, result as i64, &outs(result as i64));
                            result
                        }
                        Here::Replay => replay(ID, stringify!($name), outs) as $ret,
                    }
                }
            }
        };
    }

    /// What a call writes nowhere.
    fn none(_: i64) -> [(*mut u8, usize); 0] {
        []
    }

    /// Writing to the standard output or error shows in a replay too.
    fn shown(fd: c_int, write: impl FnOnce()) {
        if (fd == 1 || fd == 2) && TICKS.load(Ordering::Relaxed) >= SHOWN_FROM.load(Ordering::Relaxed) {
            inside(write);
        }
    }

    wrap!(read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t, |r| [filled(buf, r)]);
    wrap!(pread(fd: c_int, buf: *mut c_void, count: size_t, offset: libc::off_t) -> ssize_t, |r| [filled(buf, r)]);
    #[cfg(target_os = "linux")]
    wrap!(pread64(fd: c_int, buf: *mut c_void, count: size_t, offset: libc::off_t) -> ssize_t, |r| [filled(buf, r)]);
    wrap!(readv(fd: c_int, iov: *const libc::iovec, count: c_int) -> ssize_t, |r| spread(iov, count, r));
    wrap!(preadv(fd: c_int, iov: *const libc::iovec, count: c_int, offset: libc::off_t) -> ssize_t, |r| spread(iov, count, r));
    wrap!(recv(fd: c_int, buf: *mut c_void, length: size_t, flags: c_int) -> ssize_t, |r| [filled(buf, r)]);
    wrap!(
        recvfrom(
            fd: c_int,
            buf: *mut c_void,
            length: size_t,
            flags: c_int,
            from: *mut libc::sockaddr,
            from_length: *mut libc::socklen_t
        ) -> ssize_t,
        |r| std::iter::once(filled(buf, r)).chain(address(from, from_length))
    );
    wrap!(recvmsg(fd: c_int, message: *mut libc::msghdr, flags: c_int) -> ssize_t, |r| {
        let m = unsafe { &mut *message };
        let mut outs = spread(m.msg_iov, m.msg_iovlen as c_int, r);
        outs.push((m.msg_name.cast(), m.msg_namelen as usize));
        outs.push(of(&mut m.msg_namelen));
        outs.push((m.msg_control.cast(), m.msg_controllen as usize));
        outs.push(of(&mut m.msg_controllen));
        outs.push(of(&mut m.msg_flags));
        outs
    });
    wrap!(write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t, |r| none(r), |_r| shown(fd, || {
        unsafe { real(fd, buf, count) };
    }));
    wrap!(writev(fd: c_int, iov: *const libc::iovec, count: c_int) -> ssize_t, |r| none(r), |_r| shown(fd, || {
        unsafe { real(fd, iov, count) };
    }));
    wrap!(pwrite(fd: c_int, buf: *const c_void, count: size_t, offset: libc::off_t) -> ssize_t, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(pwrite64(fd: c_int, buf: *const c_void, count: size_t, offset: libc::off_t) -> ssize_t, |r| none(r));
    wrap!(pwritev(fd: c_int, iov: *const libc::iovec, count: c_int, offset: libc::off_t) -> ssize_t, |r| none(r));
    wrap!(send(fd: c_int, buf: *const c_void, length: size_t, flags: c_int) -> ssize_t, |r| none(r));
    wrap!(
        sendto(
            fd: c_int,
            buf: *const c_void,
            length: size_t,
            flags: c_int,
            to: *const libc::sockaddr,
            to_length: libc::socklen_t
        ) -> ssize_t,
        |r| none(r)
    );
    wrap!(sendmsg(fd: c_int, message: *const libc::msghdr, flags: c_int) -> ssize_t, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(sendfile(out: c_int, input: c_int, offset: *mut libc::off_t, count: size_t) -> ssize_t, |r| [of(offset)]);
    #[cfg(target_os = "linux")]
    wrap!(sendfile64(out: c_int, input: c_int, offset: *mut libc::off_t, count: size_t) -> ssize_t, |r| [of(offset)]);
    #[cfg(target_os = "linux")]
    wrap!(
        splice(
            input: c_int,
            input_offset: *mut libc::loff_t,
            out: c_int,
            out_offset: *mut libc::loff_t,
            length: size_t,
            flags: c_uint
        ) -> ssize_t,
        |r| [of(input_offset), of(out_offset)]
    );
    wrap!(accept(fd: c_int, from: *mut libc::sockaddr, length: *mut libc::socklen_t) -> c_int, |r| address(from, length));
    #[cfg(target_os = "linux")]
    wrap!(
        accept4(fd: c_int, from: *mut libc::sockaddr, length: *mut libc::socklen_t, flags: c_int) -> c_int,
        |r| address(from, length)
    );
    wrap!(connect(fd: c_int, to: *const libc::sockaddr, length: libc::socklen_t) -> c_int, |r| none(r));
    wrap!(bind(fd: c_int, to: *const libc::sockaddr, length: libc::socklen_t) -> c_int, |r| none(r));
    wrap!(listen(fd: c_int, backlog: c_int) -> c_int, |r| none(r));
    wrap!(socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int, |r| none(r));
    wrap!(socketpair(domain: c_int, kind: c_int, protocol: c_int, pair: *mut [c_int; 2]) -> c_int, |r| [of(pair)]);
    wrap!(getsockname(fd: c_int, at: *mut libc::sockaddr, length: *mut libc::socklen_t) -> c_int, |r| address(at, length));
    wrap!(getpeername(fd: c_int, at: *mut libc::sockaddr, length: *mut libc::socklen_t) -> c_int, |r| address(at, length));
    wrap!(
        getsockopt(fd: c_int, level: c_int, name: c_int, value: *mut c_void, length: *mut libc::socklen_t) -> c_int,
        |r| address(value.cast(), length)
    );
    wrap!(
        setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, length: libc::socklen_t) -> c_int,
        |r| none(r)
    );
    wrap_variadic!("x2", open(path: *const c_char, flags: c_int; mode: c_uint) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap_variadic!("x2", open64(path: *const c_char, flags: c_int; mode: c_uint) -> c_int, |r| none(r));
    wrap_variadic!("x3", openat(dir: c_int, path: *const c_char, flags: c_int; mode: c_uint) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap_variadic!("x3", openat64(dir: c_int, path: *const c_char, flags: c_int; mode: c_uint) -> c_int, |r| none(r));
    wrap!(close(fd: c_int) -> c_int, |r| none(r));
    wrap!(stat(path: *const c_char, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "linux")]
    wrap!(stat64(path: *const c_char, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    wrap!(lstat(path: *const c_char, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "linux")]
    wrap!(lstat64(path: *const c_char, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    wrap!(fstat(fd: c_int, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "linux")]
    wrap!(fstat64(fd: c_int, out: *mut libc::stat) -> c_int, |r| [of(out)]);
    wrap!(fstatat(dir: c_int, path: *const c_char, out: *mut libc::stat, flags: c_int) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "linux")]
    wrap!(fstatat64(dir: c_int, path: *const c_char, out: *mut libc::stat, flags: c_int) -> c_int, |r| [of(out)]);
    wrap!(statvfs(path: *const c_char, out: *mut libc::statvfs) -> c_int, |r| [of(out)]);
    wrap!(access(path: *const c_char, mode: c_int) -> c_int, |r| none(r));
    wrap!(chdir(path: *const c_char) -> c_int, |r| none(r));
    wrap!(chmod(path: *const c_char, mode: libc::mode_t) -> c_int, |r| none(r));
    wrap!(fchmod(fd: c_int, mode: libc::mode_t) -> c_int, |r| none(r));
    wrap!(chown(path: *const c_char, user: libc::uid_t, group: libc::gid_t) -> c_int, |r| none(r));
    wrap!(mkdir(path: *const c_char, mode: libc::mode_t) -> c_int, |r| none(r));
    wrap!(rmdir(path: *const c_char) -> c_int, |r| none(r));
    wrap!(unlink(path: *const c_char) -> c_int, |r| none(r));
    wrap!(unlinkat(dir: c_int, path: *const c_char, flags: c_int) -> c_int, |r| none(r));
    wrap!(rename(from: *const c_char, to: *const c_char) -> c_int, |r| none(r));
    wrap!(symlink(target: *const c_char, path: *const c_char) -> c_int, |r| none(r));
    wrap!(
        linkat(from_dir: c_int, from: *const c_char, to_dir: c_int, to: *const c_char, flags: c_int) -> c_int,
        |r| none(r)
    );
    wrap!(readlink(path: *const c_char, buf: *mut c_char, size: size_t) -> ssize_t, |r| [filled(buf.cast(), r)]);
    wrap!(ftruncate(fd: c_int, length: libc::off_t) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(ftruncate64(fd: c_int, length: libc::off_t) -> c_int, |r| none(r));
    wrap!(fsync(fd: c_int) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(fdatasync(fd: c_int) -> c_int, |r| none(r));
    wrap!(lseek(fd: c_int, offset: libc::off_t, whence: c_int) -> libc::off_t, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(lseek64(fd: c_int, offset: libc::off_t, whence: c_int) -> libc::off_t, |r| none(r));
    wrap!(
        utimensat(dir: c_int, path: *const c_char, times: *const libc::timespec, flags: c_int) -> c_int,
        |r| none(r)
    );
    wrap!(futimens(fd: c_int, times: *const libc::timespec) -> c_int, |r| none(r));
    wrap!(utimes(path: *const c_char, times: *const libc::timeval) -> c_int, |r| none(r));
    wrap!(dup(fd: c_int) -> c_int, |r| {
        duplicated(fd, r);
        none(r)
    });
    wrap!(dup2(from: c_int, to: c_int) -> c_int, |r| {
        duplicated(from, r);
        none(r)
    });
    wrap_variadic!("x2", fcntl(fd: c_int, command: c_int; argument: usize) -> c_int, |r| {
        if matches!(command, libc::F_DUPFD | libc::F_DUPFD_CLOEXEC) {
            duplicated(fd, r);
        }
        none(r)
    });
    #[cfg(target_os = "linux")]
    wrap_variadic!("x2", fcntl64(fd: c_int, command: c_int; argument: usize) -> c_int, |r| {
        if matches!(command, libc::F_DUPFD | libc::F_DUPFD_CLOEXEC) {
            duplicated(fd, r);
        }
        none(r)
    });
    wrap_variadic!("x2", ioctl(fd: c_int, request: c_ulong; argument: *mut c_void) -> c_int, |r| [(
        argument.cast::<u8>(),
        // What the requests the runtime makes write: a count, a terminal's
        // size or settings.
        match request {
            r if r == libc::FIONREAD as c_ulong => std::mem::size_of::<c_int>(),
            r if r == libc::TIOCGWINSZ as c_ulong => std::mem::size_of::<libc::winsize>(),
            #[cfg(target_os = "linux")]
            r if r == libc::TCGETS as c_ulong => std::mem::size_of::<libc::termios>(),
            #[cfg(target_os = "macos")]
            r if r == libc::TIOCGETA as c_ulong => std::mem::size_of::<libc::termios>(),
            _ => 0,
        }
    )]);
    wrap!(tcgetattr(fd: c_int, out: *mut libc::termios) -> c_int, |r| [of(out)]);
    wrap!(tcsetattr(fd: c_int, when: c_int, settings: *const libc::termios) -> c_int, |r| none(r));
    wrap!(pipe(fds: *mut [c_int; 2]) -> c_int, |r| [of(fds)]);
    #[cfg(target_os = "linux")]
    wrap!(pipe2(fds: *mut [c_int; 2], flags: c_int) -> c_int, |r| [of(fds)]);
    #[cfg(target_os = "linux")]
    wrap!(eventfd(initial: c_uint, flags: c_int) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(epoll_create1(flags: c_int) -> c_int, |r| {
        if r >= 0 {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.get_or_insert_with(Registry::default).instances.insert(r as c_int, r as c_int);
        }
        none(r)
    });
    #[cfg(target_os = "linux")]
    wrap!(epoll_ctl(epoll: c_int, op: c_int, fd: c_int, event: *mut libc::epoll_event) -> c_int, |r| {
        if r == 0 {
            registered(epoll, op, fd, event);
        }
        none(r)
    });
    wrap!(poll(fds: *mut libc::pollfd, count: libc::nfds_t, timeout: c_int) -> c_int, |r| [(
        fds.cast::<u8>(),
        count as usize * std::mem::size_of::<libc::pollfd>()
    )]);
    #[cfg(target_os = "linux")]
    wrap!(inotify_init1(flags: c_int) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(inotify_rm_watch(fd: c_int, watch: c_int) -> c_int, |r| none(r));
    wrap!(clock_gettime(clock: libc::clockid_t, out: *mut libc::timespec) -> c_int, |r| [of(out)]);
    wrap!(gettimeofday(out: *mut libc::timeval, zone: *mut c_void) -> c_int, |r| [of(out)]);
    wrap!(time(out: *mut libc::time_t) -> libc::time_t, |r| [of(out)]);
    wrap!(nanosleep(wanted: *const libc::timespec, left: *mut libc::timespec) -> c_int, |r| [of(left)]);
    #[cfg(target_os = "linux")]
    wrap!(
        clock_nanosleep(clock: libc::clockid_t, flags: c_int, wanted: *const libc::timespec, left: *mut libc::timespec) -> c_int,
        |r| [of(left)]
    );
    #[cfg(target_os = "linux")]
    wrap!(getrandom(buf: *mut c_void, length: size_t, flags: c_uint) -> ssize_t, |r| [filled(buf, r)]);
    wrap!(uname(out: *mut libc::utsname) -> c_int, |r| [of(out)]);
    wrap!(gethostname(name: *mut c_char, length: size_t) -> c_int, |r| [(name.cast::<u8>(), length)]);
    wrap!(sysconf(name: c_int) -> c_long, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(sched_getaffinity(pid: libc::pid_t, size: size_t, mask: *mut libc::cpu_set_t) -> c_int, |r| [(
        mask.cast::<u8>(),
        size
    )]);
    weak!(
        "getpid",
        "fork",
        "syscall",
        "getcwd",
        "realpath",
        "opendir",
        "fdopendir",
        "readdir",
        "readdir64",
        "getaddrinfo",
        "freeaddrinfo",
        "pthread_create",
        "epoll_wait"
    );

    /// The process's ID; the first call says the linker gave the program
    /// these wrappers.
    #[unsafe(export_name = "tondo_rr_getpid")]
    pub unsafe extern "C" fn getpid_wrapper() -> libc::pid_t {
        unsafe extern "C" {
            #[link_name = "tondo_real_getpid"]
            fn real() -> libc::pid_t;
        }
        WRAPPED.store(true, Ordering::Relaxed);
        match here() {
            Here::Pass => unsafe { real() },
            Here::Record => {
                let pid = inside(|| unsafe { real() });
                record(id("getpid"), pid as i64, &[]);
                pid
            }
            Here::Replay => replay(id("getpid"), "getpid", none_vec) as libc::pid_t,
        }
    }
    wrap!(getppid() -> libc::pid_t, |r| none(r));
    wrap!(getuid() -> libc::uid_t, |r| none(r));
    wrap!(geteuid() -> libc::uid_t, |r| none(r));
    wrap!(kill(pid: libc::pid_t, signal: c_int) -> c_int, |r| none(r));
    wrap!(killpg(group: libc::pid_t, signal: c_int) -> c_int, |r| none(r));
    wrap!(setsid() -> libc::pid_t, |r| none(r));
    wrap!(setpgid(pid: libc::pid_t, group: libc::pid_t) -> c_int, |r| none(r));
    wrap!(waitpid(pid: libc::pid_t, status: *mut c_int, options: c_int) -> libc::pid_t, |r| [of(status)]);
    wrap!(
        waitid(kind: libc::idtype_t, id: libc::id_t, info: *mut libc::siginfo_t, options: c_int) -> c_int,
        |r| [of(info)]
    );
    wrap!(
        posix_spawnp(
            pid: *mut libc::pid_t,
            file: *const c_char,
            actions: *const libc::posix_spawn_file_actions_t,
            attributes: *const libc::posix_spawnattr_t,
            argv: *const *mut c_char,
            envp: *const *mut c_char
        ) -> c_int,
        |r| [of(pid)]
    );

    /// A child the program forks is not the program: it records nothing.
    #[unsafe(export_name = "tondo_rr_fork")]
    pub unsafe extern "C" fn fork_wrapper() -> libc::pid_t {
        unsafe extern "C" {
            #[link_name = "tondo_real_fork"]
            fn real() -> libc::pid_t;
        }
        const ID: u32 = id("fork");
        match here() {
            Here::Pass => unsafe { real() },
            Here::Record => {
                let pid = inside(|| unsafe { real() });
                if pid == 0 {
                    PROGRAM.set(false);
                    MODE.store(OFF, Ordering::Relaxed);
                    return 0;
                }
                record(ID, pid as i64, &[]);
                pid
            }
            Here::Replay => replay(ID, "fork", none_vec) as libc::pid_t,
        }
    }

    fn none_vec(_: i64) -> Vec<(*mut u8, usize)> {
        Vec::new()
    }

    #[cfg(target_os = "linux")]
    /// The raw system calls the Rust library makes itself: those that read the
    /// world are recorded; those of memory and of the threads' own waits are
    /// not.
    #[unsafe(export_name = "tondo_rr_syscall")]
    pub unsafe extern "C" fn syscall_wrapper(
        number: c_long,
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        e: usize,
        f: usize,
    ) -> c_long {
        unsafe extern "C" {
            #[link_name = "tondo_real_syscall"]
            fn real(number: c_long, ...) -> c_long;
        }
        let passed = matches!(
            number,
            libc::SYS_futex
                | libc::SYS_gettid
                | libc::SYS_membarrier
                | libc::SYS_rseq
                | libc::SYS_sched_yield
                | libc::SYS_mmap
                | libc::SYS_munmap
                | libc::SYS_mprotect
                | libc::SYS_madvise
                | libc::SYS_mremap
                | libc::SYS_rt_sigprocmask
                | libc::SYS_rt_sigaction
                | libc::SYS_sigaltstack
        );
        let real = || unsafe { real(number, a, b, c, d, e, f) };
        let here = if passed { Here::Pass } else { here() };
        let id = id("syscall") ^ number as u32;
        let outs = |result: i64| -> Vec<(*mut u8, usize)> {
            match number {
                libc::SYS_getrandom => vec![filled(a as *mut c_void, result)],
                // `struct statx`, 256 bytes.
                libc::SYS_statx => vec![(e as *mut u8, 256)],
                _ => Vec::new(),
            }
        };
        match here {
            Here::Pass => real(),
            Here::Record => {
                let result = inside(real);
                let _outside = Outside::enter();
                record(id, result as i64, &outs(result as i64));
                result
            }
            Here::Replay => replay(id, "syscall", outs) as c_long,
        }
    }

    /// A path the C library writes into `buffer`, or into memory it allocates
    /// when `buffer` is null, as `getcwd` and `realpath` do.
    fn path_call(name: &str, buffer: *mut c_char, room: usize, real: impl FnOnce() -> *mut c_char) -> *mut c_char {
        let id = id(name);
        match here() {
            Here::Pass => real(),
            Here::Record => {
                let written = inside(real);
                let length = if written.is_null() { 0 } else { (unsafe { libc::strlen(written) }) + 1 };
                record(id, !written.is_null() as i64, &[(written.cast(), length)]);
                written
            }
            Here::Replay => {
                let mut text = Vec::new();
                let found = replay(id, name, |found| {
                    text = vec![0u8; if found != 0 { libc::PATH_MAX as usize + 1 } else { 0 }];
                    vec![(text.as_mut_ptr(), text.len())]
                });
                if found == 0 {
                    return std::ptr::null_mut();
                }
                let length = text.iter().position(|&b| b == 0).unwrap_or(text.len()) + 1;
                let into = if buffer.is_null() { unsafe { libc::malloc(length).cast::<c_char>() } } else { buffer };
                if !buffer.is_null() && length > room {
                    unsafe { *errno() = libc::ERANGE };
                    return std::ptr::null_mut();
                }
                unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), into.cast::<u8>(), length) };
                into
            }
        }
    }

    #[unsafe(export_name = "tondo_rr_getcwd")]
    pub unsafe extern "C" fn getcwd_wrapper(buffer: *mut c_char, size: size_t) -> *mut c_char {
        unsafe extern "C" {
            #[link_name = "tondo_real_getcwd"]
            fn real(buffer: *mut c_char, size: size_t) -> *mut c_char;
        }
        path_call("getcwd", buffer, size, || unsafe { real(buffer, size) })
    }

    #[unsafe(export_name = "tondo_rr_realpath")]
    pub unsafe extern "C" fn realpath_wrapper(path: *const c_char, resolved: *mut c_char) -> *mut c_char {
        unsafe extern "C" {
            #[link_name = "tondo_real_realpath"]
            fn real(path: *const c_char, resolved: *mut c_char) -> *mut c_char;
        }
        path_call("realpath", resolved, libc::PATH_MAX as usize, || unsafe { real(path, resolved) })
    }

    /// A directory a replay reads: none is open, and its entries come from
    /// the recording, one at a time, in this buffer.
    #[repr(C)]
    struct Entry([u8; std::mem::size_of::<libc::dirent>()]);

    thread_local! {
        static ENTRY: std::cell::UnsafeCell<Entry> =
            const { std::cell::UnsafeCell::new(Entry([0; std::mem::size_of::<libc::dirent>()])) };
    }

    /// A directory opened to be read: in a replay, the address it had,
    /// which only these wrappers read.
    fn open_dir(name: &str, real: impl FnOnce() -> *mut libc::DIR) -> *mut libc::DIR {
        match here() {
            Here::Pass => real(),
            Here::Record => {
                let dir = inside(real);
                record(id(name), dir as i64, &[]);
                dir
            }
            Here::Replay => replay(id(name), name, none_vec) as *mut libc::DIR,
        }
    }

    #[unsafe(export_name = "tondo_rr_opendir")]
    pub unsafe extern "C" fn opendir_wrapper(path: *const c_char) -> *mut libc::DIR {
        unsafe extern "C" {
            #[link_name = "tondo_real_opendir"]
            fn real(path: *const c_char) -> *mut libc::DIR;
        }
        open_dir("opendir", || unsafe { real(path) })
    }

    #[unsafe(export_name = "tondo_rr_fdopendir")]
    pub unsafe extern "C" fn fdopendir_wrapper(fd: c_int) -> *mut libc::DIR {
        unsafe extern "C" {
            #[link_name = "tondo_real_fdopendir"]
            fn real(fd: c_int) -> *mut libc::DIR;
        }
        open_dir("fdopendir", || unsafe { real(fd) })
    }

    fn read_entry(name: &str, real: impl FnOnce() -> *mut libc::dirent) -> *mut libc::dirent {
        let id = id(name);
        let size = std::mem::size_of::<libc::dirent>();
        match here() {
            Here::Pass => real(),
            Here::Record => {
                let entry = inside(real);
                record(id, !entry.is_null() as i64, &[(entry.cast(), size)]);
                entry
            }
            Here::Replay => ENTRY.with(|buffer| {
                let to = buffer.get().cast::<u8>();
                match replay(id, name, |_| vec![(to, size)]) {
                    0 => std::ptr::null_mut(),
                    _ => to.cast(),
                }
            }),
        }
    }

    #[unsafe(export_name = "tondo_rr_readdir")]
    pub unsafe extern "C" fn readdir_wrapper(dir: *mut libc::DIR) -> *mut libc::dirent {
        unsafe extern "C" {
            #[link_name = "tondo_real_readdir"]
            fn real(dir: *mut libc::DIR) -> *mut libc::dirent;
        }
        read_entry("readdir", || unsafe { real(dir) })
    }

    #[cfg(target_os = "linux")]
    #[unsafe(export_name = "tondo_rr_readdir64")]
    pub unsafe extern "C" fn readdir64_wrapper(dir: *mut libc::DIR) -> *mut libc::dirent {
        unsafe extern "C" {
            #[link_name = "tondo_real_readdir64"]
            fn real(dir: *mut libc::DIR) -> *mut libc::dirent;
        }
        read_entry("readdir64", || unsafe { real(dir) })
    }

    wrap!(closedir(dir: *mut libc::DIR) -> c_int, |r| none(r));
    wrap!(flock(fd: c_int, operation: c_int) -> c_int, |r| none(r));
    wrap!(shutdown(fd: c_int, how: c_int) -> c_int, |r| none(r));
    wrap!(sleep(seconds: c_uint) -> c_uint, |r| none(r));
    wrap!(pause() -> c_int, |r| none(r));
    wrap!(isatty(fd: c_int) -> c_int, |r| none(r));
    #[cfg(target_os = "linux")]
    wrap!(sysinfo(out: *mut libc::sysinfo) -> c_int, |r| [of(out)]);
    wrap!(getrusage(who: c_int, out: *mut libc::rusage) -> c_int, |r| [of(out)]);
    wrap!(getgid() -> libc::gid_t, |r| none(r));
    wrap!(chroot(path: *const c_char) -> c_int, |r| none(r));
    wrap!(fchown(fd: c_int, user: libc::uid_t, group: libc::gid_t) -> c_int, |r| none(r));
    wrap!(lchown(path: *const c_char, user: libc::uid_t, group: libc::gid_t) -> c_int, |r| none(r));
    wrap!(mkfifo(path: *const c_char, mode: libc::mode_t) -> c_int, |r| none(r));
    wrap!(pathconf(path: *const c_char, name: c_int) -> c_long, |r| none(r));
    wrap!(setuid(user: libc::uid_t) -> c_int, |r| none(r));
    wrap!(setgid(group: libc::gid_t) -> c_int, |r| none(r));
    wrap!(setgroups(count: size_t, groups: *const libc::gid_t) -> c_int, |r| none(r));

    weak!("getpwuid_r");

    /// A user's entry, whose strings are in `buffer`: in a replay, where
    /// the recorded buffer was, moved to where this one is.
    #[unsafe(export_name = "tondo_rr_getpwuid_r")]
    pub unsafe extern "C" fn getpwuid_r_wrapper(
        user: libc::uid_t,
        entry: *mut libc::passwd,
        buffer: *mut c_char,
        size: size_t,
        found: *mut *mut libc::passwd,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_getpwuid_r"]
            fn real(
                user: libc::uid_t,
                entry: *mut libc::passwd,
                buffer: *mut c_char,
                size: size_t,
                found: *mut *mut libc::passwd,
            ) -> c_int;
        }
        const ID: u32 = id("getpwuid_r");
        let real = || unsafe { real(user, entry, buffer, size, found) };
        match here() {
            Here::Pass => real(),
            Here::Record => {
                let result = inside(real);
                let mut at = buffer as usize;
                let mut there = unsafe { (*found).is_null() } as usize;
                record(ID, result as i64, &[of(entry), (buffer.cast(), size), of(&mut at), of(&mut there)]);
                result
            }
            Here::Replay => {
                let (mut at, mut missing) = (0usize, 0usize);
                let result =
                    replay(ID, "getpwuid_r", |_| vec![of(entry), (buffer.cast(), size), of(&mut at), of(&mut missing)]);
                let moved = |text: *mut c_char| -> *mut c_char {
                    if text.is_null() {
                        text
                    } else {
                        (text as usize).wrapping_sub(at).wrapping_add(buffer as usize) as *mut c_char
                    }
                };
                unsafe {
                    let e = &mut *entry;
                    e.pw_name = moved(e.pw_name);
                    e.pw_passwd = moved(e.pw_passwd);
                    e.pw_gecos = moved(e.pw_gecos);
                    e.pw_dir = moved(e.pw_dir);
                    e.pw_shell = moved(e.pw_shell);
                    #[cfg(target_os = "macos")]
                    {
                        e.pw_class = moved(e.pw_class);
                    }
                    *found = if missing != 0 { std::ptr::null_mut() } else { entry };
                }
                result as c_int
            }
        }
    }
    // `struct statx`, 256 bytes.
    #[cfg(target_os = "linux")]
    wrap!(
        statx(dir: c_int, path: *const c_char, flags: c_int, mask: c_uint, out: *mut c_void) -> c_int,
        |r| [(out.cast::<u8>(), 256)]
    );
    #[cfg(target_os = "linux")]
    wrap!(
        copy_file_range(
            input: c_int,
            input_offset: *mut libc::loff_t,
            out: c_int,
            out_offset: *mut libc::loff_t,
            length: size_t,
            flags: c_uint
        ) -> ssize_t,
        |r| [of(input_offset), of(out_offset)]
    );

    weak!("dlsym");

    #[cfg(target_os = "linux")]
    /// The functions the Rust library looks up as it runs, as `statx`,
    /// which the linker cannot give their wrappers: the lookup gives them.
    #[unsafe(export_name = "tondo_rr_dlsym")]
    pub unsafe extern "C" fn dlsym_wrapper(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe extern "C" {
            #[link_name = "tondo_real_dlsym"]
            fn real(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        let found = unsafe { real(handle, name) };
        if found.is_null() {
            return found;
        }
        match unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() {
            b"statx" => statx::wrapper as *mut c_void,
            b"copy_file_range" => copy_file_range::wrapper as *mut c_void,
            b"getrandom" => getrandom::wrapper as *mut c_void,
            _ => found,
        }
    }
    wrap!(dirfd(dir: *mut libc::DIR) -> c_int, |r| none(r));

    /// The addresses of a name, in a list the C library allocates: recorded as
    /// the parts of each, and made again in a replay in one allocation, which
    /// `freeaddrinfo` frees.
    #[unsafe(export_name = "tondo_rr_getaddrinfo")]
    pub unsafe extern "C" fn getaddrinfo_wrapper(
        node: *const c_char,
        service: *const c_char,
        hints: *const libc::addrinfo,
        out: *mut *mut libc::addrinfo,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_getaddrinfo"]
            fn real(
                node: *const c_char,
                service: *const c_char,
                hints: *const libc::addrinfo,
                out: *mut *mut libc::addrinfo,
            ) -> c_int;
        }
        const ID: u32 = id("getaddrinfo");
        const PART: usize = 4 * 4 + 128;
        match here() {
            Here::Pass => unsafe { real(node, service, hints, out) },
            Here::Record => {
                let result = inside(|| unsafe { real(node, service, hints, out) });
                let _outside = Outside::enter();
                let mut parts = Vec::new();
                let mut at = if result == 0 { unsafe { *out } } else { std::ptr::null_mut() };
                while let Some(info) = unsafe { at.as_ref() } {
                    let mut part = [0u8; PART];
                    for (index, value) in
                        [info.ai_flags, info.ai_family, info.ai_socktype, info.ai_protocol].iter().enumerate()
                    {
                        part[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
                    }
                    let length = (info.ai_addrlen as usize).min(128);
                    part[16..16 + length]
                        .copy_from_slice(unsafe { std::slice::from_raw_parts(info.ai_addr.cast(), length) });
                    parts.push(((length as u32).to_le_bytes(), part));
                    at = info.ai_next;
                }
                let mut bytes: Vec<u8> = Vec::new();
                for (length, part) in &parts {
                    bytes.extend_from_slice(length);
                    bytes.extend_from_slice(part);
                }
                record(ID, result as i64, &[(bytes.as_mut_ptr(), bytes.len())]);
                result
            }
            Here::Replay => {
                let mut bytes = vec![0u8; 64 * (PART + 4)];
                let result = replay(ID, "getaddrinfo", |_| vec![(bytes.as_mut_ptr(), bytes.len())]);
                if result != 0 {
                    return result as c_int;
                }
                // The parts the recording holds end with a zero length.
                let parts: Vec<&[u8]> = bytes.chunks(PART + 4).take_while(|chunk| chunk[..4] != [0, 0, 0, 0]).collect();
                let node = std::mem::size_of::<libc::addrinfo>() + 128;
                let memory = unsafe { libc::calloc(parts.len().max(1), node) }.cast::<u8>();
                for (index, chunk) in parts.iter().enumerate() {
                    let length = u32::from_le_bytes(chunk[..4].try_into().expect("4 bytes")) as usize;
                    let part = &chunk[4..];
                    let word = |at: usize| c_int::from_le_bytes(part[at..at + 4].try_into().expect("4 bytes"));
                    unsafe {
                        let info = memory.add(index * node).cast::<libc::addrinfo>();
                        let address = memory.add(index * node + std::mem::size_of::<libc::addrinfo>());
                        std::ptr::copy_nonoverlapping(part[16..].as_ptr(), address, length);
                        (*info).ai_flags = word(0);
                        (*info).ai_family = word(4);
                        (*info).ai_socktype = word(8);
                        (*info).ai_protocol = word(12);
                        (*info).ai_addrlen = length as libc::socklen_t;
                        (*info).ai_addr = address.cast();
                        (*info).ai_next = if index + 1 < parts.len() {
                            memory.add((index + 1) * node).cast()
                        } else {
                            std::ptr::null_mut()
                        };
                    }
                }
                unsafe { *out = memory.cast() };
                0
            }
        }
    }

    #[unsafe(export_name = "tondo_rr_freeaddrinfo")]
    pub unsafe extern "C" fn freeaddrinfo_wrapper(list: *mut libc::addrinfo) {
        unsafe extern "C" {
            #[link_name = "tondo_real_freeaddrinfo"]
            fn real(list: *mut libc::addrinfo);
        }
        match here() {
            Here::Replay => unsafe { libc::free(list.cast()) },
            _ => unsafe { real(list) },
        }
    }

    /// What each descriptor is registered with in each epoll, and back: what
    /// a program registers may be an address, as tokio's are, which another
    /// run has elsewhere, so a recording has each event by its descriptor.
    /// An epoll is the one its copies are, as mio registers with one.
    #[derive(Default)]
    struct Registry {
        instances: std::collections::BTreeMap<c_int, c_int>,
        data: std::collections::BTreeMap<(c_int, c_int), u64>,
        fds: std::collections::BTreeMap<(c_int, u64), c_int>,
        /// A kqueue's: what each descriptor and filter is registered with.
        #[cfg(target_os = "macos")]
        filters: std::collections::BTreeMap<(c_int, usize, i16), usize>,
    }

    impl Registry {
        fn instance(&self, epoll: c_int) -> c_int {
            self.instances.get(&epoll).copied().unwrap_or(epoll)
        }
    }

    static REGISTRY: std::sync::Mutex<Option<Registry>> = std::sync::Mutex::new(None);

    /// `copy` is a copy of `fd`, which may be an epoll.
    fn duplicated(fd: c_int, copy: i64) {
        let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let Some(registry) = registry.as_mut() else { return };
        let copy = copy as c_int;
        match registry.instances.get(&fd).copied() {
            Some(instance) if copy >= 0 => registry.instances.insert(copy, instance),
            _ => registry.instances.remove(&copy),
        };
    }

    #[cfg(target_os = "linux")]
    fn registered(epoll: c_int, op: c_int, fd: c_int, event: *mut libc::epoll_event) {
        let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let registry = registry.get_or_insert_with(Registry::default);
        let epoll = registry.instance(epoll);
        if let Some(data) = registry.data.remove(&(epoll, fd)) {
            registry.fds.remove(&(epoll, data));
        }
        if op != libc::EPOLL_CTL_DEL && !event.is_null() {
            let data = unsafe { event.read_unaligned() }.u64;
            registry.data.insert((epoll, fd), data);
            registry.fds.insert((epoll, data), fd);
        }
    }

    #[cfg(target_os = "linux")]
    /// Each event's data as its descriptor, or `back` as what this run
    /// registered for the descriptor.
    fn translated(epoll: c_int, events: *mut libc::epoll_event, count: c_int, back: bool) {
        let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let Some(registry) = registry.as_ref() else { return };
        let epoll = registry.instance(epoll);
        for at in 0..count.max(0) as usize {
            let event = unsafe { events.add(at) };
            let mut read = unsafe { event.read_unaligned() };
            let data = read.u64;
            let found = match back {
                false => registry.fds.get(&(epoll, data)).map(|&fd| fd as u64),
                true => registry.data.get(&(epoll, data as c_int)).copied(),
            };
            if let Some(found) = found {
                read.u64 = found;
                unsafe { event.write_unaligned(read) };
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[unsafe(export_name = "tondo_rr_epoll_wait")]
    pub unsafe extern "C" fn epoll_wait_wrapper(
        epoll: c_int,
        events: *mut libc::epoll_event,
        most: c_int,
        timeout: c_int,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_epoll_wait"]
            fn real(epoll: c_int, events: *mut libc::epoll_event, most: c_int, timeout: c_int) -> c_int;
        }
        const ID: u32 = id("epoll_wait");
        let size = std::mem::size_of::<libc::epoll_event>();
        match here() {
            Here::Pass => unsafe { real(epoll, events, most, timeout) },
            Here::Record => {
                let result = inside(|| unsafe { real(epoll, events, most, timeout) });
                let error = unsafe { *errno() };
                let _outside = Outside::enter();
                let count = result.max(0) as usize;
                let mut kept: Vec<libc::epoll_event> =
                    (0..count).map(|at| unsafe { events.add(at).read_unaligned() }).collect();
                translated(epoll, kept.as_mut_ptr(), result, false);
                unsafe { *errno() = error };
                record(ID, result as i64, &[(kept.as_mut_ptr().cast(), count * size)]);
                result
            }
            Here::Replay => {
                let result =
                    replay(ID, "epoll_wait", |r| vec![(events.cast::<u8>(), r.max(0) as usize * size)]) as c_int;
                translated(epoll, events, result, true);
                result
            }
        }
    }

    /// A thread the program starts, and which of them it is.
    struct Started {
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        argument: *mut c_void,
        made: u64,
    }

    extern "C" fn started(what: *mut c_void) -> *mut c_void {
        let what = unsafe { Box::from_raw(what.cast::<Started>()) };
        MADE.set(what.made);
        (what.start)(what.argument)
    }

    /// A thread the program starts: the recording marks where, and a replay
    /// starts it again, as what it does is not recorded.
    #[unsafe(export_name = "tondo_rr_pthread_create")]
    pub unsafe extern "C" fn pthread_create_wrapper(
        thread: *mut libc::pthread_t,
        attributes: *const libc::pthread_attr_t,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        argument: *mut c_void,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_pthread_create"]
            fn real(
                thread: *mut libc::pthread_t,
                attributes: *const libc::pthread_attr_t,
                start: extern "C" fn(*mut c_void) -> *mut c_void,
                argument: *mut c_void,
            ) -> c_int;
        }
        const ID: u32 = id("pthread_create");
        static MADE_SO_FAR: AtomicU64 = AtomicU64::new(0);
        let start_it = || {
            let made = MADE_SO_FAR.fetch_add(1, Ordering::Relaxed) + 1;
            let what = Box::into_raw(Box::new(Started { start, argument, made }));
            let result = inside(|| unsafe { real(thread, attributes, started, what.cast()) });
            if result != 0 {
                drop(unsafe { Box::from_raw(what) });
            }
            result
        };
        match here() {
            Here::Pass => unsafe { real(thread, attributes, start, argument) },
            Here::Record => {
                record(ID, 0, &[]);
                start_it()
            }
            Here::Replay => {
                replay(ID, "pthread_create", none_vec);
                THREADED.store(true, Ordering::Relaxed);
                start_it()
            }
        }
    }

    // macOS's own calls.

    #[cfg(target_os = "macos")]
    weak!("readdir_r");

    /// The way the Rust library reads a directory on macOS, as some of its
    /// versions do: an entry, and whether there was one.
    #[cfg(target_os = "macos")]
    #[unsafe(export_name = "tondo_rr_readdir_r")]
    pub unsafe extern "C" fn readdir_r_wrapper(
        dir: *mut libc::DIR,
        entry: *mut libc::dirent,
        found: *mut *mut libc::dirent,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_readdir_r"]
            fn real(dir: *mut libc::DIR, entry: *mut libc::dirent, found: *mut *mut libc::dirent) -> c_int;
        }
        const ID: u32 = id("readdir_r");
        match here() {
            Here::Pass => unsafe { real(dir, entry, found) },
            Here::Record => {
                let result = inside(|| unsafe { real(dir, entry, found) });
                let mut there = u8::from(!unsafe { *found }.is_null());
                record(ID, result as i64, &[of(entry), of(&mut there)]);
                result
            }
            Here::Replay => {
                let mut there = 0u8;
                let result = replay(ID, "readdir_r", |_| vec![of(entry), of(&mut there)]);
                unsafe { *found = if there != 0 { entry } else { std::ptr::null_mut() } };
                result as c_int
            }
        }
    }

    #[cfg(target_os = "macos")]
    wrap!(kqueue() -> c_int, |r| {
        if r >= 0 {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.get_or_insert_with(Registry::default).instances.insert(r as c_int, r as c_int);
        }
        none(r)
    });
    #[cfg(target_os = "macos")]
    wrap!(CCRandomGenerateBytes(bytes: *mut c_void, count: size_t) -> c_int, |r| [(bytes.cast::<u8>(), count)]);
    #[cfg(target_os = "macos")]
    wrap!(getentropy(buf: *mut c_void, length: size_t) -> c_int, |r| [(buf.cast::<u8>(), length)]);
    #[cfg(target_os = "macos")]
    wrap!(
        sysctl(name: *mut c_int, count: c_uint, old: *mut c_void, length: *mut size_t, new: *mut c_void, new_length: size_t) -> c_int,
        |r| [of(length), (old.cast::<u8>(), if length.is_null() { 0 } else { unsafe { *length } })]
    );
    #[cfg(target_os = "macos")]
    wrap!(
        sysctlbyname(name: *const c_char, old: *mut c_void, length: *mut size_t, new: *mut c_void, new_length: size_t) -> c_int,
        |r| [of(length), (old.cast::<u8>(), if length.is_null() { 0 } else { unsafe { *length } })]
    );
    #[cfg(target_os = "macos")]
    wrap!(confstr(name: c_int, buf: *mut c_char, length: size_t) -> size_t, |r| [(
        buf.cast::<u8>(),
        length.min(r.max(0) as usize)
    )]);
    #[cfg(target_os = "macos")]
    wrap!(statfs(path: *const c_char, out: *mut libc::statfs) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "macos")]
    wrap!(fstatfs(fd: c_int, out: *mut libc::statfs) -> c_int, |r| [of(out)]);
    #[cfg(target_os = "macos")]
    wrap!(futimes(fd: c_int, times: *const libc::timeval) -> c_int, |r| none(r));
    #[cfg(target_os = "macos")]
    wrap!(
        fsetattrlist(fd: c_int, list: *mut c_void, buffer: *mut c_void, size: size_t, options: c_ulong) -> c_int,
        |r| none(r)
    );
    #[cfg(target_os = "macos")]
    wrap!(fclonefileat(from: c_int, dir: c_int, to: *const c_char, flags: u32) -> c_int, |r| none(r));
    #[cfg(target_os = "macos")]
    wrap!(fcopyfile(from: c_int, to: c_int, state: *mut c_void, flags: u32) -> c_int, |r| none(r));
    #[cfg(target_os = "macos")]
    wrap!(copyfile_state_get(state: *mut c_void, flag: u32, out: *mut c_void) -> c_int, |r| [(
        out.cast::<u8>(),
        std::mem::size_of::<libc::off_t>()
    )]);
    #[cfg(target_os = "macos")]
    wrap!(proc_pidinfo(pid: c_int, flavor: c_int, argument: u64, buffer: *mut c_void, size: c_int) -> c_int, |r| [
        filled(buffer, r)
    ]);
    #[cfg(target_os = "macos")]
    wrap!(gethostuuid(id: *mut [u8; 16], wait: *const libc::timespec) -> c_int, |r| [of(id)]);

    /// The registrations of a kqueue's changes, as this run makes them.
    #[cfg(target_os = "macos")]
    fn noted(kq: c_int, changes: *const libc::kevent, count: c_int) {
        let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let registry = registry.get_or_insert_with(Registry::default);
        let kq = registry.instance(kq);
        for at in 0..count.max(0) as usize {
            let change = unsafe { changes.add(at).read_unaligned() };
            let key = (kq, change.ident, change.filter);
            if change.flags & libc::EV_DELETE != 0 {
                registry.filters.remove(&key);
            } else {
                registry.filters.insert(key, change.udata as usize);
            }
        }
    }

    /// The events a replay gives, with what this run registered for each
    /// descriptor and filter: what a program registers may be an address,
    /// as tokio's are, which another run has elsewhere.
    #[cfg(target_os = "macos")]
    fn given(kq: c_int, events: *mut libc::kevent, count: c_int) {
        let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let Some(registry) = registry.as_ref() else { return };
        let kq = registry.instance(kq);
        for at in 0..count.max(0) as usize {
            let mut event = unsafe { events.add(at).read_unaligned() };
            if let Some(&udata) = registry.filters.get(&(kq, event.ident, event.filter)) {
                event.udata = udata as *mut c_void;
                unsafe { events.add(at).write_unaligned(event) };
            }
        }
    }

    #[cfg(target_os = "macos")]
    weak!("kevent");

    /// A kqueue's changes and events, in one call: the changes are noted
    /// before the call, which may write its events over them, as mio's
    /// receipts are.
    #[cfg(target_os = "macos")]
    #[unsafe(export_name = "tondo_rr_kevent")]
    pub unsafe extern "C" fn kevent_wrapper(
        kq: c_int,
        changes: *const libc::kevent,
        change_count: c_int,
        events: *mut libc::kevent,
        event_count: c_int,
        timeout: *const libc::timespec,
    ) -> c_int {
        unsafe extern "C" {
            #[link_name = "tondo_real_kevent"]
            fn real(
                kq: c_int,
                changes: *const libc::kevent,
                change_count: c_int,
                events: *mut libc::kevent,
                event_count: c_int,
                timeout: *const libc::timespec,
            ) -> c_int;
        }
        const ID: u32 = id("kevent");
        let size = std::mem::size_of::<libc::kevent>();
        let real = || unsafe { real(kq, changes, change_count, events, event_count, timeout) };
        match here() {
            Here::Pass => real(),
            Here::Record => {
                {
                    let _outside = Outside::enter();
                    noted(kq, changes, change_count);
                }
                let result = inside(real);
                let _outside = Outside::enter();
                record(ID, result as i64, &[(events.cast(), result.max(0) as usize * size)]);
                result
            }
            Here::Replay => {
                {
                    let _outside = Outside::enter();
                    noted(kq, changes, change_count);
                }
                let result = replay(ID, "kevent", |r| vec![(events.cast::<u8>(), r.max(0) as usize * size)]) as c_int;
                let _outside = Outside::enter();
                given(kq, events, result);
                result
            }
        }
    }
}
