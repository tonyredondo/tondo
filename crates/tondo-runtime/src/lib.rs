//! Runtime linked into every native Tondo executable.
//!
//! The compiler's build script compiles this file with `rustc` as a staticlib
//! and embeds it; the generated code calls the `tondo_rt_*` symbols below.
//! That interface is private to the compiler and the runtime: nothing here is
//! a stable ABI.

#![allow(clippy::missing_safety_doc)]

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};

mod arrays;
mod bench;
mod boxes;
mod builder;
mod cbor;
mod civil;
mod clock;
mod codec;
mod compress;
mod config;
mod console;
mod contexts;
mod crypto;
mod cycles;
mod debugger;
mod diagnostics;
mod env;
mod events;
mod files;
mod fs;
mod functions;
mod fuzz;
mod grpc;
mod heap;
mod http;
mod image;
mod io;
mod json;
mod labels;
mod live;
mod locales;
mod lockorder;
mod logpoints;
mod maps;
mod math;
mod memory;
mod meta;
mod msgpack;
mod native;
mod net;
mod numbers;
mod panics;
mod pipes;
mod poll;
mod pprof;
mod process;
mod profiling;
mod protobuf;
mod race;
mod recorder;
mod references;
mod reflect;
mod regex;
mod regex_dialect;
mod replay;
mod server;
mod shared;
mod shown;
mod signals;
mod snapshot;
mod sqlite;
#[cfg(all(tondo_staticlib, target_os = "linux"))]
mod start;
mod strings;
mod symbols;
mod sync;
mod tasks;
mod unwinding;
// The compiler reads templates with the same syntax, and uses the rest.
#[allow(dead_code)]
mod template_syntax;
mod templates;
mod testing;
mod text;
mod time;
mod toml;
mod trace;
mod yaml;
mod zones;
pub(crate) use arrays::*;
pub(crate) use boxes::*;
pub(crate) use builder::*;
pub(crate) use console::*;
pub(crate) use functions::*;
pub(crate) use maps::*;
pub(crate) use references::*;
pub(crate) use strings::*;

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Process exit status of a panic (spec 20.10: distinct from error exits).
pub const PANIC_EXIT_CODE: i32 = 101;

pub(crate) fn panic_name(code: u32) -> &'static str {
    match code {
        1 => "bounds",
        2 => "zero-slice-step",
        3 => "integer-division-by-zero",
        4 => "overlapping-borrow",
        5 => "checked-overflow",
        6 => "array-shape-mismatch",
        7 => "assertion-failed",
        8 => "explicit-panic",
        9 => "duplicate-dynamic-map-key",
        10 => "invalid-shift-count",
        11 => "invalid-repeat-count",
        12 => "closed-channel",
        13 => "deadlock",
        14 => "reentrant-lock",
        15 => "invalid-environment-name",
        16 => "nul-in-c-string",
        17 => "invalid-panic-code",
        18 => "negative-exponent",
        19 => "lock-order",
        2001 => "test-skip-during-cleanup",
        2002 => "test-tag-conflict",
        2003 => "test-virtual-time-deadlock",
        2004 => "overlapping-test-virtual-time",
        2005 => "test-virtual-time-range",
        2006 => "test-artifact-conflict",
        2007 => "test-snapshot-mismatch",
        2008 => "test-snapshot-conflict",
        2009 => "test-bench-conflict",
        2010 => "test-fuzz-conflict",
        2011 => "test-fuzz-input-invalid",
        _ => "unknown",
    }
}

/// Each task keeps the cleanups of the scopes it executes, innermost last
/// (13.7). Generated code pushes one when a `defer` registers and pops it
/// before running it on a normal exit; a panic runs the rest. Frames stay
/// alive meanwhile: a panic ends the process, so it never unwinds the
/// native stack.
static PANICKING: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_push(code: tasks::Cleanup, closure: *mut u8) {
    tasks::push_cleanup(code, closure);
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_pop() {
    tasks::pop_cleanup();
    // A `defer` that ran before this one and panicked while a panic
    // unwinds was reported as suppressed, and is done (language 15.7).
    panics::defer_runs();
    tasks::defer_starts();
}

/// After a `defer` ran.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_end() {
    tasks::defer_ends();
}

