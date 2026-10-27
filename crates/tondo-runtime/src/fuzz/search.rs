//! The search of `tondo test --fuzz` (testing 10.6) in a worker: the call
//! it searches in tells it each input that ran, which it rates by what
//! the counters of the build reached, keeping in the corpus those that
//! reached something new; and it makes the next input from one of the
//! corpus. Workers of the same call share their corpus through its
//! directory, and stop when `tondo test` makes the stop file or the time
//! ends.

use super::shape::{Dictionary, Rng, Shapes};
use super::value::{Json, parse};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What `tondo test` tells a worker that searches, in `TONDO_FUZZ`: the
/// call it searches in, by its test and directory, and where its corpus
/// is, the stop file, and the time it ends at, as milliseconds since the
/// epoch or `0` for none, separated by tabs.
pub(crate) struct Target {
    pub test: String,
    pub call: String,
    corpus: PathBuf,
    stop: PathBuf,
    deadline: Option<std::time::SystemTime>,
}

pub(crate) fn target() -> Option<Target> {
    let text = std::env::var("TONDO_FUZZ").ok()?;
    let parts: Vec<&str> = text.split('\t').collect();
    let [test, call, corpus, stop, deadline] = parts[..] else { return None };
    let deadline = deadline.parse::<u64>().ok().filter(|&ms| ms > 0);
    Some(Target {
        test: test.to_string(),
        call: call.to_string(),
        corpus: PathBuf::from(corpus),
        stop: PathBuf::from(stop),
        deadline: deadline.map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms)),
    })
}

struct Entry {
    value: Json,
    size: usize,
}

pub(crate) struct Search {
    target: Target,
    shapes: Shapes,
    pub dictionary: Dictionary,
    corpus: Vec<Entry>,
    /// The files of the corpus directory read or written.
    files: HashSet<String>,
    /// For each counter, the ranges of counts some input reached.
    seen: Vec<u8>,
    pub reached: usize,
    pub inputs: u64,
    started: Instant,
    pub last_new: Instant,
    last_sync: Instant,
    last_report: Instant,
    rng: Rng,
    stopped: bool,
}

/// The range of a count, as a bit: 1, 2, 3, 4 to 7, 8 to 15, 16 to 31,
/// 32 to 127, and 128 or more.
fn range(count: u64) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        4..=7 => 8,
        8..=15 => 16,
        16..=31 => 32,
        32..=127 => 64,
        _ => 128,
    }
}

impl Search {
    pub(crate) fn new(target: Target, shape: &str) -> Search {
        let now = Instant::now();
        let mut search = Search {
            target,
            shapes: Shapes::read(shape),
            dictionary: Dictionary::default(),
            corpus: Vec::new(),
            files: HashSet::new(),
            seen: vec![0; crate::testing::counters().len()],
            reached: 0,
            inputs: 0,
            started: now,
            last_new: now,
            last_sync: now,
            last_report: now,
            rng: Rng::new(),
            stopped: false,
        };
        let _ = std::fs::create_dir_all(&search.target.corpus);
        search.sync();
        search
    }

    /// Clears the counters before an input runs.
    pub(crate) fn before(&mut self) {
        crate::testing::counters().fill(0);
    }

    /// Rates the input that ran: one that reached a new range of some
    /// counter joins the corpus.
    pub(crate) fn after(&mut self, json: &str) {
        self.inputs += 1;
        let mut new = false;
        for (seen, &count) in self.seen.iter_mut().zip(crate::testing::counters().iter()) {
            let bit = range(count);
            if bit & !*seen != 0 {
                if *seen == 0 {
                    self.reached += 1;
                }
                *seen |= bit;
                new = true;
            }
        }
        if new {
            self.last_new = Instant::now();
            self.keep(json, true);
        }
    }

    /// Adds an input to the corpus, and to its directory when it is new
    /// here.
    fn keep(&mut self, json: &str, write: bool) {
        let Some(value) = parse(json) else { return };
        let name = format!("{}.json", hash(json.as_bytes()));
        if !self.files.insert(name.clone()) && write {
            return;
        }
        if write {
            let path = self.target.corpus.join(&name);
            let partial = path.with_extension(format!("partial-{}", std::process::id()));
            if std::fs::write(&partial, json).is_ok() {
                let _ = std::fs::rename(&partial, &path);
            }
        }
        self.corpus.push(Entry { size: json.len(), value });
    }

    /// Takes in what other workers of the call found.
    fn sync(&mut self) {
        self.last_sync = Instant::now();
        let Ok(list) = std::fs::read_dir(&self.target.corpus) else { return };
        let mut names: Vec<String> = list
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json") && !self.files.contains(name))
            .collect();
        names.sort();
        for name in names {
            if let Ok(json) = std::fs::read_to_string(self.target.corpus.join(&name)) {
                self.files.insert(name);
                self.keep(&json, false);
            }
        }
    }

    /// Whether the search goes on: the stop file is not there and the time
    /// has not ended.
    pub(crate) fn goes_on(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        if self.inputs.is_multiple_of(64) {
            let late = self.target.deadline.is_some_and(|d| std::time::SystemTime::now() >= d);
            if late || self.target.stop.exists() {
                self.stopped = true;
                return false;
            }
            if self.last_sync.elapsed() > Duration::from_secs(2) {
                self.sync();
            }
        }
        true
    }

    /// The next input: an input of the corpus changed, or a new one when
    /// the corpus has none.
    pub(crate) fn next(&mut self, seeds: &[String]) -> String {
        if self.corpus.is_empty() {
            for seed in seeds {
                self.keep(seed, false);
            }
        }
        if self.corpus.is_empty() || self.rng.chance(50) {
            return self.shapes.generate(&self.shapes.root.clone(), &mut self.rng, &self.dictionary, 0).text();
        }
        // Newer and smaller inputs more often.
        let count = self.corpus.len();
        let pick = |rng: &mut Rng| {
            let a = rng.below(count);
            let b = count - 1 - rng.below(count.min(16));
            if rng.chance(2) { a.max(b) } else { a }
        };
        let (first, second) = (pick(&mut self.rng), pick(&mut self.rng));
        let index = if self.corpus[second].size < self.corpus[first].size { second } else { first };
        let mut value = self.corpus[index].value.clone();
        let other = self.corpus[self.rng.below(count)].value.clone();
        self.shapes.mutate(&mut value, &mut self.rng, &self.dictionary, Some(&other));
        value.text()
    }

    /// Whether a second has passed since the last report.
    pub(crate) fn reports(&mut self) -> bool {
        let due = self.last_report.elapsed() >= Duration::from_secs(1);
        if due {
            self.last_report = Instant::now();
        }
        due
    }

    /// What the search did, for `tondo test`: inputs, milliseconds, the
    /// corpus, the counters reached and those there are, and milliseconds
    /// since it found something new, separated by spaces.
    pub(crate) fn progress(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.inputs,
            self.started.elapsed().as_millis(),
            self.corpus.len(),
            self.reached,
            self.seen.len(),
            self.last_new.elapsed().as_millis()
        )
    }
}

/// The first 16 hexadecimal digits of the SHA-256 of `bytes`, which name
/// an input's file (testing 15.14).
pub(crate) fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)[..8].iter().map(|b| format!("{b:02x}")).collect()
}
