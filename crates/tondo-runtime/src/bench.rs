//! `testing.bench(name, body)` (testing 10.4). The compiler turns the call
//! into a loop that asks `tondo_rt_bench_next` how many times to run the
//! body next, until it says none. In a plain test run that is once. Under
//! `tondo test --bench`, which sets `TONDO_TEST_BENCH` to the seconds each
//! benchmark measures for, it warms up, finds how many runs make a sample,
//! takes the samples, then runs as many as five samples counting the
//! processor's instructions, cycles, mispredicted branches and first-cache
//! misses where the system lets it, apart from the samples, as counting
//! slows every switch of a thread it counts, then one more batch counting
//! allocations, and reports the samples to `tondo test` in a `bench`
//! frame.

use crate::Str;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The samples a benchmark takes, and the fewest when one run is slow.
const SAMPLES: usize = 50;
const FEWEST: usize = 5;
/// The samples' worth of runs that count the processor's events.
const COUNTED: u64 = 5;

enum Phase {
    /// A plain test run: the body once.
    Once,
    /// Runs batches of twice as many until the warm-up time is spent.
    Warming(Duration),
    /// Takes the samples, each a batch of the same size.
    Sampling(usize),
    /// Runs `COUNTED` batches at once, counting the processor's events.
    Events,
    /// One run that counts and is not counted, so that a loop's region
    /// that kept its chunk from the batches before counts from the next
    /// turn on; the size of the batch after it.
    Priming(u64),
    /// One more batch, counting its allocations: on the heap and in
    /// regions, and their bytes, before it.
    Counting([u64; 6]),
    Done,
}

struct Bench {
    name: String,
    phase: Phase,
    /// The runs of the batch that runs; none before the first.
    batch: u64,
    started: Instant,
    /// Nanoseconds a run takes, by each sample.
    samples: Vec<f64>,
    allocations: f64,
    bytes: f64,
    region_allocations: f64,
    region_bytes: f64,
    /// The regions a run set up and those that took memory, in a build that
    /// counts them (`tondo_rt_regions_set_up`).
    regions_set_up: f64,
    regions_used: f64,
    /// What counts the instructions and cycles of the runs that count
    /// them; what a run took of them, once they are done.
    counters: Option<super::counters::Counters>,
    counted: Option<super::counters::Counted>,
}

struct State {
    bench: Option<Bench>,
    /// The benchmarks of each test that ran here, by node and name.
    named: Vec<(u32, String)>,
}

static STATE: Mutex<State> = Mutex::new(State { bench: None, named: Vec::new() });

/// The allocations of a batch that counts them, and their bytes; on the
/// heap, and in regions.
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static REGION_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static REGION_BYTES: AtomicU64 = AtomicU64::new(0);
/// The regions functions set up while a batch counts, in a build that
/// counts them, and those that took memory.
static REGIONS_SET_UP: AtomicU64 = AtomicU64::new(0);
static REGIONS_USED: AtomicU64 = AtomicU64::new(0);

/// A function of a build that counts its regions starts, setting `count`
/// of them up: counted while a batch counts.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_regions_set_up(count: i64) {
    if crate::diagnostics::hooks() & crate::diagnostics::MEASURED != 0 {
        REGIONS_SET_UP.fetch_add(count as u64, Ordering::Relaxed);
    }
}

/// A region that held memory ended while a batch counts.
pub(crate) fn region_used() {
    REGIONS_USED.fetch_add(1, Ordering::Relaxed);
}

/// After a value a region made while a batch counts them.
pub(crate) fn region_allocated(size: usize) {
    REGION_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    REGION_BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

/// The values regions made while batches counted them, and their bytes.
#[cfg(test)]
pub(crate) fn region_counts() -> (u64, u64) {
    (REGION_ALLOCATIONS.load(Ordering::Relaxed), REGION_BYTES.load(Ordering::Relaxed))
}

fn counts() -> [u64; 6] {
    [&ALLOCATIONS, &BYTES, &REGION_ALLOCATIONS, &REGION_BYTES, &REGIONS_SET_UP, &REGIONS_USED]
        .map(|count| count.load(Ordering::Relaxed))
}

/// After an allocation while a batch counts them.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn allocated(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

/// How many tasks count their allocations now (`testing.allocations`).
static COUNTING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `testing.allocations(body)` or `testing.heapAllocations(body)` starts:
/// the running task's allocations from now on are counted; what it counted
/// before, of the heap alone when `heap`, to take away at the end.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_allocations_start(heap: u8) -> i64 {
    if COUNTING.fetch_add(1, Ordering::SeqCst) == 0 {
        crate::diagnostics::hook(crate::diagnostics::COUNTED);
    }
    made(heap)
}

