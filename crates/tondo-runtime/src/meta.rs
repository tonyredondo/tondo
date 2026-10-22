//! A provider or generator as it runs while compiling (27.7 of the
//! language): it reads its request and writes its response through files
//! the toolchain names, and stops at the budgets the toolchain sets. Steps
//! are counted, so a budget ends a run at the same point on every machine.

use super::{Str, str_bytes, str_new};
#[cfg(tondo_staticlib)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
#[cfg(tondo_staticlib)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicI64, Ordering};

/// The exit status of a run that went over a budget.
pub const BUDGET_EXIT_CODE: i32 = 70;

/// Steps left; negative until the first step reads the budget.
static STEPS: AtomicI64 = AtomicI64::new(-1);

fn over(what: &str) -> ! {
    let _ = writeln!(std::io::stderr(), "tondo meta: the {what} budget is spent");
    std::process::exit(BUDGET_EXIT_CODE)
}

/// One step: a call or a turn of a loop of a program built for `meta`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_meta_step() {
    let mut left = STEPS.load(Ordering::Relaxed);
    if left < 0 {
        left = std::env::var("TONDO_META_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(i64::MAX - 1);
    }
    if left == 0 {
        over("step");
    }
    STEPS.store(left - 1, Ordering::Relaxed);
}

/// The request the toolchain wrote.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_meta_request() -> *mut Str {
    let text = std::env::var_os("TONDO_META_REQUEST").and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    str_new(&text)
}

/// Writes the response for the toolchain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_meta_respond(text: *const Str) {
    if let Some(path) = std::env::var_os("TONDO_META_RESPONSE") {
        let _ = std::fs::write(path, unsafe { str_bytes(text) });
    }
}

/// The bytes a hexadecimal text writes, for a generator's inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_meta_bytes(hex: *const Str) -> *mut super::ArrayHeader {
    let hex = unsafe { str_bytes(hex) };
    let bytes: Vec<u8> = hex
        .chunks(2)
        .filter_map(|pair| std::str::from_utf8(pair).ok().and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect();
    let array = super::tondo_rt_array_new(1, bytes.len());
    for (index, byte) in bytes.into_iter().enumerate() {
        unsafe { *super::element(array, 1, index) = byte };
    }
    array
}

/// Counts the memory a `meta` program holds, up to the limit that
/// `TONDO_META_MEMORY` sets; any other program only pays a load.
#[cfg(tondo_staticlib)]
pub struct Counting;

#[cfg(tondo_staticlib)]
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(tondo_staticlib)]
static HELD: AtomicUsize = AtomicUsize::new(0);

/// Reads the memory budget, before the program starts.
#[cfg(tondo_staticlib)]
pub fn start() {
    if let Some(limit) = std::env::var("TONDO_META_MEMORY").ok().and_then(|s| s.parse().ok()) {
        LIMIT.store(limit, Ordering::Relaxed);
    }
}

#[cfg(tondo_staticlib)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let limit = LIMIT.load(Ordering::Relaxed);
        if limit != usize::MAX && HELD.fetch_add(layout.size(), Ordering::Relaxed) + layout.size() > limit {
            over("memory");
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if LIMIT.load(Ordering::Relaxed) != usize::MAX {
            HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}
