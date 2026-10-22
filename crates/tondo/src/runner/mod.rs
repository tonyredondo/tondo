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

mod human;
mod json;
mod junit;
mod owners;
mod select;
mod store;
mod worker;

pub use owners::Codeowners;
pub use select::{Glob, Order, Selector};

use crate::hir::TestNodeInfo;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Options {
    pub selector: Selector,
    /// `--shard index/count`.
    pub shard: Option<(u64, u64)>,
    pub order: Order,
    pub list: bool,
    pub jobs: usize,
    pub timeout: Duration,
    /// Additional rounds for failures (8.7).
    pub retry: u32,
    /// Times the whole selection runs (8.8).
    pub repeat: u32,
    pub codeowners: Codeowners,
    pub show_output: bool,
    pub allow_empty: bool,
    pub deny_skips: bool,
    pub allow_flaky: bool,
    /// What stdout gets: lines for people, or the JSON report.
    pub json: bool,
    /// `--report json=<path>` and `--report junit=<path>`: whether JUnit,
    /// and where.
    pub reports: Vec<(bool, PathBuf)>,
    /// Where attachments go; `target/test-artifacts` of the project if not.
    pub artifacts: Option<PathBuf>,
    /// Records missing and changed snapshots instead of failing on them.
    pub update_snapshots: bool,
}

/// The tree of the test build, with where each node is declared.
pub struct Tree {
    pub nodes: Vec<TestNodeInfo>,
    pub locations: Vec<String>,
}

