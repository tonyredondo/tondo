//! `tondo test --profile` (testing 10.5): a worker samples where it is
//! while each test runs, or while each benchmark measures, or counts what
//! it allocates then, and writes it beside what the other workers write,
//! which `tondo test` turns into a profile of each test or benchmark.

use super::diagnostics::{Frames, Labels};
use super::recorder::Sample;
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
static SAMPLES: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
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
    keep_draining();
}

/// Takes the samples of the ring to what collects them: a test's profile,
/// a profile `std.profile` makes, or none.
fn drain() {
    let (taken, _) = super::recorder::take_samples();
    if let Some((samples, _)) = SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        samples.extend(taken.iter().copied());
    }
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
        let mut labels = Labels::default();
        for (index, (at, thread, stack, set)) in samples.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let _ = write!(out, "{comma}[{at},{thread},{},{}]", frames.list(stack.frames()), labels.number(*set));
        }
        let _ = write!(out, "],\"labels\":{{{}}}", labels.json);
    }
    if settings.memory {
        out.push(']');
    }
    let _ = write!(out, ",\"frames\":{{{}}}}}", frames.json);
    let number = WRITTEN.fetch_add(1, Ordering::Relaxed);
    let path = settings.dir.join(format!("{}-{number}.json", std::process::id()));
    let _ = std::fs::write(path, out);
}

// ---------------------------------------------------------------------------
// std.profile: profiles a program makes of itself
// ---------------------------------------------------------------------------

/// The samples of `profile.startCpu`, while it runs, and when it began.
static SESSION: Mutex<Option<(Vec<Sample>, std::time::Instant)>> = Mutex::new(None);
/// The thread that empties the ring of samples into what collects them.
static DRAINING: std::sync::Once = std::sync::Once::new();

/// Empties the ring of samples every 50 ms, from now on.
fn keep_draining() {
    DRAINING.call_once(|| {
        let _ = super::tasks::try_spawn_thread(Some(128 * 1024), || {
            super::diagnostics::retire_here();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(50));
                drain();
            }
        });
    });
}

/// A profile `std.profile` made: in pprof's format, and as folded stacks.
struct Made {
    pprof: Vec<u8>,
    folded: String,
}

/// The function, file and line of an address: the program's from its
/// table, with those of the calls its code was inlined at after it, others
/// from the symbols of the executable or of the library that holds them.
fn named(address: usize) -> Vec<(String, String, i64)> {
    if let Some(described) = super::trace::describe(address) {
        let (file, line) = described.at.map_or(("", 0), |(file, line)| (file, line as i64));
        let outer = described.inlined.map(|(function, file, line, _)| (function.into(), file.into(), line as i64));
        return std::iter::once((described.function.to_string(), file.to_string(), line)).chain(outer).collect();
    }
    if let Some(name) = super::symbols::name(address) {
        return vec![(demangled(name), String::new(), 0)];
    }
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(address as *const libc::c_void, &mut info) } != 0 && !info.dli_sname.is_null() {
        let symbol = unsafe { std::ffi::CStr::from_ptr(info.dli_sname) }.to_string_lossy();
        return vec![(demangled(&symbol), String::new(), 0)];
    }
    vec![(format!("{address:#x}"), String::new(), 0)]
}

/// A symbol of Rust's manglings as Rust writes the path, without its hash
/// or the parameters of its generics; others as they are.
fn demangled(symbol: &str) -> String {
    if let Some(rest) = symbol.strip_prefix("_R").or_else(|| symbol.strip_prefix("__R")) {
        return modern(rest.as_bytes());
    }
    let Some(mut rest) = symbol.strip_prefix("_ZN").or_else(|| symbol.strip_prefix("__ZN")) else {
        return symbol.to_string();
    };
    let mut parts = Vec::new();
    while let Some(digits) = rest.find(|c: char| !c.is_ascii_digit()).filter(|&at| at > 0) {
        let Ok(length) = rest[..digits].parse::<usize>() else { break };
        let Some(part) = rest.get(digits..digits + length) else { break };
        parts.push(part);
        rest = &rest[digits + length..];
    }
    if parts.last().is_some_and(|last| last.len() == 17 && last.starts_with('h')) {
        parts.pop();
    }
    let path = parts.join("::");
    let replaced = [("$LT$", "<"), ("$GT$", ">"), ("$u20$", " "), ("$C$", ","), ("$RF$", "&"), ("..", "::")];
    replaced.iter().fold(path, |path, (from, to)| path.replace(from, to))
}

