//! `tondo test` (testing 10): builds the test executable of a project,
//! selects its tests, runs them in worker processes and reports them.
//!
//! A worker runs the tests of its plan one after another (see the
//! runtime's `testing` module) and reports each entry through frames on
//! stderr. A panic, a skip, a timeout or a crash ends the worker; another
//! starts with the rest of the plan and runs again the setups its suites
//! need.

use crate::hir::TestNodeInfo;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct Options {
    pub filter: Option<String>,
    pub exact: Option<String>,
    pub list: bool,
    pub jobs: usize,
    pub timeout: Duration,
    pub show_output: bool,
    pub allow_empty: bool,
    pub deny_skips: bool,
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Passed,
    Skipped(String),
    Error(String),
    Panic(String),
    Timeout,
    Crashed(String),
    /// Not run: a suite around it failed (`blocked-setup`) or skipped.
    Blocked(u32, bool),
}

impl Status {
    fn failed(&self) -> bool {
        matches!(self, Status::Error(_) | Status::Panic(_) | Status::Timeout | Status::Crashed(_))
    }
}

/// What one run of a test, or of a suite's setup and teardown, gave.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub node: u32,
    pub status: Status,
    /// For a suite: whether it failed in its teardown rather than its setup.
    pub teardown: bool,
    pub logs: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Outcome {
    fn new(node: u32, status: Status) -> Outcome {
        Outcome {
            node,
            status,
            teardown: false,
            logs: Vec::new(),
            tags: Vec::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

/// The tests the options select, in canonical order (testing 8.1, 8.2).
pub fn select(tree: &Tree, options: &Options) -> Result<Vec<u32>, String> {
    let mut selected: Vec<u32> = (1..tree.nodes.len() as u32)
        .filter(|&n| !tree.nodes[n as usize].suite)
        .filter(|&n| match (&options.filter, &options.exact) {
            (Some(text), _) => tree.id(n).contains(text.as_str()),
            (_, Some(id)) => {
                let target = tree.nodes.iter().position(|node| node.id == *id);
                target.is_some_and(|target| tree.within(n, target as u32))
            }
            _ => true,
        })
        .collect();
    selected.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
    Ok(selected)
}

/// The suites and tests of a selection, each once, in canonical order.
pub fn listed(tree: &Tree, selected: &[u32]) -> Vec<u32> {
    let mut nodes: Vec<u32> = Vec::new();
    for &test in selected {
        let mut node = test;
        while node != 0 {
            if !nodes.contains(&node) {
                nodes.push(node);
            }
            node = tree.nodes[node as usize].parent;
        }
    }
    nodes.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
    nodes
}

/// Runs the selected tests with `executable` from `dir`, `jobs` workers
/// at a time; `report` sees each outcome as it is known.
pub fn run(
    executable: &Path,
    dir: &Path,
    tree: &Tree,
    selected: &[u32],
    options: &Options,
    mut report: impl FnMut(&Outcome),
) -> Vec<Outcome> {
    // Each top-level suite or test goes to the lane with the fewest tests.
    let mut lanes: Vec<Vec<u32>> = vec![Vec::new(); options.jobs.max(1)];
    let mut units: Vec<(u32, Vec<u32>)> = Vec::new();
    for &test in selected {
        let mut top = test;
        while tree.nodes[top as usize].parent != 0 {
            top = tree.nodes[top as usize].parent;
        }
        match units.iter_mut().find(|(t, _)| *t == top) {
            Some((_, tests)) => tests.push(test),
            None => units.push((top, vec![test])),
        }
    }
    for (_, tests) in units {
        let lane = (0..lanes.len()).min_by_key(|&l| lanes[l].len()).unwrap_or(0);
        lanes[lane].extend(tests);
    }
    let (sender, receiver) = mpsc::channel();
    let mut outcomes = Vec::new();
    std::thread::scope(|scope| {
        for plan in lanes.into_iter().filter(|p| !p.is_empty()) {
            let sender = sender.clone();
            scope.spawn(move || lane(executable, dir, tree, plan, options.timeout, &sender));
        }
        drop(sender);
        for outcome in receiver {
            report(&outcome);
            outcomes.push(outcome);
        }
    });
    outcomes
}

/// Runs a plan in workers, one after another, until each test has an
/// outcome.
fn lane(executable: &Path, dir: &Path, tree: &Tree, plan: Vec<u32>, timeout: Duration, sender: &mpsc::Sender<Outcome>) {
    let mut remaining: VecDeque<u32> = plan.into();
    while !remaining.is_empty() {
        let before = remaining.len();
        if let Err(message) = worker(executable, dir, tree, &mut remaining, timeout, sender) {
            // The worker could not start: every test fails alike.
            for test in remaining.drain(..) {
                let _ = sender.send(Outcome::new(test, Status::Crashed(message.clone())));
            }
        }
        if remaining.len() == before
            && let Some(test) = remaining.pop_front()
        {
            let message = "the test process ended before it ran a test".to_string();
            let _ = sender.send(Outcome::new(test, Status::Crashed(message)));
        }
    }
}

enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Closed,
}

/// A worker's state while `tondo test` reads it.
struct Session {
    nonce: Vec<u8>,
    /// Entries started and not ended, innermost last: each with its outcome
    /// so far, whether a suite reached its members, and when its current
    /// phase began, if it is timed.
    running: Vec<(Outcome, bool, Option<Instant>)>,
    /// The entry that was running after each frame, by its sequence number.
    owners: Vec<Option<u32>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// Output the frames have not yet assigned, by the frame it precedes.
    pending: Vec<(u64, Vec<u8>)>,
    stdout_frames: u64,
    /// Outcomes ended by a frame, until stdout reaches that frame.
    done: Vec<(Outcome, u64)>,
}

fn worker(
    executable: &Path,
    dir: &Path,
    tree: &Tree,
    remaining: &mut VecDeque<u32>,
    timeout: Duration,
    sender: &mpsc::Sender<Outcome>,
) -> Result<(), String> {
    let plan =
        std::env::temp_dir().join(format!("tondo-plan-{}-{:?}", std::process::id(), std::thread::current().id()));
    let text: Vec<String> = remaining.iter().map(|t| t.to_string()).collect();
    std::fs::write(&plan, text.join("\n")).map_err(|e| format!("cannot write {}: {e}", plan.display()))?;
    let nonce = crate::driver::fingerprint(&[
        &std::process::id().to_le_bytes(),
        format!("{:?}{:?}", Instant::now(), std::thread::current().id()).as_bytes(),
    ]);
    let mut command = Command::new(executable);
    command
        .current_dir(dir)
        .env("TONDO_TEST_PLAN", &plan)
        .env("TONDO_TEST_NONCE", &nonce)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so that a timeout ends what it started too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
    let (events, received) = mpsc::channel();
    let readers = [
        child.stdout.take().map(|s| (Box::new(s) as Box<dyn Read + Send>, true)),
        child.stderr.take().map(|s| (Box::new(s) as Box<dyn Read + Send>, false)),
    ];
    for (mut stream, stdout) in readers.into_iter().flatten() {
        let events = events.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let bytes = buffer[..n].to_vec();
                        let _ = events.send(if stdout { Event::Stdout(bytes) } else { Event::Stderr(bytes) });
                    }
                }
            }
            let _ = events.send(Event::Closed);
        });
    }
    drop(events);
    let mut session = Session {
        nonce: format!("\x1e{nonce} ").into_bytes(),
        running: Vec::new(),
        owners: vec![None],
        stdout: Vec::new(),
        stderr: Vec::new(),
        pending: Vec::new(),
        stdout_frames: 0,
        done: Vec::new(),
    };
    let mut open = 2;
    let mut timed_out = false;
    while open > 0 {
        let deadline = session.running.last().and_then(|(_, _, started)| *started).map(|s| s + timeout);
        let wait = deadline.map_or(Duration::from_secs(3600), |d| d.saturating_duration_since(Instant::now()));
        match received.recv_timeout(wait) {
            Ok(Event::Stdout(bytes)) => session.stdout(&bytes),
            Ok(Event::Stderr(bytes)) => session.stderr(&bytes),
            Ok(Event::Closed) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                kill(&mut child);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let ready = session.stdout_frames;
        for (outcome, _) in session.done.extract_if(.., |(_, frame)| *frame <= ready) {
            send(tree, remaining, outcome, sender);
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&plan);
    session.flush_output();
    for (outcome, _) in std::mem::take(&mut session.done) {
        send(tree, remaining, outcome, sender);
    }
    // The entry that was running when the worker ended.
    if let Some((mut outcome, reached, _)) = session.running.pop() {
        if outcome.status == Status::Passed {
            outcome.status = if timed_out { Status::Timeout } else { Status::Crashed(describe(status)) };
        }
        outcome.teardown = reached;
        send(tree, remaining, outcome, sender);
    }
    Ok(())
}

/// Reports an outcome and takes its tests out of the plan: a test that
/// ran, or the tests a suite that failed or skipped in its setup blocks.
fn send(tree: &Tree, remaining: &mut VecDeque<u32>, outcome: Outcome, sender: &mpsc::Sender<Outcome>) {
    let node = outcome.node;
    let suite = tree.nodes[node as usize].suite;
    let blocked: Vec<u32> = if !suite {
        remaining.retain(|&t| t != node);
        Vec::new()
    } else if outcome.status == Status::Passed || outcome.teardown {
        Vec::new()
    } else {
        let blocked: Vec<u32> = remaining.iter().copied().filter(|&t| tree.within(t, node)).collect();
        remaining.retain(|t| !blocked.contains(t));
        blocked
    };
    let skipped = matches!(outcome.status, Status::Skipped(_));
    let _ = sender.send(outcome);
    for test in blocked {
        let _ = sender.send(Outcome::new(test, Status::Blocked(node, skipped)));
    }
}

fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
    }
    let _ = child.kill();
}