impl Tree {
    /// Writes the tree beside the executable it describes, one node a line:
    /// its parent, whether it is a suite, its identity and its location.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let lines: Vec<String> = self
            .nodes
            .iter()
            .zip(&self.locations)
            .map(|(n, location)| format!("{}\t{}\t{}\t{location}", n.parent, n.suite as u8, n.id))
            .collect();
        let partial = path.with_extension(format!("partial-{}", std::process::id()));
        std::fs::write(&partial, lines.join("\n")).map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
        std::fs::rename(&partial, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// The tree `save` wrote, if it is there.
    pub fn load(path: &Path) -> Option<Tree> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut tree = Tree { nodes: Vec::new(), locations: Vec::new() };
        for line in text.lines() {
            let mut parts = line.splitn(4, '\t');
            let parent = parts.next()?.parse().ok()?;
            let suite = parts.next()? == "1";
            let id = parts.next()?.to_string();
            let span = crate::source::Span::new(0, 0);
            tree.nodes.push(TestNodeInfo { id, parent, suite, span });
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
    /// while unwinding after it.
    Panic(String),
    Timeout,
    Crashed(String),
    /// Not run: a suite around it failed (`blocked-setup`) or skipped
    /// (`blocked-skip`).
    Blocked(u32, bool),
}

impl Status {
    /// One of the failures of an executed attempt.
    fn failed(&self) -> bool {
        matches!(self, Status::Error { .. } | Status::Panic(_) | Status::Timeout | Status::Crashed(_))
    }
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
    pub logs: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// How long its code ran.
    pub duration: Duration,
    pub artifacts: Vec<Artifact>,
    pub snapshots: Vec<Snapshot>,
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
}

impl Attempt {
    fn new(node: u32, status: Status, context: Context) -> Attempt {
        Attempt {
            node,
            status,
            teardown: false,
            round: context.round,
            iteration: context.iteration,
            unit: context.unit,
            logs: Vec::new(),
            tags: Vec::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration: Duration::ZERO,
            artifacts: Vec::new(),
            snapshots: Vec::new(),
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
    let selected = select::select(tree, &options.selector);
    if selected.is_empty() {
        let message = match &options.selector {
            Selector::Filter(text) => format!("no test matches --filter {text:?}"),
            Selector::Glob(_) => "no suite or test matches --glob".to_string(),
            Selector::Exact(id) => format!("no suite or test is {id:?}; `tondo test --list` lists them"),
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
    let mut context = json::Context {
        tree,
        options,
        owners: &owners,
        ownership: &ownership,
        plan: &plan,
        rounds: &[],
        snapshots: (&snapshots.before, &snapshots.before, None),
        compiled: true,
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
    let artifacts = store::Artifacts {
        root: options.artifacts.clone().unwrap_or_else(|| dir.join("target/test-artifacts")),
        error: std::sync::Mutex::new(None),
    };
    let entries = std::env::temp_dir().join(format!("tondo-snapshots-{}", std::process::id()));
    std::fs::write(&entries, snapshots.for_worker()).map_err(|e| Failure::Output(e.to_string()))?;
    let stores = worker::Stores {
        artifacts: &artifacts,
        snapshots: &snapshots,
        entries: &entries,
        update: options.update_snapshots,
    };
    let run = worker::Run { executable, dir, tree, options, stores };
    let (attempts, rounds) = campaign(&run, &plan, &mut printer);
    let _ = std::fs::remove_file(&entries);
    let aggregates = aggregates(tree, &attempts);
    let failed = failed(&aggregates, &attempts, options);
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
    let report = context.report(&aggregates);
    if options.json {
        say!("{}", report.text());
    } else {
        human::report(tree, &printer, &aggregates, &owners, options, failed, started.elapsed());
    }
    for (junit, path) in &options.reports {
        let text =
            if *junit { junit::report(tree, &context, &aggregates, &report, options) } else { report.text() + "\n" };
        publish(path, &text)?;
    }
    if let Some(error) = artifacts.error.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(Failure::Output(error));
    }
    Ok(failed)
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
    let tree = Tree { nodes: Vec::new(), locations: Vec::new() };
    let snapshots = store::Snapshots::read(dir, &package(dir)).map_err(Failure::Input)?;
    let context = json::Context {
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
        say!("{}", report.text());
    }
    for (junit, path) in &options.reports {
        if !junit {
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
}

/// Runs the plan once per iteration, then the retry rounds.
/// The retry rounds that ran: each one's units, with their tests.
type Rounds = Vec<Vec<(u32, Vec<u32>)>>;

fn campaign(run: &worker::Run, plan: &[u32], printer: &mut human::Printer) -> (Vec<Attempt>, Rounds) {
    let (tree, options) = (run.tree, run.options);
    let mut attempts = Vec::new();
    let mut rounds = Vec::new();
    for iteration in 1..=options.repeat {
        printer.begin(iteration, 0, plan);
        // Each top-level suite or test is a unit of the first round.
        let mut units: Vec<Vec<u32>> = Vec::new();
        for &test in plan {
            match units.last_mut() {
                Some(unit) if top(tree, unit[0]) == top(tree, test) => unit.push(test),
                _ => units.push(vec![test]),
            }
        }
        let context = Context { round: 0, iteration, unit: None };
        let units = units.into_iter().map(|u| (u, context)).collect();
        attempts.extend(worker::execute(run, units, |a| printer.attempt(a)));
    }
    for round in 1..=options.retry {
        let units = retry_units(tree, plan, &attempts);
        if units.is_empty() {
            break;
        }
        let tests: Vec<u32> = units.iter().flat_map(|(_, tests)| tests.iter().copied()).collect();
        printer.begin(1, round, &tests);
        rounds.push(units.clone());
        let units =
            units.into_iter().map(|(unit, tests)| (tests, Context { round, iteration: 1, unit: Some(unit) })).collect();
        attempts.extend(worker::execute(run, units, |a| printer.attempt(a)));
    }
    (attempts, rounds)
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
    }) || (options.repeat > 1 && attempts.iter().any(|a| a.status != Status::Passed))
}

/// Where the test build of the project at `path` is kept: a directory
/// named after everything the build reads, with the executable and its tree.
pub fn build_dir(path: &Path) -> Result<PathBuf, String> {
    let identity = crate::driver::compiler_identity();
    let mut parts = vec![identity.into_bytes()];
    for input in crate::driver::test_inputs(path)? {
        parts.push(input.as_os_str().as_encoded_bytes().to_vec());
        parts.push(std::fs::read(&input).map_err(|e| format!("cannot read {}: {e}", input.display()))?);
    }
    let key = crate::driver::fingerprint(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
    Ok(crate::driver::cache_dir().join("test").join(key))
}
