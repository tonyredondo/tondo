//! `TONDO_CHECK_SCHEDULER=1` (toolchain 7.17): a thread that looks at the
//! scheduler every millisecond and holds what language 16.14 promises. It
//! counts in its own looks, so that a process the system stops breaks no
//! rule; a rule broken ends the program with `P0020` and its dump, as the
//! runtime's fault and not the program's. Off under the explorer, where one
//! task runs at a time and no worker is added, by design.

use super::*;
use std::collections::HashMap;

/// Looks a task may stay ready, or a timer due, while a worker sleeps and
/// nothing told it to look.
const READY_LOOKS: u64 = 100;
/// Looks a worker may stay on one task while others wait and none is free,
/// with none beside it: past the watcher's two intervals, ten and a hundred
/// milliseconds, with room for a monitor the system runs late.
const HELD_LOOKS: u64 = 500;
/// Looks the monitor may sleep while a task woken beside a worker waits.
const ASLEEP_LOOKS: u64 = 100;
/// Looks the workers may be past their bound, as one starts or retires.
const OVER_LOOKS: u64 = 2;

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
}

/// What one look saw.
#[derive(Default, Debug)]
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
    /// Since which look a worker has slept with nothing told to look.
    untold: Option<u64>,
    /// Since which look the monitor has slept.
    asleep: Option<u64>,
    /// Since which look each slot has run one task while others waited and
    /// none was free, with none beside it, and its count then.
    held: HashMap<usize, (u64, u64)>,
    /// Since which look the workers have been past their bound.
    over: Option<u64>,
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

/// The four rules over one look more (16.14), with `given` workers.
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
    let untold = look.idle > 0 && !look.told;
    watch.untold = if untold { watch.untold.or(Some(now)) } else { None };
    watch.asleep = if look.monitor_asleep { watch.asleep.or(Some(now)) } else { None };
    let looks = |since: u64| now - since + 1;
    let oldest =
        watch.ready.iter().min_by_key(|(_, (since, _))| *since).map(|(&task, &(since, place))| (task, since, place));
    let shown = shown;

    // A ready task, or a due timer, while a worker sleeps untold.
    if let Some(untold) = watch.untold.filter(|&since| looks(since) >= READY_LOOKS) {
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
                    "{waiting}, while a worker has slept for {} looks and nothing told it to look: a wake the scheduler lost, not the program's (16.14)",
                    looks(untold)
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

    // The monitor asleep while a task woken beside a worker waits.
    if let (Some(asleep), Some((task, since, place))) = (watch.asleep, oldest)
        && matches!(place, Place::Next(_))
        && looks(asleep) >= ASLEEP_LOOKS
        && looks(since) >= ASLEEP_LOOKS
    {
        return Some(Broken {
            rule: "monitor-asleep",
            headline: format!(
                "the monitor has slept for {} looks while {} has been ready {} for {} looks: nothing watches it (16.14)",
                looks(asleep),
                name(task),
                shown(place),
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
        look.told = queue.waking.iter().any(|&waking| waking) || queue.starting.is_some();
        look.workers = queue.workers;
        look.surplus = queue.surplus;
    }
    look.told |= SEEKING.iter().any(|seeking| seeking.load(Ordering::SeqCst));
    look.monitor_asleep = MONITOR_ASLEEP.load(Ordering::SeqCst);
    look.standing_in = super::foreign::standing_in();
    for (slot, next) in nexts().enumerate() {
        let task = next.task.load(Ordering::SeqCst);
        if !task.is_null() {
            look.ready.push((task as usize, Place::Next(slot)));
        }
        look.ready.extend(next.tasks().0.iter().map(|&task| (task as usize, Place::Own(slot))));
        let in_c = next.in_c.load(Ordering::Acquire);
        look.slots.push(Slot {
            alive: next.alive.load(Ordering::Acquire),
            turns: next.turns.load(Ordering::Relaxed),
            held: next.held.load(Ordering::Relaxed),
            beside: next.beside.load(Ordering::Relaxed),
            in_c: !in_c.is_null() && unsafe { (*in_c).depth.load(Ordering::SeqCst) } > 0,
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
            line.push_str(", in a call of C");
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
    fn run(look: &Look, looks: u64, given: usize) -> Option<Broken> {
        let mut watch = Watch::default();
        (0..looks).find_map(|_| judge(&mut watch, look, given, &name))
    }

    fn busy(turns: u64) -> Slot {
        Slot { alive: true, turns, ..Slot::default() }
    }

    /// Today's first path: a task woken beside a worker that computes, a
    /// worker asleep and nothing told it: a wake lost (testing/models/tests/
    /// next.to's counterexample).
    #[test]
    fn a_task_ready_beside_a_busy_worker_with_one_asleep_breaks_the_first_rule() {
        let look = Look {
            ready: vec![(7, Place::Next(0))],
            idle: 1,
            workers: 2,
            monitor_asleep: true,
            slots: vec![busy(3), Slot { alive: true, ..Slot::default() }],
            ..Look::default()
        };
        let broken = run(&look, 200, 2).expect("no rule broke");
        assert_eq!(broken.rule, "ready-while-idle");
        assert!(broken.headline.starts_with("task 7 has been ready for 100 looks beside worker 0"), "{broken:?}");
        // Told to look, the worker is on its way, and the monitor watches:
        // no rule breaks.
        let told = Look { told: true, monitor_asleep: false, ..look };
        assert_eq!(run(&told, 200, 2), None);
    }

    /// A due timer with a worker asleep and untold breaks it too.
    #[test]
    fn a_due_timer_with_a_worker_asleep_breaks_the_first_rule() {
        let look = Look { due: Some(Instant::now()), idle: 1, workers: 2, ..Look::default() };
        assert_eq!(run(&look, 200, 2).map(|b| b.rule), Some("ready-while-idle"));
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

    /// The third path: the monitor asleep while a task woken beside the one
    /// worker waits (workers_woken_asleep.to).
    #[test]
    fn the_monitor_asleep_beside_a_woken_task_breaks_the_fourth_rule() {
        let look = Look {
            ready: vec![(7, Place::Next(0))],
            workers: 1,
            monitor_asleep: true,
            slots: vec![Slot { held: true, ..busy(3) }],
            ..Look::default()
        };
        assert_eq!(run(&look, 200, 1).map(|b| b.rule), Some("monitor-asleep"));
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
            monitor_asleep: true,
            slots: vec![busy(3), Slot { alive: true, ..Slot::default() }],
            ..Look::default()
        };
        let broken = run(&look, 200, 2).expect("no rule broke");
        let report = report_text(&broken, &look, &name);
        let expected = [
            "panic[P0020]: scheduler-rule: task 7 has been ready for 100 looks beside worker 0, woken by the task it runs, while a worker has slept for 100 looks and nothing told it to look: a wake the scheduler lost, not the program's (16.14)",
            "worker 0: 3 tasks started",
            "worker 1: 0 tasks started",
            "ready: task 7 beside worker 0, woken by the task it runs",
            "workers: 2 started, 1 asleep, none told to look, 0 retirements owed, 0 standing in",
            "monitor: asleep",
            "timers: none due",
        ];
        assert_eq!(report.lines().collect::<Vec<_>>(), expected);
    }
}
