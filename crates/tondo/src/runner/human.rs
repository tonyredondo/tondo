//! What `tondo test` prints for people (testing 9.3): a line per test in
//! canonical order as soon as it is known, then what failed, was flaky or
//! skipped, with every attempt and what it wrote.

use super::{Aggregate, Attempt, Options, Status, Tree};
use std::collections::HashMap;
use std::time::Duration;

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
    tree: &'a Tree,
    plan: Vec<u32>,
    expected: Vec<u32>,
    known: HashMap<u32, Attempt>,
    printed: usize,
    timeout: Duration,
}

impl<'a> Printer<'a> {
    pub(super) fn new(tree: &'a Tree, plan: &[u32], options: &Options) -> Printer<'a> {
        let mut plan = plan.to_vec();
        plan.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
        let timeout = options.timeout;
        let quiet = options.json;
        Printer { quiet, tree, plan, expected: Vec::new(), known: HashMap::new(), printed: 0, timeout }
    }

    /// A pass over `tests`: an iteration of the plan, or a retry round.
    pub(super) fn begin(&mut self, iteration: u32, round: u32, tests: &[u32]) {
        if !self.quiet && round > 0 {
            say!("\nretry round {round}:");
        } else if !self.quiet && iteration > 1 {
            say!("\niteration {iteration}:");
        }
        self.expected = self.plan.iter().copied().filter(|t| tests.contains(t)).collect();
        self.known.clear();
        self.printed = 0;
    }

    pub(super) fn attempt(&mut self, attempt: &Attempt) {
        if self.quiet || self.tree.suite(attempt.node) {
            return;
        }
        self.known.insert(attempt.node, attempt.clone());
        while let Some(attempt) = self.expected.get(self.printed).and_then(|t| self.known.get(t)) {
            say!("test {} ... {}", self.tree.id(attempt.node), short(self.tree, &attempt.status));
            self.printed += 1;
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
        Status::Timeout => "TIMEOUT".into(),
        Status::Crashed(_) => "CRASHED".into(),
        Status::Error { .. } | Status::Panic(_) => "FAILED".into(),
    }
}

/// What an attempt ended with, in full.
fn detail(tree: &Tree, attempt: &Attempt, timeout: Duration) -> String {
    match &attempt.status {
        Status::Passed => String::new(),
        Status::Error { message, location, .. } => {
            let location = location.as_deref().unwrap_or(&tree.locations[attempt.node as usize]);
            format!("error: {message}\n --> {location}")
        }
        Status::Panic(text) => text.clone(),
        Status::Timeout => format!("timed out after {}s", timeout.as_secs_f64()),
        Status::Crashed(how) => how.clone(),
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
                let when = match (attempt.iteration, attempt.round) {
                    (1, 0) => String::new(),
                    (iteration, 0) => format!(", iteration {iteration}"),
                    (_, round) => format!(", retry round {round}"),
                };
                let status = match &attempt.status {
                    Status::Passed => "passed".to_string(),
                    _ => detail(tree, attempt, printer.timeout()),
                };
                text.push_str(&format!("attempt {}{when}: {status}\n", index + 1));
                text.push_str(&written(attempt));
            }
        }
        sections.push(text);
    }
    if !sections.is_empty() {
        say!("\n{}", sections.join("\n"));
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
