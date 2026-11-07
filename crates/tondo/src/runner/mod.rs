//! `tondo test` (testing 10): selects the tests of a test build, runs them
//! in worker processes, retries or repeats them, and reports.

/// Prints a line to stdout; a reader that went away, as `| head`, is not
/// an error.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

mod bench;
mod coverage;
mod document;
mod fuzz;
mod human;
mod interrupt;
mod json;
pub(crate) use json::base64;
mod junit;
mod orders;
mod owners;
mod select;
mod store;
mod worker;

pub use coverage::save_counted;
pub use owners::Codeowners;
pub use select::{Glob, Order, Selector, affected};

/// Whether `tondo test` was interrupted, which ends `--watch` too.
pub fn interrupted() -> bool {
    interrupt::requested()
}

/// Handles interruptions from now on, before tests run, as `--watch`
/// waits for changes.
pub fn listen() {
    interrupt::listen();
}

use crate::hir::TestNodeInfo;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Options {
    pub selector: Selector,
    /// `--shard index/count`.
    pub shard: Option<(u64, u64)>,
    pub order: Order,
    pub list: bool,
    /// `--models`: only the tests that check a model.
    pub models: bool,
    pub jobs: usize,
    pub timeout: Duration,
    /// Additional rounds for failures (8.7).
    pub retry: u32,
    /// Times the whole selection runs (8.8).
    pub repeat: u32,
    /// `--schedules count`: as many runs of the selection, each with its
    /// tasks in another order (8.10), from a seed, and whether `tondo test`
    /// chose it.
    pub schedules: Option<(u32, u64, bool)>,
    /// `--schedules all`: every order of the tasks of each test, up to as
    /// many, and `--schedules at:0.2.1`, one given by its choices (8.10).
    pub orders: Option<Orders>,
    /// `--record dir`: each test runs alone, recorded (toolchain 7.6), and
    /// the recordings of those that fail stay in the directory.
    pub record: Option<PathBuf>,
    /// `--lock-order`: locks taken in orders that can deadlock fail the
    /// test that took them.
    pub lock_order: bool,
    pub codeowners: Codeowners,
    pub show_output: bool,
    pub allow_empty: bool,
    pub deny_skips: bool,
    pub allow_flaky: bool,
    /// What stdout gets: lines for people, or the JSON report.
    pub json: bool,
    /// With `json`, a line of JSON for each start and outcome as tests run,
    /// then the report (15.9).
    pub events: bool,
    /// Each `--report`: what it is, and where it goes.
    pub reports: Vec<(Report, PathBuf)>,
    /// `--coverage`: counts what runs of the project's code and says how
    /// much ran (10.3), as a report of coverage also does.
    pub coverage: bool,
    /// What runs the tests' executable when it is for another machine:
    /// qemu's user emulation (toolchain 7.8).
    pub launcher: Option<PathBuf>,
    /// Where attachments go; `target/test-artifacts` of the project if not.
    pub artifacts: Option<PathBuf>,
    /// Records missing and changed snapshots instead of failing on them.
    pub update_snapshots: bool,
    /// `--bench`: runs the benchmarks alone, each measuring for this long
    /// (10.4).
    pub bench: Option<Duration>,
    /// `--save-baseline`: where the benchmarks' measures are written.
    pub save_baseline: Option<PathBuf>,
    /// What the benchmarks are compared with.
    pub baseline: Option<Baseline>,
    /// `--max-regression`, as a fraction: a benchmark slower or allocating
    /// more than that fails the run.
    pub max_regression: Option<f64>,
    /// `--profile`: a profile of each test, or of each benchmark (10.5).
    pub profile: Option<crate::profile::Kind>,
    /// `--fuzz`: searches for inputs that fail the calls of
    /// `testing.fuzz` (10.6).
    pub fuzz: Option<Fuzz>,
}

/// What `--fuzz` searches with: until when, how long an input and how
/// much memory a worker may take, and where the corpus is kept.
#[derive(Clone)]
pub struct Fuzz {
    pub time: Option<Duration>,
    pub timeout: Duration,
    pub memory: u64,
    pub corpus: Option<PathBuf>,
}

/// What `--bench` compares with: a baseline saved before, or the tests of
/// another revision, run on this machine one after another with these.
pub enum Baseline {
    Saved(PathBuf),
    Revision(Revision),
}

/// The tests of a revision, built: its name and commit, where its project
/// is, its executable and its tree.
pub struct Revision {
    pub name: String,
    pub commit: String,
    pub dir: PathBuf,
    pub executable: PathBuf,
    pub tree: Tree,
}

/// What a `--report` writes (10, 15.5, 15.10).
#[derive(Clone, Copy, PartialEq)]
pub enum Report {
    Json,
    Junit,
    Coverage,
    Lcov,
    /// For people: the tests, benchmarks and coverage of a run (15.13).
    Markdown,
    Html,
}

impl Options {
    /// Whether the build counts what runs, for coverage.
    pub fn counting(&self) -> bool {
        self.coverage || self.reports.iter().any(|(r, _)| matches!(r, Report::Coverage | Report::Lcov))
    }
}

/// The tree of the test build, with where each node is declared.
pub struct Tree {
    pub nodes: Vec<TestNodeInfo>,
    pub locations: Vec<String>,
    /// The state machines of the program no test checks with
    /// `model.machine`: each name and where it is.
    pub machines: Vec<(String, String)>,
}

impl Tree {
    /// Writes the tree beside the executable it describes, one node a line:
    /// its parent, whether it is a suite, a benchmark and a fuzz test, its
    /// identity and its location.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let lines: Vec<String> = self
            .nodes
            .iter()
            .zip(&self.locations)
            .map(|(n, location)| {
                let flags = format!("{}\t{}\t{}\t{}", n.bench as u8, n.fuzz as u8, n.model as u8, n.tasks as u8);
                format!("{}\t{}\t{flags}\t{}\t{location}", n.parent, n.suite as u8, n.id)
            })
            .collect();
        let machines: Vec<String> = self.machines.iter().map(|(name, at)| format!("{name}\t{at}")).collect();
        let written = path.with_extension("machines");
        std::fs::write(&written, machines.join("\n"))
            .map_err(|e| format!("cannot write {}: {e}", written.display()))?;
        let partial = path.with_extension(format!("partial-{}", std::process::id()));
        std::fs::write(&partial, lines.join("\n")).map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
        std::fs::rename(&partial, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// The tree `save` wrote, if it is there.
    pub fn load(path: &Path) -> Option<Tree> {
        let text = std::fs::read_to_string(path).ok()?;
        let machines = std::fs::read_to_string(path.with_extension("machines")).unwrap_or_default();
        let machines = machines.lines().filter_map(|line| line.split_once('\t')).map(|(n, at)| (n.into(), at.into()));
        let mut tree = Tree { nodes: Vec::new(), locations: Vec::new(), machines: machines.collect() };
        for line in text.lines() {
            let mut parts = line.splitn(8, '\t');
            let parent = parts.next()?.parse().ok()?;
            let suite = parts.next()? == "1";
            let bench = parts.next()? == "1";
            let fuzz = parts.next()? == "1";
            let model = parts.next()? == "1";
            let tasks = parts.next()? == "1";
            let id = parts.next()?.to_string();
            let span = crate::source::Span::new(0, 0);
            tree.nodes.push(TestNodeInfo { id, parent, suite, bench, fuzz, model, tasks, span, whole: span });
            tree.locations.push(parts.next()?.to_string());
        }
        Some(tree)
    }

    fn within(&self, mut node: u32, ancestor: u32) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            if node == 0 {
                return false;
            }
            node = self.nodes[node as usize].parent;
        }
    }

    fn id(&self, node: u32) -> &str {
        &self.nodes[node as usize].id
    }

    fn suite(&self, node: u32) -> bool {
        self.nodes[node as usize].suite
    }

    /// The file that declares a node, from its location.
    fn file(&self, node: u32) -> &str {
        let location = &self.locations[node as usize];
        let mut parts = location.rsplitn(3, ':');
        let (_, _) = (parts.next(), parts.next());
        parts.next().unwrap_or(location)
    }
}

