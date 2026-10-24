//! The runner inside a test build (testing 7). `tondo test` starts the
//! executable as a worker with a plan, the tests to run in order; the
//! worker runs them one after another, entering and leaving their suites,
//! and reports each entry through frames on stderr. A panic, a skip or a
//! crash ends the worker, and `tondo test` starts another for the rest of
//! the plan.
//!
//! A frame is `\x1e<nonce> <sequence> <kind> <node> <length>\n` and then
//! `length` bytes. Each frame also writes `\x1e<nonce> <sequence>\n` to
//! stdout, so that `tondo test` gives each entry the output written before
//! it.

use crate::{ArrayHeader, MapObject, Str, str_bytes};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A node of the tree, as the compiler lays it out.
#[repr(C)]
pub struct Node {
    parent: u64,
    suite: u64,
    id: *const Str,
}

/// A member a suite gives the runner: its node, and the glue that runs
/// its `fn()` value.
#[repr(C)]
pub struct Member {
    node: u64,
    glue: unsafe extern "C" fn(*mut u8),
    object: *mut u8,
}

/// The code of `testing.skip`'s panic (see the compiler's `hir::SKIP`).
pub const SKIP: u32 = 2000;

struct Runner {
    ids: Vec<String>,
    parents: Vec<u32>,
    suites: Vec<bool>,
    /// The tests still to run.
    plan: VecDeque<u32>,
    nonce: String,
    sequence: u64,
    /// The suites entered and the test that runs, innermost last.
    running: Vec<u32>,
    /// The error the entry that just ended returned.
    error: Option<String>,
    /// Where the last error left a test or a setup.
    location: Option<String>,
    /// Suites whose setup reached their members.
    reached: Vec<bool>,
    /// The tags of each entry.
    tags: HashMap<u32, HashMap<String, String>>,
    /// The names an entry attached and the snapshots it checked.
    attached: HashSet<(u32, String)>,
    checked: HashSet<(u32, String)>,
    /// The snapshot store, by test or suite and name, and whether this run
    /// updates it instead of checking against it.
    snapshots: HashMap<(String, String), String>,
    update: bool,
}

static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);

/// Whether the process runs tests.
pub(crate) static ACTIVE: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Runner) -> R) -> Option<R> {
    RUNNER.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(f)
}

impl Runner {
    fn frame(&mut self, kind: &str, node: u32, payload: &[u8]) {
        crate::flush_stdout();
        self.sequence += 1;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "\x1e{} {}", self.nonce, self.sequence);
        let _ = out.flush();
        let mut frame = format!("\x1e{} {} {kind} {node} {}\n", self.nonce, self.sequence, payload.len()).into_bytes();
        frame.extend_from_slice(payload);
        let _ = std::io::stderr().lock().write_all(&frame);
    }

    /// Whether `node` is `ancestor` or inside it.
    fn within(&self, mut node: u32, ancestor: u32) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            if node == 0 {
                return false;
            }
            node = self.parents[node as usize];
        }
    }

    /// The entry whose code runs, or the root.
    fn current(&self) -> u32 {
        self.running.last().copied().unwrap_or(0)
    }
}

/// Called before the tests run, with the tree.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_start(table: *const Node, count: usize) {
    let nodes = unsafe { std::slice::from_raw_parts(table, count) };
    let parents: Vec<u32> = nodes.iter().map(|n| n.parent as u32).collect();
    let suites: Vec<bool> = nodes.iter().map(|n| n.suite != 0).collect();
    // The plan, or every test when the executable runs on its own.
    let plan = match std::env::var_os("TONDO_TEST_PLAN") {
        Some(path) => std::fs::read_to_string(path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .filter(|&n: &u32| (n as usize) < count)
            .collect(),
        None => (1..count as u32).filter(|&n| !suites[n as usize]).collect(),
    };
    let nonce = std::env::var("TONDO_TEST_NONCE").unwrap_or_else(|_| "tondo".into());
    let ids = nodes.iter().map(|n| String::from_utf8_lossy(unsafe { str_bytes(n.id) }).into_owned()).collect();
    let snapshots = std::env::var_os("TONDO_TEST_SNAPSHOTS").map(|p| read_store(&p)).unwrap_or_default();
    let runner = Runner {
        ids,
        attached: HashSet::new(),
        checked: HashSet::new(),
        snapshots,
        update: std::env::var_os("TONDO_TEST_UPDATE").is_some(),
        reached: vec![false; count],
        parents,
        suites,
        plan,
        nonce,
        sequence: 0,
        running: Vec::new(),
        error: None,
        location: None,
        tags: HashMap::new(),
    };
    *RUNNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(runner);
    ACTIVE.store(true, Ordering::Relaxed);
    if std::env::var_os("TONDO_TEST_PLAN").is_some() {
        stop_on_request();
    }
}

