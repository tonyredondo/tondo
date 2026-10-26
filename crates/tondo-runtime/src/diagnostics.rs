//! Diagnostics of a running program (toolchain 7.5): the runtime's counters
//! and the program's own (`std.diagnostics`), which `tondo ps` and `tondo
//! counters` read through a Unix socket in a directory only the user can
//! enter, `$TMPDIR/tondo-<uid>/<pid>`. A thread of its own serves it, so a
//! program whose workers are all busy or stuck still answers.

use super::Str;
use std::cell::Cell;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What allocations and the counts of objects do beyond their work, a bit
/// each in one word that their fast path reads once: keep to a budget of
/// memory (a `meta` program), count for the tools (a program that starts
/// with `TONDO_DIAGNOSTICS=1`, is traced, or leaves a dump at a crash),
/// count objects exactly (the leak check), or count the allocations of a
/// benchmark. A program nobody watches pays
/// for nothing but that load.
static HOOKS: AtomicU8 = AtomicU8::new(if cfg!(tondo_staticlib) { 0 } else { EXACT });
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) const BUDGET: u8 = 1;
pub(crate) const WATCHED: u8 = 2;
const EXACT: u8 = 4;
/// Counts allocations for a benchmark (`testing.bench`).
pub(crate) const MEASURED: u8 = 8;

#[inline(always)]
pub(crate) fn hooks() -> u8 {
    HOOKS.load(Ordering::Relaxed)
}

/// Turns a hook on, before the program starts.
pub(crate) fn hook(bit: u8) {
    HOOKS.fetch_or(bit, Ordering::Relaxed);
}

/// Turns a hook off.
pub(crate) fn unhook(bit: u8) {
    HOOKS.fetch_and(!bit, Ordering::Relaxed);
}

#[inline]
pub(crate) fn enabled() -> bool {
    hooks() & WATCHED != 0
}

/// A count of the runtime: a total that only grows, or a level.
pub(crate) struct Count(AtomicI64);

impl Count {
    pub(crate) const fn new() -> Count {
        Count(AtomicI64::new(0))
    }

    pub(crate) fn add(&self, amount: i64) {
        if enabled() {
            self.0.fetch_add(amount, Ordering::Relaxed);
        }
    }

    pub(crate) fn max(&self, value: i64) {
        if enabled() {
            self.0.fetch_max(value, Ordering::Relaxed);
        }
    }

    fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Counts itself in a level while it lives.
pub(crate) struct Open(&'static Count);

impl Open {
    pub(crate) fn new(count: &'static Count) -> Open {
        count.add(1);
        Open(count)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.0.add(-1);
    }
}

pub(crate) static TASKS_SPAWNED: Count = Count::new();
pub(crate) static WORKERS_REPLACED: Count = Count::new();
pub(crate) static COLLECTIONS: Count = Count::new();
pub(crate) static COLLECTED: Count = Count::new();
/// Microseconds the workers stood still for the collector, in all and at
/// most at once.
pub(crate) static COLLECTOR_PAUSED: Count = Count::new();
pub(crate) static COLLECTOR_LONGEST: Count = Count::new();
pub(crate) static NET_CONNECTIONS: Count = Count::new();
pub(crate) static NET_READ: Count = Count::new();
pub(crate) static NET_WRITTEN: Count = Count::new();
pub(crate) static HTTP_CLIENT_REQUESTS: Count = Count::new();
pub(crate) static HTTP_SERVER_REQUESTS: Count = Count::new();
pub(crate) static HTTP_SERVER_ACTIVE: Count = Count::new();
pub(crate) static PROCESSES_STARTED: Count = Count::new();

// ---------------------------------------------------------------------------
// Memory and objects
// ---------------------------------------------------------------------------

/// What one thread allocated, freed and made of objects. Only its thread
/// adds to it, with no atomic operation, which on many cores would make a
/// program that allocates much slower; the tools read the sum of every
/// block. A block outlives its thread, and the next thread to start takes
/// over a runtime thread's.
#[repr(align(128))]
struct Block {
    allocated: AtomicU64,
    freed: AtomicU64,
    allocations: AtomicU64,
    objects: AtomicI64,
    /// The block made before this one.
    next: *const Block,
    /// The next spare block, while this one is.
    spare: AtomicPtr<Block>,
}

/// The last block made, from which every block can be reached.
static BLOCKS: AtomicPtr<Block> = AtomicPtr::new(std::ptr::null_mut());
/// Blocks whose threads ended, for the next ones, behind a lock that
/// allocates nothing: a thread takes one while it allocates, and a
/// `Mutex` may allocate when it is first locked, as on macOS.
static SPARE: AtomicPtr<Block> = AtomicPtr::new(std::ptr::null_mut());
static SPARE_LOCK: AtomicBool = AtomicBool::new(false);

fn with_spare<R>(f: impl FnOnce(&AtomicPtr<Block>) -> R) -> R {
    while SPARE_LOCK.swap(true, Ordering::Acquire) {
        std::hint::spin_loop();
    }
    let result = f(&SPARE);
    SPARE_LOCK.store(false, Ordering::Release);
    result
}
/// The block of a thread whose memory does not count: one that serves the
/// tools, so that looking at a program does not change what it shows.
const UNCOUNTED: *const Block = std::ptr::dangling::<Block>();

thread_local! {
    static BLOCK: Cell<*const Block> = const { Cell::new(std::ptr::null()) };
}

fn block() -> Option<&'static Block> {
    let block = BLOCK.with(|cell| {
        if cell.get().is_null() {
            cell.set(new_block());
        }
        cell.get()
    });
    (block != UNCOUNTED).then(|| unsafe { &*block })
}

/// A spare block, or one from the system's allocator, which counts
/// nothing.
#[cold]
fn new_block() -> *const Block {
    let spare = with_spare(|head| {
        let block = head.load(Ordering::Relaxed);
        if let Some(taken) = unsafe { block.as_ref() } {
            head.store(taken.spare.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        block
    });
    if !spare.is_null() {
        return spare;
    }
    let layout = std::alloc::Layout::new::<Block>();
    let block = unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) } as *mut Block;
    if block.is_null() {
        return UNCOUNTED;
    }
    let mut head = BLOCKS.load(Ordering::Acquire);
    loop {
        let zero = || AtomicU64::new(0);
        let new = Block {
            allocated: zero(),
            freed: zero(),
            allocations: zero(),
            objects: AtomicI64::new(0),
            next: head,
            spare: AtomicPtr::new(std::ptr::null_mut()),
        };
        unsafe { block.write(new) };
        match BLOCKS.compare_exchange_weak(head, block, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return block,
            Err(now) => head = now,
        }
    }
}

/// The sum of a field over every block.
fn total(field: impl Fn(&Block) -> i64) -> i64 {
    let mut sum = 0;
    let mut block = BLOCKS.load(Ordering::Acquire) as *const Block;
    while let Some(next) = unsafe { block.as_ref() } {
        sum += field(next);
        block = next.next;
    }
    sum
}

fn bump(counter: &AtomicU64, by: u64) {
    counter.store(counter.load(Ordering::Relaxed).wrapping_add(by), Ordering::Relaxed);
}

/// Gives this thread's block to the next thread, as a runtime thread does
/// when it ends; what the thread does after that does not count, as for
/// the threads that serve the tools.
pub(crate) fn retire_here() {
    let block = BLOCK.with(|cell| cell.replace(UNCOUNTED));
    if let Some(retired) = unsafe { block.as_ref() }.filter(|_| block != UNCOUNTED) {
        with_spare(|head| {
            retired.spare.store(head.load(Ordering::Relaxed), Ordering::Relaxed);
            head.store(block as *mut Block, Ordering::Relaxed);
        });
    }
}

/// After an allocation of a program that is watched.
#[cold]
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn allocated(address: *mut u8, size: usize) {
    if let Some(block) = block() {
        bump(&block.allocated, size as u64);
        bump(&block.allocations, 1);
        super::heap::allocated(address as usize, size);
        super::heap::track(address as usize, size);
    }
}

/// Before a program that is watched frees memory.
#[cold]
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn freed(address: *mut u8, size: usize) {
    super::heap::freed(address as usize);
    super::heap::untrack(address as usize);
    if let Some(block) = block() {
        bump(&block.freed, size as u64);
    }
}

/// Objects counted in one place: with `TONDO_RT_CHECK_LEAKS`, so that the
/// count at exit is exact, and in the runtime's own tests.
static OBJECTS: AtomicI64 = AtomicI64::new(0);

/// The heap objects of the program, strings and boxes, that are alive.
pub(crate) struct Objects;

impl Objects {
    #[inline(always)]
    pub(crate) fn made(&self) {
        if hooks() & (EXACT | WATCHED) != 0 {
            self.count(1);
        }
    }

    #[inline(always)]
    pub(crate) fn gone(&self) {
        if hooks() & (EXACT | WATCHED) != 0 {
            self.count(-1);
        }
    }

