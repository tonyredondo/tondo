//! What the runtime keeps for each thread (docs/refactor.md, phase 1). A task
//! may resume on another thread after any wait, so what a worker keeps is in
//! one `Worker`, lent by `worker`, which never inlines; a value
//! kept per thread elsewhere, in code that runs on a task's stack, is right
//! only while no wait comes between where it is written and where it is
//! read. Each one there is, is here with why that holds; a new one is either
//! a field of `Worker`, or added here with its reason.

use std::path::Path;

/// The thread-locals besides the worker's, by file and name, with why.
const KNOWN: &[(&str, &str, &str)] = &[
    ("tasks/mod.rs", "WORKER", "what a worker keeps: lent by `worker` alone"),
    ("tasks/threads.rs", "RUNTIME_THREAD", "set as a runtime thread starts, read before a task moves off a host's"),
    ("tasks/threads.rs", "KEPT", "what the runtime keeps for the thread, read through `kept`, never inlined"),
    ("tasks/mod.rs", "LOOSE_CLEANUPS", "cleanups registered outside any task, as in unit tests"),
    ("contexts.rs", "SHOWING", "outside any task; a task keeps its own"),
    ("cycles.rs", "OWES", "the thread owes the collector until its next safe point, which it reaches as it runs"),
    ("cycles.rs", "BUSY", "set while this thread collects, which never waits"),
    ("debugger/stopping.rs", "PANIC_REGISTERS", "written and read in one panic's report, which never waits"),
    ("debugger/stopping.rs", "PANIC_NUMBERS", "written and read in one panic's report, which never waits"),
    ("debugger/bindings.rs", "SHOWING", "a view the debugger shows runs to its end"),
    ("diagnostics.rs", "BLOCK", "the threads that serve the tools, which run no task"),
    ("fs.rs", "HOST_ERROR", "the threads that do a task's blocking work, which run no task"),
    ("io.rs", "HELD", "set while a future is polled, which never waits: what it lets go waits for the poll's end"),
    ("json.rs", "SPARE", "readers and writers let go, taken and given back by calls that never wait, on any thread"),
    (
        "tasks/group.rs",
        "SPARE_GROUPS",
        "groups let go, taken and given back by calls that never wait, through functions never inlined, on any thread",
    ),
    (
        "tasks/join.rs",
        "SPARE_BLOCKS",
        "blocks of tasks that ended, taken and given back by calls that never wait, on any thread",
    ),
    ("heap.rs", "UNTIL", "a sample of the heap per thread, whichever task allocates"),
    ("heap.rs", "SEED", "a sample of the heap per thread, whichever task allocates"),
    ("heap.rs", "TRACKING", "set inside the registry of objects, which never waits"),
    ("heap.rs", "QUIET", "set while the debugger works at a probe, which never waits"),
    ("panics.rs", "STAGED", "a panic's code and data, staged by calls that never wait until it is raised"),
    ("recorder.rs", "BUFFER", "events kept per thread, whichever task they are of"),
    ("recorder.rs", "THREAD", "a thread's number in a trace"),
    ("regex.rs", "ERROR", "written by a call and read by the next, with no wait between"),
    ("replay.rs", "PROGRAM", "a recording's threads, of the program and of C, not its tasks"),
    ("replay.rs", "INSIDE", "set inside a wrapper of a call of C, which a task does not leave"),
    ("replay.rs", "NUMBER", "a recording's threads, of the program and of C, not its tasks"),
    ("replay.rs", "HOLDS", "a recording's threads, of the program and of C, not its tasks"),
    ("replay.rs", "CALLED", "a recording's threads, of the program and of C, not its tasks"),
    ("replay.rs", "MADE", "a recording's threads, of the program and of C, not its tasks"),
    ("replay/calls.rs", "ENTRY", "a directory read in a replay by the wrappers of C, one call at a time"),
    ("shared.rs", "LOCAL", "a thread's place in the reclamation of shared collections, whichever task runs"),
    ("shared.rs", "DEPTH", "set while a shared collection is read, which never waits"),
    ("trace.rs", "LAST", "where the thread's latest tasks were started, for a fault's report"),
];

/// The files of Rust under `dir`, however deep.
fn sources(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

#[test]
fn a_thread_keeps_nothing_but_what_is_known() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tondo-runtime/src");
    let mut files = Vec::new();
    sources(&dir, &mut files);
    let mut found = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        // By its path from src, as `tasks/mod.rs`.
        let file = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        let mut rest = text.as_str();
        while let Some(at) = rest.find("thread_local! {") {
            // The block ends at the brace as far in as its start.
            let line = rest[..at].rsplit('\n').next().unwrap_or("");
            let indent = &line[..line.len() - line.trim_start().len()];
            rest = &rest[at..];
            let end = rest.find(&format!("\n{indent}}}")).unwrap_or(rest.len());
            for line in rest[..end].lines() {
                // With its visibility, if any, as `pub(super) static`.
                let line = line.trim_start();
                let line = line.strip_prefix("pub").map_or(line, |l| l.split_once(' ').map_or(l, |(_, l)| l));
                if let Some(name) = line.strip_prefix("static ").and_then(|l| l.split(':').next()) {
                    found.push((file.clone(), name.trim().to_string()));
                }
            }
            rest = &rest[end..];
        }
    }
    let unknown: Vec<String> = found
        .iter()
        .filter(|(file, name)| !KNOWN.iter().any(|(f, n, _)| f == file && n == name))
        .map(|(file, name)| format!("{file}: {name}"))
        .collect();
    assert!(
        unknown.is_empty(),
        "thread-locals of the runtime that are not known: {unknown:?}\na task may resume on another thread after \
         any wait: what a worker keeps goes in `Worker`, in tasks/mod.rs, lent by `worker`; one that must be \
         per thread is added to KNOWN, in this test, with why no wait comes between where it is written and read"
    );
    let gone: Vec<&str> = KNOWN
        .iter()
        .filter(|(f, n, _)| !found.iter().any(|(file, name)| file == f && name == n))
        .map(|k| k.1)
        .collect();
    assert!(gone.is_empty(), "KNOWN names thread-locals the runtime no longer has: {gone:?}");
}
