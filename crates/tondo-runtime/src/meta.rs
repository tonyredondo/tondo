//! A provider or generator as it runs while compiling (27.7 of the
//! language): it reads its request and writes its response through files
//! the toolchain names, and stops at the budgets the toolchain sets. Steps
//! are counted, so a budget ends a run at the same point on every machine.

use super::config::config;
use super::{Str, str_bytes};
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
        left = config().meta_steps.as_deref().and_then(|s| s.parse().ok()).unwrap_or(i64::MAX - 1);
    }
    if left == 0 {
        over("step");
    }
    STEPS.store(left - 1, Ordering::Relaxed);
}

/// The request the toolchain wrote.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_meta_request(region: *mut crate::regions::Region) -> *mut Str {
    let text = config().meta_request.as_ref().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    crate::strings::str_in(region, &text)
}

/// Writes the response for the toolchain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_meta_respond(text: *const Str) {
    if let Some(path) = &config().meta_response {
        let _ = std::fs::write(path, unsafe { str_bytes(text) });
    }
}

/// The bytes a hexadecimal text writes, for a generator's inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_meta_bytes(
    hex: *const Str,
    region: *mut crate::regions::Region,
) -> *mut super::ArrayHeader {
    let hex = unsafe { str_bytes(hex) };
    let bytes: Vec<u8> = hex
        .chunks(2)
        .filter_map(|pair| std::str::from_utf8(pair).ok().and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect();
    crate::arrays::byte_array_in(region, &bytes)
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
unsafe extern "C" {
    fn mi_malloc(size: usize) -> *mut u8;
    fn mi_realloc(block: *mut u8, size: usize) -> *mut u8;
    fn mi_usable_size(block: *const u8) -> usize;
}

/// Memory none of the hooks counts, for the chunks of regions: what a
/// region holds is counted by value, apart from the heap, and a chunk is
/// no allocation of the program's.
pub(crate) unsafe fn uncounted_alloc(layout: Layout) -> *mut u8 {
    #[cfg(tondo_staticlib)]
    return unsafe { Heap.alloc(layout) };
    #[cfg(not(tondo_staticlib))]
    return unsafe { std::alloc::System.alloc(layout) };
}

pub(crate) unsafe fn uncounted_dealloc(block: *mut u8, layout: Layout) {
    #[cfg(tondo_staticlib)]
    unsafe {
        Heap.dealloc(block, layout)
    }
    #[cfg(not(tondo_staticlib))]
    unsafe {
        std::alloc::System.dealloc(block, layout)
    }
}

/// mimalloc's blocks are aligned to 8 bytes at least: what needs no more
/// takes its plain functions, which skip the work of the aligned ones.
#[cfg(tondo_staticlib)]
const PLAIN: usize = 8;

/// The bytes a block can hold, which may be more than were asked for: what
/// a string being built grows into before it moves. Exactly those asked
/// for while a budget counts the bytes of each block. A watch counts the
/// bytes the blocks hold, so that a program watched, as a test is, grows
/// its strings as it does otherwise.
#[cfg(tondo_staticlib)]
pub(crate) fn usable(block: *const u8, asked: usize) -> usize {
    use super::diagnostics::{BUDGET, hooks};
    match hooks() & BUDGET {
        0 => unsafe { mi_usable_size(block) },
        _ => asked,
    }
}

#[cfg(not(tondo_staticlib))]
pub(crate) fn usable(_: *const u8, asked: usize) -> usize {
    asked
}

#[cfg(tondo_staticlib)]
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(tondo_staticlib)]
static HELD: AtomicUsize = AtomicUsize::new(0);

/// Reads the memory budget, before the program starts.
#[cfg(tondo_staticlib)]
pub fn start() {
    if let Some(limit) = config().meta_memory.as_deref().and_then(|s| s.parse().ok()) {
        LIMIT.store(limit, Ordering::Relaxed);
        super::diagnostics::hook(super::diagnostics::BUDGET);
    }
}

#[cfg(tondo_staticlib)]
impl Counting {
    /// A block for `layout`, held against the budget and watched. Memory
    /// runs short only after a collection of garbage cycles found none to
    /// free (language 16.7); the collector itself may go past the budget
    /// while it frees.
    #[inline(always)]
    unsafe fn take(&self, layout: Layout, hooks: u8) -> *mut u8 {
        use super::diagnostics::{BUDGET, WATCHED};
        if hooks & BUDGET != 0 {
            budget(layout.size());
        }
        let get = || unsafe { if layout.align() <= PLAIN { mi_malloc(layout.size()) } else { Heap.alloc(layout) } };
        let mut block = get();
        if block.is_null() && super::cycles::collect_for_memory() {
            block = get();
        }
        if hooks & WATCHED != 0 && !block.is_null() {
            super::diagnostics::allocated(block, layout.size(), unsafe { mi_usable_size(block) });
        }
        block
    }
}

/// Counts a block that is new, or that grew, for a benchmark and for
/// `testing.allocations`.
#[cfg(tondo_staticlib)]
#[inline(always)]
fn counted(hooks: u8, size: usize) {
    use super::diagnostics::{COUNTED, MEASURED};
    if hooks & MEASURED != 0 {
        super::bench::allocated(size);
    }
    if hooks & COUNTED != 0 {
        super::tasks::count_allocation();
    }
}

#[cfg(tondo_staticlib)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let hooks = super::diagnostics::hooks();
        let block = unsafe { self.take(layout, hooks) };
        if !block.is_null() {
            counted(hooks, layout.size());
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
            super::diagnostics::freed(ptr, unsafe { mi_usable_size(ptr) });
        }
        unsafe { Heap.dealloc(ptr, layout) }
    }

    /// The system's, which grows and shrinks a block in place when it can.
    /// A benchmark and `testing.allocations` count a block that grows as an
    /// allocation.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        use super::diagnostics::{BUDGET, WATCHED, hooks};
        let hooks = hooks();
        if hooks & (BUDGET | WATCHED) != 0 {
            return unsafe { self.held_realloc(ptr, layout, new_size, hooks) };
        }
        let get = || unsafe {
            if layout.align() <= PLAIN { mi_realloc(ptr, new_size) } else { Heap.realloc(ptr, layout, new_size) }
        };
        let mut block = get();
        if block.is_null() && super::cycles::collect_for_memory() {
            block = get();
        }
        if !block.is_null() && new_size > layout.size() {
            counted(hooks, new_size);
        }
        block
    }
}

