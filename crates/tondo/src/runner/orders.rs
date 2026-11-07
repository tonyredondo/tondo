//! Which orders of a test differ from one that ran (testing 8.10): two steps
//! of different tasks can only change what comes of them if they touch the
//! same thing, so a run says what each step touched and only the orders that
//! turn such a pair around are worth running. This is the dynamic partial
//! order reduction of Flanagan and Godefroid.

use std::collections::HashMap;

/// A choice of the run: what it took, among how many, and which tasks those
/// are, with no tasks for a choice of anything else.
struct Choice {
    made: u32,
    count: u32,
    tasks: Vec<u64>,
}

/// What a task did from when it ran until it gave way.
#[derive(Default)]
struct Step {
    task: u64,
    /// The choice that gave it its turn, if there was one.
    choice: Option<usize>,
    touched: Vec<u64>,
    /// What it only comes after, which is no race whenever it comes.
    after: Vec<u64>,
    started: Vec<u64>,
    /// It ended in a wait, and what it waits for it touches again.
    waits: bool,
}

/// The lines a run wrote (`TONDO_RT_ORDER_OUT`).
fn read(written: &str) -> (Vec<Choice>, Vec<Step>) {
    let (mut choices, mut steps) = (Vec::new(), Vec::<Step>::new());
    let mut choice = None;
    for line in written.lines() {
        let mut words = line.split(' ');
        let word = words.next().unwrap_or_default();
        let mut number = || words.next().and_then(|text| text.parse::<u64>().ok());
        match word {
            "d" => {
                let (Some(made), Some(count)) = (number(), number()) else { continue };
                let tasks: Vec<u64> =
                    words.next().unwrap_or_default().split(',').filter_map(|task| task.parse().ok()).collect();
                choice = (!tasks.is_empty()).then_some(choices.len());
                choices.push(Choice { made: made as u32, count: count as u32, tasks });
            }
            "s" => {
                let Some(task) = number() else { continue };
                // Each of its steps sees whether it was cancelled.
                steps.push(Step { task, choice: choice.take(), touched: vec![task], ..Step::default() });
            }
            "t" => {
                if let (Some(object), Some(step)) = (number(), steps.last_mut()) {
                    step.touched.push(object);
                }
            }
            "h" => {
                if let (Some(object), Some(step)) = (number(), steps.last_mut()) {
                    step.after.push(object);
                }
            }
            "n" => {
                if let (Some(task), Some(step)) = (number(), steps.last_mut()) {
                    step.started.push(task);
                }
            }
            "b" => {
                if let Some(step) = steps.last_mut() {
                    step.waits = true;
                }
            }
            _ => {}
        }
    }
    (choices, steps)
}

