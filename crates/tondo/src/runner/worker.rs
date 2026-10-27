//! The worker processes that run tests (testing 7.1). A worker runs the
//! tests of its plan one after another (see the runtime's `testing`
//! module) and reports each entry through frames on stderr. A panic, a
//! skip, a timeout or a crash ends the worker; another starts with the
//! rest of the plan and runs again the setups its suites need.

use super::interrupt;
use super::store::{Artifacts, Snapshots};
use super::{
    Artifact, Attempt, Benchmark, Context, FuzzInput, Fuzzed, Options, Searched, Snapshot, Status, Tree, VirtualClock,
};
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

/// Where attachments go, the snapshots to check, the file the workers
/// read them from, and whether this run updates them.
pub(super) struct Stores<'a> {
    pub artifacts: &'a Artifacts,
    pub snapshots: &'a Snapshots,
    pub entries: &'a Path,
    /// The inputs of the fuzz store, for the worker.
    pub fuzzed: &'a Path,
    pub update: bool,
    /// With coverage, where each worker writes its counts.
    pub counts: Option<&'a Path>,
    /// With profiles, what of and where each worker writes them.
    pub profiles: Option<(crate::profile::Kind, &'a Path)>,
}

/// What every worker of a run shares: the executable, the project it runs
/// in, the tree, the timeout and the stores.
pub(super) struct Run<'a> {
    pub executable: &'a Path,
    pub dir: &'a Path,
    pub tree: &'a Tree,
    pub options: &'a Options,
    pub stores: Stores<'a>,
    /// How long each benchmark measures for, under `--bench`.
    pub bench: Option<std::time::Duration>,
    /// Under `--fuzz`, the calls the workers search in, and when the
    /// search ends, as milliseconds since the epoch, or `0` for never.
    pub targets: &'a [Target],
    pub until: u64,
    /// While reducing an input that failed: the inputs to try instead.
    pub replay: Option<&'a Replay>,
}

/// The inputs a worker tries in a call, as `tondo test` reduces one that
/// failed there: a file of one input a line.
pub(super) struct Replay {
    pub test: u32,
    pub call: String,
    pub file: std::path::PathBuf,
}

/// A call of `testing.fuzz` the search runs in: its test, its name, its
/// corpus directory and the file that stops its workers.
pub(super) struct Target {
    pub test: u32,
    pub call: String,
    pub corpus: std::path::PathBuf,
    pub stop: std::path::PathBuf,
}

/// What the workers tell as they run: a suite or test that started, in a
/// pass, what it wrote (to stdout or not), the outcome of an attempt, or
/// how the search of a call of `testing.fuzz` goes.
pub(super) enum Note {
    Started(u32, Context),
    Wrote(u32, Context, bool, Vec<u8>),
    Ended(Box<Attempt>),
    Fuzzing(u32, String, super::Searched),
}

/// Runs each unit's tests, starting each unit in a new worker, `--jobs`
/// units at a time; `report` sees each start and attempt as it is known.
pub(super) fn execute(run: &Run, units: Vec<(Vec<u32>, Context)>, mut report: impl FnMut(&Note)) -> Vec<Attempt> {
    // A search runs each of its workers at once.
    let threads = match run.targets.is_empty() {
        true => run.options.jobs.max(1).min(units.len()),
        false => units.len(),
    };
    let queue = Mutex::new(VecDeque::from(units));
    let (sender, receiver) = mpsc::channel();
    let mut attempts = Vec::new();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (sender, queue) = (sender.clone(), &queue);
            scope.spawn(move || {
                while let Some((plan, context)) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
                    && !interrupt::requested()
                {
                    lane(run, plan, context, &sender);
                }
            });
        }
        drop(sender);
        for note in receiver {
            report(&note);
            if let Note::Ended(attempt) = note {
                attempts.push(*attempt);
            }
        }
    });
    attempts
}

/// Runs a plan in workers, one after another, until each test has an
/// outcome.
fn lane(run: &Run, plan: Vec<u32>, context: Context, sender: &mpsc::Sender<Note>) {
    let mut remaining: VecDeque<u32> = plan.into();
    while !remaining.is_empty() && !interrupt::requested() {
        let before = remaining.len();
        let started = worker(run, &mut remaining, context, sender);
        if interrupt::requested() {
            break;
        }
        if let Err(message) = started {
            // The worker could not start: every test fails alike.
            for test in remaining.drain(..) {
                let _ =
                    sender.send(Note::Ended(Box::new(Attempt::new(test, Status::Crashed(message.clone()), context))));
            }
        }
        if remaining.len() == before
            && let Some(test) = remaining.pop_front()
        {
            let message = "the test process ended before it ran a test".to_string();
            let _ = sender.send(Note::Ended(Box::new(Attempt::new(test, Status::Crashed(message), context))));
        }
    }
}

enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Closed,
}

/// A worker's state while `tondo test` reads it.
struct Session<'a> {
    tree: &'a Tree,
    stores: &'a Stores<'a>,
    nonce: Vec<u8>,
    context: Context,
    /// Entries started and not ended, innermost last: each with its outcome
    /// so far, whether a suite reached its members, and when its current
    /// phase began, if it is timed.
    running: Vec<(Attempt, bool, Option<Instant>)>,
    /// The entry that was running after each frame, by its sequence number.
    owners: Vec<Option<u32>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// Output the frames have not yet assigned, by the frame it precedes.
    pending: Vec<(u64, Vec<u8>)>,
    stdout_frames: u64,
    /// Starts and outcomes told by a frame, until stdout reaches that
    /// frame.
    done: Vec<(Note, u64)>,
}

fn worker(
    run: &Run,
    remaining: &mut VecDeque<u32>,
    context: Context,
    sender: &mpsc::Sender<Note>,
) -> Result<(), String> {
    let (executable, dir, tree, mut timeout, stores) =
        (run.executable, run.dir, run.tree, run.options.timeout, &run.stores);
    let plan =
        std::env::temp_dir().join(format!("tondo-plan-{}-{:?}", std::process::id(), std::thread::current().id()));
    let text: Vec<String> = remaining.iter().map(|t| t.to_string()).collect();
    std::fs::write(&plan, text.join("\n")).map_err(|e| format!("cannot write {}: {e}", plan.display()))?;
    let nonce = crate::driver::fingerprint(&[
        &std::process::id().to_le_bytes(),
        format!("{:?}{:?}", Instant::now(), std::thread::current().id()).as_bytes(),
    ]);
    let mut command = match &run.options.launcher {
        Some(launcher) => {
            let mut command = Command::new(launcher);
            command.arg(executable);
            command
        }
        None => Command::new(executable),
    };
    command
        .current_dir(dir)
        .env("TONDO_TEST_PLAN", &plan)
        .env("TONDO_TEST_NONCE", &nonce)
        .env("TONDO_TEST_SNAPSHOTS", stores.entries)
        .env("TONDO_TEST_FUZZ", stores.fuzzed)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stores.update {
        command.env("TONDO_TEST_UPDATE", "1");
    }
    if let Some(counts) = stores.counts {
        command.env("TONDO_TEST_COVERAGE", counts);
    }
    if let Some((kind, dir)) = stores.profiles {
        command.env("TONDO_TEST_PROFILE", format!("{}:{}", kind.name(), dir.display()));
    }
    if let Some(time) = run.bench {
        command.env("TONDO_TEST_BENCH", time.as_secs_f64().to_string());
    }
    // A worker that searches runs its test until the search ends.
    if let Some(target) = context.target.and_then(|t| run.targets.get(t as usize)) {
        let call = super::fuzz::directory(&target.call);
        let (corpus, stop) = (target.corpus.display(), target.stop.display());
        let literals = executable.with_file_name("dictionary");
        let (test, until, literals) = (tree.id(target.test), run.until, literals.display());
        let fuzz = run.options.fuzz.as_ref();
        let (limit, memory) = (fuzz.map_or(1000, |f| f.timeout.as_millis()), fuzz.map_or(2048, |f| f.memory));
        let fields = format!("{test}\t{call}\t{corpus}\t{stop}\t{until}\t{literals}\t{limit}\t{memory}");
        command.env("TONDO_FUZZ", fields);
        if let Some(replay) = run.replay {
            let (test, call) = (tree.id(replay.test), super::fuzz::directory(&replay.call));
            command.env("TONDO_FUZZ_REPLAY", format!("{test}\t{call}\t{}", replay.file.display()));
        }
        timeout = Duration::from_secs(10 * 365 * 24 * 3600);
    }
    if let Some(seed) = run.options.schedule(context.iteration) {
        command.env("TONDO_RT_SCHEDULE", format!("{seed:x}"));
    }
    // Its own process group, so that a timeout ends what it started too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let Some(child) = interrupt::start(&mut command) else { return Ok(()) };
    let mut child = child.map_err(|e| format!("cannot start {}: {e}", executable.display()))?;
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
        tree,
        stores,
        nonce: format!("\x1e{nonce} ").into_bytes(),
        context,
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
        for (note, _) in session.done.extract_if(.., |(_, frame)| *frame <= ready) {
            send(tree, remaining, note, sender);
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let killed = std::os::unix::process::ExitStatusExt::signal(&status) == Some(libc::SIGKILL);
    #[cfg(not(unix))]
    let killed = false;
    interrupt::ended(&child, killed);
    let _ = std::fs::remove_file(&plan);
    session.flush_output();
    for (note, _) in std::mem::take(&mut session.done) {
        send(tree, remaining, note, sender);
    }
    // What the interruption cut has no outcome.
    if interrupt::requested() {
        return Ok(());
    }
    // The entry that was running when the worker ended.
    if let Some((mut outcome, reached, started)) = session.running.pop() {
        if let Some(started) = started {
            outcome.duration += Instant::now() - started;
        }
        if outcome.status == Status::Passed {
            outcome.status = if timed_out {
                Status::Timeout
            } else {
                Status::Crashed(crate::observe::ended(status, "the test process"))
            };
        }
        outcome.teardown = reached;
        send(tree, remaining, Note::Ended(Box::new(outcome)), sender);
    }
    // The suites around it ended with the worker, their setup done.
    while let Some((mut outcome, _, started)) = session.running.pop() {
        if let Some(started) = started {
            outcome.duration += Instant::now() - started;
        }
        send(tree, remaining, Note::Ended(Box::new(outcome)), sender);
    }
    Ok(())
}

/// Reports a start, or an outcome and takes its tests out of the plan: a
/// test that ran, or the tests a suite that failed or skipped in its setup
/// blocks.
fn send(tree: &Tree, remaining: &mut VecDeque<u32>, note: Note, sender: &mpsc::Sender<Note>) {
    let outcome = match note {
        Note::Ended(outcome) => *outcome,
        started => {
            let _ = sender.send(started);
            return;
        }
    };
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
    let context = Context { round: outcome.round, iteration: outcome.iteration, unit: outcome.unit, target: None };
    let _ = sender.send(Note::Ended(Box::new(outcome)));
    // The suites inside that held those tests are blocked too.
    let mut suites: Vec<u32> = Vec::new();
    for &test in &blocked {
        let mut inner = tree.nodes[test as usize].parent;
        while inner != node && inner != 0 {
            if !suites.contains(&inner) {
                suites.push(inner);
            }
            inner = tree.nodes[inner as usize].parent;
        }
    }
    for node_blocked in suites.into_iter().chain(blocked) {
        let _ = sender.send(Note::Ended(Box::new(Attempt::new(node_blocked, Status::Blocked(node, skipped), context))));
    }
}

fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
    }
    let _ = child.kill();
}

