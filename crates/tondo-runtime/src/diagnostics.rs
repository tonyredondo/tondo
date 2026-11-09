//! Diagnostics of a running program (toolchain 7.5): the runtime's counters
//! and the program's own (`std.diagnostics`), which `tondo ps` and `tondo
//! counters` read through a Unix socket in a directory only the user can
//! enter, `$TMPDIR/tondo-<uid>/<pid>`. A thread of its own serves it, so a
//! program whose workers are all busy or stuck still answers.

use super::Str;
use super::config::config;
use std::cell::Cell;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

mod dumps;
mod serving;
mod tracing;
pub(crate) use dumps::*;
pub(crate) use serving::*;
use tracing::*;

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

/// Where the program comes from, as the compiler wrote it.
static ROOTS: AtomicPtr<super::Str> = AtomicPtr::new(std::ptr::null_mut());

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_source_roots(text: *mut super::Str) {
    ROOTS.store(text, Ordering::Relaxed);
}

/// Where the program comes from, for the tools.
pub(crate) fn roots() -> String {
    match ROOTS.load(Ordering::Relaxed) {
        text if text.is_null() => String::new(),
        text => String::from_utf8_lossy(unsafe { super::str_bytes(text) }).into_owned(),
    }
}

/// A request the program serves, as a dump shows it: its method, path,
/// when it came, and the task that answers it, when one does.
struct Flight {
    id: u64,
    method: String,
    path: String,
    since: Instant,
    task: u64,
    steps: Vec<Step>,
}

/// What a task of a request did, so many milliseconds after it came, and
/// from where.
struct Step {
    ms: u128,
    task: u64,
    did: Did,
    at: Vec<usize>,
}

/// What a request's timeline shows.
pub(crate) enum Did {
    /// It takes the request.
    Took,
    Spawned(u64),
    /// It waited so many milliseconds, for what is said.
    Waited(&'static str, u128),
    Ended,
    Panicked(String),
}

/// How many steps a request keeps, and how many answered requests stay.
const STEPS: usize = 64;
const SERVED: usize = 32;

/// The requests answered last, the oldest first, and how long each took.
static ANSWERED: Mutex<std::collections::VecDeque<(Flight, u128)>> = Mutex::new(std::collections::VecDeque::new());

/// A request in flight in words, by its number in a task. A thread that
/// stopped may hold the list, and then it does not say.
pub(crate) fn flight_text(flight: u64) -> Option<String> {
    let flights = FLIGHTS.try_lock().ok()?;
    flights.iter().find(|found| found.id + 1 == flight).map(Flight::text)
}

/// Adds to the timeline of a request in flight, by its number in a task.
pub(crate) fn step(flight: u64, task: u64, did: Did, at: Vec<usize>) {
    let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(flight) = flights.iter_mut().find(|found| found.id + 1 == flight)
        && flight.steps.len() < STEPS
    {
        flight.steps.push(Step { ms: flight.since.elapsed().as_millis(), task, did, at });
    }
}

impl Flight {
    fn text(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// The requests in flight, in a program that dumps or is watched.
static FLIGHTS: Mutex<Vec<Flight>> = Mutex::new(Vec::new());

/// A request in `FLIGHTS`, until it ends.
pub(crate) struct Flown(u64);

impl Flown {
    /// In a program that shows them, a request that came.
    pub(crate) fn new(method: &str, path: &str) -> Option<Flown> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        enabled().then(|| {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let (method, path) = (method.to_string(), path.to_string());
            let flight = Flight { id, method, path, since: Instant::now(), task: 0, steps: Vec::new() };
            FLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).push(flight);
            Flown(id)
        })
    }

    /// The task that takes it, which the events show it came to: hyper
    /// reads it where the workers poll a connection, at no task.
    pub(crate) fn taken(&self) {
        if !super::events::on() {
            return;
        }
        let flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        let text = flights.iter().find(|flight| flight.id == self.0).map(Flight::text);
        drop(flights);
        if let Some(text) = text {
            super::events::add(super::events::What::Request(text));
        }
    }

    /// `task` answers it.
    pub(crate) fn answered_by(&self, task: u64) {
        let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flight) = flights.iter_mut().find(|flight| flight.id == self.0) {
            flight.task = task;
        }
        drop(flights);
        super::tasks::join_flight(self.0 + 1);
        step(self.0 + 1, task, Did::Took, Vec::new());
    }
}

impl Drop for Flown {
    fn drop(&mut self) {
        let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = flights.iter().position(|flight| flight.id == self.0) {
            let flight = flights.swap_remove(index);
            drop(flights);
            let took = flight.since.elapsed().as_millis();
            if super::events::on() {
                super::events::add(super::events::What::Answered(flight.text(), took as u64));
            }
            let mut answered = ANSWERED.lock().unwrap_or_else(|e| e.into_inner());
            if answered.len() == SERVED {
                answered.pop_front();
            }
            answered.push_back((flight, took));
        }
    }
}

/// The requests in flight: the number of each, what it is and how long it
/// has taken.
fn flights_now() -> Vec<(u64, String, std::time::Duration)> {
    let flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    flights.iter().map(|flight| (flight.id, flight.text(), flight.since.elapsed())).collect()
}