/// The path of a symbol of Rust's v0 mangling: its identifiers, each its
/// length and its text, past the disambiguators (`s…_`) and the
/// references back (`B…_`).
fn modern(bytes: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'1'..=b'9' => {
                let digits = bytes[at..].iter().take_while(|b| b.is_ascii_digit()).count();
                let length: usize =
                    std::str::from_utf8(&bytes[at..at + digits]).ok().and_then(|d| d.parse().ok()).unwrap_or(0);
                at += digits;
                if bytes.get(at) == Some(&b'_') {
                    at += 1;
                }
                let Some(name) = bytes.get(at..at + length) else { break };
                parts.push(String::from_utf8_lossy(name).into_owned());
                at += length;
            }
            b's' | b'B' => {
                let end = bytes[at + 1..].iter().position(|b| !b.is_ascii_alphanumeric()).map(|p| at + 1 + p);
                match end.filter(|&end| bytes[end] == b'_') {
                    Some(end) => at = end + 1,
                    None => at += 1,
                }
            }
            _ => at += 1,
        }
    }
    parts.join("::")
}

/// A profile of `kind`s from stacks, each with its values and labels,
/// that took `duration` nanoseconds.
fn made(
    kinds: &[(&str, &str)],
    period: (&str, &str, i64),
    default: Option<&str>,
    samples: &[Weighed],
    duration: i64,
) -> Made {
    let program = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let mut builder = super::pprof::Builder::new(&program, kinds, (period.0, period.1), period.2);
    if let Some(default) = default {
        builder.default_type(default);
    }
    let mut names: HashMap<usize, Vec<(String, String, i64)>> = HashMap::new();
    let mut folded: HashMap<String, i64> = HashMap::new();
    for (stack, values, labels) in samples {
        for &address in stack {
            names.entry(address).or_insert_with(|| named(address));
        }
        let frames: Vec<super::pprof::Frame> = stack
            .iter()
            .flat_map(|address| {
                names[address].iter().map(|(function, file, line)| super::pprof::Frame {
                    address: *address as u64,
                    function,
                    file,
                    line: *line,
                })
            })
            .collect();
        let tags: Vec<(&str, &str)> = labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        builder.sample(&frames, values, &tags);
        let line: Vec<&str> =
            stack.iter().rev().flat_map(|address| names[address].iter().rev().map(|named| named.0.as_str())).collect();
        *folded.entry(line.join(";")).or_default() += values.last().copied().unwrap_or(0);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let pprof = builder.finish(now.as_nanos() as i64 - duration, duration);
    let mut lines: Vec<(String, i64)> = folded.into_iter().collect();
    lines.sort();
    Made { pprof, folded: lines.iter().map(|(stack, count)| format!("{stack} {count}\n")).collect() }
}

/// A stack, its values, and its labels.
type Weighed = (Vec<usize>, Vec<i64>, Vec<(String, String)>);

/// `profile.startCpu()`: false when a profile of the processor time is
/// being made already.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_cpu_start() -> u8 {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.is_some() {
        return 0;
    }
    *session = Some((Vec::new(), std::time::Instant::now()));
    drop(session);
    let _ = super::recorder::take_samples();
    super::recorder::sample_threads();
    keep_draining();
    1
}

/// `CpuProfile.stop()`: what was sampled since `startCpu`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_cpu_stop() -> *mut super::native::NativeObject {
    drain();
    let Some((samples, started)) = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return super::native::new(Made { pprof: Vec::new(), folded: String::new() });
    };
    // A trace or the tests' profiles go on sampling.
    if !super::recorder::on() && !on() {
        super::recorder::stop_sampling();
    }
    let period = 1_000_000;
    let weighed: Vec<Weighed> = samples
        .iter()
        .map(|(_, thread, stack, set)| {
            let mut labels = vec![("thread".to_string(), thread.to_string())];
            labels.extend(super::labels::get(*set).unwrap_or_default());
            (stack.frames().to_vec(), vec![1, period], labels)
        })
        .collect();
    let took = started.elapsed().as_nanos() as i64;
    let kinds = [("samples", "count"), ("cpu", "nanoseconds")];
    super::native::new(made(&kinds, ("cpu", "nanoseconds", period), None, &weighed, took))
}