/// The order this run is one of: which step of which task came before which
/// in what they touched, and what the other choices took. Runs that agree on
/// that are the same order, whatever came first of what shares nothing.
pub(super) fn class(written: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let (choices, steps) = read(written);
    let mut count: HashMap<u64, u32> = HashMap::new();
    let mut last: HashMap<u64, (u64, u32)> = HashMap::new();
    let mut edges = Vec::new();
    let mut carried: HashMap<u64, Vec<u64>> = HashMap::new();
    for step in &steps {
        let number = count.entry(step.task).or_default();
        *number += 1;
        let here = (step.task, *number);
        let waited = carried.remove(&step.task).unwrap_or_default();
        let touched: Vec<u64> = step.touched.iter().chain(&waited).copied().collect();
        for object in &touched {
            if let Some(&before) = last.get(object).filter(|before| before.0 != step.task) {
                edges.push((before, here));
            }
            last.insert(*object, here);
        }
        if step.waits {
            carried.insert(step.task, touched);
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    edges.hash(&mut hasher);
    count.iter().collect::<std::collections::BTreeMap<_, _>>().hash(&mut hasher);
    choices.iter().filter(|choice| choice.tasks.is_empty()).for_each(|choice| choice.made.hash(&mut hasher));
    hasher.finish()
}

/// What a task knows came before it: of each task, the last step.
type Clock = HashMap<u64, usize>;

fn learn(clock: &mut Clock, known: &Clock) {
    for (&task, &seen) in known {
        let mine = clock.entry(task).or_insert(seen);
        *mine = seen.max(*mine);
    }
}

/// An order to run: the choices that lead to it, then the tasks to run next.
pub(super) type Order = (Vec<u32>, Vec<u64>);

/// The orders worth running after a run that wrote `written` and was given
/// its first `given` choices; with `every`, each order there is past those.
pub(super) fn others(written: &str, every: bool, given: usize) -> Vec<Order> {
    let (choices, steps) = read(written);
    let path = |at: usize| -> Vec<u32> { choices[..at].iter().map(|choice| choice.made).collect() };
    let other = |at: usize, other: u32| -> Order {
        let mut path = path(at);
        path.push(other);
        (path, Vec::new())
    };
    let mut found = Vec::new();
    // A choice of what no step says anything about has no way to be reduced.
    for (at, choice) in choices.iter().enumerate().skip(if every { given } else { 0 }) {
        if every || choice.tasks.is_empty() {
            found.extend((0..choice.count).filter(|&made| made != choice.made).map(|made| other(at, made)));
        }
    }
    if every {
        return found;
    }
    let mut clocks: HashMap<u64, Clock> = HashMap::new();
    let mut known: Vec<Clock> = Vec::new();
    let mut last: HashMap<u64, usize> = HashMap::new();
    let mut carried: HashMap<u64, (Vec<u64>, Vec<u64>)> = HashMap::new();
    for (index, step) in steps.iter().enumerate() {
        let mut clock = clocks.remove(&step.task).unwrap_or_default();
        clock.insert(step.task, index);
        let (waited, waited_after) = carried.remove(&step.task).unwrap_or_default();
        let touched: Vec<u64> = step.touched.iter().chain(&waited).copied().collect();
        let after: Vec<u64> = step.after.iter().chain(&waited_after).copied().collect();
        for at in after.iter().filter_map(|object| last.get(object)) {
            learn(&mut clock, &known[*at]);
        }
        let mut before: Vec<usize> = touched.iter().filter_map(|object| last.get(object).copied()).collect();
        before.retain(|&at| steps[at].task != step.task);
        before.sort_unstable();
        before.dedup();
        for &at in &before {
            let task = steps[at].task;
            if clock.get(&task).is_none_or(|&seen| seen < at) {
                // Turned around: what does not need the earlier step goes
                // first, up to this one.
                if let Some(choice) = steps[at].choice {
                    let after = |between: usize| known[between].get(&task).is_some_and(|&seen| seen >= at);
                    let tasks = (at + 1..index).filter(|&between| !after(between)).map(|between| steps[between].task);
                    found.push((path(choice), tasks.chain([step.task]).collect()));
                }
            }
        }
        for &at in &before {
            learn(&mut clock, &known[at]);
        }
        for object in &touched {
            last.insert(*object, index);
        }
        for started in &step.started {
            clocks.insert(*started, clock.clone());
        }
        if step.waits {
            carried.insert(step.task, (touched, after));
        }
        known.push(clock.clone());
        clocks.insert(step.task, clock);
    }
    // A task that could run and never did: nothing says what it would touch.
    let mut ran = HashMap::new();
    for (index, step) in steps.iter().enumerate() {
        ran.insert(step.task, index);
    }
    for (index, step) in steps.iter().enumerate() {
        let Some(at) = step.choice else { continue };
        for (position, task) in choices[at].tasks.iter().enumerate() {
            if ran.get(task).is_none_or(|&last| last < index) {
                found.push(other(at, position as u32));
            }
        }
    }
    found
}

/// The choices a run made, to give it again.
pub(super) fn path(written: &str) -> Vec<u32> {
    read(written).0.iter().map(|choice| choice.made).collect()
}