/// What the running task allocated while it was counted: on the heap, and
/// in regions unless `heap`.
fn made(heap: u8) -> i64 {
    let (on_heap, in_regions) = crate::tasks::allocations_made();
    (on_heap + if heap != 0 { 0 } else { in_regions }) as i64
}

/// `testing.allocations(body)` ends: how many allocations the task made
/// since `before`, of the heap alone when `heap`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_allocations_since(before: i64, heap: u8) -> i64 {
    let made = made(heap) - before;
    if COUNTING.fetch_sub(1, Ordering::SeqCst) == 1 {
        crate::diagnostics::unhook(crate::diagnostics::COUNTED);
    }
    made
}

/// Whether benchmarks measure: `tondo test --bench`.
pub(crate) fn measures() -> bool {
    measuring().is_some()
}

/// The time each benchmark measures for, under `tondo test --bench`.
fn measuring() -> Option<Duration> {
    static TIME: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *TIME.get_or_init(|| {
        let seconds: f64 = super::config::config().test_bench.as_deref()?.parse().ok()?;
        (seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
    })
}

/// Starts the benchmark `name` of the test that runs: a name that is empty
/// or that the test already measured is the panic `P2009`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_bench_start(name: *const Str, location: *const Str) {
    let name = String::from_utf8_lossy(unsafe { crate::str_bytes(name) }).into_owned();
    let node = crate::testing::current();
    let fail = |message: String| {
        let message = crate::str_new(message.as_bytes());
        unsafe { crate::tondo_rt_panic(2009, message, location) }
    };
    if name.is_empty() || name.chars().any(char::is_control) {
        fail(format!("{name:?} cannot name a benchmark: it is empty or has a control character"));
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.named.iter().any(|(n, named)| *n == node && *named == name) {
        drop(state);
        fail(format!("this test already has a benchmark named {name:?}: give each its own name"));
    }
    state.named.push((node, name.clone()));
    let phase = match measuring() {
        Some(time) => Phase::Warming(warm_up(time)),
        None => Phase::Once,
    };
    let started = Instant::now();
    state.bench = Some(Bench {
        name,
        phase,
        batch: 0,
        started,
        samples: Vec::new(),
        allocations: 0.0,
        bytes: 0.0,
        region_allocations: 0.0,
        regions_set_up: 0.0,
        regions_used: 0.0,
        region_bytes: 0.0,
        counters: None,
        counted: None,
    });
}

/// How long a benchmark warms up for: a tenth of its time, within reason.
fn warm_up(time: Duration) -> Duration {
    (time / 10).clamp(Duration::from_millis(50), Duration::from_secs(1))
}

/// How many times to run the body next, after the batch before it ended;
/// `0` when the benchmark is done.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_bench_next() -> i64 {
    let now = Instant::now();
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bench) = state.bench.as_mut() else { return 0 };
    if bench.batch == 0 {
        bench.batch = 1;
        bench.started = Instant::now();
        return 1;
    }
    let elapsed = now - bench.started;
    let per_run = elapsed.as_nanos() as f64 / bench.batch as f64;
    let time = measuring().unwrap_or_default();
    bench.phase = match bench.phase {
        Phase::Once => Phase::Done,
        Phase::Warming(left) if left > elapsed => {
            bench.batch = (bench.batch * 2).min(1 << 40);
            Phase::Warming(left - elapsed)
        }
        Phase::Warming(_) => {
            // Batches that take the time between them; one run each, and
            // fewer samples, when a run is slow.
            let each = time.as_nanos() as f64 / SAMPLES as f64;
            bench.batch = (each / per_run.max(1.0)).floor().clamp(1.0, (1u64 << 40) as f64) as u64;
            let samples = (time.as_nanos() as f64 / per_run.max(1.0)) as usize;
            // A profile of the benchmark covers what it measures.
            crate::profiling::resume();
            Phase::Sampling(samples.clamp(FEWEST, SAMPLES))
        }
        Phase::Sampling(count) if bench.samples.len() + 1 < count => {
            bench.samples.push(per_run);
            Phase::Sampling(count)
        }
        Phase::Sampling(_) => {
            bench.samples.push(per_run);
            crate::profiling::pause();
            bench.counters = super::counters::Counters::start();
            match bench.counters.is_some() {
                true => {
                    bench.batch *= COUNTED;
                    Phase::Events
                }
                false => count_allocations(bench),
            }
        }
        Phase::Events => {
            let runs = bench.batch as f64;
            let counted = bench.counters.take().map(|counters| counters.read());
            bench.counted = counted.map(|counted| counted.map(|count| count.map(|count| count / runs)));
            bench.batch /= COUNTED;
            count_allocations(bench)
        }
        Phase::Priming(batch) => {
            bench.batch = batch;
            Phase::Counting(counts())
        }
        Phase::Counting(before) => {
            crate::diagnostics::unhook(crate::diagnostics::MEASURED);
            let runs = bench.batch as f64;
            let [allocations, bytes, region_allocations, region_bytes, set_up, used] =
                std::array::from_fn(|at| (counts()[at] - before[at]) as f64 / runs);
            (bench.allocations, bench.bytes) = (allocations, bytes);
            (bench.region_allocations, bench.region_bytes) = (region_allocations, region_bytes);
            (bench.regions_set_up, bench.regions_used) = (set_up, used);
            Phase::Done
        }
        Phase::Done => Phase::Done,
    };
    if matches!(bench.phase, Phase::Done) {
        return 0;
    }
    let batch = bench.batch as i64;
    bench.started = Instant::now();
    batch
}