fn run_cleanups() {
    tasks::run_cleanups();
}

/// Reports a language panic (spec 15.7, codes in 22.2), runs the pending
/// `defer` cleanups and terminates. A panic inside a cleanup is reported
/// as suppressed and the remaining cleanups still run. `message` may be
/// null; `location` is a static `file:line:column` string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic(code: u32, message: *const Str, location: *const Str) -> ! {
    unsafe { end_in_panic(code, message, location) }
}

/// A panic where compiled code unwinds when the call returns: one that a
/// `recover` of the running task stops is recorded, and the call returns
/// (language 15.9); any other ends the program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_unwind(code: u32, message: *const Str, location: *const Str) {
    if unsafe { panics::record(code, message, location) } {
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// A panic of the runtime: recorded when a `recover` of the running task
/// stops it, and the caller returns at once with a value nobody uses;
/// otherwise the program ends here.
pub(crate) fn raise(code: u32, message: &str, location: *const Str) {
    let message = str_new(message.as_bytes());
    if unsafe { panics::record(code, message, location) } {
        unsafe { tondo_rt_str_release(message) };
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// Reports a panic nothing recovers and ends the program.
unsafe fn end_in_panic(mut code: u32, message: *const Str, location: *const Str) -> ! {
    // `testing.failNow` reports as a failed assertion, which no `recover` stops.
    if code == panics::FAIL_NOW {
        code = 7;
    }
    flush_stdout();
    let message = (!message.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned());
    let mut location = String::from_utf8_lossy(unsafe { str_bytes(location) }).into_owned();
    // A panic inside the standard library points at the program's call.
    if location.starts_with("<std>/") {
        location = trace::caller().unwrap_or(location);
    }
    let suppressed = PANICKING.fetch_add(1, Ordering::SeqCst) > 0;
    // `testing.skip(reason)` ends a test as skipped, unless cleanup runs.
    if code == testing::SKIP && suppressed {
        code = 2001;
    } else if code == testing::SKIP {
        testing::terminal(code, &format!("{}\n --> {location}", message.unwrap_or_default()));
        run_cleanups();
        tasks::end_all(0)
    }
    // An assertion's data are its condition and the sides it compared.
    if code == 7
        && let Some(message) = &message
    {
        let line = |prefix: &str| message.lines().find_map(|l| l.strip_prefix(prefix)).map(str::to_string);
        let condition = line("  condition: ")
            .or_else(|| line("condition: "))
            .unwrap_or_else(|| message.lines().next().unwrap_or_default().to_string());
        let mut data = vec![("condition", panics::Datum::Text(condition))];
        if let (Some(left), Some(right)) = (line("  left:  "), line("  right: ")) {
            data.push(("left", panics::Datum::Text(left)));
            data.push(("right", panics::Datum::Text(right)));
        }
        panics::language_data(data);
    }
    let resumed = panics::resumed_stack();
    let panic = panics::Panic::new(code, panic_name(code), message.unwrap_or_default(), location);
    let text = panic.headline();
    let location = &panic.location;
    let prefix = if suppressed { "suppressed while unwinding: " } else { "" };
    if recorder::on() {
        let task = tasks::current_id();
        recorder::record(recorder::Event::Panic { task, message: format!("{text}\n --> {location}") });
    }
    if events::on() {
        events::add(events::What::Panicked(format!("{text} at {location}")));
    }
    // A debugger sees where the program's code panicked first.
    if !suppressed {
        debugger::panicked(&text);
    }
    let frames = resumed.as_deref().map(panics::frames_of);
    let stack = resumed.unwrap_or_else(trace::report);
    let report = format!("{prefix}{text}{}{stack}", panic.place());
    // A test build reports it to `tondo test` instead, with its data.
    testing::panic_data(&panic);
    if !testing::terminal(code, &report) {
        match panics::json_reports() {
            true => {
                let frames = frames.unwrap_or_else(trace::report_json);
                let _ = writeln!(std::io::stderr(), "{}", panic.json(&frames, false, suppressed));
            }
            false => {
                let _ = writeln!(std::io::stderr(), "{report}");
            }
        }
        if !suppressed {
            // Where the walk of the stack starts, before the frames of the
            // dump, which may lose it.
            diagnostics::faulted_at(0, trace::frame_pointer(), tasks::stack_bounds().unwrap_or((0, 0)));
            diagnostics::crash_dump("panic", &format!("{text}\n --> {location}"), trace::here_frames(), true);
        }
    }
    run_cleanups();
    // The other tasks run their cleanups too, before the process ends.
    tasks::end_all(PANIC_EXIT_CODE)
}

/// Panics with `code` and a message the runtime writes.
pub(crate) fn panic_with(code: u32, message: &str, location: *const Str) -> ! {
    let message = str_new(message.as_bytes());
    unsafe { tondo_rt_panic(code, message, location) }
}

/// The message of `index` outside the `len` elements of a `noun` (`P0001`).
pub(crate) fn out_of_bounds(index: i64, len: usize, noun: &str) -> String {
    if len == 0 {
        return format!("index {index} is out of bounds: the {noun} is empty");
    }
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("index {index} is out of bounds for {article} {noun} of length {len}: {}", valid_indices(len - 1, len))
}

/// The indices from 0 to `last`, and those from the end of `len` elements.
fn valid_indices(last: usize, len: usize) -> String {
    match (last, len) {
        (0, 0) => "the only valid index is 0".into(),
        (0, 1) => "the only valid indices are 0 and -1".into(),
        (_, 0) => format!("valid indices are 0 to {last}"),
        (_, 1) => format!("valid indices are 0 to {last}, and -1 from the end"),
        _ => format!("valid indices are 0 to {last}, and -{len} to -1 from the end"),
    }
}

/// A panic of compiled code whose message the runtime writes from the
/// values involved. `detail` holds the operation from bit 16, the
/// signedness of the right operand in bit 9 and of the left one in bit 8,
/// and the width of the left one in bits below; `ty` names its type.
/// Operations: `+ - * / %`, negation, `<< >>` from 0; for `P0001`, 0
/// indexes an array, 1 a C array and 2 a deque.
/// The values come in memory, `[a, b]`: as arguments, they would ask the
/// code that checks to keep them in the registers of a call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) -> ! {
    let message = unsafe { values_message(code, detail, values, ty) };
    panic_with(code, &message, location)
}

/// `tondo_rt_panic_values` where compiled code unwinds when it returns
/// (language 15.9).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values_unwind(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) {
    let message = unsafe { values_message(code, detail, values, ty) };
    raise(code, &message, location)
}

/// The message of a panic of compiled code, from its values, whose data it
/// stages.
unsafe fn values_message(code: u32, detail: u32, values: *const [u64; 4], ty: *const Str) -> String {
    // Two values of 128 bits, each extended by its signedness.
    let [a_low, a_high, b_low, b_high] = unsafe { *values };
    let (a, b) = ((a_high as i128) << 64 | a_low as i128, (b_high as i128) << 64 | b_low as i128);
    let (bits, op) = (detail & 0xff, detail >> 16);
    let show = |value: i128, signed: bool| if signed { value.to_string() } else { (value as u128).to_string() };
    let (left, right) = (show(a, detail & 0x100 != 0), show(b, detail & 0x200 != 0));
    let (a, b) = (a as i64, b as i64);
    let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
    let operator =
        ["+", "-", "*", "/", "%", "-", "<<", ">>", "pow", "nextPowerOfTwo"].get(op as usize).copied().unwrap_or("?");
    let range = || {
        let unused = 128 - bits.clamp(1, 128);
        match detail & 0x100 != 0 {
            true => format!("{} to {}", i128::MIN >> unused, i128::MAX >> unused),
            false => format!("0 to {}", u128::MAX >> unused),
        }
    };
    let int = |value: &str| value.parse::<i64>().map_or(panics::Datum::Text(value.to_string()), panics::Datum::Int);
    let data = match code {
        1 => vec![("index", panics::Datum::Int(a)), ("length", panics::Datum::Int(b))],
        3 => vec![("left", int(&left))],
        5 => vec![
            ("operation", panics::Datum::Text(operator.to_string())),
            ("left", int(&left)),
            ("right", int(&right)),
            ("type", panics::Datum::Text(ty.to_string())),
        ],
        6 => vec![("left", panics::Datum::Int(a)), ("right", panics::Datum::Int(b))],
        10 => vec![("count", int(&right)), ("width", panics::Datum::Int(i64::from(bits)))],
        18 => vec![("exponent", int(&right))],
        _ => Vec::new(),
    };
    panics::language_data(data);
    match code {
        1 if op == 1 => {
            format!("index {a} is out of bounds for the C array {ty}: the index must be from 0 to {}", b - 1)
        }
        1 if op == 2 => out_of_bounds(a, b as usize, "deque"),
        1 => out_of_bounds(a, b as usize, "array"),
        3 => format!("{left} {operator} 0"),
        5 if op == 5 => format!("-({left}) does not fit in {ty} ({})", range()),
        5 if op == 8 => format!("{left}.pow({right}) does not fit in {ty} ({})", range()),
        5 if op == 9 => format!("{left}.nextPowerOfTwo() does not fit in {ty} ({})", range()),
        5 if op == 10 => format!("{left} does not fit in {ty}"),
        18 => format!("{left}.pow({right}): the exponent must be 0 or more"),
        5 => format!("{left} {operator} {right} does not fit in {ty} ({})", range()),
        6 => format!("an array of length {a} cannot become the C array {ty}: it needs exactly {b} elements"),
        10 => format!("{left} {operator} {right}: a shift of {ty} must be from 0 to {}", bits - 1),
        _ => String::new(),
    }
}

/// Reports the error a fallible `main` returned (spec 20.10), of the type
/// `ty`: its text, or with `TONDO_PANIC_FORMAT=json` a line of
/// `tondo-panic-0.1/1` (toolchain 7.5).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_report_error(description: *const Str, ty: *const Str) {
    flush_stdout();
    let text = String::from_utf8_lossy(unsafe { str_bytes(description) });
    if panics::json_reports() {
        let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
        let _ = writeln!(
            std::io::stderr(),
            "{{\"code\":null,\"data\":null,\"error_type\":{},\"format\":\"tondo-panic-0.1/1\",\"kind\":\"error\",\"message\":{},\"name\":null,\"recovered\":false,\"source\":null,\"stack\":[]}}",
            diagnostics::quoted(&ty),
            diagnostics::quoted(&text),
        );
        return;
    }
    let _ = writeln!(std::io::stderr(), "error: {text}");
}

