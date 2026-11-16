//! Regions: memory a frame takes by moving a pointer and gives back whole
//! when it ends, for the values the compiler shows nothing keeps past the
//! call (`regions.rs` of the compiler says which). A region is a record in
//! the frame that owns it, the current chunk's free room and the chunks it
//! took; a function that builds what its caller lets go is given the
//! caller's region. `tondo_rt_region_alloc` moves the pointer when the
//! chunk has room, and takes a new chunk when it has none; leaving the
//! region gives its chunks back, those of the usual size to the thread's
//! own few and then to a list the threads share, for the next region to
//! take without a lock or the allocator: a task that builds a tree in each
//! turn may go on in another thread.
//!
//! A value in a region carries `REGION` in its count, which goes on
//! counting below it, so that a change knows when it holds the only
//! reference and may write in place; its last release does nothing, as the
//! region lets go of it whole, and of what it holds, which a region made
//! too.
//!
//! While a benchmark, `testing.allocations`, a profile or the tools count
//! what the program allocates, each value a region makes is counted too,
//! apart from the heap's: a region made meanwhile keeps its end at its
//! pointer, so that the generated code finds no room and each value comes
//! to `tondo_rt_region_alloc`'s slow path, which counts it, as does a
//! loop's region from its next turn. A program nobody watches never takes
//! that path for a value.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// The bit of the count of a value in a region.
pub(crate) const REGION: usize = 1 << 61;

/// The count of the last reference to a value in a region.
pub(crate) const LAST_IN_REGION: usize = REGION | 1;

/// Whether a count is that of a value's only reference, in a region or not.
#[inline(always)]
pub(crate) fn only(rc: &AtomicUsize) -> bool {
    rc.load(Ordering::Acquire) & !REGION == 1
}

/// A frame's region: the free room of its current chunk, and its chunks,
/// the current one first while there is one. All zero before its first
/// value; `end` at `ptr` while what it makes is counted.
#[repr(C)]
pub struct Region {
    ptr: usize,
    end: usize,
    chunks: *mut Chunk,
}

/// A chunk of a region, its bytes after it: its size, and `SAMPLED` when a
/// sample of what is alive lies in it, which goes with it.
#[repr(C)]
struct Chunk {
    next: *mut Chunk,
    size: usize,
}

const SAMPLED: usize = 1;

impl Chunk {
    fn bytes(&self) -> usize {
        self.size & !SAMPLED
    }
}

/// The usual size of a chunk, header included; a value larger than its
/// room takes a chunk of its own.
const CHUNK: usize = 32 * 1024;
const CHUNK_HEADER: usize = std::mem::size_of::<Chunk>();
/// The chunks of the usual size kept for the next region, 32 MiB.
const KEPT: usize = 1024;

/// Chunks kept, linked, and how many.
struct Kept {
    first: *mut Chunk,
    count: usize,
}

unsafe impl Send for Kept {}

impl Kept {
    unsafe fn push(&mut self, chunk: *mut Chunk) {
        unsafe { (*chunk).next = self.first };
        self.first = chunk;
        self.count += 1;
    }

    unsafe fn pop(&mut self) -> Option<*mut Chunk> {
        let chunk = self.first;
        if chunk.is_null() {
            return None;
        }
        self.first = unsafe { (*chunk).next };
        self.count -= 1;
        Some(chunk)
    }
}

static FREE: std::sync::Mutex<Kept> = std::sync::Mutex::new(Kept { first: std::ptr::null_mut(), count: 0 });

/// The chunks a thread keeps for its next regions, before the list the
/// threads share, so that a function that enters and leaves a region at
/// each call takes no lock: kept with what the runtime keeps for the
/// thread, which allocates nothing more for them.
const LOCAL: usize = 8;

/// This thread's chunks, lent to `f`.
fn local<R>(f: impl FnOnce(&mut Kept) -> R) -> R {
    let kept = super::tasks::kept();
    let (first, count) = kept.chunks.get();
    let mut local = Kept { first: first as *mut Chunk, count };
    let result = f(&mut local);
    kept.chunks.set((local.first as *mut u8, local.count));
    result
}

