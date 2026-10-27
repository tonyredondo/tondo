//! `tondo test --fuzz` (testing 10.6): first the tests that fuzz run as
//! without it, which tells their calls of `testing.fuzz`; then each call
//! that passed searches, in workers that run its test with `TONDO_FUZZ`
//! naming it, until Ctrl-C, `--fuzz-time` or a failure. An input that
//! fails goes to the fuzz store, and the attempts of a test's workers
//! become one.

use super::worker::{self, Note, Target};
use super::{Attempt, Context, Fuzz, Fuzzed, Searched, Tree, human, interrupt};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