#[cfg(tondo_staticlib)]
impl Counting {
    /// `realloc` while a budget or a watch counts memory. A budget sees a
    /// block freed and another allocated. A watch sees the block freed and
    /// the one the system gives allocated, which may be the same, so that a
    /// program watched, as a test is, keeps the blocks it keeps otherwise.
    #[cold]
    #[inline(never)]
    unsafe fn held_realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize, hooks: u8) -> *mut u8 {
        use super::diagnostics::BUDGET;
        if hooks & BUDGET != 0 {
            let block = unsafe { self.take(Layout::from_size_align_unchecked(new_size, layout.align()), hooks) };
            if !block.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr, block, layout.size().min(new_size));
                    self.dealloc(ptr, layout);
                }
                if new_size > layout.size() {
                    counted(hooks, new_size);
                }
            }
            return block;
        }
        // Before the block may go back, where another thread may take it.
        let held = unsafe { mi_usable_size(ptr) };
        super::diagnostics::freed(ptr, held);
        let get = || unsafe {
            if layout.align() <= PLAIN { mi_realloc(ptr, new_size) } else { Heap.realloc(ptr, layout, new_size) }
        };
        let mut block = get();
        if block.is_null() && super::cycles::collect_for_memory() {
            block = get();
        }
        match block.is_null() {
            true => super::diagnostics::allocated(ptr, layout.size(), held),
            false => {
                super::diagnostics::allocated(block, new_size, unsafe { mi_usable_size(block) });
                if new_size > layout.size() {
                    counted(hooks, new_size);
                }
            }
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