/// The state of one attempt (testing 9.1).
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Passed,
    /// The reason, then ` --> ` and where `testing.skip` was called.
    Skipped(String),
    /// A recoverable error that left the test or the setup.
    Error {
        message: String,
        error_type: String,
        location: Option<String>,
    },
    /// The panic's report, as the runtime prints it, with any suppressed
    /// while unwinding after it; and its data, as JSON, when it has any.
    Panic(String, Option<String>),
    /// Past `--timeout`, with where its tasks were when it was ended.
    Timeout(String),
    Crashed(String),
    /// Under `--fuzz`, an input that ran past `--fuzz-timeout` or held more
    /// than `--fuzz-memory` (testing 10.6): which, and what it did.
    Limit {
        timeout: bool,
        message: String,
    },
    /// Not run: a suite around it failed (`blocked-setup`) or skipped
    /// (`blocked-skip`).
    Blocked(u32, bool),
}

impl Status {
    /// One of the failures of an executed attempt.
    fn failed(&self) -> bool {
        matches!(
            self,
            Status::Error { .. } | Status::Panic(..) | Status::Timeout(_) | Status::Crashed(_) | Status::Limit { .. }
        )
    }
}

/// What going through the orders of a test came to (`--schedules all`).
#[derive(Clone, Debug)]
pub struct Explored {
    pub orders: usize,
    /// None was left to try.
    pub complete: bool,
    /// The choices of the order it failed in, as `at:` takes them.
    pub failed_at: Option<String>,
}

/// One run of a test, or of a suite's setup and teardown.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub node: u32,
    pub status: Status,
    /// For a suite: whether it failed in its teardown rather than its setup.
    pub teardown: bool,
    /// `0` for the first round, then each retry round (8.7).
    pub round: u32,
    /// From `1`, each run of the whole selection (8.8).
    pub iteration: u32,
    /// The suite or test a retry round runs again.
    pub unit: Option<u32>,
    pub explored: Option<Explored>,
    pub logs: Vec<String>,
    /// What looks wrong without failing it, as an update tasks can lose.
    pub warnings: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// How long its code ran.
    pub duration: Duration,
    pub artifacts: Vec<Artifact>,
    pub snapshots: Vec<Snapshot>,
    /// The virtual clocks it ran, in order (testing 7.9).
    pub virtual_time: Vec<VirtualClock>,
    /// What its benchmarks measured, in order (10.4).
    pub benchmarks: Vec<Benchmark>,
    /// What each model it checked came to, as `std.model` says it.
    pub models: Vec<serde_json::Value>,
    /// What its calls of `testing.fuzz` did, in order (10.6).
    pub fuzz: Vec<Fuzzed>,
}

/// What a call of `testing.fuzz` did: its name, how many inputs it ran,
/// the one it failed with, if any, and under `--fuzz` what its search did.
#[derive(Clone, Debug)]
pub struct Fuzzed {
    pub name: String,
    pub inputs: u64,
    pub input: Option<FuzzInput>,
    pub search: Option<Searched>,
}

/// What a search did: the inputs it tried, for how long, how many it kept
/// in the corpus, the counters some input reached and those there are,
/// and how long since it found something new.
#[derive(Clone, Copy, Debug, Default)]
pub struct Searched {
    pub inputs: u64,
    pub elapsed_ms: u64,
    pub corpus: u64,
    pub reached: u64,
    pub total: u64,
    pub new_ms: u64,
}

impl Searched {
    /// What the runtime says, as numbers separated by spaces.
    pub(super) fn read(text: &str) -> Option<Searched> {
        let numbers: Vec<u64> = text.split(' ').filter_map(|n| n.parse().ok()).collect();
        let [inputs, elapsed_ms, corpus, reached, total, new_ms] = numbers[..] else { return None };
        Some(Searched { inputs, elapsed_ms, corpus, reached, total, new_ms })
    }
}

/// An input a test failed with: its JSON, and its file in the fuzz store,
/// from the package's directory, or the index of the seed it is.
#[derive(Clone, Debug)]
pub struct FuzzInput {
    pub json: String,
    pub file: Option<String>,
    pub seed: Option<u64>,
    /// While reducing it, which of the inputs tried it is.
    pub tried: Option<usize>,
    /// Once reduced, how many bytes it had before.
    pub reduced_from: Option<usize>,
}

/// What a benchmark measured: the runs of each sample, the nanoseconds a
/// run took by sample, and the allocations of a run and their bytes.
#[derive(Clone, Debug)]
pub struct Benchmark {
    pub name: String,
    pub runs: u64,
    pub samples: Vec<f64>,
    pub allocations: f64,
    pub bytes: f64,
}

/// Where a virtual clock got, and how many times it moved by itself, by
/// `advance`, and settled.
#[derive(Clone, Debug)]
pub struct VirtualClock {
    pub elapsed_ns: u64,
    pub automatic: u64,
    pub explicit: u64,
    pub settles: u64,
}

/// What `testing.attach` attached.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub name: String,
    pub media_type: String,
    pub size: usize,
    pub sha256: String,
}

/// What `testing.snapshot` checked: `matched`, `missing`, `mismatched`,
/// `created` or `updated`, with the value it had.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub name: String,
    pub status: String,
    /// The value in the store, if it had one.
    pub expected: Option<String>,
    pub actual: String,
}

/// The round, iteration and unit the attempts of a run belong to.
#[derive(Clone, Copy)]
struct Context {
    round: u32,
    iteration: u32,
    unit: Option<u32>,
    /// Under `--fuzz`, the call of `testing.fuzz` the worker searches in,
    /// by its index in the run's targets.
    target: Option<u32>,
}

impl Attempt {
    fn new(node: u32, status: Status, context: Context) -> Attempt {
        Attempt {
            explored: None,
            node,
            status,
            teardown: false,
            round: context.round,
            iteration: context.iteration,
            unit: context.unit,
            logs: Vec::new(),
            warnings: Vec::new(),
            tags: Vec::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration: Duration::ZERO,
            artifacts: Vec::new(),
            snapshots: Vec::new(),
            virtual_time: Vec::new(),
            benchmarks: Vec::new(),
            models: Vec::new(),
            fuzz: Vec::new(),
        }
    }
}