    #[cold]
    fn count(&self, by: i64) {
        if hooks() & EXACT != 0 {
            OBJECTS.fetch_add(by, Ordering::Relaxed);
        } else if let Some(block) = block() {
            block.objects.store(block.objects.load(Ordering::Relaxed) + by, Ordering::Relaxed);
        }
    }

    pub(crate) fn live(&self) -> usize {
        (OBJECTS.load(Ordering::Relaxed) + total(|b| b.objects.load(Ordering::Relaxed))).max(0) as usize
    }
}

// ---------------------------------------------------------------------------
// The program's counters
// ---------------------------------------------------------------------------

/// A counter or gauge of `std.diagnostics`, which lives as long as the
/// program.
struct Metric {
    name: String,
    gauge: bool,
    value: AtomicI64,
}

static METRICS: Mutex<Vec<&'static Metric>> = Mutex::new(Vec::new());

/// The counter (`kind` 0) or gauge (1) named `name`, the same one each
/// time; 0 when the name is the other kind's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_metric(name: *const Str, kind: i64) -> i64 {
    let name = String::from_utf8_lossy(unsafe { super::str_bytes(name) }).into_owned();
    let gauge = kind == 1;
    let mut metrics = METRICS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(metric) = metrics.iter().find(|m| m.name == name) {
        return if metric.gauge == gauge { *metric as *const Metric as i64 } else { 0 };
    }
    let metric: &'static Metric = Box::leak(Box::new(Metric { name, gauge, value: AtomicI64::new(0) }));
    metrics.push(metric);
    metric as *const Metric as i64
}

fn metric(id: i64) -> &'static Metric {
    unsafe { &*(id as *const Metric) }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_metric_add(id: i64, amount: i64) {
    metric(id).value.fetch_add(amount, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_metric_set(id: i64, value: i64) {
    metric(id).value.store(value, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_metric_value(id: i64) -> i64 {
    metric(id).value.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// One counter as `tondo counters` reads it: a total that only grows
/// (`gauge` false), whose rate the tool shows, or a level.
struct Sample {
    name: String,
    gauge: bool,
    unit: &'static str,
    value: i64,
}

fn sample() -> Vec<Sample> {
    let mut out = Vec::new();
    let mut add = |name: &str, gauge: bool, unit: &'static str, value: i64| {
        out.push(Sample { name: name.into(), gauge, unit, value });
    };
    let allocated = total(|b| b.allocated.load(Ordering::Relaxed) as i64);
    let freed = total(|b| b.freed.load(Ordering::Relaxed) as i64);
    add("memory.held", true, "bytes", (allocated - freed).max(0));
    add("memory.allocated", false, "bytes", allocated);
    add("memory.allocations", false, "", total(|b| b.allocations.load(Ordering::Relaxed) as i64));
    add("memory.objects", true, "", super::LIVE_OBJECTS.live() as i64);
    add("collector.collections", false, "", COLLECTIONS.get());
    add("collector.freed", false, "", COLLECTED.get());
    add("collector.paused", false, "us", COLLECTOR_PAUSED.get());
    add("collector.longest", true, "us", COLLECTOR_LONGEST.get());
    let scheduler = super::tasks::scheduler();
    add("tasks.live", true, "", scheduler.live as i64);
    add("tasks.ready", true, "", scheduler.ready as i64);
    add("tasks.spawned", false, "", TASKS_SPAWNED.get());
    add("workers.total", true, "", scheduler.workers as i64);
    add("workers.idle", true, "", scheduler.idle as i64);
    add("workers.in_c", true, "", scheduler.in_c as i64);
    add("workers.replaced", false, "", WORKERS_REPLACED.get());
    add("workers.c_calls", false, "", scheduler.c_calls as i64);
    add("timers.pending", true, "", scheduler.timers as i64);
    add("net.connections", false, "", NET_CONNECTIONS.get());
    add("net.read", false, "bytes", NET_READ.get());
    add("net.written", false, "bytes", NET_WRITTEN.get());
    add("http.client.requests", false, "", HTTP_CLIENT_REQUESTS.get());
    add("http.server.requests", false, "", HTTP_SERVER_REQUESTS.get());
    add("http.server.active", true, "", HTTP_SERVER_ACTIVE.get());
    let open = descriptors();
    add("sockets.tcp", true, "", open.tcp);
    add("sockets.listening", true, "", open.listening);
    add("sockets.udp", true, "", open.udp);
    add("sockets.unix", true, "", open.unix);
    add("descriptors.files", true, "", open.files);
    add("descriptors.pipes", true, "", open.pipes);
    add("descriptors.total", true, "", open.total);
    add("processes.running", true, "", super::process::running() as i64);
    add("processes.started", false, "", PROCESSES_STARTED.get());
    add("os.cpu", false, "us", cpu_time());
    add("os.resident", true, "bytes", resident());
    add("os.threads", true, "", super::tasks::os_threads().unwrap_or(0) as i64);
    for metric in METRICS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        add(&format!("program.{}", metric.name), metric.gauge, "", metric.value.load(Ordering::Relaxed));
    }
    out
}

/// The process's open descriptors by what they are.
#[derive(Default)]
struct Descriptors {
    total: i64,
    files: i64,
    pipes: i64,
    tcp: i64,
    listening: i64,
    udp: i64,
    unix: i64,
}

fn descriptors() -> Descriptors {
    let mut open = Descriptors::default();
    let Ok(entries) = std::fs::read_dir("/dev/fd") else { return open };
    let numbers: Vec<i32> = entries.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    let ours = SOCKET.get().map(|path| path.as_os_str().as_encoded_bytes());
    // The listing's own descriptor is closed by now, and `fstat` skips it.
    for fd in numbers {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            continue;
        }
        open.total += 1;
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => open.files += 1,
            libc::S_IFIFO => open.pipes += 1,
            libc::S_IFSOCK => {
                let option = |name: libc::c_int| {
                    let (mut value, mut size): (libc::c_int, libc::socklen_t) = (0, 4);
                    unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, name, (&raw mut value).cast(), &mut size) };
                    value
                };
                let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut size = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                unsafe { libc::getsockname(fd, (&raw mut address).cast(), &mut size) };
                // Those of the diagnostics themselves, whose address is
                // their socket's path, do not count.
                let path = unsafe { &*(&raw const address).cast::<libc::sockaddr_un>() }.sun_path;
                let path: Vec<u8> = path.iter().take_while(|&&c| c != 0).map(|c| c.to_ne_bytes()[0]).collect();
                match () {
                    _ if address.ss_family as i32 == libc::AF_UNIX && ours == Some(&path[..]) => open.total -= 1,
                    _ if address.ss_family as i32 == libc::AF_UNIX => open.unix += 1,
                    _ if option(libc::SO_TYPE) == libc::SOCK_DGRAM => open.udp += 1,
                    _ if listening(fd) => open.listening += 1,
                    _ => open.tcp += 1,
                }
            }
            _ => {}
        }
    }
    open
}

