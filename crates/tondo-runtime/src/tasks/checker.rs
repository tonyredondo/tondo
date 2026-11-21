//! `TONDO_CHECK_SCHEDULER=1` (toolchain 7.17): a thread that looks at the
//! scheduler every millisecond and holds what language 16.14 promises. It
//! counts in its own looks, so that a process the system stops breaks no
//! rule; a rule broken ends the program with `P0020` and its dump, as the
//! runtime's fault and not the program's. Off under the explorer, where one
//! task runs at a time and no worker is added, by design.

use super::*;
use std::collections::HashMap;

/// Looks a task may stay ready, or a timer due, while a worker sleeps. A
/// worker told to look takes it within a look, whatever says it was told:
/// the checker judges by what it sees, as a flag of the scheduler's left
/// set once hid a wake lost. One the system runs late has not looked yet
/// either, so the rule waits as long as `HELD_LOOKS` does.
const READY_LOOKS: u64 = 500;
/// Looks a worker may stay on one task while others wait and none is free,
/// with none beside it: past the watcher's two intervals, ten and a hundred
/// milliseconds, with room for a monitor the system runs late.
const HELD_LOOKS: u64 = 500;
/// Looks the monitor may go without looking, by its own count of looks,
/// while a task woken beside a worker waits: asleep, or run late by the
/// system, which the bound must leave room for, as `HELD_LOOKS` does.
const STILL_LOOKS: u64 = 500;
/// Looks a domain may stay marked as sought while every worker sleeps: a
/// seeker finds a task, or none and stops seeking before it sleeps, so a
/// mark with every worker asleep is one with no worker behind it, but for a
/// seeker the system runs late.
const SOUGHT_LOOKS: u64 = 500;
/// Looks the workers may be past their bound, as one starts or retires.
const OVER_LOOKS: u64 = 2;
/// Looks a worker may stay in one call of C while others wait and none is
/// free, with none in its place: the monitor replaces it after a look of a
/// millisecond or two. A rule that fires when the monitor has not acted yet
/// is broken also by a monitor the system runs late, so it waits as long as
/// `HELD_LOOKS` does for the watcher; the stalls of one thread a test makes
/// must stay under the margin these two leave.
const IN_C_LOOKS: u64 = 500;

/// Where a ready task waits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Place {
    /// Next to the worker of that slot, woken by the task it runs.
    Next(usize),
    /// In the queue of the tasks that worker's tasks started.
    Own(usize),
    /// In the queue every worker takes from.
    Shared,
}

/// A worker's slot, as one look saw it.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Slot {
    pub(super) alive: bool,
    pub(super) turns: u64,
    pub(super) held: bool,
    pub(super) beside: bool,
    pub(super) in_c: bool,
    /// The calls of C its worker has made, which a call that goes on does
    /// not move.
    pub(super) c_calls: u64,
    /// Another worker stands in its place for the call it is in.
    pub(super) replaced: bool,
}

/// What one look saw.
#[derive(Default, Debug, Clone)]
pub(super) struct Look {
    /// The ready tasks, by identity, and where each waits.
    pub(super) ready: Vec<(usize, Place)>,
    /// The earliest timer, if it is due.
    pub(super) due: Option<Instant>,
    /// Workers asleep, and whether one was told to look or one looks.
    pub(super) idle: usize,
    pub(super) told: bool,
    /// Workers started, retirements owed, and those that stand in for a
    /// worker in a call of C or one a tool holds.
    pub(super) workers: usize,
    pub(super) surplus: usize,
    pub(super) standing_in: usize,
    pub(super) monitor_asleep: bool,
    /// The monitor's count of its looks.
    pub(super) monitor_looks: u64,
    /// The domains marked as sought (`SEEKING`).
    pub(super) sought: Vec<usize>,
    pub(super) slots: Vec<Slot>,
}

/// What the looks before saw, by rule.
#[derive(Default)]
pub(super) struct Watch {
    pub(super) look: u64,
    /// Since which look each ready task has been ready, and where.
    ready: HashMap<usize, (u64, Place)>,
    /// Since which look the earliest timer, by when it falls, has been due.
    due: Option<(Instant, u64)>,
    /// Since which look a worker has slept.
    sleeping: Option<u64>,
    /// The monitor's count of its looks, and since which look it has not
    /// moved.
    still: Option<(u64, u64)>,
    /// Since which look each domain has been marked as sought while every
    /// worker slept.
    sought: HashMap<usize, u64>,
    /// Since which look each slot has run one task while others waited and
    /// none was free, with none beside it, and its count then.
    held: HashMap<usize, (u64, u64)>,
    /// Since which look the workers have been past their bound.
    over: Option<u64>,
    /// Since which look each slot has been in one call of C while others
    /// waited and none was free, with none in its place, and its calls then.
    in_c: HashMap<usize, (u64, u64)>,
    /// Since which look the workers have been fewer than their number.
    under: Option<u64>,
}