/// A node's aggregate over its attempts (9.1): its state, `flaky-pass` when
/// it passed last after another state, and the attempt that decides it.
pub struct Aggregate {
    pub node: u32,
    pub status: Status,
    pub flaky: bool,
    pub decisive: usize,
    pub attempts: Vec<Attempt>,
}

fn aggregate(node: u32, attempts: Vec<Attempt>) -> Aggregate {
    let last = attempts.len() - 1;
    let (status, flaky, decisive) = if attempts[last].status == Status::Passed {
        (Status::Passed, attempts.iter().any(|a| a.status != Status::Passed), last)
    } else {
        // The latest executed failure, so a later block or skip hides none.
        let decisive = attempts.iter().rposition(|a| a.status.failed()).unwrap_or(last);
        (attempts[decisive].status.clone(), false, decisive)
    };
    Aggregate { node, status, flaky, decisive, attempts }
}

/// Each node's aggregate, in canonical order.
fn aggregates(tree: &Tree, attempts: &[Attempt]) -> Vec<Aggregate> {
    let mut nodes: Vec<u32> = attempts.iter().map(|a| a.node).collect();
    nodes.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
    nodes.dedup();
    nodes
        .into_iter()
        .map(|node| aggregate(node, attempts.iter().filter(|a| a.node == node).cloned().collect()))
        .collect()
}