/// A thread ends: the chunks it kept go to the list the threads share.
pub(crate) fn thread_ended((first, count): (*mut u8, usize)) {
    let mut local = Kept { first: first as *mut Chunk, count };
    let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
    while let Some(chunk) = unsafe { local.pop() } {
        super::diagnostics::regions_took(0, -(CHUNK as i64));
        match free.count < KEPT {
            true => unsafe { free.push(chunk) },
            false => unsafe { super::meta::uncounted_dealloc(chunk as *mut u8, chunk_layout(CHUNK)) },
        }
    }
}

/// The bytes of the chunks the threads share for the next regions.
pub(crate) fn shared_kept() -> usize {
    FREE.lock().unwrap_or_else(|e| e.into_inner()).count * CHUNK
}

fn chunk_layout(size: usize) -> Layout {
    Layout::from_size_align(size, 16).expect("region chunk too large")
}

/// A chunk of `size` bytes, a kept one when it is of the usual size: the
/// thread's, else the threads'.
unsafe fn take(size: usize) -> *mut Chunk {
    if size == CHUNK {
        if let Some(chunk) = local(|kept| unsafe { kept.pop() }) {
            super::diagnostics::regions_took(CHUNK as i64, -(CHUNK as i64));
            return chunk;
        }
        if let Some(chunk) = unsafe { FREE.lock().unwrap_or_else(|e| e.into_inner()).pop() } {
            super::diagnostics::regions_took(CHUNK as i64, 0);
            return chunk;
        }
    }
    super::diagnostics::regions_took(size as i64, 0);
    let layout = chunk_layout(size);
    let chunk = unsafe { super::meta::uncounted_alloc(layout) } as *mut Chunk;
    if chunk.is_null() {
        handle_alloc_error(layout);
    }
    unsafe { chunk.write(Chunk { next: std::ptr::null_mut(), size }) };
    chunk
}

/// Room for `size` bytes in the region, aligned to 16.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_alloc(region: *mut Region, size: usize) -> *mut u8 {
    let size = (size + 15) & !15;
    unsafe {
        let r = &mut *region;
        if r.ptr != 0 && r.ptr + size <= r.end {
            let at = r.ptr;
            r.ptr += size;
            return at as *mut u8;
        }
        more(r, size)
    }
}

/// Room when `end` leaves none: in the current chunk while what the region
/// makes is counted, and counted; else in a new chunk, or in one of its own
/// for a large value, which leaves the current chunk's room for the next.
#[cold]
#[inline(never)]
unsafe fn more(r: &mut Region, size: usize) -> *mut u8 {
    use super::diagnostics::{COUNTED, MEASURED, WATCHED, hooks};
    let hooks = hooks() & (MEASURED | COUNTED | WATCHED);
    unsafe {
        let (at, chunk) = if r.ptr != 0 && r.ptr + size <= r.chunks as usize + CHUNK {
            r.ptr += size;
            (r.ptr - size, r.chunks)
        } else if size + CHUNK_HEADER > CHUNK {
            let chunk = take(size + CHUNK_HEADER);
            // After the current chunk, which stays first.
            let before = if r.ptr != 0 { &mut (*r.chunks).next } else { &mut r.chunks };
            (*chunk).next = *before;
            *before = chunk;
            (chunk as usize + CHUNK_HEADER, chunk)
        } else {
            let chunk = take(CHUNK);
            (*chunk).next = r.chunks;
            r.chunks = chunk;
            r.ptr = chunk as usize + CHUNK_HEADER + size;
            (r.ptr - size, chunk)
        };
        r.end = match (r.ptr, hooks) {
            (0, _) => 0,
            (_, 0) => r.chunks as usize + CHUNK,
            (ptr, _) => ptr,
        };
        if hooks != 0 {
            counted(hooks, at, size, chunk);
        }
        at as *mut u8
    }
}