fn describe(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return format!("the test process ended by signal {signal}");
    }
    format!("the test process exited with status {}", status.code().unwrap_or(-1))
}

impl Session {
    fn owner(&self) -> Option<usize> {
        (!self.running.is_empty()).then(|| self.running.len() - 1)
    }

    /// Output before the stdout marker of frame `n` belongs to the entry
    /// that ran after frame `n - 1`.
    fn stdout(&mut self, bytes: &[u8]) {
        self.stdout.extend_from_slice(bytes);
        loop {
            let Some(at) = find(&self.stdout, &self.nonce) else {
                // Keep what could be the start of a marker.
                let keep = self.stdout.len().saturating_sub(self.nonce.len() + 24);
                if keep > 0 {
                    let text: Vec<u8> = self.stdout.drain(..keep).collect();
                    self.pending.push((self.stdout_frames + 1, text));
                }
                break;
            };
            let Some(end) = self.stdout[at..].iter().position(|&b| b == b'\n') else { break };
            let text: Vec<u8> = self.stdout[..at].to_vec();
            self.pending.push((self.stdout_frames + 1, text));
            self.stdout_frames += 1;
            self.stdout.drain(..at + end + 1);
        }
        self.assign_output();
    }

    fn assign_output(&mut self) {
        let known = self.owners.len() as u64;
        let mut rest = Vec::new();
        for (frame, text) in std::mem::take(&mut self.pending) {
            if frame > known {
                rest.push((frame, text));
                continue;
            }
            let owner = self.owners[frame as usize - 1];
            self.give_stdout(owner, &text);
        }
        self.pending = rest;
    }