/// Under `tondo test`, `SIGTERM` is its request to stop (testing 7.10):
/// every task is cancelled, the one that runs a test too, so their
/// `defer`s run, and the worker ends without starting another test.
#[cfg(unix)]
fn stop_on_request() {
    static PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn requested(_: i32) {
        let byte = 1u8;
        unsafe { libc::write(PIPE.load(Ordering::Relaxed), (&raw const byte).cast(), 1) };
    }
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    PIPE.store(fds[1], Ordering::Relaxed);
    let listener = std::thread::Builder::new().stack_size(64 * 1024).spawn(move || {
        let mut byte = 0u8;
        while unsafe { libc::read(fds[0], (&raw mut byte).cast(), 1) } != 1 {}
        crate::tasks::abandon(128 + libc::SIGTERM);
    });
    if listener.is_ok() {
        unsafe { libc::signal(libc::SIGTERM, requested as *const () as libc::sighandler_t) };
    }
}

#[cfg(not(unix))]
fn stop_on_request() {}

/// Runs the members of a suite, or of the root, that the plan holds
/// next, until the next test of the plan is not inside them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_members(members: *const Member, count: usize) {
    let members = unsafe { std::slice::from_raw_parts(members, count) };
    // The suite whose setup just ended: its members begin.
    let suite = with(|r| {
        let suite = members.first().map_or(0, |m| r.parents[m.node as usize]);
        if suite != 0 {
            r.reached[suite as usize] = true;
            r.frame("members", suite, b"");
        }
        suite
    });
    let Some(suite) = suite else { return };
    // A worker asked to stop starts nothing more.
    while !crate::tasks::cancelled() {
        let next = with(|r| {
            let next = *r.plan.front()?;
            let member = members.iter().position(|m| r.within(next, m.node as u32))?;
            let node = members[member].node as u32;
            if !r.suites[node as usize] {
                r.plan.pop_front();
            }
            r.running.push(node);
            r.error = None;
            r.location = None;
            r.reached[node as usize] = false;
            r.frame("start", node, b"");
            Some((member, node))
        });
        let Some(Some((member, node))) = next else { break };
        let state = (!with(|r| r.suites[node as usize]).unwrap_or(true)).then(State::save);
        let member = &members[member];
        unsafe { (member.glue)(member.object) };
        // A panic in another task cancelled this one: its cleanups run as
        // it returns, and the worker ends.
        if crate::tasks::cancelled() {
            return;
        }
        let leaked = state.map(State::restore).unwrap_or_default();
        let dirty = !leaked.is_empty();
        with(|r| {
            r.running.pop();
            // Tasks the test left running fail it, unless it failed already.
            if dirty && r.error.is_none() {
                let count = match leaked.len() {
                    1 => "1 task".to_string(),
                    n => format!("{n} tasks"),
                };
                let message = format!(
                    "the test left {count} running; wait for it, cancel it, or close what it waits on{}",
                    leaked.join("\n")
                );
                r.error = Some(format!("\0{message}\0"));
            }
            let failed = r.error.take().or_else(|| {
                (r.suites[node as usize] && !r.reached[node as usize])
                    .then(|| "\0the setup ended before its members\0".to_string())
            });
            match failed {
                Some(error) => {
                    // A suite that fails in its setup blocks its tests.
                    if r.suites[node as usize] {
                        let blocked: Vec<u32> = r.plan.iter().copied().filter(|&t| r.within(t, node)).collect();
                        r.plan.retain(|t| !blocked.contains(t));
                    }
                    r.frame("error", node, error.as_bytes());
                }
                None => r.frame("end", node, b""),
            }
        });
        // A test that left tasks running ends this worker; another runs
        // the rest of the plan.
        if dirty {
            crate::flush_stdout();
            std::process::exit(0);
        }
    }
    if suite != 0 {
        with(|r| r.frame("teardown", suite, b""));
    }
}

/// What a test may change in the process that the next one should not
/// see: the working directory, the environment and the tasks alive.
struct State {
    directory: Option<std::path::PathBuf>,
    environment: Vec<(OsString, OsString)>,
    tasks: std::collections::HashSet<usize>,
}

impl State {
    fn save() -> State {
        State {
            directory: std::env::current_dir().ok(),
            environment: std::env::vars_os().collect(),
            tasks: crate::tasks::alive(),
        }
    }

