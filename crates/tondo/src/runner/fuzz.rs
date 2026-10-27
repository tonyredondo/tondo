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
    let corpus = fuzz.corpus.clone().unwrap_or_else(|| run.dir.join("target/fuzz"));
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
    let units: Vec<(Vec<u32>, Context)> = (0..workers)
        .map(|n| {
            let index = n % targets.len();
            (vec![targets[index].test], Context { target: Some(index as u32), ..context })
        })
        .collect();
    let searched = worker::execute(&searching, units, |note| match note {
        Note::Fuzzing(node, call, progress) => printer.fuzzing(*node, call, progress),
        // A call that failed stops its other workers.
        Note::Ended(attempt) => {
            for fuzzed in attempt.fuzz.iter().filter(|f| f.input.is_some()) {
                if let Some(target) = targets.iter().find(|t| t.test == attempt.node && t.call == fuzzed.name) {
                    let _ = std::fs::write(&target.stop, "");
                }
            }
        }
        _ => {}
    });
    interrupt::soften(Vec::new());
    let _ = std::fs::remove_dir_all(&stops);
    printer.fuzzing_ends();
    // Each test's attempts become one, with what failed kept.
    let mut by_test: BTreeMap<u32, Vec<Attempt>> = BTreeMap::new();
    for attempt in searched.into_iter().filter(|a| !tree.suite(a.node)) {
        by_test.entry(attempt.node).or_default().push(attempt);
    }
    let mut attempts: Vec<Attempt> =
        first.into_iter().filter(|a| tree.suite(a.node) || !by_test.contains_key(&a.node)).collect();
    for (_, mut list) in by_test {
        list.sort_by_key(|a| !a.status.failed());
        let mut merged = list[0].clone();
        merged.fuzz = merge(&list);
        for fuzzed in &mut merged.fuzz {
            // What the search found, as small as it fails the same way.
            if let Some(input) = fuzzed.input.as_mut().filter(|i| i.file.is_none() && i.seed.is_none())
                && let Some(index) = targets.iter().position(|t| t.test == merged.node && t.call == fuzzed.name)
                && !interrupt::requested()
            {
                printer.reducing(tree.id(merged.node), &fuzzed.name, input.json.len());
                let reducing = worker::Run { stores: worker::Stores { ..run.stores }, ..searching };
                reduce(&reducing, index, input, &merged.status);
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
        Status::Timeout => "timeout".into(),
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
            if items.len() > 1 {
                out.push(Value::Array(items[items.len() / 2..].to_vec()));
                out.push(Value::Array(items[..items.len() / 2].to_vec()));
            }
            for index in 0..items.len().min(32) {
                let mut fewer = items.clone();
                fewer.remove(index);
                out.push(Value::Array(fewer));
            }
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
            if chars.len() > 1 {
                out.push(Value::String(chars[chars.len() / 2..].iter().collect()));
                out.push(Value::String(chars[..chars.len() / 2].iter().collect()));
            }
            for index in 0..chars.len().min(32) {
                let mut fewer = chars.clone();
                fewer.remove(index);
                out.push(Value::String(fewer.into_iter().collect()));
            }
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
/// writes it, saying how many bytes it had.
fn reduce(run: &worker::Run, target: usize, input: &mut FuzzInput, failure: &Status) {
    let Some(wanted) = signature(failure) else { return };
    let Ok(mut current) = serde_json::from_str::<Value>(&input.json) else { return };
    let original = input.json.len();
    let deadline = Instant::now() + Duration::from_secs(30);
    let file = std::env::temp_dir().join(format!("tondo-fuzz-tried-{}", std::process::id()));
    // The input itself first: it fails again, as `json.encode` writes it.
    let mut candidates = vec![current.clone()];
    candidates.extend(smaller(&current));
    let mut reduced = false;
    let test = run.targets[target].test;
    'rounds: while Instant::now() < deadline && !interrupt::requested() {
        let mut offset = 0;
        let mut kept = None;
        while offset < candidates.len() && Instant::now() < deadline && !interrupt::requested() {
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
                kept = Some((offset + tried, failed.map(|f| f.json).unwrap_or_default()));
                break;
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
}
