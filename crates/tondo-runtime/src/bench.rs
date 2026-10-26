//! `testing.bench(name, body)` (testing 10.4). The compiler turns the call
//! into a loop that asks `tondo_rt_bench_next` how many times to run the
//! body next, until it says none. In a plain test run that is once. Under
//! `tondo test --bench`, which sets `TONDO_TEST_BENCH` to the seconds each
//! benchmark measures for, it warms up, finds how many runs make a sample,
//! takes the samples, then runs one more batch counting allocations, and
//! reports the samples to `tondo test` in a `bench` frame.

use crate::Str;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The samples a benchmark takes, and the fewest when one run is slow.
const SAMPLES: usize = 50;
const FEWEST: usize = 5;

enum Phase {
    /// A plain test run: the body once.
    Once,
    /// Runs batches of twice as many until the warm-up time is spent.
    Warming(Duration),
    /// Takes the samples, each a batch of the same size.
    Sampling(usize),
    /// One more batch, counting its allocations.
    Counting(u64, u64),
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
}

struct State {
    bench: Option<Bench>,
    /// The benchmarks of each test that ran here, by node and name.
    named: Vec<(u32, String)>,
}

static STATE: Mutex<State> = Mutex::new(State { bench: None, named: Vec::new() });

/// The allocations of a batch that counts them, and their bytes.
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// After an allocation while a batch counts them.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn allocated(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

/// Whether benchmarks measure: `tondo test --bench`.
pub(crate) fn measures() -> bool {
    measuring().is_some()
}

/// The time each benchmark measures for, under `tondo test --bench`.
fn measuring() -> Option<Duration> {
    static TIME: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *TIME.get_or_init(|| {
        let seconds: f64 = std::env::var("TONDO_TEST_BENCH").ok()?.parse().ok()?;
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
    state.bench = Some(Bench { name, phase, batch: 0, started, samples: Vec::new(), allocations: 0.0, bytes: 0.0 });
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
        Phase::Sampling(count) => {
            bench.samples.push(per_run);
            match bench.samples.len() < count {
                true => Phase::Sampling(count),
                false => {
                    crate::profiling::pause();
                    let before = (ALLOCATIONS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed));
                    crate::diagnostics::hook(crate::diagnostics::MEASURED);
                    Phase::Counting(before.0, before.1)
                }
            }
        }
        Phase::Counting(allocations, bytes) => {
            crate::diagnostics::unhook(crate::diagnostics::MEASURED);
            let runs = bench.batch as f64;
            bench.allocations = (ALLOCATIONS.load(Ordering::Relaxed) - allocations) as f64 / runs;
            bench.bytes = (BYTES.load(Ordering::Relaxed) - bytes) as f64 / runs;
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

/// Ends the benchmark that runs: when it measured and its body did not
/// fail, tells `tondo test` its samples, as `name`, the runs of each
/// sample, the nanoseconds of a run by sample, and the allocations of a
/// run and their bytes, separated by NUL.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_bench_end(completed: u8) {
    let bench = STATE.lock().unwrap_or_else(|e| e.into_inner()).bench.take();
    crate::diagnostics::unhook(crate::diagnostics::MEASURED);
    let Some(bench) = bench.filter(|b| completed != 0 && !b.samples.is_empty()) else { return };
    crate::profiling::write(crate::testing::current(), Some(&bench.name));
    let samples: Vec<String> = bench.samples.iter().map(|s| format!("{s:.3}")).collect();
    let payload =
        format!("{}\0{}\0{}\0{:.3}\0{:.3}", bench.name, bench.batch, samples.join(" "), bench.allocations, bench.bytes);
    crate::testing::report("bench", payload.as_bytes());
}

/// `testing.keep(value)`: a value the optimizer cannot see is unused.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_keep(value: i64) {
    std::hint::black_box(value);
}