    /// Restores the directory and the environment; the tasks the test
    /// started that are still alive, each where it waits and was started.
    fn restore(self) -> Vec<String> {
        if let Some(directory) = &self.directory {
            let _ = std::env::set_current_dir(directory);
        }
        let saved: HashMap<OsString, OsString> = self.environment.into_iter().collect();
        for (name, value) in std::env::vars_os() {
            match saved.get(&name) {
                // SAFETY: between tests no Tondo code of the test runs; the
                // runtime's other threads do not read the environment.
                Some(old) if *old != value => unsafe { std::env::set_var(&name, old) },
                Some(_) => {}
                None => unsafe { std::env::remove_var(&name) },
            }
        }
        for (name, value) in &saved {
            if std::env::var_os(name).is_none() {
                unsafe { std::env::set_var(name, value) };
            }
        }
        // A finished task is freed just after it ends.
        for _ in 0..100 {
            if crate::tasks::alive().iter().all(|task| self.tasks.contains(task)) {
                return Vec::new();
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        crate::tasks::leaked(&self.tasks)
    }
}

/// The error a test or a setup returned, as `main` reports it: its type,
/// its description and where it left, separated by NUL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_error(description: *const Str, error_type: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(description) }).into_owned();
    let error_type = String::from_utf8_lossy(unsafe { str_bytes(error_type) }).into_owned();
    with(|r| {
        let location = r.location.take().unwrap_or_default();
        r.error = Some(format!("{error_type}\0{text}\0{location}"));
    });
}

/// Where an error leaves a test or a setup: its `fail` or its `?`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_where(location: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(location) }).into_owned();
    with(|r| r.location = Some(text));
}

/// `testing.log(message)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_log(message: *const Str) {
    let bytes = unsafe { str_bytes(message) };
    with(|r| {
        let node = r.current();
        r.frame("log", node, bytes)
    });
}

/// `testing.tags(values)`: all of them or, when a key already has another
/// value, none and the panic `P2002` for the smallest such key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_tags(map: *mut MapObject) {
    let pairs = pairs(map);
    let conflict = with(|r| {
        let node = r.current();
        let tags = r.tags.entry(node).or_default();
        let conflict = pairs.iter().filter(|(k, v)| tags.get(k).is_some_and(|old| old != v)).map(|(k, _)| k).min();
        if let Some(key) = conflict {
            return Some(key.clone());
        }
        let mut added = Vec::new();
        for (key, value) in &pairs {
            if tags.insert(key.clone(), value.clone()).is_none() {
                added.push((key.clone(), value.clone()));
            }
        }
        for (key, value) in added {
            r.frame("tag", node, format!("{key}\0{value}").as_bytes());
        }
        None
    });
    if let Some(Some(key)) = conflict {
        let message = crate::str_new(format!("`{key}` already has another value").as_bytes());
        let location = crate::str_new(b"testing.tags");
        unsafe { crate::tondo_rt_panic(2002, message, location) };
    }
}

/// The snapshot entries `tondo test` gives the worker: records of three
/// lengths on a line, then the identity, name and value they measure.
fn read_store(path: &std::ffi::OsStr) -> HashMap<(String, String), String> {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut entries = HashMap::new();
    let mut rest = &bytes[..];
    while let Some(end) = rest.iter().position(|&b| b == b'\n') {
        let lengths: Vec<usize> =
            String::from_utf8_lossy(&rest[..end]).split(' ').filter_map(|n| n.parse().ok()).collect();
        let [id, name, value] = lengths[..] else { break };
        let body = &rest[end + 1..];
        if body.len() < id + name + value {
            break;
        }
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        entries.insert((text(&body[..id]), text(&body[id..id + name])), text(&body[id + name..id + name + value]));
        rest = &body[id + name + value..];
    }
    entries
}

/// Why a name cannot name an artifact or a snapshot (testing 9.7).
fn invalid_name(name: &str) -> Option<String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return Some(format!("{name:?} is not a name: it is empty, or has `/`, `\\` or a control character"));
    }
    None
}

/// A media type: `token/token`, without parameters.
fn valid_media(media: &str) -> bool {
    let token =
        |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&'*+-.^_`|~".contains(&b));
    media.split_once('/').is_some_and(|(a, b)| token(a) && token(b))
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

/// The entries of a `Map[String, String]`, in order.
fn pairs(map: *mut MapObject) -> Vec<(String, String)> {
    if map.is_null() {
        return Vec::new();
    }
    let map = unsafe { &*map };
    (0..map.len())
        .map(|index| {
            let entry = unsafe { map.entries.as_ptr().add(index * map.entry_size) };
            let key = unsafe { *(entry as *const *const Str) };
            let value = unsafe { *(entry.add(map.value_offset) as *const *const Str) };
            (text(key), text(value))
        })
        .collect()
}

/// The elements of an `Array[String]`.
fn strings(array: *mut ArrayHeader) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    (0..unsafe { (*array).len }).map(|i| text(unsafe { *(crate::element(array, 8, i) as *const *const Str) })).collect()
}

