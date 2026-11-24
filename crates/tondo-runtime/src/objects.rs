//! Objects of the runtime in a region (toolchain 7.18). An object the
//! compiled code makes where only its locals hold it goes in their region:
//! its last reference, the count `LAST_IN_REGION`, drops its value as the
//! heap's last reference does, which lets go of what it holds outside, as
//! a descriptor, and leaves its memory to the region. No value of a region
//! holds one (`regions.rs` of the compiler), so its count reaches its last
//! reference where it would on the heap, and nothing is left for the
//! region's end to drop.
//!
//! An object's count is its first word.

use super::regions::{LAST_IN_REGION, Region};
use std::sync::atomic::{AtomicUsize, Ordering, fence};

/// `object`, whose first word is its count, in `region` when one is given,
/// with the count of a region's last reference, or on the heap with one
/// reference.
pub(crate) unsafe fn make<T>(region: *mut Region, object: T) -> *mut T {
    const { assert!(std::mem::align_of::<T>() <= 16, "a region aligns to 16 bytes") };
    if region.is_null() {
        super::LIVE_OBJECTS.made();
        return Box::into_raw(Box::new(object));
    }
    unsafe {
        let at = super::regions::tondo_rt_region_alloc(region, std::mem::size_of::<T>()) as *mut T;
        at.write(object);
        (*(at as *const AtomicUsize)).store(LAST_IN_REGION, Ordering::Relaxed);
        at
    }
}

/// What a release of an object with the count `rc` leaves to do: `None`
/// while references are left; for the last, whether the object is the
/// heap's, to be dropped and freed, or a region's, whose value alone is
/// dropped.
pub(crate) fn dropped(rc: &AtomicUsize) -> Option<bool> {
    let before = match rc.load(Ordering::Relaxed) {
        super::strings::IMMORTAL => return None,
        // The last reference: no other thread holds one to count with.
        last @ (1 | LAST_IN_REGION) => last,
        _ => rc.fetch_sub(1, Ordering::Release),
    };
    match before {
        1 => {
            fence(Ordering::Acquire);
            Some(true)
        }
        LAST_IN_REGION => {
            fence(Ordering::Acquire);
            Some(false)
        }
        _ => None,
    }
}
