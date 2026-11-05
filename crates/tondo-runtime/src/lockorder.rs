//! Locks taken in orders that can deadlock (toolchain 7.5): each lock of
//! `std.sync` a task takes while it holds another is an edge of a graph,
//! and a cycle in it is a deadlock that did not happen yet. With
//! `TONDO_LOCK_ORDER`, in a debugger or with `TONDO_DUMP`, and only for
//! tasks that hold a lock: the rest cost a load.

use super::Str;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

const OFF: u8 = 0;
const SAYS: u8 = 1;
const FAILS: u8 = 2;
static MODE: AtomicU8 = AtomicU8::new(OFF);

#[derive(Default)]
struct Graph {
    /// From each lock, those taken while it was held, with where each of
    /// the two was taken.
    after: HashMap<usize, Vec<(usize, String, String)>>,
    /// The cycles said already, by their first edge.
    said: HashSet<(usize, usize)>,
    found: Vec<String>,
}

static GRAPH: Mutex<Option<Graph>> = Mutex::new(None);

/// Starts looking, as the environment says; `watched` is a program that
/// dumps or is debugged, which only keeps what it finds.
pub(crate) fn start(watched: bool) {
    let mode = match std::env::var("TONDO_LOCK_ORDER").as_deref() {
        Ok("fail") => FAILS,
        Ok("off") => OFF,
        Ok(_) => SAYS,
        Err(_) if watched => SAYS,
        Err(_) => OFF,
    };
    MODE.store(mode, Ordering::Relaxed);
}

#[inline]
pub(crate) fn on() -> bool {
    MODE.load(Ordering::Relaxed) != OFF
}

fn place(location: *const Str) -> String {
    match location.is_null() {
        true => "somewhere".to_string(),
        false => String::from_utf8_lossy(unsafe { super::str_bytes(location) }).into_owned(),
    }
}

/// A task that holds `held` is about to take `object`: says what cycle that
/// closes, once, as a panic to raise when the tests asked for that.
#[cold]
pub(crate) fn taking(
    object: usize,
    exclusive: bool,
    location: *const Str,
    held: &[(usize, bool, *const Str)],
) -> Option<String> {
    let mut graph = GRAPH.lock().unwrap_or_else(|e| e.into_inner());
    let graph = graph.get_or_insert_with(Graph::default);
    let mut found = None;
    // Two readers do not wait for each other.
    for &(lock, _, taken) in held.iter().filter(|&&(lock, alone, _)| lock != object && (alone || exclusive)) {
        let known = graph.after.get(&lock).is_some_and(|after| after.iter().any(|edge| edge.0 == object));
        if known {
            continue;
        }
        let way = path(graph, object, lock);
        graph.after.entry(lock).or_default().push((object, place(taken), place(location)));
        if let Some(way) = way.filter(|_| graph.said.insert((lock, object))) {
            let here = format!("{} then {}", place(taken), place(location));
            let text =
                format!("locks taken in orders that can deadlock: here {here}, and elsewhere {}", way.join(", "));
            graph.found.push(text.clone());
            found = Some(text);
        }
    }
    let text = found?;
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "tondo runtime: {text}");
    (MODE.load(Ordering::Relaxed) == FAILS).then_some(text)
}

/// The edges from `from` to `to`, each in words.
fn path(graph: &Graph, from: usize, to: usize) -> Option<Vec<String>> {
    let (mut seen, mut left) = (HashSet::new(), vec![(from, Vec::new())]);
    while let Some((lock, way)) = left.pop() {
        if lock == to {
            return Some(way);
        }
        if !seen.insert(lock) {
            continue;
        }
        for (next, first, then) in graph.after.get(&lock).into_iter().flatten() {
            let mut longer: Vec<String> = way.clone();
            longer.push(format!("{first} then {then}"));
            left.push((*next, longer));
        }
    }
    None
}

/// A lock goes: another may come at its address.
pub(crate) fn forget(object: usize) {
    if let Some(graph) = GRAPH.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        graph.after.remove(&object);
        graph.after.values_mut().for_each(|after| after.retain(|edge| edge.0 != object));
    }
}

/// What it found, for a dump: a thread that stopped may hold the graph.
pub(crate) fn found() -> Vec<String> {
    GRAPH.try_lock().ok().and_then(|graph| graph.as_ref().map(|graph| graph.found.clone())).unwrap_or_default()
}