/// A rule broken: its headline, and the state under it.
#[derive(Debug, PartialEq)]
pub(super) struct Broken {
    pub(super) rule: &'static str,
    pub(super) headline: String,
}

/// Where a ready task waits, in the report's words.
fn shown(place: Place) -> String {
    match place {
        Place::Next(slot) => format!("beside worker {slot}, woken by the task it runs"),
        Place::Own(slot) => format!("in the queue of worker {slot}"),
        Place::Shared => "in the shared queue".to_string(),
    }
}

/// The seven rules over one look more (16.14), with `given` workers.
pub(super) fn judge(watch: &mut Watch, look: &Look, given: usize, name: &dyn Fn(usize) -> String) -> Option<Broken> {
    watch.look += 1;
    let now = watch.look;
    // How long each ready task has been ready.
    let mut ready = HashMap::new();
    for &(task, place) in &look.ready {
        let since = watch.ready.get(&task).filter(|(_, was)| *was == place).map_or(now, |&(since, _)| since);
        ready.insert(task, (since, place));
    }
    watch.ready = ready;
    watch.due = match (look.due, watch.due) {
        (Some(at), Some((was, since))) if at == was => Some((at, since)),
        (Some(at), _) => Some((at, now)),
        (None, _) => None,
    };
    watch.sleeping = if look.idle > 0 { watch.sleeping.or(Some(now)) } else { None };
    watch.still = match watch.still {
        Some((count, since)) if count == look.monitor_looks => Some((count, since)),
        _ => Some((look.monitor_looks, now)),
    };
    let all_asleep = look.workers > 0 && look.idle >= look.workers;
    watch.sought = look
        .sought
        .iter()
        .filter(|_| all_asleep)
        .map(|&domain| (domain, watch.sought.get(&domain).copied().unwrap_or(now)))
        .collect();
    let looks = |since: u64| now - since + 1;
    let oldest =
        watch.ready.iter().min_by_key(|(_, (since, _))| *since).map(|(&task, &(since, place))| (task, since, place));
    let shown = shown;

    // A ready task, or a due timer, while a worker sleeps.
    if let Some(sleeping) = watch.sleeping.filter(|&since| looks(since) >= READY_LOOKS) {
        let waiting = match (oldest, watch.due) {
            (Some((task, since, place)), _) if looks(since) >= READY_LOOKS => {
                Some(format!("{} has been ready for {} looks {}", name(task), looks(since), shown(place)))
            }
            (_, Some((_, since))) if looks(since) >= READY_LOOKS => {
                Some(format!("a timer has been due for {} looks", looks(since)))
            }
            _ => None,
        };
        if let Some(waiting) = waiting {
            return Some(Broken {
                rule: "ready-while-idle",
                headline: format!(
                    "{waiting}, while a worker has slept for {} looks: a wake the scheduler lost, not the program's (16.14)",
                    looks(sleeping)
                ),
            });
        }
    }

    // A worker held past the watcher's intervals, with none beside it.
    let waits = !watch.ready.is_empty() || watch.due.is_some();
    let room = look.workers < 2 * given;
    for (slot, state) in look.slots.iter().enumerate() {
        let watched = state.alive && !state.beside && !state.held && !state.in_c && waits && look.idle == 0 && room;
        let entry = watch.held.get(&slot).copied().filter(|&(turns, _)| watched && turns == state.turns);
        match entry {
            Some((_, since)) if looks(since) >= HELD_LOOKS => {
                return Some(Broken {
                    rule: "held-without-beside",
                    headline: format!(
                        "worker {slot} has run one task for {} looks while others wait and no worker is free, and has none beside it: the watcher did not keep its word (16.14)",
                        looks(since)
                    ),
                });
            }
            Some(_) => {}
            None if watched => {
                watch.held.insert(slot, (state.turns, now));
            }
            None => {
                watch.held.remove(&slot);
            }
        }
    }

    // More workers than twice those given, the stand-ins aside.
    let counted = look.workers.saturating_sub(look.surplus + look.standing_in);
    watch.over = if counted > 2 * given { watch.over.or(Some(now)) } else { None };
    if watch.over.is_some_and(|since| looks(since) >= OVER_LOOKS) {
        return Some(Broken {
            rule: "workers-past-bound",
            headline: format!(
                "the program has {counted} workers, besides those standing in for a call of C or a task a tool holds, where twice its number is {}: past the bound of 16.14",
                2 * given
            ),
        });
    }

    // A worker held in one call of C, with none in its place.
    for (slot, state) in look.slots.iter().enumerate() {
        let watched = state.alive && state.in_c && !state.replaced && waits && look.idle == 0;
        let entry = watch.in_c.get(&slot).copied().filter(|&(calls, _)| watched && calls == state.c_calls);
        match entry {
            Some((_, since)) if looks(since) >= IN_C_LOOKS => {
                return Some(Broken {
                    rule: "held-in-c",
                    headline: format!(
                        "worker {slot} has been in one call of C for {} looks while others wait and no worker is free, and nothing stands in its place: the monitor did not replace it (16.14)",
                        looks(since)
                    ),
                });
            }
            Some(_) => {}
            None if watched => {
                watch.in_c.insert(slot, (state.c_calls, now));
            }
            None => {
                watch.in_c.remove(&slot);
            }
        }
    }

    // Fewer workers than their number: those owed retirement count as
    // retired already, as a worker counts as started from when it is.
    let remaining = look.workers.saturating_sub(look.surplus);
    watch.under = if remaining < given { watch.under.or(Some(now)) } else { None };
    if watch.under.is_some_and(|since| looks(since) >= OVER_LOOKS) {
        return Some(Broken {
            rule: "workers-below-number",
            headline: format!(
                "the program has {remaining} workers once those owed retirement retire, where it was given {given}: fewer than its number (16.14)"
            ),
        });
    }

    // The monitor not looking while a task woken beside a worker waits.
    if let (Some((_, still)), Some((task, since, place))) = (watch.still, oldest)
        && matches!(place, Place::Next(_))
        && looks(still) >= STILL_LOOKS
        && looks(since) >= STILL_LOOKS
    {
        return Some(Broken {
            rule: "monitor-still",
            headline: format!(
                "the monitor has not looked for {} looks while {} has been ready {} for {} looks: nothing watches it (16.14)",
                looks(still),
                name(task),
                shown(place),
                looks(since)
            ),
        });
    }

    // A domain marked as sought while every worker sleeps: no worker behind
    // the mark, and a task made ready from outside wakes nobody. Where a
    // worker is awake the first rule sees what is left of it.
    if let Some((&domain, &since)) = watch.sought.iter().min_by_key(|(_, since)| **since)
        && looks(since) >= SOUGHT_LOOKS
    {
        return Some(Broken {
            rule: "sought-while-asleep",
            headline: format!(
                "domain {domain} has been marked as sought for {} looks while every worker slept: no worker looks for its tasks, and nothing wakes one for a task made ready from outside (16.14)",
                looks(since)
            ),
        });
    }
    None
}

