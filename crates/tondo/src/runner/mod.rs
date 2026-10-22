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
mod owners;
mod select;
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
    Skipped(String),
    Error(String),
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
        matches!(self, Status::Error(_) | Status::Panic(_) | Status::Timeout | Status::Crashed(_))
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
    let owners = owners::resolve(dir, &options.codeowners).map_err(Failure::Input)?;
    let owners: Vec<Vec<String>> = (0..tree.nodes.len() as u32).map(|n| owners.of(tree.file(n))).collect();
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
        if options.allow_empty {
            say!("{message}");
            return Ok(false);
        }
        return Err(Failure::Empty(message));
    }
    let selected = match options.shard {
        Some((index, count)) => {
            selected.into_iter().filter(|&t| select::shard_of(tree.id(t), count) == index).collect()
        }
        None => selected,
    };
    let plan = select::plan(tree, &selected, &options.order);
    if let Order::Random(seed, _) = options.order {
        say!("seed {seed:016x}");
    }
    if options.list {
        human::list(tree, &plan, &owners);
        return Ok(false);
    }
    let started = std::time::Instant::now();
    let mut printer = human::Printer::new(tree, &plan, options);
    let attempts = campaign(dir, executable, tree, &plan, options, &mut printer);
    let aggregates = aggregates(tree, &attempts);
    let failed = failed(&aggregates, &attempts, options);
    human::report(tree, &printer, &aggregates, &owners, options, failed, started.elapsed());
    Ok(failed)
}

/// Why `tondo test` ran nothing: an input it could not use, or no tests.
pub enum Failure {
    Input(String),
    Empty(String),
}

/// Runs the plan once per iteration, then the retry rounds.
fn campaign(
    dir: &Path,
    executable: &Path,
    tree: &Tree,
    plan: &[u32],
    options: &Options,
    printer: &mut human::Printer,
) -> Vec<Attempt> {
    let mut attempts = Vec::new();
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
        attempts.extend(worker::execute(executable, dir, tree, units, options, |a| printer.attempt(a)));
    }
    for round in 1..=options.retry {
        let units = retry_units(tree, plan, &attempts);
        if units.is_empty() {
            break;
        }
        let tests: Vec<u32> = units.iter().flat_map(|(_, tests)| tests.iter().copied()).collect();
        printer.begin(1, round, &tests);
        let units =
            units.into_iter().map(|(unit, tests)| (tests, Context { round, iteration: 1, unit: Some(unit) })).collect();
        attempts.extend(worker::execute(executable, dir, tree, units, options, |a| printer.attempt(a)));
    }
    attempts
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