/// Builds nothing: selects, lists or runs the tests of a built test
/// executable and reports them; whether the run failed (testing 9.6).
pub fn test(dir: &Path, executable: &Path, tree: &Tree, options: &Options) -> Result<bool, Failure> {
    let ownership = owners::resolve(dir, &options.codeowners).map_err(Failure::Input)?;
    let owners: Vec<Vec<String>> = (0..tree.nodes.len() as u32).map(|n| ownership.of(tree.file(n))).collect();
    let snapshots = store::Snapshots::read(dir, &package(dir)).map_err(Failure::Input)?;
    let mut selected = select::select(tree, &options.selector);
    // `--bench` runs the benchmarks alone, and `--fuzz` the tests that
    // fuzz.
    if options.bench.is_some() {
        selected.retain(|&test| tree.nodes[test as usize].bench);
    }
    if options.fuzz.is_some() {
        selected.retain(|&test| tree.nodes[test as usize].fuzz);
    }
    if options.models {
        selected.retain(|&test| tree.nodes[test as usize].model);
    }
    // Each `--exact` names a suite or test.
    let missing: Vec<String> = match &options.selector {
        Selector::Exact(ids) => ids
            .iter()
            .filter(|id| !(1..tree.nodes.len() as u32).any(|node| tree.id(node) == id.as_str()))
            .map(|id| format!("{id:?}"))
            .collect(),
        _ => Vec::new(),
    };
    if selected.is_empty() || !missing.is_empty() {
        let message = match &options.selector {
            Selector::All if options.bench.is_some() => "the project has no benchmarks: a test that calls \
                `testing.bench(\"name\", () { ... })` in its body is one"
                .into(),
            Selector::All if options.models => "the project checks no models: a test that calls \
                `model.check`, `model.machine`, `model.follows` or `model.watch` in its body is one"
                .into(),
            Selector::All if options.fuzz.is_some() => "the project has no fuzz tests: a test that calls \
                `testing.fuzz(\"parse\", [\"[1]\"], (text: String) { _ = json.parse(text) })` in its body is one"
                .into(),
            _ if options.fuzz.is_some() && missing.is_empty() => {
                "no fuzz test is selected: --fuzz runs the tests that call `testing.fuzz`".into()
            }
            _ if options.bench.is_some() && missing.is_empty() => {
                "no benchmark is selected: --bench runs the tests that call `testing.bench`".into()
            }
            Selector::Filter(text) => format!("no test matches --filter {text:?}"),
            Selector::Glob(_) => "no suite or test matches --glob".to_string(),
            Selector::Exact(_) => {
                format!("no suite or test is {}; `tondo test --list` lists them", missing.join(" or "))
            }
            Selector::All => {
                "the project has no tests: write `test name { ... }` in a `_test.to` file or under `tests/`".into()
            }
        };
        if !options.allow_empty {
            return Err(Failure::Empty(message));
        }
        if !options.json {
            say!("{message}");
        }
    }
    let selected = match options.shard {
        Some((index, count)) => {
            selected.into_iter().filter(|&t| select::shard_of(tree.id(t), count) == index).collect()
        }
        None => selected,
    };
    let plan = select::plan(tree, &selected, &options.order);
    if let (Order::Random(seed, _), false) = (&options.order, options.json) {
        say!("seed {seed:016x}");
    }
    if let (Some((count, seed, _)), false, None) = (options.schedules, options.json, &options.orders) {
        say!("{count} orders of the tasks from seed {seed:016x}");
    }
    let none = bench::Baselines::default();
    let mut context = json::Context {
        tree,
        options,
        owners: &owners,
        ownership: &ownership,
        plan: &plan,
        rounds: &[],
        snapshots: (&snapshots.before, &snapshots.before, None),
        compiled: true,
        baselines: &none,
        profiles: &[],
    };
    if options.list {
        if options.json {
            say!("{}", context.list().text());
        } else {
            human::list(tree, &plan, &owners);
        }
        return Ok(false);
    }
    let started = std::time::Instant::now();
    let mut printer = human::Printer::new(tree, &plan, options);
    printer.baselines = match &options.baseline {
        Some(Baseline::Saved(path)) => bench::read_baseline(path).map_err(Failure::Input)?,
        Some(Baseline::Revision(revision)) => bench::Baselines {
            measures: Default::default(),
            source: Some(json::obj(vec![
                ("kind", json::str("revision")),
                ("ref", json::str(revision.name.clone())),
                ("commit", json::str(revision.commit.clone())),
            ])),
        },
        None => bench::Baselines::default(),
    };
    let artifacts = store::Artifacts {
        root: options.artifacts.clone().unwrap_or_else(|| dir.join("target/test-artifacts")),
        error: std::sync::Mutex::new(None),
    };
    let entries = std::env::temp_dir().join(format!("tondo-snapshots-{}", std::process::id()));
    std::fs::write(&entries, snapshots.for_worker()).map_err(|e| Failure::Output(e.to_string()))?;
    // The inputs each call of `testing.fuzz` failed with before (10.6).
    let fuzzed = std::env::temp_dir().join(format!("tondo-fuzz-{}", std::process::id()));
    std::fs::write(&fuzzed, store::fuzz_inputs(dir, &package(dir))).map_err(|e| Failure::Output(e.to_string()))?;
    // With coverage, where the workers write their counts.
    let counts =
        options.counting().then(|| std::env::temp_dir().join(format!("tondo-coverage-{}", std::process::id())));
    if let Some(counts) = &counts {
        let _ = std::fs::remove_dir_all(counts);
        std::fs::create_dir_all(counts).map_err(|e| Failure::Output(e.to_string()))?;
    }
    // With profiles, where the workers write what they collected.
    let collected = options.profile.map(|kind| {
        let dir = std::env::temp_dir().join(format!("tondo-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (kind, dir)
    });
    if let Some((_, dir)) = &collected {
        std::fs::create_dir_all(dir).map_err(|e| Failure::Output(e.to_string()))?;
    }
    let stores = worker::Stores {
        artifacts: &artifacts,
        snapshots: &snapshots,
        entries: &entries,
        fuzzed: &fuzzed,
        update: options.update_snapshots,
        counts: counts.as_deref(),
        profiles: collected.as_ref().map(|(kind, dir)| (*kind, dir.as_path())),
    };
    let run = worker::Run {
        executable,
        dir,
        tree,
        options,
        stores,
        bench: options.bench,
        targets: &[],
        until: 0,
        replay: None,
        order: None,
    };
    interrupt::listen();
    let (attempts, rounds) = match &options.baseline {
        Some(Baseline::Revision(revision)) => (against(&run, revision, &plan, &mut printer), Vec::new()),
        _ if let Some(searching) = &options.fuzz => (fuzz::campaign(&run, &plan, &mut printer, searching), Vec::new()),
        _ if let Some(orders) = &options.orders => (explore(&run, &plan, &mut printer, orders), Vec::new()),
        _ => campaign(&run, &plan, &mut printer),
    };
    let _ = std::fs::remove_file(&entries);
    let _ = std::fs::remove_file(&fuzzed);
    // Tests did not run, so there is no report, and snapshots stay (7.10).
    if interrupt::requested() {
        let ran: std::collections::HashSet<u32> = attempts.iter().map(|a| a.node).filter(|&n| !tree.suite(n)).collect();
        printer.rest();
        let message = format!(
            "interrupted: {} of {} tests ran; no report was written and no snapshot changed",
            ran.len(),
            plan.len()
        );
        if options.json {
            eprintln!("tondo: {message}");
        } else {
            say!("\n{message}");
        }
        return Err(Failure::Interrupted(interrupt::forced()));
    }
    let aggregates = aggregates(tree, &attempts);
    let mut failed = failed(&aggregates, &attempts, options);
    // The runs of the tests that failed stay recorded, to replay.
    let (recordings, parted) = match &options.record {
        Some(recordings) => recorded(recordings, executable, tree, &attempts).map_err(Failure::Output)?,
        None => Default::default(),
    };
    let profiles = match &collected {
        Some((kind, from)) => {
            let written = profiled(tree, executable, *kind, from, dir, Path::new("target/profiles"));
            let _ = std::fs::remove_dir_all(from);
            written.map_err(Failure::Output)?
        }
        None => Vec::new(),
    };
    let baselines = std::mem::take(&mut printer.baselines);
    // What each test's benchmarks measured, the last time it ran.
    let measured: Vec<(String, &Benchmark)> = aggregates
        .iter()
        .flat_map(|a| a.attempts[a.decisive].benchmarks.iter().map(move |b| (tree.id(a.node).to_string(), b)))
        .collect();
    // A benchmark slower or allocating more than allowed fails the run.
    let regressions: Vec<String> = measured
        .iter()
        .filter_map(|(test, b)| {
            let base = baselines.measures.get(&(test.clone(), b.name.clone()))?;
            let why = bench::compare(b, base, options.max_regression).regressed?;
            Some(format!("{test} {}: {why}", b.name))
        })
        .collect();
    failed |= !regressions.is_empty();
    if let (Some(path), Some(time)) = (&options.save_baseline, options.bench) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Failure::Output(format!("cannot create {}: {e}", parent.display())))?;
        }
        publish(path, &(bench::baseline(bench::environment(time), &measured).text() + "\n"))?;
    }
    // Updated snapshots are published only after a run without failures,
    // skips or blocks (9.7).
    let after = if options.update_snapshots {
        let clean = aggregates.iter().all(|a| a.status == Status::Passed && !a.flaky);
        let values: Vec<((String, String), String)> = attempts
            .iter()
            .flat_map(|a| a.snapshots.iter().map(move |s| (a.node, s)))
            .filter(|(_, s)| s.status == "created" || s.status == "updated")
            .map(|(node, s)| ((tree.id(node).to_string(), s.name.clone()), s.actual.clone()))
            .collect();
        match clean {
            true => Some((snapshots.publish(values).map_err(Failure::Output)?, true)),
            false => Some((snapshots.before.clone(), false)),
        }
    } else {
        None
    };
    write_manifest(tree, &artifacts, &aggregates)?;
    context.snapshots = match &after {
        Some((after, published)) => (&snapshots.before, after, Some(*published)),
        None => (&snapshots.before, &snapshots.before, None),
    };
    context.rounds = &rounds;
    context.baselines = &baselines;
    context.profiles = &profiles;
    let report = context.report(&aggregates);
    let covered = match &counts {
        Some(counts) => {
            let covered = coverage::collect(counts, &executable.with_file_name("counted")).map_err(Failure::Output);
            let _ = std::fs::remove_dir_all(counts);
            Some(covered?)
        }
        None => None,
    };
    printer.regressions = regressions;
    // The files of other systems, not built here, which coverage does not
    // count as code that did not run (language 6.8).
    let system = crate::driver::system();
    let elsewhere = crate::project::sources(dir, "package", None)
        .map(|(files, _)| {
            files
                .into_iter()
                .filter(|file| !crate::project::belongs(&file.path, Some(system)))
                .map(|file| format!("`{}`", file.path.strip_prefix(dir).unwrap_or(&file.path).display()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let elsewhere = (covered.is_some() && !elsewhere.is_empty())
        .then(|| format!("Not built on {}, so not counted: {}.", system.shown(), elsewhere.join(", ")));
    if options.json {
        say!("{}", json::reported(&report, options));
        for recording in &recordings {
            eprintln!("tondo: recorded: tondo debug --replay {}", recording.display());
        }
        for (test, text) in &parted {
            eprintln!("tondo: {test} passed and failed:\n{text}");
        }
    } else {
        human::report(tree, &printer, &aggregates, &owners, options, failed, started.elapsed());
        if !recordings.is_empty() {
            say!("\nthe runs that failed, recorded, to debug the same again:");
            for recording in &recordings {
                say!("  tondo debug --replay {}", recording.display());
            }
        }
        for (test, text) in &parted {
            say!("\n{test} passed and failed:\n{}", text.trim_end());
        }
        if !profiles.is_empty() {
            say!("\nprofiles, for `tondo profile show <file>`:");
            for (_, _, path) in &profiles {
                say!("  {}", path.display());
            }
        }
        if let Some(covered) = covered.as_ref().filter(|_| options.coverage) {
            print!("{}", covered.summary());
            if let Some(elsewhere) = &elsewhere {
                say!("{elsewhere}");
            }
        }
    }
    let document = || document::Document {
        title: format!("Tests of {}", package(dir)),
        failed,
        counts: tallies(tree, &aggregates),
        elapsed: started.elapsed(),
        failures: aggregates
            .iter()
            .filter(|a| a.status.failed() || matches!(a.status, Status::Blocked(_, false)))
            .map(|a| {
                let kind = if tree.suite(a.node) { "suite " } else { "" };
                (format!("{kind}{}", tree.id(a.node)), human::detail(tree, &a.attempts[a.decisive], options.timeout))
            })
            .collect(),
        skipped: aggregates
            .iter()
            .filter_map(|a| match &a.status {
                Status::Skipped(reason) if !tree.suite(a.node) => Some((tree.id(a.node).to_string(), reason.clone())),
                _ => None,
            })
            .collect(),
        benchmarks: measured
            .iter()
            .map(|(test, b)| {
                let base = baselines.measures.get(&(test.clone(), b.name.clone()));
                (test.clone(), *b, base.map(|base| bench::compare(b, base, options.max_regression)))
            })
            .collect(),
        machine: options.bench.map(bench::machine),
        baseline: options.baseline.as_ref().map(|baseline| match baseline {
            Baseline::Saved(path) => format!("the baseline {}", path.display()),
            Baseline::Revision(r) => format!("{} ({}), run alongside", r.name, &r.commit[..r.commit.len().min(12)]),
        }),
        max_regression: options.max_regression,
        regressions: &printer.regressions,
        coverage: covered.as_ref().map(coverage::Coverage::rows),
        elsewhere: elsewhere.clone(),
    };
    for (kind, path) in &options.reports {
        let text = match (kind, &covered) {
            (Report::Junit, _) => junit::report(tree, &context, &aggregates, &report, options),
            (Report::Json, _) => report.text() + "\n",
            (Report::Markdown, _) => document::markdown(&document()),
            (Report::Html, _) => document::html(&document()),
            (Report::Coverage, Some(covered)) => covered.json().text() + "\n",
            (Report::Lcov, Some(covered)) => covered.lcov(),
            (_, None) => unreachable!("a report of coverage counts"),
        };
        publish(path, &text)?;
    }
    if let Some(error) = artifacts.error.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(Failure::Output(error));
    }
    Ok(failed)
}

/// The profiles the workers collected in `from`, each made into pprof in
/// `to` of the project `dir`, named after its test and benchmark: each
/// test's id, benchmark and file, from the project's directory.
fn profiled(
    tree: &Tree,
    executable: &Path,
    kind: crate::profile::Kind,
    from: &Path,
    dir: &Path,
    to: &Path,
) -> Result<Vec<(String, Option<String>, PathBuf)>, String> {
    let mut written = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    if !entries.is_empty() {
        let at = dir.join(to);
        std::fs::create_dir_all(&at).map_err(|e| format!("cannot create {}: {e}", at.display()))?;
    }
    for entry in entries {
        let Ok(collected) = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&entry).unwrap_or_default())
        else {
            continue;
        };
        let Some(node) = collected["node"].as_u64().filter(|&n| (n as usize) < tree.nodes.len()) else { continue };
        let test = tree.id(node as u32).to_string();
        let bench = collected["bench"].as_str().map(str::to_string);
        let empty = serde_json::Map::new();
        let frames = collected["frames"].as_object().unwrap_or(&empty);
        let labels = collected["labels"].as_object().unwrap_or(&empty);
        let samples = collected["samples"].as_array().cloned().unwrap_or_default();
        let memory = serde_json::json!({ "memory": { "rate": collected["rate"], "samples": [], "allocated": collected["allocated"] } });
        let program = executable.display().to_string();
        let (builder, count) = crate::profile::build(kind, &program, frames, labels, &samples, Some(&memory))?;
        // A test too short to be sampled has no profile.
        if count == 0 {
            continue;
        }
        // From the first sample to the last.
        let times: Vec<i64> = samples.iter().filter_map(|s| s[0].as_i64()).collect();
        let duration = times.iter().max().zip(times.iter().min()).map_or(0, |(last, first)| last - first);
        let name = |text: &str| -> String {
            text.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect()
        };
        let mut file = test.replace("::", ".");
        if let Some(bench) = &bench {
            file.push_str(&format!("--{}", name(bench)));
        }
        let path = to.join(format!("{}.{}.pprof", name(&file), kind.name()));
        let bytes = builder.finish(0, duration);
        std::fs::write(dir.join(&path), bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push((test, bench, path));
    }
    Ok(written)
}

/// The tests that passed, failed, skipped, were blocked and passed after
/// failing.
fn tallies(tree: &Tree, aggregates: &[Aggregate]) -> [usize; 5] {
    let tests: Vec<&Aggregate> = aggregates.iter().filter(|a| !tree.suite(a.node)).collect();
    let count = |f: &dyn Fn(&Aggregate) -> bool| tests.iter().filter(|a| f(a)).count();
    [
        count(&|a| a.status == Status::Passed && !a.flaky),
        count(&|a| a.status.failed()),
        count(&|a| matches!(a.status, Status::Skipped(_) | Status::Blocked(_, true))),
        count(&|a| matches!(a.status, Status::Blocked(_, false))),
        count(&|a| a.flaky),
    ]
}

/// The package's name, from its manifest.
fn package(dir: &Path) -> String {
    crate::deps::resolve(dir, None).ok().map(|p| p[0].name.clone()).unwrap_or_default()
}

/// The manifest of the artifact store (15.8), when the run attached any.
fn write_manifest(tree: &Tree, artifacts: &store::Artifacts, aggregates: &[Aggregate]) -> Result<(), Failure> {
    let mut objects: Vec<(String, usize)> = Vec::new();
    let mut attachments: Vec<(String, usize, String, String, String)> = Vec::new();
    for aggregate in aggregates {
        for (index, attempt) in aggregate.attempts.iter().enumerate() {
            for artifact in &attempt.artifacts {
                if !objects.iter().any(|(h, _)| *h == artifact.sha256) {
                    objects.push((artifact.sha256.clone(), artifact.size));
                }
                let node = tree.id(aggregate.node).to_string();
                let entry =
                    (node, index + 1, artifact.name.clone(), artifact.media_type.clone(), artifact.sha256.clone());
                attachments.push(entry);
            }
        }
    }
    if attachments.is_empty() {
        return Ok(());
    }
    objects.sort();
    attachments.sort();
    use json::Value;
    let text = |t: &str| Value::Str(t.to_string());
    let object = |(hash, size): &(String, usize)| {
        Value::Object(vec![
            ("sha256".into(), text(hash)),
            ("size".into(), text(&size.to_string())),
            ("path".into(), text(&format!("objects/{hash}"))),
        ])
    };
    let attachment = |(node, attempt, name, media, hash): &(String, usize, String, String, String)| {
        Value::Object(vec![
            ("node".into(), text(node)),
            ("attempt".into(), Value::Int(*attempt as i64)),
            ("name".into(), text(name)),
            ("media_type".into(), text(media)),
            ("sha256".into(), text(hash)),
        ])
    };
    let manifest = Value::Object(vec![
        ("format".into(), text("tondo-test-artifacts-0.1/1")),
        ("objects".into(), Value::Array(objects.iter().map(object).collect())),
        ("attachments".into(), Value::Array(attachments.iter().map(attachment).collect())),
    ]);
    store::write_atomically(&artifacts.root.join("manifest.json"), (manifest.text() + "\n").as_bytes())
        .map_err(Failure::Output)
}

/// The report of a test build that did not compile (15.6), where asked.
pub fn uncompiled(dir: &Path, options: &Options) -> Result<(), Failure> {
    let ownership = owners::resolve(dir, &options.codeowners).map_err(Failure::Input)?;
    let tree = Tree { nodes: Vec::new(), locations: Vec::new(), machines: Vec::new() };
    let snapshots = store::Snapshots::read(dir, &package(dir)).map_err(Failure::Input)?;
    let none = bench::Baselines::default();
    let context = json::Context {
        baselines: &none,
        profiles: &[],
        tree: &tree,
        options,
        owners: &[],
        ownership: &ownership,
        plan: &[],
        rounds: &[],
        snapshots: (&snapshots.before, &snapshots.before, None),
        compiled: false,
    };
    let report = context.report(&[]);
    if options.json {
        say!("{}", json::reported(&report, options));
    }
    for (kind, path) in &options.reports {
        if *kind == Report::Json {
            publish(path, &(report.text() + "\n"))?;
        }
    }
    Ok(())
}

/// Writes a report whole, or not at all.
fn publish(path: &Path, text: &str) -> Result<(), Failure> {
    let partial = path.with_extension(format!("partial-{}", std::process::id()));
    std::fs::write(&partial, text)
        .and_then(|_| std::fs::rename(&partial, path))
        .map_err(|e| Failure::Output(format!("cannot write the report {}: {e}", path.display())))
}

/// Why `tondo test` could not report: an input it could not use, no tests,
/// or a report it could not write.
pub enum Failure {
    Input(String),
    Empty(String),
    Output(String),
    /// The run was interrupted; `true` when a worker had to be killed.
    Interrupted(bool),
}

/// Runs the plan once per iteration, then the retry rounds.
/// The retry rounds that ran: each one's units, with their tests.
type Rounds = Vec<Vec<(u32, Vec<u32>)>>;

fn campaign(run: &worker::Run, plan: &[u32], printer: &mut human::Printer) -> (Vec<Attempt>, Rounds) {
    let (tree, options) = (run.tree, run.options);
    let mut attempts = Vec::new();
    let mut rounds = Vec::new();
    for iteration in (1..=options.iterations()).take_while(|_| !interrupt::requested()) {
        printer.begin(iteration, 0, plan);
        let units = units(tree, plan);
        let context = Context { round: 0, iteration, unit: None, target: None };
        let units = units.into_iter().map(|u| (u, context)).collect();
        attempts.extend(worker::execute(run, units, |note| printer.note(note)));
    }
    for round in 1..=options.retry {
        let units = retry_units(tree, plan, &attempts);
        if units.is_empty() || interrupt::requested() {
            break;
        }
        let tests: Vec<u32> = units.iter().flat_map(|(_, tests)| tests.iter().copied()).collect();
        printer.begin(1, round, &tests);
        rounds.push(units.clone());
        let units = units
            .into_iter()
            .map(|(unit, tests)| (tests, Context { round, iteration: 1, unit: Some(unit), target: None }))
            .collect();
        attempts.extend(worker::execute(run, units, |note| printer.note(note)));
    }
    (attempts, rounds)
}

/// The units of a first round: each top-level suite or test, with its
/// tests of the plan.
fn units(tree: &Tree, plan: &[u32]) -> Vec<Vec<u32>> {
    let mut units: Vec<Vec<u32>> = Vec::new();
    for &test in plan {
        match units.last_mut() {
            Some(unit) if top(tree, unit[0]) == top(tree, test) => unit.push(test),
            _ => units.push(vec![test]),
        }
    }
    units
}

/// The rounds of `--against`, which alternate which side runs first.
const ROUNDS: u32 = 4;

/// `--against` (10.4): each unit of the plan runs in the revision's tests,
/// when they have its tests, and in these, on this machine, in rounds that
/// alternate which goes first, each with a part of the time; the samples of
/// each side are put together, so that a machine that speeds up or slows
/// down weighs on both. What the revision's benchmarks measure is the
/// baseline.
fn against(run: &worker::Run, revision: &Revision, plan: &[u32], printer: &mut human::Printer) -> Vec<Attempt> {
    let (tree, options) = (run.tree, run.options);
    let bench = run.bench.map(|time| time / ROUNDS);
    let ours = worker::Run { stores: worker::Stores { ..run.stores }, bench, ..*run };
    let stores = worker::Stores { update: false, counts: None, ..run.stores };
    let (executable, dir, theirs_tree) = (&revision.executable, &revision.dir, &revision.tree);
    let theirs = worker::Run {
        executable,
        dir,
        tree: theirs_tree,
        options,
        stores,
        bench,
        targets: &[],
        until: 0,
        replay: None,
        order: None,
    };
    let node = |id: &str| (1..revision.tree.nodes.len() as u32).find(|&n| revision.tree.id(n) == id);
    printer.begin(1, 0, plan);
    let context = Context { round: 0, iteration: 1, unit: None, target: None };
    let mut attempts = Vec::new();
    for unit in units(tree, plan) {
        let mapped: Vec<u32> = unit.iter().filter_map(|&t| node(tree.id(t))).collect();
        unit.iter().for_each(|&test| printer.note(&worker::Note::Started(test, context)));
        let mut measured: Vec<Attempt> = Vec::new();
        for round in 0..ROUNDS {
            for first in [round % 2 == 0, round % 2 == 1] {
                if interrupt::requested() {
                    return attempts;
                }
                if first && !mapped.is_empty() {
                    for attempt in worker::execute(&theirs, vec![(mapped.clone(), context)], |_| {}) {
                        let id = revision.tree.id(attempt.node).to_string();
                        for benchmark in attempt.benchmarks {
                            let key = (id.clone(), benchmark.name.clone());
                            match printer.baselines.measures.get_mut(&key) {
                                Some(kept) => joined(kept, benchmark),
                                None => _ = printer.baselines.measures.insert(key, benchmark),
                            }
                        }
                    }
                } else if !first {
                    for attempt in worker::execute(&ours, vec![(unit.clone(), context)], |_| {}) {
                        match measured.iter_mut().find(|a| a.node == attempt.node) {
                            // A round that fails decides.
                            Some(kept) if attempt.status != Status::Passed => *kept = attempt,
                            Some(kept) => {
                                for benchmark in attempt.benchmarks {
                                    match kept.benchmarks.iter_mut().find(|b| b.name == benchmark.name) {
                                        Some(found) => joined(found, benchmark),
                                        None => kept.benchmarks.push(benchmark),
                                    }
                                }
                            }
                            None => measured.push(attempt),
                        }
                    }
                }
            }
        }
        for attempt in measured {
            printer.note(&worker::Note::Ended(Box::new(attempt.clone())));
            attempts.push(attempt);
        }
    }
    attempts
}

/// A benchmark's samples of another round, with what it allocated last.
fn joined(kept: &mut Benchmark, more: Benchmark) {
    kept.samples.extend(more.samples);
    (kept.runs, kept.allocations, kept.bytes) = (more.runs, more.allocations, more.bytes);
}

fn top(tree: &Tree, mut node: u32) -> u32 {
    while tree.nodes[node as usize].parent != 0 {
        node = tree.nodes[node as usize].parent;
    }
    node
}

/// What the next retry round runs (8.7): each test that failed, each suite
/// whose own setup or teardown failed with the tests of the plan inside it,
/// and a test blocked by a suite that no longer fails; a suite absorbs the
/// units inside it. Skips, blocks and other states are not retried.
fn retry_units(tree: &Tree, plan: &[u32], attempts: &[Attempt]) -> Vec<(u32, Vec<u32>)> {
    let latest = |node: u32| attempts.iter().rev().find(|a| a.node == node);
    let mut units: Vec<u32> = Vec::new();
    for &test in plan {
        match latest(test).map(|a| &a.status) {
            Some(status) if status.failed() => units.push(test),
            Some(Status::Blocked(suite, false)) if !latest(*suite).is_some_and(|a| a.status.failed()) => {
                units.push(test)
            }
            _ => {}
        }
    }
    for suite in (1..tree.nodes.len() as u32).filter(|&n| tree.suite(n)) {
        if latest(suite).is_some_and(|a| a.status.failed()) {
            units.push(suite);
        }
    }
    let absorbed =
        |unit: u32| units.iter().any(|&outer| outer != unit && tree.suite(outer) && tree.within(unit, outer));
    let mut kept: Vec<(u32, Vec<u32>)> = units
        .iter()
        .filter(|&&unit| !absorbed(unit))
        .map(|&unit| (unit, plan.iter().copied().filter(|&t| tree.within(t, unit)).collect::<Vec<u32>>()))
        .filter(|(_, tests)| !tests.is_empty())
        .collect();
    let first = |tests: &[u32]| plan.iter().position(|t| tests.contains(t)).unwrap_or(usize::MAX);
    kept.sort_by(|a, b| first(&a.1).cmp(&first(&b.1)).then_with(|| tree.id(a.0).cmp(tree.id(b.0))));
    kept.dedup_by_key(|(unit, _)| *unit);
    kept
}

/// Whether the run fails (9.6): a failure or a block, a flaky pass without
/// `--allow-flaky`, a skip with `--deny-skips`, or with `--repeat` any
/// attempt that did not pass.
fn failed(aggregates: &[Aggregate], attempts: &[Attempt], options: &Options) -> bool {
    aggregates.iter().any(|a| {
        a.status.failed()
            || matches!(a.status, Status::Blocked(_, false))
            || (a.flaky && !options.allow_flaky)
            || (options.deny_skips && matches!(a.status, Status::Skipped(_) | Status::Blocked(_, true)))
    }) || (options.iterations() > 1 && attempts.iter().any(|a| a.status != Status::Passed))
}

impl Options {
    /// How many times the selection runs: `--repeat`, or `--schedules`.
    pub(super) fn iterations(&self) -> u32 {
        self.schedules.map_or(self.repeat, |(count, ..)| count)
    }

    /// The seed of the order of tasks an iteration runs with (8.10).
    pub(super) fn schedule(&self, iteration: u32) -> Option<u64> {
        self.schedules.filter(|_| self.orders.is_none()).map(|(_, seed, _)| schedule_seed(seed, iteration))
    }
}

/// Which orders of the tasks a test runs in, by their choices (8.10).
#[derive(Clone, Debug)]
pub enum Orders {
    All(usize),
    At(String),
}

/// The orders `--schedules all` tries of a test, unless it says.
pub const ORDERS: usize = 20_000;

/// `--schedules all` (8.10): each test runs in the order of its first choices,
/// and then again for every other choice it could have made, those with fewer
/// changes first, as most faults need few, until none is left or it fails.
fn explore(run: &worker::Run, plan: &[u32], printer: &mut human::Printer, orders: &Orders) -> Vec<Attempt> {
    let (tree, options) = (run.tree, run.options);
    printer.begin(1, 0, plan);
    let context = Context { round: 0, iteration: 1, unit: None, target: None };
    let out = std::env::temp_dir().join(format!("tondo-orders-{}", std::process::id()));
    let mut attempts = Vec::new();
    // Every order there is, to hold the reduction to it in the tests.
    let every = std::env::var_os("TONDO_ORDERS_EVERY").is_some();
    for &test in plan.iter().filter(|&&test| !tree.suite(test)) {
        let (limit, first) = match orders {
            Orders::All(limit) => (*limit, (Vec::new(), Vec::new())),
            Orders::At(path) => (1, (path.split('.').filter_map(|choice| choice.parse().ok()).collect(), Vec::new())),
        };
        // What a model never did in any order: in none of them.
        let mut never: Option<Vec<[std::collections::BTreeSet<String>; 2]>> = None;
        let mut tried = std::collections::HashSet::from([first.clone()]);
        let mut classes = std::collections::HashSet::new();
        let mut pending: std::collections::VecDeque<orders::Order> = [first].into();
        let (mut ran, mut last) = (0, Vec::new());
        let (mut left_out, mut failed_at) = (false, None);
        while let Some((given, next)) = pending.pop_front() {
            if ran == limit || interrupt::requested() {
                left_out = true;
                break;
            }
            let dotted = |numbers: Vec<String>| numbers.join(".");
            let text = dotted(given.iter().map(u32::to_string).collect());
            let text = format!("{text}/{}", dotted(next.iter().map(u64::to_string).collect()));
            let order = (text, out.clone());
            let one = worker::Run { stores: worker::Stores { ..run.stores }, order: Some(&order), ..*run };
            last = worker::execute(&one, vec![(vec![test], context)], |_| {});
            ran += 1;
            let written = std::fs::read_to_string(&out).unwrap_or_default();
            // Another run of an order gives what the first did.
            let another = classes.insert(orders::class(&written));
            if last.iter().any(|attempt| attempt.status.failed()) {
                let text = dotted(orders::path(&written).iter().map(u32::to_string).collect());
                if !options.json {
                    say!("{}: failed in order {ran}", tree.id(test));
                    say!("again in that order: tondo test --exact {} --schedules at:{text}", tree.id(test));
                    say!("debug it in that order: tondo debug --test {} --schedule at:{text}", tree.id(test));
                }
                failed_at = Some(text);
                pending.clear();
                break;
            }
            // What a model never did, and the properties that said nothing:
            // in none of the orders.
            for (at, model) in last.iter().flat_map(|attempt| &attempt.models).enumerate() {
                let names = |key: &str| -> std::collections::BTreeSet<String> {
                    model[key].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
                };
                let here = [names("never"), names("vacuous")];
                let all = never.get_or_insert_with(Vec::new);
                match all.get_mut(at) {
                    Some(before) => {
                        for (kept, now) in before.iter_mut().zip(&here) {
                            kept.retain(|name| now.contains(name));
                        }
                    }
                    None => all.push(here),
                }
            }
            if !another && !every {
                continue;
            }
            for mut other in orders::others(&written, every, given.len()) {
                while other.1.is_empty() && other.0.last() == Some(&0) {
                    other.0.pop();
                }
                if tried.insert(other.clone()) {
                    pending.push_back(other);
                }
            }
            if pending.len() > limit {
                left_out = true;
                pending.truncate(limit);
            }
        }
        // A failure said where it stopped.
        if matches!(orders, Orders::All(_)) && failed_at.is_none() {
            let how = if left_out { "not all of them: `--schedules all:<n>` tries more" } else { "all that differ" };
            let count = match classes.len() {
                1 => "1 order".to_string(),
                count => format!("{count} orders"),
            };
            match options.json {
                true => eprintln!("tondo: {}: {count}, {how}", tree.id(test)),
                false => say!("{}: {count}, {how}", tree.id(test)),
            }
        }
        for attempt in &mut last {
            for (model, [none, idle]) in attempt.models.iter_mut().zip(never.iter().flatten()) {
                model["never"] = serde_json::json!(none);
                model["vacuous"] = serde_json::json!(idle);
            }
        }
        for mut attempt in last.drain(..) {
            let failed_at = failed_at.clone();
            let complete = !left_out && failed_at.is_none();
            attempt.explored = Some(Explored { orders: classes.len(), complete, failed_at });
            printer.note(&worker::Note::Ended(Box::new(attempt.clone())));
            attempts.push(attempt);
        }
    }
    let _ = std::fs::remove_file(&out);
    attempts
}

/// The recordings of the runs that failed, and where a test that passed and
/// failed parted: its id, and that in words.
type Recorded = (Vec<PathBuf>, Vec<(String, String)>);

/// The recordings of the attempts that failed, made a whole with the tests'
/// executable; those of the attempts that did not fail go, but for one of a
/// test that also failed, which says where a run that passed and one that
/// failed part: for each such test, its id and that.
fn recorded(recordings: &Path, executable: &Path, tree: &Tree, attempts: &[Attempt]) -> Result<Recorded, String> {
    let tests = || attempts.iter().filter(|a| !tree.suite(a.node));
    let dir_of =
        |attempt: &Attempt| recordings.join(worker::recording(tree, attempt.node, attempt.iteration, attempt.round));
    let went_well = |attempt: &Attempt| matches!(attempt.status, Status::Passed | Status::Skipped(_));
    let kept_up = |dir: &Path| {
        std::fs::copy(executable, dir.join("program")).map_err(|e| format!("cannot write {}: {e}", dir.display()))
    };
    let mut kept = Vec::new();
    let mut parted = Vec::new();
    let mut compared: Vec<u32> = Vec::new();
    for attempt in tests().filter(|a| dir_of(a).join("run.json").is_file()) {
        let dir = dir_of(attempt);
        if went_well(attempt) {
            continue;
        }
        kept_up(&dir)?;
        kept.push(dir.clone());
        let passed = tests().find(|a| a.node == attempt.node && matches!(a.status, Status::Passed));
        if let Some(passed) = passed.filter(|_| !compared.contains(&attempt.node)) {
            let good = dir_of(passed);
            if good.join("run.json").is_file() {
                compared.push(attempt.node);
                kept_up(&good)?;
                let text = match crate::debug::compared(&good, &dir) {
                    Ok(found) => crate::debug::compared_text(&found, &good, &dir),
                    Err(message) => format!("the two runs do not compare: {message}\n"),
                };
                parted.push((tree.id(attempt.node).to_string(), text));
            }
        }
    }
    // What nothing compares with goes.
    let compared_with = |attempt: &&Attempt| {
        compared.contains(&attempt.node)
            && tests()
                .find(|a| a.node == attempt.node && matches!(a.status, Status::Passed))
                .is_some_and(|first| (first.iteration, first.round) == (attempt.iteration, attempt.round))
    };
    for attempt in tests().filter(|a| went_well(a)).filter(|a| !compared_with(a)) {
        let _ = std::fs::remove_dir_all(dir_of(attempt));
    }
    Ok((kept, parted))
}

/// The seed of iteration `iteration` of `--schedules` from `seed`: the
/// first is the seed itself, so `--schedules 1 --seed` repeats any one.
pub fn schedule_seed(seed: u64, iteration: u32) -> u64 {
    if iteration <= 1 {
        return seed;
    }
    // SplitMix64 of the seed and the iteration.
    let mut z = seed.wrapping_add(u64::from(iteration).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What a test build run outside `tondo test`, as under a debugger, reads
/// from the project at `dir`: its snapshots and the inputs of its fuzz
/// store, each written to a file, as the variables that name them.
pub fn test_environment(dir: &Path) -> Result<Vec<(&'static str, PathBuf)>, String> {
    let package = package(dir);
    let snapshots = store::Snapshots::read(dir, &package)?;
    let id = std::process::id();
    let (entries, fuzzed) = (
        std::env::temp_dir().join(format!("tondo-debug-snapshots-{id}")),
        std::env::temp_dir().join(format!("tondo-debug-fuzz-{id}")),
    );
    std::fs::write(&entries, snapshots.for_worker()).map_err(|e| e.to_string())?;
    std::fs::write(&fuzzed, store::fuzz_inputs(dir, &package)).map_err(|e| e.to_string())?;
    Ok(vec![("TONDO_TEST_SNAPSHOTS", entries), ("TONDO_TEST_FUZZ", fuzzed)])
}

/// Where the test build of the project at `path` is kept: a directory
/// named after everything the build reads, with the executable and its tree.
pub fn build_dir(
    path: &Path,
    statically: bool,
    debug: bool,
    counting: bool,
    fuzz: bool,
    live: bool,
    target: Option<crate::target::Target>,
) -> Result<PathBuf, String> {
    let identity = crate::driver::compiler_identity();
    let linking = format!(
        "{}{}{}{}{}{}",
        if statically { "static" } else { "" },
        if debug { "debug" } else { "" },
        if live { "live" } else { "" },
        if counting { "coverage" } else { "" },
        if fuzz { "fuzz" } else { "" },
        target.map_or("", crate::target::Target::name)
    );
    let mut parts = vec![identity.into_bytes(), linking.as_bytes().to_vec()];
    for input in crate::driver::test_inputs(path)? {
        parts.push(input.as_os_str().as_encoded_bytes().to_vec());
        parts.push(std::fs::read(&input).map_err(|e| format!("cannot read {}: {e}", input.display()))?);
    }
    let key = crate::driver::fingerprint(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let project = std::path::absolute(path).map_err(|e| e.to_string())?;
    Ok(crate::driver::build_entry("test", &[project.as_os_str().as_encoded_bytes(), linking.as_bytes()], &key))
}
