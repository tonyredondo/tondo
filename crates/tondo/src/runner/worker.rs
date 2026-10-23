//! The worker processes that run tests (testing 7.1). A worker runs the
//! tests of its plan one after another (see the runtime's `testing`
//! module) and reports each entry through frames on stderr. A panic, a
//! skip, a timeout or a crash ends the worker; another starts with the
//! rest of the plan and runs again the setups its suites need.

use super::store::{Artifacts, Snapshots};
use super::{Artifact, Attempt, Context, Options, Snapshot, Status, Tree, VirtualClock};
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
    pub update: bool,
}

/// What every worker of a run shares: the executable, the project it runs
/// in, the tree, the timeout and the stores.
pub(super) struct Run<'a> {
    pub executable: &'a Path,
    pub dir: &'a Path,
    pub tree: &'a Tree,
    pub options: &'a Options,
    pub stores: Stores<'a>,
}

/// Runs each unit's tests, starting each unit in a new worker, `--jobs`
/// units at a time; `report` sees each attempt as it is known.
pub(super) fn execute(run: &Run, units: Vec<(Vec<u32>, Context)>, mut report: impl FnMut(&Attempt)) -> Vec<Attempt> {
    let threads = run.options.jobs.max(1).min(units.len());
    let queue = Mutex::new(VecDeque::from(units));
    let (sender, receiver) = mpsc::channel();
    let mut attempts = Vec::new();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (sender, queue) = (sender.clone(), &queue);
            scope.spawn(move || {
                while let Some((plan, context)) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() {
                    lane(run, plan, context, &sender);
                }
            });
        }
        drop(sender);
        for attempt in receiver {
            report(&attempt);
            attempts.push(attempt);
        }
    });
    attempts
}

/// Runs a plan in workers, one after another, until each test has an
/// outcome.
fn lane(run: &Run, plan: Vec<u32>, context: Context, sender: &mpsc::Sender<Attempt>) {
    let mut remaining: VecDeque<u32> = plan.into();
    while !remaining.is_empty() {
        let before = remaining.len();
        if let Err(message) = worker(run, &mut remaining, context, sender) {
            // The worker could not start: every test fails alike.
            for test in remaining.drain(..) {
                let _ = sender.send(Attempt::new(test, Status::Crashed(message.clone()), context));
            }
        }
        if remaining.len() == before
            && let Some(test) = remaining.pop_front()
        {
            let message = "the test process ended before it ran a test".to_string();
            let _ = sender.send(Attempt::new(test, Status::Crashed(message), context));
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
    /// Outcomes ended by a frame, until stdout reaches that frame.
    done: Vec<(Attempt, u64)>,
}

fn worker(
    run: &Run,
    remaining: &mut VecDeque<u32>,
    context: Context,
    sender: &mpsc::Sender<Attempt>,
) -> Result<(), String> {
    let (executable, dir, tree, timeout, stores) =
        (run.executable, run.dir, run.tree, run.options.timeout, &run.stores);
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
        .env("TONDO_TEST_SNAPSHOTS", stores.entries)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stores.update {
        command.env("TONDO_TEST_UPDATE", "1");
    }
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
    if let Some((mut outcome, reached, started)) = session.running.pop() {
        if let Some(started) = started {
            outcome.duration += Instant::now() - started;
        }
        if outcome.status == Status::Passed {
            outcome.status = if timed_out { Status::Timeout } else { Status::Crashed(describe(status)) };
        }
        outcome.teardown = reached;
        send(tree, remaining, outcome, sender);
    }
    // The suites around it ended with the worker, their setup done.
    while let Some((mut outcome, _, started)) = session.running.pop() {
        if let Some(started) = started {
            outcome.duration += Instant::now() - started;
        }
        send(tree, remaining, outcome, sender);
    }
    Ok(())
}

/// Reports an outcome and takes its tests out of the plan: a test that
/// ran, or the tests a suite that failed or skipped in its setup blocks.
fn send(tree: &Tree, remaining: &mut VecDeque<u32>, outcome: Attempt, sender: &mpsc::Sender<Attempt>) {
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
    let context = Context { round: outcome.round, iteration: outcome.iteration, unit: outcome.unit };
    let _ = sender.send(outcome);
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
        let _ = sender.send(Attempt::new(node_blocked, Status::Blocked(node, skipped), context));
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
        let name = match signal {
            4 => "SIGILL",
            6 => "SIGABRT",
            7 if cfg!(target_os = "linux") => "SIGBUS",
            10 if cfg!(target_os = "macos") => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            11 => "SIGSEGV",
            15 => "SIGTERM",
            _ => return format!("the test process ended by signal {signal}"),
        };
        return format!("the test process ended by signal {signal} ({name})");
    }
    format!("the test process exited with status {}", status.code().unwrap_or(-1))
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
        } else if let Some((done, _)) = self.done.iter_mut().rev().find(|(o, _)| Some(o.node) == owner) {
            done.stdout.extend_from_slice(text);
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
        }
    }

    fn frame(&mut self, kind: &str, node: u32, payload: &[u8]) {
        let sequence = self.owners.len() as u64;
        let text = String::from_utf8_lossy(payload).into_owned();
        let now = Instant::now();
        match kind {
            "start" => self.running.push((Attempt::new(node, Status::Passed, self.context), false, Some(now))),
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