/// Whether the checker runs: asked for, and not under the explorer, a
/// recording or its replay.
fn asked() -> bool {
    crate::config::config().check_scheduler.as_deref().is_some_and(|value| value == "1")
        && exploring().is_none()
        && !crate::replay::on()
}

/// Starts the checker once, when the workers start.
pub(super) fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !asked() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    super::stalls::start();
    spawn_thread(Some(256 * 1024), || {
        let mut watch = Watch::default();
        let given = crate::config::workers();
        loop {
            std::thread::sleep(Duration::from_millis(1));
            if stopping() {
                return;
            }
            let look = looked();
            if let Some(broken) = judge(&mut watch, &look, given, &named) {
                report(&broken, &look);
            }
        }
    });
}

/// A task as the report names it.
fn named(task: usize) -> String {
    let task = task as *mut Task;
    // Every task the scheduler holds is alive while the set is locked.
    let alive = tasks();
    if !alive.iter().any(|&t| t as *mut Task == task) {
        return "a task that has ended".into();
    }
    unsafe { if (*task).main { format!("task {} (main)", (*task).id) } else { format!("task {}", (*task).id) } }
}

/// One look at the scheduler, taking each lock on its own.
fn looked() -> Look {
    let mut look = Look::default();
    {
        let queue = queue();
        look.ready.extend(queue.runnable.iter().map(|&task| (task as usize, Place::Shared)));
        look.due = queue.timers.peek().map(|timer| timer.at).filter(|&at| at <= clock_now());
        look.idle = queue.idle;
        look.told = queue.waking.iter().any(|&waking| waking);
        look.workers = queue.workers;
        look.surplus = queue.surplus;
    }
    look.told |= SEEKING.iter().any(|seeking| seeking.load(Ordering::SeqCst));
    look.monitor_asleep = MONITOR_ASLEEP.load(Ordering::SeqCst);
    look.monitor_looks = super::foreign::MONITOR_LOOKS.load(Ordering::Relaxed);
    look.sought = (0..super::domains::count()).filter(|&domain| SEEKING[domain].load(Ordering::SeqCst)).collect();
    look.standing_in = super::foreign::standing_in();
    for (slot, next) in nexts().enumerate() {
        let task = next.task.load(Ordering::SeqCst);
        if !task.is_null() {
            look.ready.push((task as usize, Place::Next(slot)));
        }
        look.ready.extend(next.tasks().0.iter().map(|&task| (task as usize, Place::Own(slot))));
        let in_c = next.in_c.load(Ordering::Acquire);
        // A thread of C's calls hold no worker (`native`).
        let calling = unsafe { in_c.as_ref() }
            .filter(|state| state.depth.load(Ordering::SeqCst) > 0 && !state.native.load(Ordering::SeqCst));
        look.slots.push(Slot {
            alive: next.alive.load(Ordering::Acquire),
            turns: next.turns.load(Ordering::Relaxed),
            held: next.held.load(Ordering::Relaxed),
            beside: next.beside.load(Ordering::Relaxed),
            in_c: calling.is_some(),
            c_calls: calling.map_or(0, |state| state.calls.load(Ordering::SeqCst) as u64),
            replaced: calling.is_some_and(|state| state.replaced.load(Ordering::SeqCst)),
        });
    }
    look
}