impl Session<'_> {
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
        } else if let Some(done) = self.done.iter_mut().rev().find_map(|(note, _)| match note {
            Note::Ended(outcome) if Some(outcome.node) == owner => Some(outcome),
            _ => None,
        }) {
            done.stdout.extend_from_slice(text);
        } else {
            return;
        }
        self.wrote(owner.expect("an owner"), true, text);
    }

    /// Tells what an entry wrote, before its end if that waits.
    fn wrote(&mut self, node: u32, stdout: bool, text: &[u8]) {
        if text.is_empty() {
            return;
        }
        let note = Note::Wrote(node, self.context, stdout, text.to_vec());
        match self.done.iter().position(|(note, _)| matches!(note, Note::Ended(o) if o.node == node)) {
            Some(at) => {
                let sequence = self.done[at].1;
                self.done.insert(at, (note, sequence));
            }
            None => self.done.push((note, self.owners.len() as u64 - 1)),
        }
    }

    /// Output left when the worker ends belongs to what ran last.
    fn flush_output(&mut self) {
        // What stderr kept back in case a marker started there.
        let rest = std::mem::take(&mut self.stderr);
        self.give_stderr(&rest);
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
            self.wrote(self.running[index].0.node, false, text);
        }
    }

    fn frame(&mut self, kind: &str, node: u32, payload: &[u8]) {
        let sequence = self.owners.len() as u64;
        let text = String::from_utf8_lossy(payload).into_owned();
        let now = Instant::now();
        match kind {
            "start" => {
                self.running.push((Attempt::new(node, Status::Passed, self.context), false, Some(now)));
                self.done.push((Note::Started(node, self.context), sequence));
            }
            "members" => {
                if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                    if let Some(started) = entry.2 {
                        entry.0.duration += now - started;
                    }
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
                    let (mut outcome, reached, started) = self.running.remove(position);
                    if let Some(started) = started {
                        outcome.duration += now - started;
                    }
                    if kind == "error" {
                        let mut parts = text.splitn(3, '\0');
                        let error_type = parts.next().unwrap_or_default().to_string();
                        let message = parts.next().unwrap_or_default().to_string();
                        let location = parts.next().filter(|l| !l.is_empty()).map(str::to_string);
                        outcome.status = Status::Error { message, error_type, location };
                    }
                    outcome.teardown = reached && kind == "error";
                    self.done.push((Note::Ended(Box::new(outcome)), sequence));
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
            "attach" => {
                let mut parts = payload.splitn(3, |&b| b == 0);
                let (name, media, bytes) = (parts.next(), parts.next(), parts.next());
                if let (Some(name), Some(media), Some(bytes)) = (name, media, bytes)
                    && let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                {
                    let sha256 = self.stores.artifacts.store(bytes);
                    entry.0.artifacts.push(Artifact {
                        name: String::from_utf8_lossy(name).into_owned(),
                        media_type: String::from_utf8_lossy(media).into_owned(),
                        size: bytes.len(),
                        sha256,
                    });
                }
            }
            "snapshot" => {
                let mut parts = text.splitn(3, '\0');
                let (status, name, actual) = (parts.next(), parts.next(), parts.next());
                if let (Some(status), Some(name), Some(actual)) = (status, name, actual) {
                    let expected = self.stores.snapshots.expected(self.tree.id(node), name);
                    if let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node) {
                        entry.0.snapshots.push(Snapshot {
                            name: name.to_string(),
                            status: status.to_string(),
                            expected,
                            actual: actual.to_string(),
                        });
                    }
                }
            }
            "virtual" => {
                let numbers: Vec<u64> = text.split(' ').filter_map(|n| n.parse().ok()).collect();
                if let [elapsed_ns, automatic, explicit, settles] = numbers[..]
                    && let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                {
                    entry.0.virtual_time.push(VirtualClock { elapsed_ns, automatic, explicit, settles });
                }
            }
            "bench" => {
                let parts: Vec<&str> = text.split('\0').collect();
                if let [name, runs, samples, allocations, bytes] = parts[..]
                    && let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                {
                    entry.0.benchmarks.push(Benchmark {
                        name: name.to_string(),
                        runs: runs.parse().unwrap_or(1),
                        samples: samples.split(' ').filter_map(|s| s.parse().ok()).collect(),
                        allocations: allocations.parse().unwrap_or_default(),
                        bytes: bytes.parse().unwrap_or_default(),
                    });
                    // The timeout is each benchmark's: its time starts again.
                    if let Some(started) = entry.2.as_mut() {
                        entry.0.duration += now - *started;
                        *started = now;
                    }
                }
            }
            "fuzz" => {
                let parts: Vec<&str> = text.split('\0').collect();
                if let [name, inputs, json, file, seed, failed, searched, tried] = parts[..]
                    && let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                {
                    let input = (failed == "1").then(|| FuzzInput {
                        json: json.to_string(),
                        file: (!file.is_empty()).then(|| file.to_string()),
                        seed: seed.parse().ok(),
                        tried: tried.parse().ok(),
                        reduced_from: None,
                    });
                    let (name, inputs, search) =
                        (name.to_string(), inputs.parse().unwrap_or(0), Searched::read(searched));
                    entry.0.fuzz.push(Fuzzed { name, inputs, input, search });
                }
            }
            // An input of the search ran past its limit: the worker ends.
            "limit" => {
                if let Some((kind, message)) = text.split_once('\0')
                    && let Some(entry) = self.running.iter_mut().rev().find(|(o, _, _)| o.node == node)
                {
                    entry.0.status = Status::Limit { timeout: kind == "timeout", message: message.to_string() };
                }
            }
            "fuzzing" => {
                if let Some((name, progress)) = text.split_once('\0')
                    && let Some(searched) = Searched::read(progress)
                {
                    self.done.push((Note::Fuzzing(node, name.to_string(), searched), sequence));
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
