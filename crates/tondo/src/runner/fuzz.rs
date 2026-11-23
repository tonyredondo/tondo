//! `tondo test --fuzz` (testing 10.6): first the tests that fuzz run as
//! without it, which tells their calls of `testing.fuzz`; then each call
//! that passed searches, in workers that run its test with `TONDO_FUZZ`
//! naming it, until Ctrl-C, `--fuzz-time` or a failure. An input that
//! fails goes to the fuzz store, and the attempts of a test's workers
//! become one.

use super::worker::{self, Note, Replay, Target};
use super::{Attempt, Context, Fuzz, FuzzInput, Fuzzed, Searched, Status, Tree, human, interrupt};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The directory of a call under its test's: its name, with what is not a
/// letter, a digit, `.`, `_` or `-` as `_` (testing 15.14).
pub(crate) fn directory(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect()
}

/// Where a call's inputs go under a store or a corpus: the directories of
/// its test's identity, without the package, then the call's.
fn place(tree: &Tree, test: u32, call: &str) -> PathBuf {
    let id = tree.id(test);
    let mut path: PathBuf = id.split("::").skip(1).collect();
    path.push(directory(call));
    path
}

pub(super) fn campaign(run: &worker::Run, plan: &[u32], printer: &mut human::Printer, fuzz: &Fuzz) -> Vec<Attempt> {
    let tree = run.tree;
    // As without `--fuzz`: the seeds and the inputs that failed before.
    let units = super::units(tree, plan);
    let context = Context { round: 0, iteration: 1, unit: None, target: None };
    let first = worker::execute(run, units.into_iter().map(|u| (u, context)).collect(), |_| {});
    // The test runs in the project's directory, where a path from here,
    // as `tondo test app` or `--fuzz-corpus corpus` gives it, is another.
    let corpus = fuzz.corpus.clone().unwrap_or_else(|| run.dir.join("target/fuzz"));
    let corpus = std::path::absolute(&corpus).unwrap_or(corpus);
    let stops = std::env::temp_dir().join(format!("tondo-fuzz-stop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stops);
    let _ = std::fs::create_dir_all(&stops);
    let mut targets = Vec::new();
    for attempt in first.iter().filter(|a| !tree.suite(a.node) && !a.status.failed()) {
        for fuzzed in &attempt.fuzz {
            let index = targets.len();
            targets.push(Target {
                test: attempt.node,
                call: fuzzed.name.clone(),
                corpus: corpus.join(place(tree, attempt.node, &fuzzed.name)),
                stop: stops.join(index.to_string()),
            });
        }
    }
    if targets.is_empty() || interrupt::requested() {
        let _ = std::fs::remove_dir_all(&stops);
        return first;
    }
    let workers = run.options.jobs.max(targets.len());
    let until = fuzz.time.map_or(0, |time| {
        let end = std::time::SystemTime::now() + time;
        end.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
    });
    printer.fuzzing_begins(targets.len(), workers, fuzz.time);
    interrupt::soften(targets.iter().map(|t| t.stop.clone()).collect());
    let searching = worker::Run { stores: worker::Stores { ..run.stores }, targets: &targets, until, ..*run };
    // Each round's attempts merged by test; a round searches again the calls
    // a worker left at an input past --fuzz-timeout that ran in time alone,
    // with the time that is left.
    let mut rounds: Vec<BTreeMap<u32, Attempt>> = Vec::new();
    let mut searched_again = vec![0u32; targets.len()];
    let mut active: Vec<usize> = (0..targets.len()).collect();
    loop {
        // Each call's workers, as at the beginning.
        let units: Vec<(Vec<u32>, Context)> = (0..workers)
            .map(|n| n % targets.len())
            .filter(|index| active.contains(index))
            .map(|index| (vec![targets[index].test], Context { target: Some(index as u32), ..context }))
            .collect();
        let mut judged = Vec::new();
        // The workers that search, in all and of each call.
        let mut running = units.len();
        let mut live = vec![0usize; targets.len()];
        for (_, context) in &units {
            live[context.target.unwrap_or_default() as usize] += 1;
        }
        let searched = worker::execute(&searching, units, |note| match note {
            Note::Fuzzing(node, call, progress) => printer.fuzzing(*node, call, progress),
            // A call that failed stops its other workers. One past the
            // timeout runs again alone first, and stops them only if it
            // fails then too.
            Note::Ended(attempt) => {
                running = running.saturating_sub(1);
                let call = attempt.fuzz.iter().find(|f| f.search.is_some()).and_then(|searched| {
                    targets.iter().position(|t| t.test == attempt.node && t.call == searched.name)
                });
                if let Some(call) = call {
                    live[call] = live[call].saturating_sub(1);
                }
                let verdict = timed_out(&attempt.status).then(|| judge(&searching, attempt)).flatten();
                if let Some(passed) = verdict.as_ref().filter(|v| !v.status.failed()) {
                    // Its call is searched no more until the round ends.
                    if live[passed.target] == 0 && running > 0 {
                        printer.fuzz_waits(tree.id(attempt.node), &targets[passed.target].call);
                    }
                    judged.extend(verdict);
                    return;
                }
                judged.extend(verdict);
                for fuzzed in attempt.fuzz.iter().filter(|f| f.input.is_some()) {
                    if let Some(target) = targets.iter().find(|t| t.test == attempt.node && t.call == fuzzed.name) {
                        let _ = std::fs::write(&target.stop, "");
                    }
                }
            }
            _ => {}
        });
        let mut again = Vec::new();
        let mut by_test: BTreeMap<u32, Vec<Attempt>> = BTreeMap::new();
        for mut attempt in searched.into_iter().filter(|a| !tree.suite(a.node)) {
            if timed_out(&attempt.status) {
                apply(&judged, &mut attempt, &mut again);
            }
            by_test.entry(attempt.node).or_default().push(attempt);
        }
        rounds.push(by_test.into_iter().map(|(node, list)| (node, merged(list))).collect());
        // A call that also failed in this round, or whose time is up, is done.
        let failed = |index: &usize| {
            let target = &targets[*index];
            rounds.last().and_then(|round| round.get(&target.test)).is_some_and(|attempt| {
                attempt.status.failed() && attempt.fuzz.iter().any(|f| f.name == target.call && f.input.is_some())
            })
        };
        again.retain(|index| !failed(index));
        again.sort_unstable();
        again.dedup();
        let left = until == 0 || now_ms() < until;
        if again.is_empty() || !left || interrupt::requested() {
            break;
        }
        for &index in &again {
            searched_again[index] += 1;
            let _ = std::fs::remove_file(&targets[index].stop);
        }
        active = again;
    }
    interrupt::soften(Vec::new());
    let _ = std::fs::remove_dir_all(&stops);
    printer.fuzzing_ends();
    // Each test's rounds become one, one after the other, with what failed kept.
    let mut by_test: BTreeMap<u32, Attempt> = BTreeMap::new();
    for round in rounds {
        for (node, attempt) in round {
            match by_test.get_mut(&node) {
                Some(earlier) => after(earlier, attempt),
                None => {
                    by_test.insert(node, attempt);
                }
            }
        }
    }
    let mut attempts: Vec<Attempt> =
        first.into_iter().filter(|a| tree.suite(a.node) || !by_test.contains_key(&a.node)).collect();
    for (_, mut merged) in by_test {
        for fuzzed in &mut merged.fuzz {
            if let Some(index) = targets.iter().position(|t| t.test == merged.node && t.call == fuzzed.name) {
                fuzzed.searched_again = searched_again[index];
            }
            // What the search found, as small as it fails the same way.
            if let Some(input) = fuzzed.input.as_mut().filter(|i| i.file.is_none() && i.seed.is_none())
                && let Some(index) = targets.iter().position(|t| t.test == merged.node && t.call == fuzzed.name)
                && !interrupt::requested()
            {
                printer.reducing(tree.id(merged.node), &fuzzed.name, input.json.len());
                let reducing = worker::Run { stores: worker::Stores { ..run.stores }, ..searching };
                fuzzed.timeouts_not_repeated += reduce(&reducing, index, input, &merged.status);
            }
            store(run.dir, tree, merged.node, fuzzed);
        }
        attempts.push(merged);
    }
    // Each test's line, as without `--fuzz`.
    let tests: Vec<u32> = attempts.iter().map(|a| a.node).filter(|&n| !tree.suite(n)).collect();
    printer.begin(1, 0, &tests);
    for attempt in attempts.iter().filter(|a| !tree.suite(a.node)) {
        printer.note(&Note::Ended(Box::new(attempt.clone())));
    }
    attempts
}

/// What the calls of a test did in all its workers: the inputs summed,
/// the corpus and the counters reached the most any worker had, and the
/// input one failed with.
fn merge(attempts: &[Attempt]) -> Vec<Fuzzed> {
    let mut merged: Vec<Fuzzed> = Vec::new();
    for fuzzed in attempts.iter().flat_map(|a| &a.fuzz) {
        let Some(kept) = merged.iter_mut().find(|f| f.name == fuzzed.name) else {
            merged.push(fuzzed.clone());
            continue;
        };
        kept.inputs += fuzzed.inputs;
        kept.timeouts_not_repeated += fuzzed.timeouts_not_repeated;
        if kept.input.is_none() {
            kept.input = fuzzed.input.clone();
        }
        kept.search = match (kept.search, fuzzed.search) {
            (Some(a), Some(b)) => Some(Searched {
                inputs: a.inputs + b.inputs,
                elapsed_ms: a.elapsed_ms.max(b.elapsed_ms),
                corpus: a.corpus.max(b.corpus),
                reached: a.reached.max(b.reached),
                total: a.total.max(b.total),
                new_ms: a.new_ms.min(b.new_ms),
            }),
            (a, b) => a.or(b),
        };
    }
    merged
}

/// A test's attempts in the workers of one round as one, with the one
/// that failed first.
fn merged(mut list: Vec<Attempt>) -> Attempt {
    list.sort_by_key(|a| !a.status.failed());
    let mut merged = list[0].clone();
    merged.fuzz = merge(&list);
    merged
}

/// A test's attempt of a later round after the earlier ones: what the
/// searches did added up, as they ran one after the other, and what failed
/// kept.
fn after(earlier: &mut Attempt, later: Attempt) {
    let mut fuzz = std::mem::take(&mut earlier.fuzz);
    for fuzzed in &later.fuzz {
        let Some(kept) = fuzz.iter_mut().find(|f| f.name == fuzzed.name) else {
            fuzz.push(fuzzed.clone());
            continue;
        };
        kept.inputs += fuzzed.inputs;
        kept.timeouts_not_repeated += fuzzed.timeouts_not_repeated;
        if kept.input.is_none() {
            kept.input = fuzzed.input.clone();
        }
        kept.search = match (kept.search, fuzzed.search) {
            (Some(a), Some(b)) => Some(Searched {
                inputs: a.inputs + b.inputs,
                elapsed_ms: a.elapsed_ms + b.elapsed_ms,
                corpus: a.corpus.max(b.corpus),
                reached: a.reached.max(b.reached),
                total: a.total.max(b.total),
                new_ms: b.new_ms,
            }),
            (a, b) => a.or(b),
        };
    }
    if !earlier.status.failed() && later.status.failed() {
        *earlier = Attempt { fuzz, ..later };
    } else {
        earlier.fuzz = fuzz;
    }
}

/// Whether a worker ended at an input that ran past `--fuzz-timeout`.
fn timed_out(status: &Status) -> bool {
    matches!(status, Status::Limit { timeout: true, .. })
}

/// What an input that ran past `--fuzz-timeout` did when it ran again
/// alone: its test, its call's target, the input, and how it ended.
struct Judged {
    test: u32,
    target: usize,
    json: String,
    status: Status,
}

/// Runs again, alone, in a worker of its own, the input an attempt ended
/// past `--fuzz-timeout` with (testing 10.6).
fn judge(run: &worker::Run, attempt: &Attempt) -> Option<Judged> {
    if interrupt::requested() {
        return None;
    }
    let (target, input) = attempt.fuzz.iter().find_map(|fuzzed| {
        let input = fuzzed.input.as_ref()?;
        let target = run.targets.iter().position(|t| t.test == attempt.node && t.call == fuzzed.name)?;
        Some((target, input))
    })?;
    let status = alone(run, target, &input.json)?;
    Some(Judged { test: attempt.node, target, json: input.json.clone(), status })
}

/// An attempt that ended past `--fuzz-timeout` takes what its input did
/// alone: one that ended in time is no failure, as a busy machine can stop
/// a process that long; the attempt passes, counts it, and its call goes
/// in `again`, to search on with the time left.
fn apply(judged: &[Judged], attempt: &mut Attempt, again: &mut Vec<usize>) {
    for fuzzed in &mut attempt.fuzz {
        let Some(input) = fuzzed.input.as_ref() else { continue };
        let Some(verdict) = judged.iter().find(|j| j.test == attempt.node && j.json == input.json) else { continue };
        if verdict.status.failed() {
            // The timeout again, or what it does when nothing stops it.
            attempt.status = verdict.status.clone();
        } else {
            fuzzed.input = None;
            fuzzed.timeouts_not_repeated += 1;
            again.push(verdict.target);
            attempt.status = Status::Passed;
        }
        return;
    }
}

/// What `json`, an input of a target's call, does when it runs alone in a
/// new worker: how its attempt ended, if it ran.
fn alone(run: &worker::Run, target: usize, json: &str) -> Option<Status> {
    let file = std::env::temp_dir().join(format!("tondo-fuzz-again-{}", std::process::id()));
    std::fs::write(&file, json).ok()?;
    let test = run.targets[target].test;
    let replay = Replay { test, call: run.targets[target].call.clone(), file: file.clone() };
    let trying = worker::Run { replay: Some(&replay), stores: worker::Stores { ..run.stores }, ..*run };
    let context = Context { round: 0, iteration: 1, unit: None, target: Some(target as u32) };
    let attempts = worker::execute(&trying, vec![(vec![test], context)], |_| {});
    let _ = std::fs::remove_file(&file);
    attempts.into_iter().find(|a| a.node == test).map(|a| a.status)
}

/// Milliseconds since the Unix epoch, as `until` counts them.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Writes an input the search found failing to the fuzz store, named by
/// its hash, and says where it is.
fn store(dir: &Path, tree: &Tree, test: u32, fuzzed: &mut Fuzzed) {
    let Some(input) = fuzzed.input.as_mut().filter(|i| i.file.is_none() && i.seed.is_none()) else { return };
    let name = format!("{}.json", &crate::hash::sha256_hex(input.json.as_bytes())[..16]);
    let relative = Path::new("fuzz").join(place(tree, test, &fuzzed.name)).join(name);
    let path = dir.join(&relative);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if super::store::write_atomically(&path, input.json.as_bytes()).is_ok() {
        input.file = Some(relative.display().to_string());
    }
}

/// What makes two failures the same, for reducing: the status, the code,
/// and where it happened.
fn signature(status: &Status) -> Option<String> {
    let at = |text: &str| text.lines().find_map(|l| l.trim_start().strip_prefix("--> ").map(str::to_string));
    Some(match status {
        Status::Panic(text, _) => {
            let code = text.split(']').next().unwrap_or_default();
            format!("panic {code} {}", at(text).unwrap_or_default())
        }
        Status::Error { error_type, location, .. } => format!("error {error_type} {location:?}"),
        Status::Limit { timeout, .. } => format!("limit {timeout}"),
        Status::Timeout(_) => "timeout".into(),
        Status::Crashed(_) => "crashed".into(),
        _ => return None,
    })
}

/// How simple an input is: the length of its text, then the size of its
/// numbers; a reduction makes it simpler.
fn simplicity(value: &Value) -> (usize, f64) {
    fn numbers(value: &Value) -> f64 {
        match value {
            Value::Number(n) => n.as_f64().unwrap_or(0.0).abs(),
            Value::Array(items) => items.iter().map(numbers).sum(),
            Value::Object(members) => members.values().map(numbers).sum(),
            _ => 0.0,
        }
    }
    (value.to_string().len(), numbers(value))
}

/// Simpler values than `value`, each changed in one place: lists without
/// halves or elements, maps without members, shorter texts, numbers
/// nearer 0, `false`, and `null`.
fn smaller(value: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut paths = Vec::new();
    walk(value, &mut Vec::new(), &mut paths);
    for path in paths {
        let Some(node) = at(value, &path) else { continue };
        for replacement in replacements(node) {
            let mut changed = value.clone();
            if let Some(slot) = at_mut(&mut changed, &path) {
                *slot = replacement;
                out.push(changed);
            }
        }
    }
    let before = simplicity(value);
    out.retain(|candidate| simplicity(candidate) < before);
    out.sort_by(|a, b| simplicity(a).partial_cmp(&simplicity(b)).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out.truncate(500);
    out
}

fn replacements(node: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match node {
        Value::Array(items) if !items.is_empty() => {
            out.push(Value::Array(Vec::new()));
            out.extend(without_runs(items).into_iter().map(Value::Array));
        }
        Value::Object(members) => {
            for key in members.keys().take(32) {
                let mut fewer = members.clone();
                fewer.remove(key);
                out.push(Value::Object(fewer));
            }
        }
        Value::String(text) if !text.is_empty() => {
            let chars: Vec<char> = text.chars().collect();
            out.push(Value::String(String::new()));
            out.extend(without_runs(&chars).into_iter().map(|fewer| Value::String(fewer.into_iter().collect())));
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64().filter(|&i| i != 0) {
                out.extend([0, i / 2, i - i.signum()].map(Value::from));
            } else if let Some(f) = n.as_f64().filter(|&f| f != 0.0) {
                out.extend([0.0, f.trunc(), (f / 2.0).trunc()].map(Value::from));
            }
        }
        Value::Bool(true) => out.push(Value::Bool(false)),
        _ => {}
    }
    if !node.is_null() {
        out.push(Value::Null);
    }
    out
}

/// `items` without one run of them: of half of them, a quarter and so on
/// down to one, at each offset that is a multiple of the run's length. A
/// reduction takes out a long stretch in one round where it still fails
/// without it, and reaches every element, so that its rounds, each a
/// worker started, grow with the length's logarithm and not the length.
fn without_runs<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    let mut run = items.len() / 2;
    while run > 0 {
        for start in (0..items.len()).step_by(run) {
            let mut fewer = items[..start].to_vec();
            fewer.extend_from_slice(&items[(start + run).min(items.len())..]);
            out.push(fewer);
        }
        run /= 2;
    }
    out
}

fn walk(value: &Value, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(path.clone());
    let children: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(members) => members.values().collect(),
        _ => return,
    };
    for (index, child) in children.into_iter().enumerate() {
        path.push(index);
        walk(child, path, out);
        path.pop();
    }
}

fn at<'a>(value: &'a Value, path: &[usize]) -> Option<&'a Value> {
    let Some((&first, rest)) = path.split_first() else { return Some(value) };
    match value {
        Value::Array(items) => at(items.get(first)?, rest),
        Value::Object(members) => at(members.values().nth(first)?, rest),
        _ => None,
    }
}