fn fail(code: u32, message: String, location: *const Str) -> ! {
    let message = crate::str_new(message.as_bytes());
    unsafe { crate::tondo_rt_panic(code, message, location) }
}

/// `testing.attach(name, mediaType, data)` (testing 9.7): `tondo test`
/// stores the bytes; a bad or repeated name is the panic `P2006`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_attach(
    name: *const Str,
    media: *const Str,
    data: *mut ArrayHeader,
    location: *const Str,
) {
    let (name, media) = (text(name), text(media));
    if let Some(why) = invalid_name(&name) {
        fail(2006, why, location);
    }
    if !valid_media(&media) {
        fail(2006, format!("{media:?} is not a media type such as \"application/json\""), location);
    }
    let bytes = if data.is_null() { Vec::new() } else { unsafe { crate::io::array_bytes(data) } };
    let repeated = with(|r| {
        let node = r.current();
        if !r.attached.insert((node, name.clone())) {
            return true;
        }
        let mut payload = format!("{name}\0{media}\0").into_bytes();
        payload.extend_from_slice(&bytes);
        r.frame("attach", node, &payload);
        false
    });
    if repeated == Some(true) {
        fail(2006, format!("an artifact named {name:?} is already attached here"), location);
    }
}

/// `testing.snapshot(name, value, ignore:, matches:, replace:)` (testing
/// 9.7): the value is a `String`, or its JSON when `json`; compares it with
/// the store, or records it when updating. A missing or different value,
/// or one that does not have the form `matches` asks for, is the panic
/// `P2007`; a bad or repeated name, or a bad path or pattern, `P2008`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_test_snapshot(
    name: *const Str,
    value: *const Str,
    json: u8,
    ignore: *mut ArrayHeader,
    matches: *mut MapObject,
    replace: *mut MapObject,
    location: *const Str,
) {
    let name = text(name);
    if let Some(why) = invalid_name(&name) {
        fail(2008, why, location);
    }
    let actual =
        match crate::snapshot::render(text(value), json != 0, &strings(ignore), &pairs(matches), &pairs(replace)) {
            Ok(actual) => actual,
            Err(crate::snapshot::Refused::Mismatch(why)) => {
                fail(2007, format!("in the snapshot {name:?}, {why}"), location)
            }
            Err(crate::snapshot::Refused::Invalid(why)) => {
                fail(2008, format!("in the snapshot {name:?}, {why}"), location)
            }
        };
    let outcome = with(|r| {
        let node = r.current();
        if !r.checked.insert((node, name.clone())) {
            return Err(format!("the snapshot {name:?} is already checked here"));
        }
        let expected = r.snapshots.get(&(r.ids[node as usize].clone(), name.clone()));
        let status = match (expected, r.update) {
            (Some(expected), _) if *expected == actual => "matched",
            (None, false) => "missing",
            (Some(_), false) => "mismatched",
            (None, true) => "created",
            (Some(_), true) => "updated",
        };
        r.frame("snapshot", node, format!("{status}\0{name}\0{actual}").as_bytes());
        Ok(status)
    });
    match outcome {
        Some(Err(why)) => fail(2008, why, location),
        Some(Ok("missing")) => {
            fail(2007, format!("there is no snapshot {name:?}; `tondo test --update-snapshots` records it"), location)
        }
        Some(Ok("mismatched")) => fail(
            2007,
            format!("the snapshot {name:?} differs; `tondo test --update-snapshots` records the new value"),
            location,
        ),
        _ => {}
    }
}

/// Reports what a virtual clock did once its closure ends: where it got,
/// and how many times it moved by itself, by `advance` and settled.
pub(crate) fn virtual_time(now: u64, automatic: u64, explicit: u64, settles: u64) {
    with(|r| {
        let node = r.current();
        r.frame("virtual", node, format!("{now} {automatic} {explicit} {settles}").as_bytes());
    });
}

/// Reports the panic or the skip that ends the worker; whether the
/// process runs tests.
pub(crate) fn terminal(code: u32, text: &str) -> bool {
    with(|r| {
        let node = r.current();
        r.frame(if code == SKIP { "skip" } else { "panic" }, node, text.as_bytes());
    })
    .is_some()
}