/// Whether a stream socket listens for connections.
#[cfg(target_os = "linux")]
fn listening(fd: i32) -> bool {
    let (mut value, mut size): (libc::c_int, libc::socklen_t) = (0, 4);
    unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ACCEPTCONN, (&raw mut value).cast(), &mut size) };
    value != 0
}

/// macOS does not answer `SO_ACCEPTCONN`: a stream socket with no peer
/// listens, or is about to connect.
#[cfg(not(target_os = "linux"))]
fn listening(fd: i32) -> bool {
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    unsafe { libc::getpeername(fd, (&raw mut address).cast(), &mut size) != 0 }
}

/// The processor time the process used, in microseconds.
fn cpu_time() -> i64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS's microseconds are an `i32`.
    #[allow(clippy::unnecessary_cast)]
    let micros = |t: libc::timeval| t.tv_sec as i64 * 1_000_000 + t.tv_usec as i64;
    micros(usage.ru_utime) + micros(usage.ru_stime)
}

/// The memory of the process that is in RAM, in bytes.
#[cfg(target_os = "linux")]
fn resident() -> i64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: i64 = statm.split_whitespace().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as i64
}

#[cfg(target_os = "macos")]
fn resident() -> i64 {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let read = unsafe { libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast(), size) };
    if read == size { info.pti_resident_size as i64 } else { 0 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resident() -> i64 {
    0
}

// ---------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------

static STARTED: OnceLock<Instant> = OnceLock::new();
static SOCKET: OnceLock<PathBuf> = OnceLock::new();

/// Where the processes of this user listen: `$TMPDIR/tondo-<uid>`.
fn directory() -> PathBuf {
    std::env::temp_dir().join(format!("tondo-{}", unsafe { libc::geteuid() }))
}

/// Listens for the tools, unless `TONDO_DIAGNOSTICS=0`. A directory that
/// is not the user's alone, or a socket that cannot be made, leaves the
/// program without diagnostics and otherwise as it was.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn start() {
    STARTED.get_or_init(Instant::now);
    super::recorder::thread();
    if std::env::var_os("TONDO_RT_CHECK_LEAKS").is_some() {
        hook(EXACT);
    }
    let on = |name: &str| std::env::var_os(name).is_some_and(|value| value == "1");
    let debugged = on("TONDO_DEBUG");
    let live = super::live::on() && std::env::var_os("TONDO_DIAGNOSTICS").is_none_or(|value| value != "0");
    // `cpu`: a profile of processor time, which counting the memory would
    // change.
    let processor = std::env::var_os("TONDO_TRACE").is_some_and(|value| value == "cpu");
    let traced = on("TONDO_TRACE") || processor;
    if !on("TONDO_DIAGNOSTICS") && !traced && !debugged && !live && std::env::var_os("TONDO_DUMP").is_none() {
        return;
    }
    if !processor || on("TONDO_DIAGNOSTICS") {
        super::heap::prepare();
        hook(WATCHED);
    }
    // `tondo debug -- program` debugs that program, not those it starts.
    if debugged {
        unsafe { std::env::remove_var("TONDO_DEBUG") };
        super::debugger::WAIT.store(true, Ordering::Relaxed);
    }
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let directory = directory();
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&directory);
    let private = std::fs::symlink_metadata(&directory)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0);
    if !private {
        return;
    }
    let path = directory.join(std::process::id().to_string());
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    let _ = SOCKET.set(path);
    // `atexit` is not a symbol of glibc's shared library.
    unsafe extern "C" {
        fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
    }
    extern "C" fn remove(_: *mut u8) {
        end_trace(&mut SESSION.lock().unwrap_or_else(|e| e.into_inner()), "exit");
        super::debugger::exited();
        if let Some(path) = SOCKET.get() {
            let _ = std::fs::remove_file(path);
        }
    }
    unsafe { __cxa_atexit(remove, std::ptr::null_mut(), std::ptr::null_mut()) };
    // A runtime thread, which a deadlock does not take for one of C.
    if super::tasks::try_spawn_thread(Some(128 * 1024), move || serve(listener)).is_err() {
        return;
    }
    // `tondo trace -- program` traces from the start: the program waits
    // for it, a while, before `main`. Its own programs are not traced.
    if traced {
        unsafe { std::env::remove_var("TONDO_TRACE") };
        super::recorder::start();
        let session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        let waited = TRACED.wait_timeout_while(session, std::time::Duration::from_secs(5), |s| s.is_none());
        drop(waited);
    }
}