/// Ends the program with `P0020`: the rule, the state under it, the dump.
fn report(broken: &Broken, look: &Look) -> ! {
    use std::io::Write;
    flush_stdout();
    crate::terminal::give_back();
    let _ = writeln!(std::io::stderr(), "{}", report_text(broken, look, &named));
    crate::diagnostics::crash_dump("scheduler-rule", &broken.headline, Vec::new(), true);
    std::process::exit(crate::PANIC_EXIT_CODE)
}

/// The report of a rule broken: its headline, then the state of the
/// scheduler at the look that broke it.
fn report_text(broken: &Broken, look: &Look, name: &dyn Fn(usize) -> String) -> String {
    let mut report = format!("panic[P0020]: scheduler-rule: {}", broken.headline);
    for (slot, state) in look.slots.iter().enumerate().filter(|(_, state)| state.alive) {
        let mut line = format!("\nworker {slot}: {} tasks started", state.turns);
        if state.held {
            line.push_str(", one beside it");
        }
        if state.beside {
            line.push_str(", put beside a held one");
        }
        if state.in_c {
            line.push_str(if state.replaced { ", in a call of C, another in its place" } else { ", in a call of C" });
        }
        report.push_str(&line);
    }
    for &(task, place) in &look.ready {
        report.push_str(&format!("\nready: {} {}", name(task), shown(place)));
    }
    report.push_str(&format!(
        "\nworkers: {} started, {} asleep{}, {} retirements owed, {} standing in",
        look.workers,
        look.idle,
        if look.told { ", one told to look" } else { ", none told to look" },
        look.surplus,
        look.standing_in
    ));
    report.push_str(&format!("\nmonitor: {}", if look.monitor_asleep { "asleep" } else { "watching" }));
    if !look.sought.is_empty() {
        let domains: Vec<String> = look.sought.iter().map(usize::to_string).collect();
        report.push_str(&format!("\ndomains sought: {}", domains.join(", ")));
    }
    report.push_str(match look.due {
        Some(_) => "\ntimers: one due",
        None => "\ntimers: none due",
    });
    report
}

#[cfg(test)]
mod tests {
    use super::{Broken, Look, Place, Slot, Watch, judge, report_text};
    use std::time::Instant;

    fn name(task: usize) -> String {
        format!("task {task}")
    }