/// Counts a value a region made, for what counts: a benchmark, the task's
/// `testing.allocations`, and the tools, which sample what is alive by the
/// chunk it is in.
#[cold]
unsafe fn counted(hooks: u8, at: usize, size: usize, chunk: *mut Chunk) {
    use super::diagnostics::{COUNTED, MEASURED, WATCHED};
    if hooks & MEASURED != 0 {
        super::bench::region_allocated(size);
    }
    if hooks & COUNTED != 0 {
        super::tasks::count_region_allocation();
    }
    if hooks & WATCHED != 0 && super::diagnostics::region_allocated(at, size) {
        unsafe { (*chunk).size |= SAMPLED };
    }
}

/// The chunk goes, or is emptied: the samples alive in it go too.
unsafe fn emptied(chunk: *mut Chunk) {
    unsafe {
        if (*chunk).size & SAMPLED != 0 {
            super::heap::region_freed(chunk as usize, (*chunk).bytes());
            (*chunk).size &= !SAMPLED;
        }
    }
}

/// Gives the region's chunks back, the usual ones to the thread's list up
/// to what it keeps, then to the threads' up to what that keeps, and
/// leaves it empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_leave(region: *mut Region) {
    unsafe {
        let r = &mut *region;
        let mut chunk = r.chunks;
        let (mut given, mut kept) = (0, 0);
        let mut shared = Kept { first: std::ptr::null_mut(), count: 0 };
        while !chunk.is_null() {
            let next = (*chunk).next;
            emptied(chunk);
            let size = (*chunk).size;
            given += size as i64;
            if size != CHUNK {
                super::meta::uncounted_dealloc(chunk as *mut u8, chunk_layout(size));
            } else if local(|kept| (kept.count < LOCAL).then(|| kept.push(chunk)).is_some()) {
                kept += CHUNK as i64;
            } else {
                shared.push(chunk);
            }
            chunk = next;
        }
        if shared.count > 0 {
            let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
            while let Some(chunk) = shared.pop() {
                match free.count < KEPT {
                    true => free.push(chunk),
                    false => super::meta::uncounted_dealloc(chunk as *mut u8, chunk_layout(CHUNK)),
                }
            }
        }
        super::diagnostics::regions_took(-given, kept);
        *r = Region { ptr: 0, end: 0, chunks: std::ptr::null_mut() };
    }
}

/// A new array of `len` elements, for the caller to write, in the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_new_in(region: *mut Region, size: usize, len: usize) -> *mut ArrayHeader {
    unsafe { arrays::array_in(region, size, len, len) }
}

/// A new box of `size` bytes, for the caller to write, in the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_box_new_in(region: *mut Region, size: usize) -> *mut boxes::BoxHeader {
    unsafe { boxes::box_in(region, size) }
}

/// Appends the value at `value` to the array in `slot`, as
/// `tondo_rt_array_push` does, growing it in the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_push_in(
    region: *mut Region,
    slot: *mut *mut ArrayHeader,
    value: *const u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe {
        arrays::reserve_in(slot, size, 0, 1, false, retain, release, region);
        let a = *slot;
        std::ptr::copy_nonoverlapping(value, arrays::element(a, size, (*a).len), size);
        (*a).len += 1;
    }
}

/// Room for `extra` more elements in the array in `slot`, as
/// `tondo_rt_array_reserve` gives, in the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_reserve_in(
    region: *mut Region,
    slot: *mut *mut ArrayHeader,
    size: usize,
    extra: isize,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe { arrays::reserve_hint(slot, size, extra, retain, release, region) }
}