/// Answers every tool from this one thread: each line one sends, with a
/// line of JSON.
fn serve(listener: UnixListener) {
    retire_here();
    let _ = listener.set_nonblocking(true);
    let mut clients: Vec<(UnixStream, Vec<u8>)> = Vec::new();
    loop {
        let descriptors = std::iter::once(listener.as_raw_fd()).chain(clients.iter().map(|(c, _)| c.as_raw_fd()));
        let mut polled: Vec<libc::pollfd> =
            descriptors.map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        // While a tool traces, what was recorded goes to it ten times a
        // second.
        let tracing = SESSION.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let timeout = if tracing { 100 } else { -1 };
        if unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, timeout) } < 0 {
            continue;
        }
        let mut ready = polled[1..].iter().map(|p| p.revents != 0);
        clients.retain_mut(|(client, pending)| {
            let kept = !ready.next().unwrap_or(false) || answer(client, pending);
            if !kept {
                let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
                if session.as_ref().is_some_and(|s| s.fd == client.as_raw_fd()) {
                    super::recorder::stop();
                    *session = None;
                }
                if super::debugger::is_client(client.as_raw_fd()) {
                    super::debugger::detach();
                }
            }
            kept
        });
        push(&mut SESSION.lock().unwrap_or_else(|e| e.into_inner()));
        if polled[0].revents != 0 {
            while let Ok((client, _)) = listener.accept() {
                let _ = client.set_nonblocking(false);
                let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(1)));
                clients.push((client, Vec::new()));
            }
        }
    }
}