/// The batch that counts allocations starts, after a run that primes it.
fn count_allocations(bench: &mut Bench) -> Phase {
    crate::diagnostics::hook(crate::diagnostics::MEASURED);
    let batch = std::mem::replace(&mut bench.batch, 1);
    Phase::Priming(batch)
}

/// Ends the benchmark that runs: when it measured and its body did not
/// fail, tells `tondo test` its samples, as `name`, the runs of each
/// sample, the nanoseconds of a run by sample, the allocations of a run
/// and their bytes, and the instructions, cycles, mispredicted branches
/// and first-cache misses of data and of instructions of a run, each empty
/// when not counted, then the values a run made in regions and their
/// bytes, and the regions it set up, in a build that counts them, and
/// those that took memory, separated by NUL.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_bench_end(completed: u8) {
    let bench = STATE.lock().unwrap_or_else(|e| e.into_inner()).bench.take();
    crate::diagnostics::unhook(crate::diagnostics::MEASURED);
    let Some(bench) = bench.filter(|b| completed != 0 && !b.samples.is_empty()) else { return };
    crate::profiling::write(crate::testing::current(), Some(&bench.name));
    let samples: Vec<String> = bench.samples.iter().map(|s| format!("{s:.3}")).collect();
    let counted: Vec<String> = bench
        .counted
        .unwrap_or_default()
        .iter()
        .map(|count| count.map_or(String::new(), |count| format!("{count:.3}")))
        .collect();
    let (name, batch, allocations, bytes) = (&bench.name, bench.batch, bench.allocations, bench.bytes);
    let (region_allocations, region_bytes) = (bench.region_allocations, bench.region_bytes);
    let (set_up, used) = (bench.regions_set_up, bench.regions_used);
    let payload = format!(
        "{name}\0{batch}\0{}\0{allocations:.3}\0{bytes:.3}\0{}\0{region_allocations:.3}\0{region_bytes:.3}\0{set_up:.3}\0{used:.3}",
        samples.join(" "),
        counted.join("\0")
    );
    crate::testing::report("bench", payload.as_bytes());
}

/// `testing.keep(value)`: a value the optimizer cannot see is unused.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_keep(value: i64) {
    std::hint::black_box(value);
}