    fn give_stdout(&mut self, owner: Option<u32>, text: &[u8]) {
        if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| Some(o.node) == owner) {
            entry.0.stdout.extend_from_slice(text);
        } else if let Some((done, _)) = self.done.iter_mut().rev().find(|(o, _)| Some(o.node) == owner) {
            done.stdout.extend_from_slice(text);
        }
    }

    /// Output left when the worker ends belongs to what ran last.
    fn flush_output(&mut self) {
        let rest = std::mem::take(&mut self.stdout);
        self.pending.push((self.owners.len() as u64, rest));
        self.assign_output();
        let owner = self.owners.last().copied().flatten();
        for (_, text) in std::mem::take(&mut self.pending) {
            self.give_stdout(owner, &text);
        }
    }

    fn stderr(&mut self, bytes: &[u8]) {
        self.stderr.extend_from_slice(bytes);
        loop {
            let Some(at) = find(&self.stderr, &self.nonce) else {
                let keep = self.stderr.len().saturating_sub(self.nonce.len() + 64);
                let text: Vec<u8> = self.stderr.drain(..keep).collect();
                self.give_stderr(&text);
                return;
            };
            let text: Vec<u8> = self.stderr.drain(..at).collect();
            self.give_stderr(&text);
            let Some(end) = self.stderr.iter().position(|&b| b == b'\n') else { return };
            let header = String::from_utf8_lossy(&self.stderr[self.nonce.len()..end]).into_owned();
            let parts: Vec<&str> = header.split(' ').collect();
            let [_, kind, node, length] = parts[..] else {
                self.stderr.drain(..=end);
                continue;
            };
            let (Ok(node), Ok(length)) = (node.parse::<u32>(), length.parse::<usize>()) else {
                self.stderr.drain(..=end);
                continue;
            };
            if self.stderr.len() < end + 1 + length {
                return;
            }
            let payload = self.stderr[end + 1..end + 1 + length].to_vec();
            let kind = kind.to_string();
            self.stderr.drain(..end + 1 + length);
            self.frame(&kind, node, &payload);
        }
    }

    fn give_stderr(&mut self, text: &[u8]) {
        if let Some(index) = self.owner() {
            self.running[index].0.stderr.extend_from_slice(text);
        }
    }

    fn frame(&mut self, kind: &str, node: u32, payload: &[u8]) {
        let sequence = self.owners.len() as u64;
        let text = String::from_utf8_lossy(payload).into_owned();
        let now = Instant::now();
        match kind {
            "start" => self.running.push((Outcome::new(node, Status::Passed), false, Some(now))),
            "members" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                    (entry.1, entry.2) = (true, None);
                }
            }
            "teardown" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                    entry.2 = Some(now);
                }
            }
            "end" | "error" => {
                if let Some(position) = self.running.iter().rposition(|(o, _, _)| o.node == node) {
                    let (mut outcome, reached, _) = self.running.remove(position);
                    if kind == "error" {
                        outcome.status = Status::Error(text);
                    }
                    outcome.teardown = reached && kind == "error";
                    self.done.push((outcome, sequence));
                }
            }
            "log" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                    entry.0.logs.push(text);
                }
            }
            "tag" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                    && let Some((key, value)) = text.split_once('\0')
                {
                    entry.0.tags.push((key.to_string(), value.to_string()));
                }
            }
            "skip" | "panic" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                    // The first terminal counts; a panic while it cleans up wins over a skip.
                    entry.0.status = match (&entry.0.status, kind) {
                        (Status::Passed, "skip") => Status::Skipped(text),
                        (Status::Passed | Status::Skipped(_), _) => Status::Panic(text),
                        (Status::Panic(first), _) => Status::Panic(format!("{first}\n{text}")),
                        (other, _) => other.clone(),
                    };
                }
            }
            _ => {}
        }
        self.owners.push(self.running.last().map(|(o, _, _)| o.node));
        self.assign_output();
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
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

/// The outcome of each test and suite, the last one of each node when it
/// ran more than once, in canonical order.
pub fn summary(tree: &Tree, outcomes: &[Outcome]) -> Vec<Outcome> {
    let mut latest: HashMap<u32, Outcome> = HashMap::new();
    for outcome in outcomes {
        let entry = latest.entry(outcome.node).or_insert_with(|| outcome.clone());
        // A suite that failed once stays failed.
        if !(entry.status.failed() && !outcome.status.failed()) {
            *entry = outcome.clone();
        }
    }
    let mut list: Vec<Outcome> = latest.into_values().collect();
    list.sort_by(|a, b| tree.id(a.node).cmp(tree.id(b.node)));
    list
}

/// Whether the outcomes fail the run (testing 9.6).
pub fn failed(outcomes: &[Outcome], deny_skips: bool) -> bool {
    outcomes.iter().any(|o| {
        o.status.failed()
            || matches!(o.status, Status::Blocked(_, false))
            || (deny_skips && matches!(o.status, Status::Skipped(_) | Status::Blocked(_, true)))
    })
}
