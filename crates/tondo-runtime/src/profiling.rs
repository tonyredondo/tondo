//! `tondo test --profile` (testing 10.5): a worker samples where it is
//! while each test runs, or while each benchmark measures, or counts what
//! it allocates then, and writes it beside what the other workers write,
//! which `tondo test` turns into a profile of each test or benchmark.

use super::diagnostics::Frames;
use super::recorder::Stack;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What the tests are profiled for, and where it goes:
/// `TONDO_TEST_PROFILE` is `cpu:<dir>` or `memory:<dir>`.
struct Settings {
    memory: bool,
    dir: PathBuf,
}

fn settings() -> Option<&'static Settings> {
    static SETTINGS: std::sync::OnceLock<Option<Settings>> = std::sync::OnceLock::new();
    SETTINGS
        .get_or_init(|| {
            let value = std::env::var("TONDO_TEST_PROFILE").ok()?;
            let (kind, dir) = value.split_once(':')?;
            Some(Settings { memory: kind == "memory", dir: PathBuf::from(dir) })
        })
        .as_ref()
}

/// Whether the tests are profiled.
pub(crate) fn on() -> bool {
    settings().is_some()
}

static COLLECTING: AtomicBool = AtomicBool::new(false);
/// The samples of what is profiled now.
static SAMPLES: Mutex<Vec<(u64, u32, Stack)>> = Mutex::new(Vec::new());
/// What each stack had allocated when collecting began, and what it
/// allocated while collecting.
type Allocations = HashMap<Vec<usize>, (f64, f64)>;
static BEFORE: Mutex<Option<Allocations>> = Mutex::new(None);
static ALLOCATED: Mutex<Option<Allocations>> = Mutex::new(None);
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Readies the worker: samples its threads, emptied into what is
/// collected every 50 ms, or counts what it allocates.
pub(crate) fn start() {
    let Some(settings) = settings() else { return };
    if settings.memory {
        super::heap::prepare();
        super::diagnostics::hook(super::diagnostics::WATCHED);
        return;
    }
    super::recorder::sample_threads();
    let _ = super::tasks::try_spawn_thread(Some(128 * 1024), || {
        super::diagnostics::retire_here();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drain();
        }
    });
}

fn drain() {
    let (taken, _) = super::recorder::take_samples();
    if COLLECTING.load(Ordering::Acquire) {
        SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).extend(taken);
    }
}

/// Collects from now on, anew.
pub(crate) fn resume() {
    let Some(settings) = settings() else { return };
    if settings.memory {
        *ALLOCATED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *BEFORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(allocations());
    } else {
        COLLECTING.store(false, Ordering::Release);
        drain();
        SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).clear();
        COLLECTING.store(true, Ordering::Release);
    }
}

/// Stops collecting, keeping what was collected until it is written.
pub(crate) fn pause() {
    let Some(settings) = settings() else { return };
    if settings.memory {
        let Some(before) = BEFORE.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let mut allocated = ALLOCATED.lock().unwrap_or_else(|e| e.into_inner());
        let allocated = allocated.get_or_insert_default();
        for (stack, (objects, bytes)) in allocations() {
            let (was_objects, was_bytes) = before.get(&stack).copied().unwrap_or_default();
            if objects > was_objects {
                let entry = allocated.entry(stack).or_default();
                entry.0 += objects - was_objects;
                entry.1 += bytes - was_bytes;
            }
        }
    } else {
        drain();
        COLLECTING.store(false, Ordering::Release);
    }
}

fn allocations() -> Allocations {
    super::heap::allocations().0.into_iter().map(|(objects, bytes, stack)| (stack, (objects, bytes))).collect()
}

/// Writes what was collected of the test `node`, or of its benchmark
/// `bench`, and forgets it.
pub(crate) fn write(node: u32, bench: Option<&str>) {
    let Some(settings) = settings() else { return };
    pause();
    let mut frames = Frames::default();
    let bench = bench.map_or("null".to_string(), super::diagnostics::quoted);
    let mut out = format!("{{\"node\":{node},\"bench\":{bench},");
    if settings.memory {
        let allocated = ALLOCATED.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
        let _ = write!(out, "\"kind\":\"memory\",\"rate\":{},\"allocated\":[", super::heap::RATE);
        for (index, (stack, (objects, bytes))) in allocated.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let _ = write!(
                out,
                "{comma}{{\"objects\":{objects:.1},\"bytes\":{bytes:.0},\"stack\":{}}}",
                frames.list(stack)
            );
        }
    } else {
        let samples = std::mem::take(&mut *SAMPLES.lock().unwrap_or_else(|e| e.into_inner()));
        out.push_str("\"kind\":\"cpu\",\"samples\":[");
        for (index, (at, thread, stack)) in samples.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let _ = write!(out, "{comma}[{at},{thread},{}]", frames.list(stack.frames()));
        }
    }
    let _ = write!(out, "],\"frames\":{{{}}}}}", frames.json);
    let number = WRITTEN.fetch_add(1, Ordering::Relaxed);
    let path = settings.dir.join(format!("{}-{number}.json", std::process::id()));
    let _ = std::fs::write(path, out);
}