/// A request as JSON, with its timeline.
fn flight_json(flight: &Flight, ms: u128, frames: &mut Frames) -> String {
    let steps: Vec<String> = flight
        .steps
        .iter()
        .map(|step| {
            let detail = match &step.did {
                Did::Took => "\"what\":\"took\"".to_string(),
                Did::Spawned(other) => format!("\"what\":\"spawned\",\"other\":{other}"),
                Did::Waited(on, took) => format!("\"what\":\"waited\",\"on\":{},\"took_ms\":{took}", quoted(on)),
                Did::Ended => "\"what\":\"ended\"".to_string(),
                Did::Panicked(text) => format!("\"what\":\"panicked\",\"text\":{}", quoted(text)),
            };
            format!("{{\"ms\":{},\"task\":{},{detail},\"at\":{}}}", step.ms, step.task, frames.list(&step.at))
        })
        .collect();
    format!(
        "{{\"id\":{},\"method\":{},\"path\":{},\"ms\":{ms},\"task\":{},\"steps\":[{}]}}",
        flight.id,
        quoted(&flight.method),
        quoted(&flight.path),
        flight.task,
        steps.join(",")
    )
}

/// The requests in flight and those answered last as JSON: a thread that
/// stopped may hold a list, and then a dump goes without it.
fn flights_json(frames: &mut Frames) -> String {
    let Ok(flights) = FLIGHTS.try_lock() else { return String::new() };
    let Ok(answered) = ANSWERED.try_lock() else { return String::new() };
    let now: Vec<String> = flights.iter().map(|f| flight_json(f, f.since.elapsed().as_millis(), frames)).collect();
    let served: Vec<String> = answered.iter().map(|(f, took)| flight_json(f, *took, frames)).collect();
    format!(",\"requests\":[{}],\"served\":[{}]", now.join(","), served.join(","))
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
pub(crate) static GRPC_CLIENT_CALLS: Count = Count::new();
pub(crate) static GRPC_CLIENT_ACTIVE: Count = Count::new();
pub(crate) static GRPC_CLIENT_FAILED: Count = Count::new();
pub(crate) static GRPC_SERVER_CALLS: Count = Count::new();
pub(crate) static GRPC_SERVER_ACTIVE: Count = Count::new();
pub(crate) static GRPC_SERVER_FAILED: Count = Count::new();
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

/// `diagnostics.metrics()`: each counter `tondo counters` shows, as
/// `name\tunit\tgauge\tvalue`, `gauge` 1 for a level.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_metrics() -> *mut crate::ArrayHeader {
    let lines: Vec<String> =
        sample().iter().map(|s| format!("{}\t{}\t{}\t{}", s.name, s.unit, s.gauge as u8, s.value)).collect();
    crate::string_array(&lines)
}

/// Counts for the tools from the start: the `main` of a program that reads
/// its metrics calls it before the program starts.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_metrics_watch() {
    super::heap::prepare();
    hook(WATCHED);
}

/// `diagnostics.stack()`: the frames of compiled code of the running task,
/// innermost first, as `function\tfile\tline\tcolumn`, without those of
/// `std.diagnostics` itself.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_diagnostics_stack() -> *mut crate::ArrayHeader {
    let frames: Vec<String> = super::trace::here_compiled()
        .into_iter()
        .skip_while(|(_, file, ..)| *file == "<std>/diagnostics.to")
        .map(|(function, file, line, column)| format!("{function}\t{file}\t{line}\t{column}"))
        .collect();
    crate::string_array(&frames)
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
    add("grpc.client.calls", false, "", GRPC_CLIENT_CALLS.get());
    add("grpc.client.active", true, "", GRPC_CLIENT_ACTIVE.get());
    add("grpc.client.failed", false, "", GRPC_CLIENT_FAILED.get());
    add("grpc.server.calls", false, "", GRPC_SERVER_CALLS.get());
    add("grpc.server.active", true, "", GRPC_SERVER_ACTIVE.get());
    add("grpc.server.failed", false, "", GRPC_SERVER_FAILED.get());
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
    let ours = socket();
    let ours = ours.as_ref().map(|path| path.as_os_str().as_encoded_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_limit_of_a_cgroup_and_its_parents_counts() {
        let root = std::env::temp_dir().join(format!("tondo-cgroup-{}", std::process::id()));
        let inside = root.join("kubepods/pod1");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(root.join("memory.max"), "max\n").unwrap();
        std::fs::write(root.join("kubepods/memory.max"), "1000\n").unwrap();
        std::fs::write(inside.join("memory.max"), "5000\n").unwrap();
        assert_eq!(cgroup_limit("0::/kubepods/pod1\n", &root), Some(1000));
        // Without a limit anywhere, and in version 1.
        assert_eq!(cgroup_limit("0::/\n", &root), None);
        std::fs::create_dir_all(root.join("memory/app")).unwrap();
        std::fs::write(root.join("memory/app/memory.limit_in_bytes"), "2000\n").unwrap();
        assert_eq!(cgroup_limit("4:cpu:/x\n5:memory:/app\n", &root), Some(2000));
        let _ = std::fs::remove_dir_all(&root);
    }
}