fn at_mut<'a>(value: &'a mut Value, path: &[usize]) -> Option<&'a mut Value> {
    let Some((&first, rest)) = path.split_first() else { return Some(value) };
    match value {
        Value::Array(items) => at_mut(items.get_mut(first)?, rest),
        Value::Object(members) => at_mut(members.values_mut().nth(first)?, rest),
        _ => None,
    }
}

/// Reduces an input that failed (testing 10.6): tries simpler ones in a
/// worker that runs only them, keeping each that fails the same way, for
/// at most 30 seconds; the input becomes the last kept, as `json.encode`
/// writes it, saying how many bytes it had and whether the 30 seconds
/// ended before no simpler one failed. One that ran past
/// `--fuzz-timeout` is kept only if it does again alone; it gives how many
/// ran in time then.
fn reduce(run: &worker::Run, target: usize, input: &mut FuzzInput, failure: &Status) -> u32 {
    let Some(wanted) = signature(failure) else { return 0 };
    let Ok(mut current) = serde_json::from_str::<Value>(&input.json) else { return 0 };
    let mut in_time = 0;
    let original = input.json.len();
    let deadline = Instant::now() + Duration::from_secs(30);
    let file = std::env::temp_dir().join(format!("tondo-fuzz-tried-{}", std::process::id()));
    // The input itself first: it fails again, as `json.encode` writes it.
    let mut candidates = vec![current.clone()];
    candidates.extend(smaller(&current));
    let mut reduced = false;
    let mut ended = false;
    let test = run.targets[target].test;
    'rounds: while !candidates.is_empty() && !interrupt::requested() {
        let mut offset = 0;
        let mut kept = None;
        while offset < candidates.len() && !interrupt::requested() {
            if Instant::now() >= deadline {
                ended = true;
                break 'rounds;
            }
            let lines: Vec<String> = candidates[offset..].iter().map(Value::to_string).collect();
            if std::fs::write(&file, lines.join("\n")).is_err() {
                break 'rounds;
            }
            let replay = Replay { test, call: run.targets[target].call.clone(), file: file.clone() };
            let trying = worker::Run { replay: Some(&replay), stores: worker::Stores { ..run.stores }, ..*run };
            let context = Context { round: 0, iteration: 1, unit: None, target: Some(target as u32) };
            let attempts = worker::execute(&trying, vec![(vec![test], context)], |_| {});
            let Some(attempt) = attempts.into_iter().find(|a| a.node == test) else { break 'rounds };
            let failed =
                attempt.fuzz.iter().filter(|f| f.name == run.targets[target].call).find_map(|f| f.input.clone());
            let Some(tried) = failed.as_ref().and_then(|f| f.tried) else { break };
            if signature(&attempt.status).as_deref() == Some(wanted.as_str()) {
                let json = failed.map(|f| f.json).unwrap_or_default();
                if !timed_out(&attempt.status) || alone(run, target, &json).is_none_or(|status| status.failed()) {
                    kept = Some((offset + tried, json));
                    break;
                }
                in_time += 1;
            }
            offset += tried + 1;
        }
        let Some((index, json)) = kept else { break };
        input.json = json;
        if index == 0 && !reduced {
            // The input as it was, written as `json.encode` writes it.
            reduced = true;
            candidates = smaller(&current);
            continue;
        }
        current = candidates[index].clone();
        reduced = true;
        candidates = smaller(&current);
    }
    let _ = std::fs::remove_file(&file);
    if reduced && input.json.len() < original {
        input.reduced_from = Some(original);
    }
    input.reduction_timed_out = ended;
    in_time
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs_of(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        without_runs(&chars).into_iter().map(|fewer| fewer.into_iter().collect()).collect()
    }

    #[test]
    fn a_text_loses_runs_of_every_length_down_to_one() {
        let runs = runs_of("abcdefgh");
        let halves = ["efgh", "abcd"];
        let quarters = ["cdefgh", "abefgh", "abcdgh", "abcdef"];
        let ones = ["bcdefgh", "acdefgh", "abdefgh", "abcefgh", "abcdfgh", "abcdegh", "abcdefh", "abcdefg"];
        assert_eq!(runs, [&halves[..], &quarters, &ones].concat());
        // The last run of an odd length is shorter.
        assert_eq!(runs_of("abcde")[..3], ["cde", "abe", "abcd"]);
        assert!(runs_of("a").is_empty());
    }

    #[test]
    fn every_element_of_a_long_list_can_go() {
        let items: Vec<Value> = (0..40).map(Value::from).collect();
        let candidates = replacements(&Value::Array(items.clone()));
        let mut fewer = items.clone();
        fewer.remove(39);
        assert!(candidates.contains(&Value::Array(fewer)));
        assert!(candidates.contains(&Value::Array(items[20..].to_vec())));
    }
}