/// An internal invariant of the runtime or the generated code was broken.
fn runtime_fault(message: &str) -> ! {
    let _outside = replay::Outside::enter();
    flush_stdout();
    let _ = writeln!(std::io::stderr(), "tondo runtime fault: {message}{}", trace::report());
    diagnostics::crash_dump("fault", message, Vec::new(), false);
    std::process::abort()
}

/// Exit status when `TONDO_RT_CHECK_LEAKS` finds live strings at exit.
pub const LEAK_EXIT_CODE: i32 = 99;

pub fn check_leaks() -> Option<usize> {
    config::config().check_leaks.as_ref()?;
    // How long these waits take is no part of a recording.
    let _outside = replay::Outside::enter();
    tasks::wait_foreign_calls();
    tasks::wait_tasks_freed();
    // The contexts' initial values live as long as the program, and so do
    // the channels of signals.
    contexts::end();
    signals::end();
    // Garbage cycles are not leaks: the collector frees them. A thread of
    // the runtime may still be letting go of an object, as a read of a
    // stream that a reply had and a client closed: it gets a second.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        cycles::collect_all();
        let live = LIVE_OBJECTS.live();
        if live == 0 || std::time::Instant::now() >= deadline {
            return (live != 0).then_some(live);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

// ---------------------------------------------------------------------------
// Process entry
// ---------------------------------------------------------------------------

#[cfg(tondo_staticlib)]
#[global_allocator]
static ALLOCATOR: meta::Counting = meta::Counting;

/// `count` values of `T` in memory of their own, zeroed, which the system
/// gives a page at a time as they are written; null if it has none.
pub(crate) fn zeroed<T>(count: usize) -> *mut T {
    let size = count * std::mem::size_of::<T>();
    let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
    let memory = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0) };
    if memory == libc::MAP_FAILED { std::ptr::null_mut() } else { memory.cast() }
}