/// Reads what a tool sent and answers its whole lines; false once it is
/// gone, or sends a line longer than any request.
fn answer(client: &mut UnixStream, pending: &mut Vec<u8>) -> bool {
    let mut buffer = [0; 1024];
    let count = match client.read(&mut buffer) {
        Ok(0) | Err(_) => return false,
        Ok(count) => count,
    };
    pending.extend_from_slice(&buffer[..count]);
    while let Some(end) = pending.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = pending.drain(..=end).collect();
        let line = String::from_utf8_lossy(&line).trim().to_string();
        let reply = match line.as_str() {
            _ if super::debugger::is_client(client.as_raw_fd())
                && let Some(reply) = super::debugger::command(&line) =>
            {
                reply
            }
            "debug" => super::debugger::attach(client),
            "info" => info(),
            "live" => super::live::describe(),
            "barrier" => super::live::hold(),
            "release" => super::live::let_go(),
            _ if line.starts_with("again ") => super::live::again(&line["again ".len()..]),
            _ if line.starts_with("patch ") => super::live::patch(&line["patch ".len()..]),
            "counters" => counters(),
            "memory" => {
                let mut frames = Frames::default();
                let memory = memory(&mut frames);
                format!("{{\"memory\":{memory},\"frames\":{{{}}}}}", frames.json)
            }
            "trace" => trace(client),
            "dump" => dump("request", "", None, Some(std::time::Duration::from_millis(250))),
            "stop" => {
                let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
                if session.as_ref().is_some_and(|s| s.fd == client.as_raw_fd()) {
                    end_trace(&mut session, "stop");
                    continue;
                }
                "{\"error\":\"this tool does not trace the program\"}".to_string()
            }
            other => format!("{{\"error\":{}}}", quoted(&format!("unknown request `{other}`"))),
        };
        if writeln!(client, "{reply}").is_err() {
            return false;
        }
        super::debugger::answered();
    }
    pending.len() < 4096
}

// ---------------------------------------------------------------------------
// Tracing
// ---------------------------------------------------------------------------

/// The tool that traces the program, and the addresses it knows the frames
/// of.
struct Session {
    stream: UnixStream,
    fd: i32,
    described: std::collections::HashSet<usize>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static TRACED: std::sync::Condvar = std::sync::Condvar::new();

fn trace(client: &UnixStream) -> String {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.is_some() {
        return "{\"error\":\"another tool traces this program\"}".into();
    }
    let Ok(stream) = client.try_clone() else { return "{\"error\":\"cannot trace\"}".into() };
    *session = Some(Session { stream, fd: client.as_raw_fd(), described: Default::default() });
    if !super::recorder::on() {
        super::recorder::start();
    }
    TRACED.notify_all();
    format!("{{\"tracing\":true,\"clock\":{}}}", super::recorder::now())
}

/// Sends the tool what was recorded since the last time; a tool that is
/// gone ends the trace.
fn push(session: &mut Option<Session>) {
    let Some(tracer) = session.as_mut() else { return };
    let line = recorded(tracer);
    if writeln!(tracer.stream, "{line}").is_err() {
        super::recorder::stop();
        *session = None;
    }
}

/// Ends the trace: what is left goes to the tool, and then why it ended;
/// as the program exits, with its memory, which is not there to ask for
/// afterwards.
fn end_trace(session: &mut Option<Session>, why: &str) {
    if session.is_none() {
        return;
    }
    super::recorder::stop();
    push(session);
    if let Some(mut tracer) = session.take() {
        let mut frames = Frames { described: std::mem::take(&mut tracer.described), json: String::new() };
        let memory = if why == "exit" { memory(&mut frames) } else { "null".into() };
        let _ = writeln!(tracer.stream, "{{\"end\":\"{why}\",\"memory\":{memory},\"frames\":{{{}}}}}", frames.json);
    }
}

/// The samples of the memory alive and what each stack allocated in all,
/// as JSON (toolchain 7.15).
fn memory(frames: &mut Frames) -> String {
    let (samples, dropped) = super::heap::alive();
    let (allocated, unstacked) = super::heap::allocations();
    let mut out =
        format!("{{\"rate\":{},\"dropped\":{dropped},\"unstacked\":{unstacked},\"samples\":[", super::heap::RATE);
    for (index, (size, stack)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = write!(out, "{comma}{{\"size\":{size},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("],\"allocated\":[");
    for (index, (objects, bytes, stack)) in allocated.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ =
            write!(out, "{comma}{{\"objects\":{objects:.1},\"bytes\":{bytes:.0},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("]}");
    out
}