/// Empties the region for a loop's next turn: the chunk the next value
/// would go in stays, emptied, and the others go back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_reset(region: *mut Region) {
    unsafe {
        let r = &mut *region;
        use super::diagnostics::{COUNTED, MEASURED, WATCHED, hooks};
        let current = if r.ptr != 0 { r.chunks } else { std::ptr::null_mut() };
        // A loop's region counts from the turn after counting starts.
        let counting = hooks() & (MEASURED | COUNTED | WATCHED) != 0;
        let mut kept = std::ptr::null_mut();
        let mut chunk = r.chunks;
        let mut others = Region { ptr: 0, end: 0, chunks: std::ptr::null_mut() };
        while !chunk.is_null() {
            let next = (*chunk).next;
            if chunk == current && (*chunk).bytes() == CHUNK {
                kept = chunk;
            } else {
                (*chunk).next = others.chunks;
                others.chunks = chunk;
            }
            chunk = next;
        }
        tondo_rt_region_leave(&mut others);
        if kept.is_null() {
            *r = Region { ptr: 0, end: 0, chunks: std::ptr::null_mut() };
            return;
        }
        emptied(kept);
        (*kept).next = std::ptr::null_mut();
        let ptr = kept as usize + CHUNK_HEADER;
        let end = if counting { ptr } else { kept as usize + CHUNK };
        *r = Region { ptr, end, chunks: kept };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region { ptr: 0, end: 0, chunks: std::ptr::null_mut() }
    }

    /// Values follow one another in a chunk, aligned; a large one takes a
    /// chunk of its own and leaves the current one's room to the next.
    #[test]
    fn values_follow_one_another() {
        let mut r = region();
        unsafe {
            let a = tondo_rt_region_alloc(&mut r, 24);
            let b = tondo_rt_region_alloc(&mut r, 8);
            assert_eq!(a as usize % 16, 0);
            assert_eq!(b as usize, a as usize + 32);
            let large = tondo_rt_region_alloc(&mut r, CHUNK * 2);
            assert_ne!(large as usize, b as usize + 16);
            std::ptr::write_bytes(large, 7, CHUNK * 2);
            let c = tondo_rt_region_alloc(&mut r, 16);
            assert_eq!(c as usize, b as usize + 16);
            tondo_rt_region_leave(&mut r);
            assert_eq!((r.ptr, r.end), (0, 0));
            assert!(r.chunks.is_null());
        }
    }

    /// A region that fills its chunk takes another, and the next region
    /// takes a chunk the first gave back.
    #[test]
    fn chunks_are_kept_for_the_next_region() {
        let mut r = region();
        unsafe {
            let first = tondo_rt_region_alloc(&mut r, 64);
            for _ in 0..CHUNK / 64 {
                tondo_rt_region_alloc(&mut r, 64);
            }
            assert!(!(*r.chunks).next.is_null());
            tondo_rt_region_leave(&mut r);
            // The thread keeps them, within what it keeps.
            assert!((1..=LOCAL).contains(&local(|kept| kept.count)));
            let mut again = region();
            let reused = tondo_rt_region_alloc(&mut again, 64);
            assert_eq!(reused as usize % 16, 0);
            assert_ne!(first as usize, 0);
            tondo_rt_region_leave(&mut again);
        }
    }

    /// An array in a region is written in place while it is the only
    /// reference, copied when shared, and its last release lets go of
    /// nothing; one that outgrows its room moves to the heap, leaving its
    /// storage to the region.
    #[test]
    fn an_array_in_a_region() {
        let mut r = region();
        unsafe {
            let a = tondo_rt_array_new_in(&mut r, 8, 2);
            *(arrays::element(a, 8, 0) as *mut i64) = 4;
            *(arrays::element(a, 8, 1) as *mut i64) = 5;
            assert!(arrays::unique(a));
            arrays::tondo_rt_array_retain(a);
            assert!(!arrays::unique(a));
            arrays::tondo_rt_array_release(a, 8, None);
            assert!(arrays::unique(a));
            let mut slot = a;
            arrays::tondo_rt_array_reserve(&mut slot, 8, 100, None, None);
            assert_ne!(slot, a);
            assert_eq!(*(arrays::element(slot, 8, 1) as *const i64), 5);
            arrays::tondo_rt_array_release(slot, 8, None);
            let b = tondo_rt_array_new_in(&mut r, 8, 0);
            arrays::tondo_rt_array_release(b, 8, None);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// A box in a region is written in place while it is the only
    /// reference, and its last release lets go of nothing.
    #[test]
    fn a_box_in_a_region() {
        let mut r = region();
        unsafe {
            let b = tondo_rt_box_new_in(&mut r, 16);
            let mut slot = b;
            boxes::tondo_rt_box_unique(&mut slot, None, None);
            assert_eq!(slot, b);
            boxes::tondo_rt_box_retain(b);
            boxes::tondo_rt_box_unique(&mut slot, None, None);
            assert_ne!(slot, b);
            boxes::tondo_rt_box_release(slot, None);
            boxes::tondo_rt_box_release(b, None);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// An array pushed to in a region grows in it, from `[]` or from an
    /// array of the heap, which goes.
    #[test]
    fn an_array_grows_in_its_region() {
        let mut r = region();
        unsafe {
            let mut slot = arrays::tondo_rt_array_new(8, 0);
            for value in 0..100i64 {
                tondo_rt_array_push_in(&mut r, &mut slot, &value as *const i64 as *const u8, 8, None, None);
            }
            assert_eq!((*slot).len, 100);
            assert_eq!(*(arrays::element(slot, 8, 99) as *const i64), 99);
            assert!(arrays::unique(slot));
            assert_ne!((*slot).rc.load(Ordering::Relaxed) & REGION, 0);
            arrays::tondo_rt_array_release(slot, 8, None);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// A loop's region keeps its current chunk for the next turn, emptied,
    /// and gives back the others.
    #[test]
    fn a_reset_keeps_the_current_chunk() {
        let mut r = region();
        unsafe {
            let first = tondo_rt_region_alloc(&mut r, 64);
            tondo_rt_region_reset(&mut r);
            assert_eq!(tondo_rt_region_alloc(&mut r, 64), first);
            for _ in 0..CHUNK / 64 {
                tondo_rt_region_alloc(&mut r, 64);
            }
            tondo_rt_region_alloc(&mut r, CHUNK * 2);
            tondo_rt_region_reset(&mut r);
            assert!(!r.chunks.is_null() && (*r.chunks).next.is_null());
            assert_eq!(r.ptr, r.chunks as usize + CHUNK_HEADER);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// While a benchmark counts, each value a region makes comes to the
    /// slow path and is counted, a loop's turn after turn; once it stops,
    /// the region goes back to moving its pointer alone.
    #[test]
    fn what_a_region_makes_is_counted_while_asked() {
        use crate::diagnostics::{MEASURED, hook, unhook};
        let mut r = region();
        unsafe {
            hook(MEASURED);
            let before = crate::bench::region_counts();
            let first = tondo_rt_region_alloc(&mut r, 24);
            assert_eq!(r.end, r.ptr);
            let second = tondo_rt_region_alloc(&mut r, 8);
            assert_eq!(second as usize, first as usize + 32);
            tondo_rt_region_reset(&mut r);
            assert_eq!(r.end, r.ptr);
            tondo_rt_region_alloc(&mut r, 16);
            let after = crate::bench::region_counts();
            unhook(MEASURED);
            assert!(after.0 - before.0 >= 3 && after.1 - before.1 >= 32 + 16 + 16);
            tondo_rt_region_alloc(&mut r, 16);
            assert!(r.end > r.ptr);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// A region that a function enters and leaves takes its chunk back from
    /// the thread, without the list the threads share.
    #[test]
    fn a_thread_keeps_chunks_for_its_next_regions() {
        let mut r = region();
        unsafe {
            tondo_rt_region_alloc(&mut r, 64);
            let chunk = r.chunks;
            tondo_rt_region_leave(&mut r);
            assert_eq!(local(|kept| kept.first), chunk);
            tondo_rt_region_alloc(&mut r, 64);
            assert_eq!(r.chunks, chunk);
            tondo_rt_region_leave(&mut r);
        }
    }

    /// A count in a region counts below its bit.
    #[test]
    fn a_count_in_a_region_counts_below_its_bit() {
        let rc = AtomicUsize::new(LAST_IN_REGION);
        assert!(only(&rc));
        rc.fetch_add(1, Ordering::Relaxed);
        assert!(!only(&rc));
        assert!(only(&AtomicUsize::new(1)));
    }
}
