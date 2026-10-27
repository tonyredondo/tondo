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
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// What `tondo test` tells a worker that searches, in `TONDO_FUZZ`: the
/// call it searches in, by its test and directory, and where its corpus
/// is, the stop file, the time it ends at, as milliseconds since the epoch
/// or `0` for none, the file of the code's literals, and the milliseconds
/// and MiB an input may take, separated by tabs.
pub(crate) struct Target {
    pub test: String,
    pub call: String,
    corpus: PathBuf,
    stop: PathBuf,
    deadline: Option<std::time::SystemTime>,
    literals: PathBuf,
    timeout: Duration,
    memory: u64,
}

pub(crate) fn target() -> Option<Target> {
    let text = std::env::var("TONDO_FUZZ").ok()?;
    let parts: Vec<&str> = text.split('\t').collect();
    let [test, call, corpus, stop, deadline, literals, timeout, memory] = parts[..] else { return None };
    let deadline = deadline.parse::<u64>().ok().filter(|&ms| ms > 0);
    Some(Target {
        test: test.to_string(),
        call: call.to_string(),
        corpus: PathBuf::from(corpus),
        stop: PathBuf::from(stop),
        deadline: deadline.map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms)),
        literals: PathBuf::from(literals),
        timeout: Duration::from_millis(timeout.parse().unwrap_or(1000)),
        memory: memory.parse().unwrap_or(2048),
    })
}

/// Since when the input that runs runs, in milliseconds from `EPOCH`, or
/// `0` while none does.
static RUNNING_SINCE: AtomicU64 = AtomicU64::new(0);
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Watches the inputs that run, from a thread of its own: one that runs
/// past `timeout`, or with the process holding more than `memory` MiB,
/// ends the worker, telling `tondo test` which and why.
fn watch(timeout: Duration, memory: u64) {
    let _ = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let since = RUNNING_SINCE.load(Ordering::Relaxed);
            if since != 0 && now_ms().saturating_sub(since) > timeout.as_millis() as u64 {
                limit("timeout", format!("an input ran longer than --fuzz-timeout, {}s", timeout.as_secs_f64()));
            }
            let held = resident() >> 20;
            if since != 0 && held > memory {
                limit(
                    "memory",
                    format!("an input left the worker holding {held} MiB, more than --fuzz-memory {memory}"),
                );
            }
        }
    });
}

/// The memory the process holds, as the allocator sees it.
fn resident() -> u64 {
    unsafe extern "C" {
        fn mi_process_info(
            elapsed: *mut usize,
            user: *mut usize,
            system: *mut usize,
            current_rss: *mut usize,
            peak_rss: *mut usize,
            current_commit: *mut usize,
            peak_commit: *mut usize,
            page_faults: *mut usize,
        );
    }
    let mut values = [0usize; 8];
    let [a, b, c, d, e, f, g, h] = values.each_mut().map(|v| v as *mut usize);
    unsafe { mi_process_info(a, b, c, d, e, f, g, h) };
    values[3] as u64
}

/// Ends the worker in the input that runs, which went past a limit.
fn limit(kind: &str, message: String) -> ! {
    super::interrupted();
    crate::testing::report("limit", format!("{kind}\0{message}").as_bytes());
    crate::flush_stdout();
    unsafe { libc::_exit(1) }
}

/// The literals of the code the build counts, as the compiler wrote them:
/// `{"texts": [...], "ints": [...]}`.
fn literals(path: &PathBuf) -> Dictionary {
    let mut dictionary = Dictionary::default();
    let Some(Json::Map(members)) = std::fs::read_to_string(path).ok().and_then(|text| parse(&text)) else {
        return dictionary;
    };
    for (key, value) in members {
        let Json::List(items) = value else { continue };
        for item in items {
            match (key.as_str(), item) {
                ("texts", Json::Text(text)) => dictionary.texts.push(text),
                ("ints", Json::Int(n)) => dictionary.ints.push(n),
                _ => {}
            }
        }
    }
    dictionary
}

/// Whether a worker searches: only then are comparisons noted.
static SEARCHING: AtomicBool = AtomicBool::new(false);
/// The integers the counted code compared lately, as a ring.
const RING: usize = 64;
static INTS: [AtomicI64; RING] = [const { AtomicI64::new(0) }; RING];
static INTS_AT: AtomicUsize = AtomicUsize::new(0);
/// The texts it compared lately, short ones.
static TEXTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What the code of a build for `--fuzz` compares: the operands of a
/// comparison of integers.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fuzz_compare(a: i64, b: i64) {
    if SEARCHING.load(Ordering::Relaxed) && a != b {
        for value in [a, b] {
            let at = INTS_AT.fetch_add(1, Ordering::Relaxed) % RING;
            INTS[at].store(value, Ordering::Relaxed);
        }
    }
}

/// The operands of a comparison of texts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fuzz_compare_text(a: *const crate::Str, b: *const crate::Str) {
    if !SEARCHING.load(Ordering::Relaxed) {
        return;
    }
    let (a, b) = unsafe { (crate::str_bytes(a), crate::str_bytes(b)) };
    if a == b {
        return;
    }
    if let Ok(mut texts) = TEXTS.try_lock()
        && texts.len() < RING
    {
        for text in [a, b].into_iter().filter(|t| (1..=64).contains(&t.len())) {
            texts.push(String::from_utf8_lossy(text).into_owned());
        }
    }
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
        let dictionary = literals(&target.literals);
        SEARCHING.store(true, Ordering::Relaxed);
        watch(target.timeout, target.memory);
        let mut search = Search {
            target,
            shapes: Shapes::read(shape),
            dictionary,
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

    /// Clears the counters before an input runs, which runs from now.
    pub(crate) fn before(&mut self) {
        crate::testing::counters().fill(0);
        RUNNING_SINCE.store(now_ms(), Ordering::Relaxed);
    }

    /// No input runs.
    pub(crate) fn idle(&self) {
        RUNNING_SINCE.store(0, Ordering::Relaxed);
    }

    /// Rates the input that ran: one that reached a new range of some
    /// counter joins the corpus.
    pub(crate) fn after(&mut self, json: &str) {
        self.idle();
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
        self.compared();
    }

    /// Takes in what the input's comparisons met, for the next inputs.
    fn compared(&mut self) {
        const MOST: usize = 4096;
        let ints = INTS_AT.swap(0, Ordering::Relaxed).min(RING);
        for slot in &INTS[..ints] {
            let value = slot.load(Ordering::Relaxed);
            if !self.dictionary.ints.contains(&value) {
                if self.dictionary.ints.len() >= MOST {
                    self.dictionary.ints.remove(0);
                }
                self.dictionary.ints.push(value);
            }
        }
        let texts = std::mem::take(&mut *TEXTS.lock().unwrap_or_else(|e| e.into_inner()));
        for text in texts {
            if !self.dictionary.texts.contains(&text) {
                if self.dictionary.texts.len() >= MOST {
                    self.dictionary.texts.remove(0);
                }
                self.dictionary.texts.push(text);
            }
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
