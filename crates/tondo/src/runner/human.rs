//! What `tondo test` prints for people (testing 9.3): a line per test in
//! canonical order as soon as it is known, then what failed, was flaky or
//! skipped, with every attempt and what it wrote.

use super::worker::Note;
use super::{Aggregate, Attempt, Options, Searched, Status, Tree};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// The suites and tests of a plan with their owners, in canonical order.
pub(super) fn list(tree: &Tree, plan: &[u32], owners: &[Vec<String>]) {
    let mut nodes: Vec<u32> = Vec::new();
    for &test in plan {
        let mut node = test;
        while node != 0 {
            if !nodes.contains(&node) {
                nodes.push(node);
            }
            node = tree.nodes[node as usize].parent;
        }
    }
    nodes.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
    for node in nodes {
        let suite = if tree.suite(node) { " (suite)" } else { "" };
        let owned = owners[node as usize].join(" ");
        let owned = if owned.is_empty() { owned } else { format!("  {owned}") };
        say!("{}{suite}{owned}", tree.id(node));
    }
}

/// Prints each test's line of a pass in canonical order, as soon as the
/// lines before it are known.
pub(super) struct Printer<'a> {
    /// Silent when stdout gets the JSON report.
    quiet: bool,
    /// The events of `--test-format events` instead (15.9).
    events: Option<&'a Options>,
    tree: &'a Tree,
    plan: Vec<u32>,
    expected: Vec<u32>,
    known: HashMap<u32, Attempt>,
    printed: usize,
    timeout: Duration,
    /// The seed of `--schedules`, to name each iteration's.
    schedules: Option<u64>,
    /// What the benchmarks compare with (10.4).
    pub(super) baselines: super::bench::Baselines,
    max_regression: Option<f64>,
    /// The benchmarks that regressed beyond `--max-regression`.
    pub(super) regressions: Vec<String>,
    /// Under `--fuzz`, how each call's search goes, and how it is shown.
    fuzz: Option<Fuzzing>,
}

/// The lines of a search: each call's, as its last report says, the lines
/// drawn on a terminal, which change in place, or when each was last
/// printed elsewhere.
struct Fuzzing {
    calls: BTreeMap<(String, String), Searched>,
    terminal: bool,
    drawn: usize,
    printed: HashMap<(String, String), Instant>,
}

