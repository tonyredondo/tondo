//! What a dump shows of the scheduler (toolchain 7.15): where each ready
//! task waits, what each worker runs, and the monitor, from the look the
//! checker takes (`checker::looked`); and, for a dump on demand, the tasks
//! that stayed ready through its looks while a worker slept.

use super::checker::{Look, Place, looked};
use super::*;
use std::collections::HashMap;
use std::fmt::Write;

/// Looks a dump on demand takes, a millisecond apart, before it says that a
/// task ready while a worker sleeps is a wake the scheduler lost: its own
/// unit, as the checker's, and not another thread's time.
pub(crate) const LOST_LOOKS: u64 = 100;

/// Where a ready task waits, as a dump says it.
pub(crate) struct Ready {
    pub(crate) place: &'static str,
    pub(crate) worker: Option<usize>,
    /// Since it was put beside its worker.
    pub(crate) waited_ms: Option<u64>,
    /// Since no other worker has taken from its worker's queue: since the
    /// queue filled from empty, or since another worker last took from it.
    pub(crate) untaken_ms: Option<u64>,
    /// Looks in a row it stayed ready where it was while a worker slept.
    pub(crate) unmoved_looks: Option<u64>,
}

impl Ready {
    /// As JSON, with the fields it has.
    pub(crate) fn json(&self) -> String {
        let field = |name: &str, value: Option<u64>| value.map_or(String::new(), |v| format!(",\"{name}\":{v}"));
        format!(
            "{{\"in\":\"{}\"{}{}{}{}}}",
            self.place,
            field("worker", self.worker.map(|w| w as u64)),
            field("waited_ms", self.waited_ms),
            field("untaken_ms", self.untaken_ms),
            field("unmoved_looks", self.unmoved_looks)
        )
    }
}

/// What the checker counted of the ready tasks when it broke its rule of a
/// task ready while a worker sleeps, for the dump of its panic.
static JUDGED: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());

pub(super) fn judged(looks: Vec<(usize, u64)>) {
    *JUDGED.lock().unwrap_or_else(|e| e.into_inner()) = looks;
}

/// The tasks a look finds ready while a worker sleeps, and where.
fn idle_ready(look: &Look) -> Vec<(usize, Place)> {
    if look.idle == 0 { Vec::new() } else { look.ready.clone() }
}

/// The tasks ready while a worker sleeps, and through how many looks in a
/// row of `LOST_LOOKS`, a millisecond apart, each stayed so where it was:
/// for a dump on demand, from a thread of its own, before the world stops.
pub(crate) fn unmoved_looks() -> Vec<(usize, u64)> {
    let mut counted: Vec<(usize, Place, u64)> =
        idle_ready(&looked()).into_iter().map(|(task, place)| (task, place, 1)).collect();
    for look in 1..LOST_LOOKS {
        if !counted.iter().any(|&(.., looks)| looks == look) {
            break;
        }
        // The pace of looking, not a wait for something to happen.
        std::thread::sleep(Duration::from_millis(1));
        let now = idle_ready(&looked());
        for (task, place, looks) in counted.iter_mut().filter(|(.., looks)| *looks == look) {
            if now.contains(&(*task, *place)) {
                *looks += 1;
            }
        }
    }
    counted.into_iter().map(|(task, _, looks)| (task, looks)).collect()
}

/// The scheduler as a dump shows it, while the world stands: each ready
/// task's place by its number, with the times its slot keeps when `timed`
/// and the looks of `unmoved`, or of the checker at its panic; and the
/// `scheduler` object, of the workers, each with the task of `shots` found
/// running on it, and the monitor.
pub(crate) fn viewed(
    shots: &[Shot],
    timed: bool,
    unmoved: &[(usize, u64)],
    panicked: bool,
) -> (HashMap<u64, Ready>, String) {
    let look = looked();
    let judged = if panicked { JUDGED.lock().unwrap_or_else(|e| e.into_inner()).clone() } else { Vec::new() };
    let now = workers::nanos();
    let ms = |since: u64| now.saturating_sub(since) / 1_000_000;
    let slots: Vec<&Next> = nexts().collect();
    // A task is read while the set holds it: one a dump that could not stop
    // the world sees may end meanwhile.
    let alive = tasks();
    let id = |task: *mut Task| alive.iter().any(|&t| t as *mut Task == task).then(|| unsafe { (*task).id });
    let mut ready = HashMap::new();
    for &(task, place) in &look.ready {
        let (name, worker) = match place {
            Place::Next(slot) => ("next", Some(slot)),
            Place::Own(slot) => ("own", Some(slot)),
            Place::Shared => ("shared", None),
        };
        let stamp = |pick: fn(&Next) -> u64| worker.and_then(|slot| slots.get(slot)).map(|next| ms(pick(next)));
        let waited_ms = matches!(place, Place::Next(_)).then(|| stamp(|n| n.since.load(Ordering::Relaxed))).flatten();
        let untaken_ms = matches!(place, Place::Own(_)).then(|| stamp(|n| n.waiting.load(Ordering::Relaxed))).flatten();
        let looks = unmoved.iter().chain(&judged).find(|(t, _)| *t == task).map(|&(_, looks)| looks);
        let Some(id) = id(task as *mut Task) else { continue };
        let (waited_ms, untaken_ms) = if timed { (waited_ms, untaken_ms) } else { (None, None) };
        ready.insert(id, Ready { place: name, worker, waited_ms, untaken_ms, unmoved_looks: looks });
    }
    let mut workers = Vec::new();
    for (slot, state) in look.slots.iter().enumerate().filter(|(_, state)| state.alive) {
        let runner = slots.get(slot).map_or(std::ptr::null_mut(), |next| next.runner.load(Ordering::Acquire));
        let on = |shot: &&Shot| shot.runner != 0 && shot.runner == runner as usize;
        let task = shots.iter().find(on).map_or(String::new(), |shot| format!("\"task\":{},", shot.id));
        workers.push(format!(
            "{{\"worker\":{slot},{task}\"turns\":{},\"held\":{},\"beside\":{},\"in_c\":{},\"replaced\":{}}}",
            state.turns, state.held, state.beside, state.in_c, state.replaced
        ));
    }
    let sought: Vec<String> = look.sought.iter().map(usize::to_string).collect();
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"workers\":[{}],\"asleep\":{},\"told\":{},\"retiring\":{},\"standing_in\":{},\"monitor\":\"{}\",\"sought\":[{}],\"timer_due\":{}}}",
        workers.join(","),
        look.idle,
        look.told,
        look.surplus,
        look.standing_in,
        if look.monitor_asleep { "asleep" } else { "watching" },
        sought.join(","),
        look.due.is_some()
    );
    drop(alive);
    (ready, out)
}