/// A line of JSON with the events and samples recorded since the last one,
/// and the frames of the addresses they hold that the tool does not know.
fn recorded(session: &mut Session) -> String {
    use super::recorder::{Event, Stop};
    let (events, dropped) = super::recorder::take_events();
    let (samples, lost) = super::recorder::take_samples();
    let mut frames = Frames { described: std::mem::take(&mut session.described), json: String::new() };
    let mut stack = |addresses: &[usize]| frames.list(addresses);
    let mut out = String::from("{\"events\":[");
    for (index, (at, thread, event)) in events.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = match event {
            Event::Run { task } => write!(out, "{comma}[\"run\",{at},{thread},{task}]"),
            Event::Stop { task, why: Stop::Done } => write!(out, "{comma}[\"stop\",{at},{thread},{task},\"done\"]"),
            Event::Stop { task, why: Stop::Yields } => {
                write!(out, "{comma}[\"stop\",{at},{thread},{task},\"yields\"]")
            }
            Event::Stop { task, why: Stop::Waits(at_stack) } => {
                let frames = stack(at_stack);
                write!(out, "{comma}[\"stop\",{at},{thread},{task},\"waits\",{frames}]")
            }
            Event::Spawn { task, parent, code, at: from } => {
                let code = stack(&[*code + 1]);
                let frames = stack(from);
                write!(out, "{comma}[\"spawn\",{at},{thread},{task},{parent},{code},{frames}]")
            }
            Event::Pause { took } => write!(out, "{comma}[\"pause\",{at},{thread},{took}]"),
            Event::Call { task, took, at: from } => {
                let frames = stack(from);
                write!(out, "{comma}[\"call\",{at},{thread},{task},{took},{frames}]")
            }
            Event::Panic { task, message } => {
                write!(out, "{comma}[\"panic\",{at},{thread},{task},{}]", quoted(message))
            }
        };
    }
    out.push_str("],\"samples\":[");
    for (index, (at, thread, sampled)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let frames = stack(sampled.frames());
        let _ = write!(out, "{comma}[{at},{thread},{frames}]");
    }
    session.described = std::mem::take(&mut frames.described);
    let _ = write!(out, "],\"frames\":{{{}}},\"dropped\":{dropped},\"lost\":{lost}}}", frames.json);
    out
}

/// What the tool shows for an address: a function of the program, with its
/// file and line where it calls another; or else the file loaded there, the
/// program's own as `""`, and how far into it the address is, which the
/// tool finds in the file's symbols.
fn frame(address: usize) -> String {
    if let Some(described) = super::trace::describe(address) {
        let function = quoted(described.function);
        let ty = described.ty.map_or(String::new(), |ty| format!(",\"type\":{}", quoted(ty)));
        return match described.at {
            Some((file, line)) => format!("{{\"function\":{function},\"file\":{},\"line\":{line}{ty}}}", quoted(file)),
            None => format!("{{\"function\":{function}}}"),
        };
    }
    match loaded(address) {
        Some((image, offset)) => format!("{{\"image\":{},\"offset\":{offset}}}", quoted(&image)),
        None => "{}".into(),
    }
}

/// The file loaded where `address` is, and the address's offset from where
/// it was loaded. A static executable has no loader to ask: its code is all
/// the program's, loaded where its ELF header is.
fn loaded(address: usize) -> Option<(String, usize)> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(address as *const libc::c_void, &mut info) } == 0 || info.dli_fname.is_null() {
        #[cfg(target_os = "linux")]
        {
            unsafe extern "C" {
                static __ehdr_start: u8;
            }
            return Some((String::new(), address.wrapping_sub(&raw const __ehdr_start as usize)));
        }
        #[cfg(not(target_os = "linux"))]
        return None;
    }
    let image = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_string_lossy().into_owned();
    Some((image, address.wrapping_sub(info.dli_fbase as usize)))
}

fn uptime_ms() -> u128 {
    STARTED.get().map_or(0, |started| started.elapsed().as_millis())
}

fn info() -> String {
    let program = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let arguments: Vec<String> = std::env::args_os().skip(1).map(|a| quoted(&a.to_string_lossy())).collect();
    format!(
        "{{\"pid\":{},\"program\":{},\"arguments\":[{}],\"uptime_ms\":{}}}",
        std::process::id(),
        quoted(&program),
        arguments.join(","),
        uptime_ms()
    )
}

fn counters() -> String {
    format!("{{\"uptime_ms\":{},\"counters\":{}}}", uptime_ms(), counter_list())
}

/// The counters as a JSON array.
fn counter_list() -> String {
    let mut out = String::from("[");
    for (index, sample) in sample().iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let kind = if sample.gauge { "gauge" } else { "counter" };
        let _ = write!(
            out,
            "{comma}{{\"name\":{},\"kind\":\"{kind}\",\"unit\":\"{}\",\"value\":{}}}",
            quoted(&sample.name),
            sample.unit,
            sample.value
        );
    }
    out.push(']');
    out
}

// ---------------------------------------------------------------------------
// Dumps
// ---------------------------------------------------------------------------

