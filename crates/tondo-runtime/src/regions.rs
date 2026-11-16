//! Regions: memory a frame takes by moving a pointer and gives back whole
//! when it ends, for the values the compiler shows nothing keeps past the
//! call (`regions.rs` of the compiler says which). A region is a record in
//! the frame that owns it, the current chunk's free room and the chunks it
//! took; a function that builds what its caller lets go is given the
//! caller's region. The generated code moves the pointer when the chunk
//! has room, and calls `tondo_rt_region_alloc` for a new chunk; leaving
//! the region gives its chunks back, those of the usual size to a list of
//! the thread's, for the next region to take without the allocator.
//!
//! A value in a region carries `REGION` in its count, which goes on
//! counting below it, so that a change knows when it holds the only
//! reference and may write in place; its last release does nothing, as the
//! region lets go of it whole, and of what it holds, which a region made
//! too.

use std::cell::Cell;
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

/// A frame's region: the free room of its current chunk, and its chunks.
/// All zero before its first value.
#[repr(C)]
pub struct Region {
    ptr: usize,
    end: usize,
    chunks: *mut Chunk,
}

/// A chunk of a region, its bytes after it.
#[repr(C)]
struct Chunk {
    next: *mut Chunk,
    size: usize,
}

/// The usual size of a chunk, header included; a value larger than its
/// room takes a chunk of its own.
const CHUNK: usize = 32 * 1024;
const CHUNK_HEADER: usize = std::mem::size_of::<Chunk>();
/// The chunks of the usual size a thread keeps for the next region.
const KEPT: usize = 64;

thread_local! {
    static FREE: Cell<*mut Chunk> = const { Cell::new(std::ptr::null_mut()) };
    static FREE_COUNT: Cell<usize> = const { Cell::new(0) };
}

fn chunk_layout(size: usize) -> Layout {
    Layout::from_size_align(size, 16).expect("region chunk too large")
}

/// A chunk of `size` bytes, a kept one when it is of the usual size.
unsafe fn take(size: usize) -> *mut Chunk {
    if size == CHUNK {
        let kept = FREE.with(|free| free.get());
        if !kept.is_null() {
            FREE.with(|free| free.set(unsafe { (*kept).next }));
            FREE_COUNT.with(|count| count.set(count.get() - 1));
            return kept;
        }
    }
    let layout = chunk_layout(size);
    let chunk = unsafe { alloc(layout) } as *mut Chunk;
    if chunk.is_null() {
        handle_alloc_error(layout);
    }
    unsafe { chunk.write(Chunk { next: std::ptr::null_mut(), size }) };
    chunk
}

/// Room for `size` bytes in the region, aligned to 16, when its current
/// chunk has none: a new chunk, or one of its own for a large value, which
/// leaves the current chunk's room for the next.
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
        if size + CHUNK_HEADER > CHUNK {
            let chunk = take(size + CHUNK_HEADER);
            (*chunk).next = r.chunks;
            r.chunks = chunk;
            return (chunk as *mut u8).add(CHUNK_HEADER);
        }
        let chunk = take(CHUNK);
        (*chunk).next = r.chunks;
        r.chunks = chunk;
        let start = chunk as usize + CHUNK_HEADER;
        r.ptr = start + size;
        r.end = chunk as usize + CHUNK;
        start as *mut u8
    }
}

/// Gives the region's chunks back, the usual ones to the thread's list up
/// to what it keeps, and leaves it empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_leave(region: *mut Region) {
    unsafe {
        let r = &mut *region;
        let mut chunk = r.chunks;
        while !chunk.is_null() {
            let next = (*chunk).next;
            let size = (*chunk).size;
            if size == CHUNK && FREE_COUNT.with(|count| count.get()) < KEPT {
                (*chunk).next = FREE.with(|free| free.get());
                FREE.with(|free| free.set(chunk));
                FREE_COUNT.with(|count| count.set(count.get() + 1));
            } else {
                dealloc(chunk as *mut u8, chunk_layout(size));
            }
            chunk = next;
        }
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
            assert!(FREE_COUNT.with(|count| count.get()) <= KEPT);
            let kept = FREE.with(|free| free.get()) as usize;
            assert_ne!(kept, 0);
            let mut again = region();
            let reused = tondo_rt_region_alloc(&mut again, 64);
            assert_eq!(reused as usize, kept + CHUNK_HEADER);
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