/// `profile.startMemory()`: samples what the program allocates from now on.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_memory_start() {
    super::heap::prepare();
    super::diagnostics::hook(super::diagnostics::WATCHED);
}

/// Whether what the program allocates is sampled.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_memory_sampled() -> u8 {
    (super::diagnostics::hooks() & super::diagnostics::WATCHED != 0) as u8
}

/// `profile.memory()`: the memory alive and what each stack allocated, from
/// the samples.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_memory() -> *mut super::native::NativeObject {
    let rate = super::heap::RATE as f64;
    let mut weighed: Vec<Weighed> = Vec::new();
    for (size, stack) in super::heap::alive().0 {
        let weight = 1.0 / (1.0 - (-(size as f64) / rate).exp());
        weighed.push((stack, vec![0, 0, weight.round() as i64, (weight * size as f64).round() as i64], Vec::new()));
    }
    for (objects, bytes, stack) in super::heap::allocations().0 {
        weighed.push((stack, vec![objects.round() as i64, bytes.round() as i64, 0, 0], Vec::new()));
    }
    // The allocator is not where the memory was asked for.
    for (stack, ..) in &mut weighed {
        let asked = stack.iter().position(|&a| !allocator(&named(a)[0].0)).unwrap_or(0);
        stack.drain(..asked);
    }
    let kinds =
        [("alloc_objects", "count"), ("alloc_space", "bytes"), ("inuse_objects", "count"), ("inuse_space", "bytes")];
    super::native::new(made(&kinds, ("space", "bytes", rate as i64), Some("inuse_space"), &weighed, 0))
}

/// Whether a function is the allocator's or its sampling's.
fn allocator(name: &str) -> bool {
    [
        "tondo_runtime::diagnostics::allocated",
        "tondo_runtime::heap::",
        "tondo_runtime::meta::Counting",
        "<tondo_runtime::meta::Counting",
        "__rust_alloc",
        "__rust_realloc",
        "__rdl_",
        "alloc::alloc::",
        "<alloc::raw_vec",
        "alloc::raw_vec",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// `profile.tasks()`: where each task is now.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_tasks() -> *mut super::native::NativeObject {
    let here: Vec<usize> = {
        let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
        super::trace::walk(super::trace::frame_pointer(), bounds).collect()
    };
    // This thread stands still too while the others stop.
    let paused = super::tasks::pause_here();
    let shots = super::cycles::without_collections(|| {
        let stopped = super::tasks::stop_world(std::time::Duration::from_millis(100));
        let shots = super::tasks::snapshot(Some(here));
        if stopped {
            super::tasks::resume_world();
        }
        shots
    });
    if paused {
        super::tasks::enter_tondo();
    }
    let weighed: Vec<Weighed> = shots
        .into_iter()
        .map(|shot| (shot.stack, vec![1], vec![("state".to_string(), shot.state.to_string())]))
        .collect();
    super::native::new(made(&[("tasks", "count")], ("tasks", "count", 1), None, &weighed, 0))
}

/// `Profile.pprof()`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_pprof(profile: *mut super::native::NativeObject) -> *mut crate::ArrayHeader {
    let bytes = super::native::get::<Made>(profile).map_or(&[][..], |made| &made.pprof[..]);
    super::io::byte_array(bytes)
}

/// `Profile.folded()`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_profile_folded(profile: *mut super::native::NativeObject) -> *mut crate::Str {
    let text = super::native::get::<Made>(profile).map_or("", |made| made.folded.as_str());
    crate::str_new(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demangles_rust_symbols() {
        assert_eq!(
            demangled("_RNvNtCsx03MogZlnX_13tondo_runtime5tasks10trampoline"),
            "tondo_runtime::tasks::trampoline"
        );
        assert_eq!(
            demangled("_RNvMs_NtCsx03MogZlnX_13tondo_runtime4metaNtB4_8Counting5alloc"),
            "tondo_runtime::meta::Counting::alloc"
        );
        assert_eq!(demangled("_ZN4core3ptr13drop_in_place17h0123456789abcdefE"), "core::ptr::drop_in_place");
        assert_eq!(demangled("tondo_main"), "tondo_main");
    }
}