/// A dump of the program, as JSON: its counters, every task and where it
/// is, and the samples of its live memory. The workers stop for it a while
/// when `wait` says, so that the running tasks show where they are; `here`
/// is the stack of this thread's own task.
fn dump(reason: &str, message: &str, here: Option<Vec<usize>>, wait: Option<std::time::Duration>) -> String {
    let (stopped, tasks) = super::cycles::without_collections(|| {
        let stopped = wait.is_some_and(super::tasks::stop_world);
        let tasks = super::tasks::snapshot(here);
        if stopped {
            super::tasks::resume_world();
        }
        (stopped, tasks)
    });
    let (samples, dropped) = super::heap::alive();
    let mut frames = Frames::default();
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let mut out = format!(
        "{{\"dump\":1,\"reason\":{},\"message\":{},\"info\":{},\"time_ms\":{time},\"stopped\":{stopped},\"counters\":{},\"tasks\":[",
        quoted(reason),
        quoted(message),
        info(),
        counter_list()
    );
    for (index, task) in tasks.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let locks: Vec<String> = task.locks.iter().map(|l| quoted(l)).collect();
        let _ = write!(
            out,
            "{comma}{{\"id\":{},\"main\":{},\"state\":\"{}\",\"cancelled\":{},\"stack\":{},\"spawned\":{},\"locks\":[{}]}}",
            task.id,
            task.main,
            task.state,
            task.cancelled,
            frames.list(&task.stack),
            frames.list(&task.spawned),
            locks.join(",")
        );
    }
    let _ = write!(out, "],\"heap\":{{\"rate\":{},\"dropped\":{dropped},\"samples\":[", super::heap::RATE);
    for (index, (size, stack)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = write!(out, "{comma}{{\"size\":{size},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("]}");
    // A debug build knows every object, and the dump who holds each.
    if let Some(objects) = super::heap::objects() {
        out.push_str(",\"objects\":{\"list\":[");
        for (index, (address, size, site, text)) in objects.list.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let text = text.as_ref().map_or(String::new(), |t| format!(",\"text\":{}", quoted(t)));
            let _ = write!(
                out,
                "{comma}{{\"address\":\"{address:#x}\",\"size\":{size},\"site\":{}{text}}}",
                frames.list(&[*site])
            );
        }
        let edges: Vec<String> = objects.edges.iter().map(|(from, to)| format!("[{from},{to}]")).collect();
        let roots: Vec<String> = objects.roots.iter().map(|(task, to)| format!("[{task},{to}]")).collect();
        let _ = write!(out, "],\"edges\":[{}],\"roots\":[{}]}}", edges.join(","), roots.join(","));
    }
    let _ = write!(out, ",\"frames\":{{{}}}}}", frames.json);
    out
}

/// With `TONDO_DUMP=<dir>`, a program that panics, faults or deadlocks
/// leaves a dump there before it ends, named after it and its process.
pub(crate) fn crash_dump(reason: &str, message: &str, here: Vec<usize>, stop: bool) {
    let Some(dir) = std::env::var_os("TONDO_DUMP") else { return };
    let program = std::env::current_exe().ok().and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()));
    let name = format!("{}-{}.dump.json", program.unwrap_or_else(|| "tondo".into()), std::process::id());
    let path = PathBuf::from(dir).join(name);
    // This thread stands still too while the others stop.
    let paused = super::tasks::pause_here();
    let text = dump(reason, message, Some(here), stop.then_some(std::time::Duration::from_millis(100)));
    if paused {
        super::tasks::enter_tondo();
    }
    let _ = match std::fs::write(&path, text) {
        Ok(()) => writeln!(std::io::stderr(), "tondo runtime: a dump of the program is in {}", path.display()),
        Err(e) => writeln!(std::io::stderr(), "tondo runtime: cannot write a dump to {}: {e}", path.display()),
    };
}

/// The frames a document refers to by address, each described once.
#[derive(Default)]
struct Frames {
    described: std::collections::HashSet<usize>,
    json: String,
}

impl Frames {
    /// The addresses as a JSON array, describing the new ones.
    fn list(&mut self, addresses: &[usize]) -> String {
        let mut out = String::from("[");
        for (index, &address) in addresses.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let _ = write!(out, "{comma}\"{address:x}\"");
            if self.described.insert(address) {
                let comma = if self.json.is_empty() { "" } else { "," };
                let _ = write!(self.json, "{comma}\"{address:x}\":{}", frame(address));
            }
        }
        out.push(']');
        out
    }
}

pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
