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
mod client;
mod clock;
mod codec;
mod compress;
mod config;
mod console;
mod contexts;
mod counters;
mod cpu;
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
pub(crate) use panics::{
    PANIC_EXIT_CODE, out_of_bounds, panic_name, panic_with, raise, runtime_fault, tondo_rt_panic, valid_indices,
};
pub(crate) use references::*;
pub(crate) use strings::*;

/// How a native gives a `Result` to the compiled code: 0, with the value
/// stored where `store` puts it, or the code of the error, which says its
/// variant plus one.
pub(crate) fn status<T, E>(result: Result<T, E>, code: impl FnOnce(E) -> i64, store: impl FnOnce(T)) -> i64 {
    match result {
        Ok(value) => {
            store(value);
            0
        }
        Err(error) => code(error),
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
    if let Some(live) = panics::check_leaks() {
        let _ = writeln!(std::io::stderr(), "tondo runtime: {live} object(s) leaked");
        return panics::LEAK_EXIT_CODE;
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