    /// Runs `looks` looks of the same state; the first rule broken.
    /// The same look over and over, with a monitor that looks, as in a
    /// program whose monitor works.
    fn run(look: &Look, looks: u64, given: usize) -> Option<Broken> {
        let mut watch = Watch::default();
        (0..looks).find_map(|at| {
            let looking = Look { monitor_looks: look.monitor_looks + at, ..look.clone() };
            judge(&mut watch, &looking, given, &name)
        })
    }

    /// The same look with a monitor whose count does not move.
    fn run_still(look: &Look, looks: u64, given: usize) -> Option<Broken> {
        let mut watch = Watch::default();
        (0..looks).find_map(|_| judge(&mut watch, look, given, &name))
    }

    fn busy(turns: u64) -> Slot {
        Slot { alive: true, turns, ..Slot::default() }
    }

    /// Today's first path: a task woken beside a worker that computes, a
    /// worker asleep: a wake lost (testing/models/tests/next.to's
    /// counterexample). Told to look or not: one told would have looked, and
    /// a flag that says one looks, left set with nobody behind it, once hid
    /// such a wake.
    #[test]
    fn a_task_ready_beside_a_busy_worker_with_one_asleep_breaks_the_first_rule() {
        let look = Look {
            ready: vec![(7, Place::Next(0))],
            idle: 1,
            workers: 2,
            slots: vec![busy(3), Slot { alive: true, ..Slot::default() }],
            ..Look::default()
        };
        let broken = run(&look, 600, 2).expect("no rule broke");
        assert_eq!(broken.rule, "ready-while-idle");
        assert!(broken.headline.starts_with("task 7 has been ready for 500 looks beside worker 0"), "{broken:?}");
        let told = Look { told: true, ..look.clone() };
        assert_eq!(run(&told, 600, 2).map(|b| b.rule), Some("ready-while-idle"));
        // Within the bound, as a worker run late, no rule breaks.
        assert_eq!(run(&look, 400, 2), None);
    }

    /// A due timer with a worker asleep breaks it too.
    #[test]
    fn a_due_timer_with_a_worker_asleep_breaks_the_first_rule() {
        let look = Look { due: Some(Instant::now()), idle: 1, workers: 2, ..Look::default() };
        assert_eq!(run(&look, 600, 2).map(|b| b.rule), Some("ready-while-idle"));
    }

    /// The second path: one worker, held by a computation while a task waits,
    /// and none put beside it.
    #[test]
    fn a_held_worker_with_none_beside_breaks_the_second_rule() {
        let look = Look { ready: vec![(7, Place::Next(0))], workers: 1, slots: vec![busy(3)], ..Look::default() };
        let broken = run(&look, 600, 1).expect("no rule broke");
        assert_eq!(broken.rule, "held-without-beside");
        // With one beside it, or at the bound, or itself beside, it holds.
        let beside = Look { slots: vec![Slot { held: true, ..busy(3) }], ..look };
        assert_eq!(run(&beside, 600, 1), None);
        let bound =
            Look { ready: vec![(7, Place::Shared)], workers: 2, slots: vec![busy(3), busy(5)], ..Look::default() };
        assert_eq!(run(&bound, 600, 1), None);
    }

    /// The chain: workers put beside ones put beside, past twice the number.
    #[test]
    fn workers_past_twice_their_number_break_the_third_rule() {
        let look = Look { workers: 3, slots: vec![busy(1), busy(1), busy(1)], ..Look::default() };
        assert_eq!(run(&look, 5, 1).map(|b| b.rule), Some("workers-past-bound"));
        // Those standing in for C or a tool, and those owed retirement, aside.
        let standing = Look { standing_in: 1, ..look };
        assert_eq!(run(&standing, 5, 1), None);
    }

    /// The third path: the monitor not looking while a task woken beside
    /// the one worker waits (workers_woken_asleep.to), by its own count of
    /// looks, whatever its flag says.
    #[test]
    fn the_monitor_still_beside_a_woken_task_breaks_the_fourth_rule() {
        let look = Look {
            ready: vec![(7, Place::Next(0))],
            workers: 1,
            monitor_looks: 9,
            slots: vec![Slot { held: true, ..busy(3) }],
            ..Look::default()
        };
        assert_eq!(run_still(&look, 600, 1).map(|b| b.rule), Some("monitor-still"));
        assert_eq!(run_still(&look, 400, 1), None);
        // A monitor that looks breaks nothing.
        assert_eq!(run(&look, 2000, 1), None);
    }

