//! A provider or generator as it runs while compiling (27.7 of the
//! language): it reads its request and writes its response through files
//! the toolchain names, and stops at the budgets the toolchain sets. Steps
//! are counted, so a budget ends a run at the same point on every machine.

use super::{Str, str_bytes, str_new};
#[cfg(tondo_staticlib)]
use std::alloc::{GlobalAlloc, Layout};
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
/// `TONDO_META_MEMORY` sets; any other program only pays a load. Memory
/// comes from mimalloc: each thread allocates from its own heap, and a
/// block freed on another goes back to it, without a lock, where the
/// system's allocator makes threads wait for each other's.
#[cfg(tondo_staticlib)]
pub struct Counting;

#[cfg(tondo_staticlib)]
use mimalloc::MiMalloc as Heap;

#[cfg(tondo_staticlib)]
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(tondo_staticlib)]
static HELD: AtomicUsize = AtomicUsize::new(0);

/// Reads the memory budget, before the program starts.
#[cfg(tondo_staticlib)]
pub fn start() {
    if let Some(limit) = std::env::var("TONDO_META_MEMORY").ok().and_then(|s| s.parse().ok()) {
        LIMIT.store(limit, Ordering::Relaxed);
        super::diagnostics::hook(super::diagnostics::BUDGET);
    }
}

#[cfg(tondo_staticlib)]
unsafe impl GlobalAlloc for Counting {
    /// Memory runs short only after a collection of garbage cycles found
    /// none to free (language 16.7); the collector itself may go past the
    /// budget while it frees.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        use super::diagnostics::{BUDGET, MEASURED, WATCHED, hooks};
        let hooks = hooks();
        if hooks & BUDGET != 0 {
            budget(layout.size());
        }
        let mut block = unsafe { Heap.alloc(layout) };
        if block.is_null() && super::cycles::collect_for_memory() {
            block = unsafe { Heap.alloc(layout) };
        }
        if hooks & WATCHED != 0 && !block.is_null() {
            super::diagnostics::allocated(block, layout.size());
        }
        if hooks & MEASURED != 0 && !block.is_null() {
            super::bench::allocated(layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        use super::diagnostics::{BUDGET, WATCHED, hooks};
        let hooks = hooks();
        if hooks & BUDGET != 0 {
            HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        if hooks & WATCHED != 0 {
            super::diagnostics::freed(ptr, layout.size());
        }
        unsafe { Heap.dealloc(ptr, layout) }
    }

    /// The system's, which grows and shrinks a block in place when it can.
    /// A budget and a watch see a block freed and another allocated; a
    /// benchmark counts a block that grows as an allocation.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        use super::diagnostics::{BUDGET, MEASURED, WATCHED, hooks};
        let hooks = hooks();
        if hooks & (BUDGET | WATCHED) != 0 {
            let block = unsafe { self.alloc(Layout::from_size_align_unchecked(new_size, layout.align())) };
            if !block.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr, block, layout.size().min(new_size));
                    self.dealloc(ptr, layout);
                }
            }
            return block;
        }
        let mut block = unsafe { Heap.realloc(ptr, layout, new_size) };
        if block.is_null() && super::cycles::collect_for_memory() {
            block = unsafe { Heap.realloc(ptr, layout, new_size) };
        }
        if hooks & MEASURED != 0 && !block.is_null() && new_size > layout.size() {
            super::bench::allocated(new_size);
        }
        block
    }
}

/// Counts an allocation of a `meta` program against its budget; one that
/// goes past it ends the program, unless a collection makes room.
#[cfg(tondo_staticlib)]
#[cold]
fn budget(size: usize) {
    let limit = LIMIT.load(Ordering::Relaxed);
    if HELD.fetch_add(size, Ordering::Relaxed) + size > limit
        && !super::cycles::collecting_here()
        && !(super::cycles::collect_for_memory() && HELD.load(Ordering::Relaxed) <= limit)
    {
        over("memory");
    }
}