/// Set in a library, whose host has its own `main`.
static LIBRARY: AtomicBool = AtomicBool::new(false);

pub(crate) fn library() -> bool {
    LIBRARY.load(Ordering::Relaxed)
}

/// Starts a library at the first call from C: the runtime reads its
/// settings and the compiled code hands it its tables. What the library
/// wrote and did not deliver yet goes out when the process ends.
pub(crate) fn start_library(start: unsafe extern "C" fn()) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        LIBRARY.store(true, Ordering::Relaxed);
        config::config();
        #[cfg(tondo_staticlib)]
        meta::start();
        diagnostics::start();
        unsafe { start() };
        unsafe extern "C" {
            fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
        }
        extern "C" fn flush(_: *mut u8) {
            flush_stdout();
        }
        unsafe { __cxa_atexit(flush, std::ptr::null_mut(), std::ptr::null_mut()) };
    });
}

/// Runs a program: the `main` the compiler generates for an executable
/// calls it with `tondo_main`, which runs the program's `main` and returns
/// the process exit status. A library has neither.
#[cfg(tondo_staticlib)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_main(
    tondo_main: unsafe extern "C" fn() -> i32,
    argc: i32,
    argv: *const *const u8,
) -> i32 {
    unsafe { env::keep_arguments(argc, argv) };
    config::config();
    replay::start();
    meta::start();
    diagnostics::start();
    signals::start();
    debugger::start_stopped();
    replay::begin();
    let mut status = tasks::run_main(tondo_main);
    if let Some(status) = tasks::ending() {
        process::cancel_live();
        flush_stdout();
        return status;
    }
    // Memory the shared collections retired, kept while readers might see it.
    shared::flush();
    if let Some(failure) = finish_stdout()
        && status == 0
    {
        status = failure;
    }
    if let Some(live) = check_leaks() {
        let _ = writeln!(std::io::stderr(), "tondo runtime: {live} object(s) leaked");
        return LEAK_EXIT_CODE;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_compare_by_scalar_order_and_free_at_zero() {
        let a = str_new("abc".as_bytes());
        let b = str_new("abd".as_bytes());
        unsafe {
            assert_eq!(tondo_rt_str_cmp(a, b), -1);
            assert_eq!(tondo_rt_str_eq(a, a), 1);
            tondo_rt_str_retain(a);
            tondo_rt_str_release(a);
            assert_eq!((*a).rc.load(Ordering::Relaxed), 1);
            tondo_rt_str_release(a);
            tondo_rt_str_release(b);
        }
    }

    #[test]
    fn builder_formats_scalars() {
        let mut builder = Builder::new(2);
        let b = &mut builder as *mut Builder;
        let s = str_new("x=".as_bytes());
        unsafe {
            tondo_rt_sb_str(b, s);
            tondo_rt_sb_i64(b, -42);
            tondo_rt_sb_i64(b, i64::MIN);
            tondo_rt_sb_u64(b, 1_000_000);
            tondo_rt_sb_bool(b, 1);
            let out = tondo_rt_sb_finish(b);
            assert_eq!(str_bytes(out), b"x=-42-92233720368547758081000000true");
            tondo_rt_str_release(out);
            tondo_rt_str_release(s);
        }
    }
}