    /// The fourth path: a domain marked as sought with every worker asleep,
    /// left so by two workers added in one look (workers_seeking.to).
    #[test]
    fn a_domain_sought_while_every_worker_sleeps_breaks_the_seventh_rule() {
        let look = Look { idle: 2, workers: 2, sought: vec![1], monitor_looks: 3, ..Look::default() };
        let broken = run(&look, 600, 2).expect("no rule broke");
        assert_eq!(broken.rule, "sought-while-asleep");
        assert!(broken.headline.starts_with("domain 1 has been marked as sought for 500 looks"), "{broken:?}");
        // A worker awake, or within the bound, as a seeker run late: nothing.
        assert_eq!(run(&Look { idle: 1, ..look.clone() }, 600, 2), None);
        assert_eq!(run(&look, 400, 2), None);
    }

    /// A task that moves, as two tasks that hand work to each other, never
    /// stays ready long enough, however long it is watched.
    #[test]
    fn tasks_that_come_and_go_break_nothing() {
        let mut watch = Watch::default();
        for at in 0..2000 {
            let look = Look {
                ready: vec![(at % 2, Place::Next(0))],
                idle: 1,
                workers: 2,
                monitor_asleep: true,
                slots: vec![busy(at as u64), Slot { alive: true, ..Slot::default() }],
                ..Look::default()
            };
            assert_eq!(judge(&mut watch, &look, 2, &name), None, "at look {at}");
        }
    }

    /// The report of P0020 says the rule's headline and, under it, each
    /// worker, each ready task where it waits, the workers' counts, the
    /// monitor and the timers, in the words `why` will use.
    #[test]
    fn the_report_of_p0020_says_whose_fault_and_the_state_under_it() {
        let look = Look {
            ready: vec![(7, Place::Next(0))],
            idle: 1,
            workers: 2,
            slots: vec![busy(3), Slot { alive: true, ..Slot::default() }],
            ..Look::default()
        };
        let broken = run(&look, 600, 2).expect("no rule broke");
        let report = report_text(&broken, &look, &name);
        let expected = [
            "panic[P0020]: scheduler-rule: task 7 has been ready for 500 looks beside worker 0, woken by the task it runs, while a worker has slept for 500 looks: a wake the scheduler lost, not the program's (16.14)",
            "worker 0: 3 tasks started",
            "worker 1: 0 tasks started",
            "ready: task 7 beside worker 0, woken by the task it runs",
            "workers: 2 started, 1 asleep, none told to look, 0 retirements owed, 0 standing in",
            "monitor: watching",
            "timers: none due",
        ];
        assert_eq!(report.lines().collect::<Vec<_>>(), expected);
    }

    /// A worker held in one call of C while others wait and none is free,
    /// and nothing put in its place: the monitor's oldest remedy undone.
    #[test]
    fn a_worker_held_in_c_with_none_in_its_place_breaks_the_fifth_rule() {
        let calling = Slot { in_c: true, c_calls: 4, ..busy(3) };
        let look = Look { ready: vec![(7, Place::Shared)], workers: 1, slots: vec![calling], ..Look::default() };
        let broken = run(&look, 600, 1).expect("no rule broke");
        assert_eq!(broken.rule, "held-in-c");
        assert!(broken.headline.starts_with("worker 0 has been in one call of C for 500 looks"), "{broken:?}");
        // With another in its place, or a worker free, it holds.
        let replaced = Look { slots: vec![Slot { replaced: true, ..calling }], ..look.clone() };
        assert_eq!(run(&replaced, 600, 1), None);
        let free = Look {
            idle: 1,
            told: true,
            workers: 2,
            slots: vec![calling, Slot { alive: true, ..Slot::default() }],
            ..look
        };
        assert_ne!(run(&free, 600, 1).map(|b| b.rule), Some("held-in-c"));
    }

    /// The workers fewer than their number, those owed retirement counted
    /// as gone; one being started counts already.
    #[test]
    fn workers_fewer_than_their_number_break_the_sixth_rule() {
        let look = Look { workers: 4, surplus: 2, slots: vec![busy(1), busy(1), busy(1), busy(1)], ..Look::default() };
        let broken = run(&look, 5, 3).expect("no rule broke");
        assert_eq!(broken.rule, "workers-below-number");
        assert!(
            broken
                .headline
                .starts_with("the program has 2 workers once those owed retirement retire, where it was given 3")
        );
        let owed = Look { surplus: 1, ..look };
        assert_eq!(run(&owed, 5, 3), None);
    }
}