impl<'a> Printer<'a> {
    pub(super) fn new(tree: &'a Tree, plan: &[u32], options: &'a Options) -> Printer<'a> {
        let mut plan = plan.to_vec();
        plan.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
        let timeout = options.timeout;
        let quiet = options.json;
        let events = options.events.then_some(options);
        let schedules = options.schedules.map(|(_, seed, _)| seed);
        let (expected, known) = (Vec::new(), HashMap::new());
        let (baselines, max_regression) = (Default::default(), options.max_regression);
        Printer {
            quiet,
            events,
            tree,
            plan,
            expected,
            known,
            printed: 0,
            timeout,
            schedules,
            baselines,
            max_regression,
            regressions: Vec::new(),
            fuzz: None,
        }
    }

    /// The search of `--fuzz` begins: how many calls, on how many workers,
    /// until when.
    pub(super) fn fuzzing_begins(&mut self, calls: usize, workers: usize, time: Option<Duration>) {
        use std::io::IsTerminal;
        let terminal = !self.quiet && std::io::stdout().is_terminal();
        self.fuzz = Some(Fuzzing { calls: BTreeMap::new(), terminal, drawn: 0, printed: HashMap::new() });
        if self.quiet {
            return;
        }
        let until = match time {
            Some(time) => format!("for {}", clock(time.as_millis() as u64)),
            None => "until Ctrl-C (--fuzz-time sets a limit)".into(),
        };
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        say!("fuzzing {calls} call{} on {workers} worker{} {until}", plural(calls), plural(workers));
    }

    /// How a call's search goes: on a terminal its line changes in place,
    /// elsewhere it is printed every 10 seconds.
    pub(super) fn fuzzing(&mut self, node: u32, call: &str, searched: &Searched) {
        if let Some(options) = self.events {
            let note = Note::Fuzzing(node, call.to_string(), *searched);
            say!("{}", super::json::note_event(self.tree, &note, options).text());
        }
        let test = self.tree.id(node);
        let Some(fuzzing) = self.fuzz.as_mut().filter(|_| !self.quiet) else { return };
        let key = (test.to_string(), call.to_string());
        fuzzing.calls.insert(key.clone(), *searched);
        if fuzzing.terminal {
            let mut out = String::new();
            if fuzzing.drawn > 0 {
                out.push_str(&format!("\x1b[{}A", fuzzing.drawn));
            }
            for ((test, call), searched) in &fuzzing.calls {
                out.push_str(&format!("\x1b[2K{}\n", search_line(test, call, searched)));
            }
            fuzzing.drawn = fuzzing.calls.len();
            print!("{out}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        } else if fuzzing.printed.get(&key).is_none_or(|at| at.elapsed() >= Duration::from_secs(10)) {
            fuzzing.printed.insert(key, Instant::now());
            say!("{}", search_line(test, call, searched));
        }
    }

    /// The search ended: the tests' lines follow.
    pub(super) fn fuzzing_ends(&mut self) {
        if let Some(fuzzing) = self.fuzz.take()
            && !self.quiet
            && !fuzzing.calls.is_empty()
        {
            say!("");
        }
    }

    /// A pass over `tests`: an iteration of the plan, or a retry round.
    pub(super) fn begin(&mut self, iteration: u32, round: u32, tests: &[u32]) {
        if self.events.is_some() {
            say!("{}", super::json::pass_event(self.tree, iteration, round, tests).text());
        }
        if !self.quiet && round > 0 {
            say!("\nretry round {round}:");
        } else if let (false, Some(seed)) = (self.quiet, self.schedules) {
            say!("\norder {iteration}, seed {:x}:", super::schedule_seed(seed, iteration));
        } else if !self.quiet && iteration > 1 {
            say!("\niteration {iteration}:");
        }
        self.expected = self.plan.iter().copied().filter(|t| tests.contains(t)).collect();
        self.known.clear();
        self.printed = 0;
    }

    pub(super) fn note(&mut self, note: &Note) {
        if let Some(options) = self.events {
            say!("{}", super::json::note_event(self.tree, note, options).text());
        }
        let Note::Ended(attempt) = note else { return };
        if self.quiet || self.tree.suite(attempt.node) {
            return;
        }
        self.known.insert(attempt.node, (**attempt).clone());
        while let Some(attempt) = self.expected.get(self.printed).and_then(|t| self.known.get(t)) {
            say!("test {} ... {}", self.tree.id(attempt.node), short(self.tree, &attempt.status));
            // What its benchmarks measured, under `--bench`.
            if !attempt.benchmarks.is_empty() {
                let lines = super::bench::lines(
                    self.tree.id(attempt.node),
                    &attempt.benchmarks,
                    &self.baselines,
                    self.max_regression,
                );
                say!("{}", lines.trim_end());
            }
            // What the search of each call did, under `--fuzz`.
            for fuzzed in &attempt.fuzz {
                if let Some(searched) = &fuzzed.search {
                    let coverage = match searched.total {
                        0 => String::new(),
                        total => format!(", coverage {}%", searched.reached * 100 / total),
                    };
                    say!(
                        "  {}  {} inputs in {}, corpus {}{coverage}",
                        fuzzed.name,
                        count(searched.inputs),
                        clock(searched.elapsed_ms),
                        searched.corpus
                    );
                }
            }
            self.printed += 1;
        }
    }

    /// After an interruption: the lines known that wait for a line before
    /// them, in canonical order.
    pub(super) fn rest(&mut self) {
        if self.quiet {
            return;
        }
        for test in &self.expected[self.printed..] {
            if let Some(attempt) = self.known.get(test) {
                say!("test {} ... {}", self.tree.id(attempt.node), short(self.tree, &attempt.status));
            }
        }
    }

    pub(super) fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// The word for a state on a test's line.
fn short(tree: &Tree, status: &Status) -> String {
    match status {
        Status::Passed => "ok".into(),
        Status::Skipped(reason) => format!("skipped: {}", reason.lines().next().unwrap_or_default()),
        Status::Blocked(suite, true) => format!("skipped: its suite {} skipped", tree.id(*suite)),
        Status::Blocked(suite, false) => format!("BLOCKED: its suite {} failed", tree.id(*suite)),
        Status::Timeout | Status::Limit { timeout: true, .. } => "TIMEOUT".into(),
        Status::Limit { timeout: false, .. } => "RESOURCE LIMIT".into(),
        Status::Crashed(_) => "CRASHED".into(),
        Status::Error { .. } | Status::Panic(_) => "FAILED".into(),
    }
}

/// What an attempt ended with, in full.
pub(super) fn detail(tree: &Tree, attempt: &Attempt, timeout: Duration) -> String {
    match &attempt.status {
        Status::Passed => String::new(),
        Status::Error { message, location, .. } => {
            let location = location.as_deref().unwrap_or(&tree.locations[attempt.node as usize]);
            format!("error: {message}\n --> {location}")
        }
        Status::Panic(text) => text.clone(),
        Status::Timeout => format!("timed out after {}s", timeout.as_secs_f64()),
        Status::Crashed(how) | Status::Limit { message: how, .. } => how.clone(),
        Status::Skipped(reason) => format!("skipped: {reason}"),
        Status::Blocked(..) => short(tree, &attempt.status),
    }
}

/// What an attempt wrote: stdout, stderr, logs and tags.
fn written(attempt: &Attempt) -> String {
    let mut text = String::new();
    for (label, bytes) in [("stdout", &attempt.stdout), ("stderr", &attempt.stderr)] {
        if !bytes.is_empty() {
            text.push_str(&format!("{label}:\n{}", String::from_utf8_lossy(bytes)));
            if !bytes.ends_with(b"\n") {
                text.push('\n');
            }
        }
    }
    if !attempt.logs.is_empty() {
        text.push_str(&format!("logs:\n{}\n", attempt.logs.join("\n")));
    }
    if !attempt.tags.is_empty() {
        let tags: Vec<String> = attempt.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
        text.push_str(&format!("tags: {}\n", tags.join(", ")));
    }
    for (index, clock) in attempt.virtual_time.iter().enumerate() {
        let elapsed = std::time::Duration::from_nanos(clock.elapsed_ns);
        text.push_str(&format!(
            "virtual clock {}: at {elapsed:?} of virtual time, moved {} times by itself and {} by advance, settled {} times\n",
            index + 1,
            clock.automatic,
            clock.explicit,
            clock.settles
        ));
    }
    // The input a call of `testing.fuzz` failed with (10.6).
    for (fuzzed, input) in attempt.fuzz.iter().filter_map(|f| Some((f, f.input.as_ref()?))) {
        let from = match (&input.file, input.seed) {
            (Some(file), _) => format!("its input in {file}"),
            (None, Some(seed)) => format!("seed {seed}"),
            (None, None) => "an input".into(),
        };
        text.push_str(&format!("fuzz `{}` failed with {from}:\n  {}\n", fuzzed.name, input.json));
    }
    for artifact in &attempt.artifacts {
        text.push_str(&format!(
            "artifact {}: {}, {} bytes, objects/{}\n",
            artifact.name, artifact.media_type, artifact.size, artifact.sha256
        ));
    }
    for snapshot in attempt.snapshots.iter().filter(|s| s.status != "matched") {
        text.push_str(&format!("snapshot {}: {}\n", snapshot.name, snapshot.status));
        // The first line that differs.
        if let Some(expected) = &snapshot.expected {
            let mut lines = expected.split('\n').zip(snapshot.actual.split('\n')).enumerate();
            let differs = lines.find(|(_, (e, a))| e != a).map(|(i, (e, a))| (i + 1, e, a));
            let (line, wanted, found) = differs.unwrap_or_else(|| {
                let line = expected.split('\n').count().min(snapshot.actual.split('\n').count()) + 1;
                (line, "(the value ends before)", "(the value ends before)")
            });
            text.push_str(&format!("  line {line}, expected: {wanted}\n  line {line}, found:    {found}\n"));
        }
    }
    text
}

/// The details of what did not simply pass, and the counts.
pub(super) fn report(
    tree: &Tree,
    printer: &Printer,
    aggregates: &[Aggregate],
    owners: &[Vec<String>],
    options: &Options,
    failed: bool,
    elapsed: Duration,
) {
    let mut sections = Vec::new();
    for aggregate in aggregates {
        let node = aggregate.node;
        let interesting = aggregate.status.failed()
            || aggregate.flaky
            || (tree.suite(node) && matches!(aggregate.status, Status::Skipped(_)))
            || options.show_output;
        if !interesting {
            continue;
        }
        let mut head = if tree.suite(node) { format!("suite {}", tree.id(node)) } else { tree.id(node).to_string() };
        let decisive = &aggregate.attempts[aggregate.decisive];
        if tree.suite(node) && aggregate.status != Status::Passed {
            head.push_str(if decisive.teardown { " (teardown)" } else { " (setup)" });
        }
        if aggregate.flaky {
            head.push_str(" (flaky: it passed after failing)");
        }
        let mut text = format!("--- {head}\n");
        if !owners[node as usize].is_empty() {
            text.push_str(&format!("owners: {}\n", owners[node as usize].join(" ")));
        }
        if aggregate.attempts.len() == 1 {
            let detail = detail(tree, decisive, printer.timeout());
            if !detail.is_empty() {
                text.push_str(&format!("{detail}\n"));
            }
            text.push_str(&written(decisive));
        } else {
            for (index, attempt) in aggregate.attempts.iter().enumerate() {
                let when = match (attempt.iteration, attempt.round, options.schedule(attempt.iteration)) {
                    (_, 0, Some(seed)) => format!(", seed {seed:x}"),
                    (1, 0, None) => String::new(),
                    (iteration, 0, None) => format!(", iteration {iteration}"),
                    (_, round, _) => format!(", retry round {round}"),
                };
                let status = match &attempt.status {
                    Status::Passed => "passed".to_string(),
                    _ => detail(tree, attempt, printer.timeout()),
                };
                text.push_str(&format!("attempt {}{when}: {status}\n", index + 1));
                text.push_str(&written(attempt));
            }
        }
        // The first order that failed, to run again.
        if let Some(seed) =
            aggregate.attempts.iter().find(|a| a.status != Status::Passed).and_then(|a| options.schedule(a.iteration))
        {
            text.push_str(&format!(
                "again in that order: tondo test --exact {} --schedules 1 --seed {seed:x}\n",
                tree.id(node)
            ));
        }
        sections.push(text);
    }
    if !sections.is_empty() {
        say!("\n{}", sections.join("\n"));
    }
    if !printer.regressions.is_empty() {
        let max = options.max_regression.unwrap_or_default() * 100.0;
        say!("\nbenchmarks that regressed beyond {max}%:");
        for regression in &printer.regressions {
            say!("  {regression}");
        }
    }
    let tests: Vec<&Aggregate> = aggregates.iter().filter(|a| !tree.suite(a.node)).collect();
    let count = |f: &dyn Fn(&Aggregate) -> bool| tests.iter().filter(|a| f(a)).count();
    let flaky = count(&|a| a.flaky);
    let flaky = if flaky > 0 { format!(" {flaky} flaky;") } else { String::new() };
    say!(
        "{}. {} passed;{flaky} {} failed; {} skipped; {} blocked; {:.2}s",
        if failed { "FAILED" } else { "ok" },
        count(&|a| a.status == Status::Passed && !a.flaky),
        count(&|a| a.status.failed()),
        count(&|a| matches!(a.status, Status::Skipped(_) | Status::Blocked(_, true))),
        count(&|a| matches!(a.status, Status::Blocked(_, false))),
        elapsed.as_secs_f64()
    );
}

/// A call's search in a line: its test and name, how long, the inputs and
/// how many a second, the corpus, the share of counters reached, and how
/// long since something new.
fn search_line(test: &str, call: &str, searched: &Searched) -> String {
    let test = test.rsplit("::").next().unwrap_or(test);
    let rate = match searched.elapsed_ms {
        0 => 0,
        ms => searched.inputs * 1000 / ms,
    };
    let coverage = match searched.total {
        0 => String::new(),
        total => format!("  coverage {}%", searched.reached * 100 / total),
    };
    format!(
        "  {test} \u{203a} {call}  {}  {} inputs  {}/s  corpus {}{coverage}  new {} ago",
        clock(searched.elapsed_ms),
        count(searched.inputs),
        count(rate),
        searched.corpus,
        clock(searched.new_ms),
    )
}

/// A duration as a clock: `0:42`, `12:05`, `1:02:05`.
fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..3600 => format!("{}:{:02}", seconds / 60, seconds % 60),
        _ => format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60),
    }
}

/// A count as people read it: `912`, `41k`, `1.7M`.
fn count(n: u64) -> String {
    match n {
        0..10_000 => n.to_string(),
        10_000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}
