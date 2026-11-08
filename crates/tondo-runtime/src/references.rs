//! References (docs/refactor.md, phase 5): the object of a `Ref[T]`, its
//! strong and weak counts, and its value.

use super::*;

/// Header of the object of a `Ref[T]` (16.10), its identity: the value
/// follows. `weak` counts the weak references, plus one while any strong
/// one lives; the value goes with the last strong reference and the
/// memory with the last of either. `dead` is set by the cycle collector on
/// garbage whose counts have not reached zero yet.
#[repr(C)]
pub struct RefHeader {
    pub(crate) rc: AtomicUsize,
    weak: AtomicUsize,
    size: usize,
    dead: AtomicBool,
}

pub(crate) const REF_HEADER: usize = std::mem::size_of::<RefHeader>();

pub(crate) fn ref_layout(size: usize) -> Layout {
    Layout::from_size_align(REF_HEADER + size, 8).expect("reference too large")
}

/// A new identity with one reference; the caller stores the value. A live
/// build keeps the value apart, where a reload can put one of another size
/// (toolchain 7.10).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_ref_new(size: usize) -> *mut RefHeader {
    let apart = live::on();
    let cell = if apart { 8 } else { size };
    let layout = ref_layout(cell);
    unsafe {
        let ptr = alloc(layout) as *mut RefHeader;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        let header =
            RefHeader { rc: AtomicUsize::new(1), weak: AtomicUsize::new(1), size: cell, dead: AtomicBool::new(false) };
        ptr.write(header);
        if apart {
            let value = libc::calloc(1, size.max(1));
            ((ptr as *mut u8).add(REF_HEADER) as *mut *mut libc::c_void).write(value);
        }
        LIVE_OBJECTS.made();
        ptr
    }
}

/// Where a reference's value is: after its header, or in a live build
/// where its header says.
pub(crate) fn ref_payload(r: *mut u8) -> *mut u8 {
    let at = unsafe { r.add(REF_HEADER) };
    match live::on() {
        true => unsafe { (at as *mut *mut u8).read() },
        false => at,
    }
}

/// In a live build, where a reference's value is now: a reload converts
/// it into one of another size.
pub(crate) fn ref_set_payload(r: *mut u8, value: *mut u8) {
    unsafe { (r.add(REF_HEADER) as *mut *mut u8).write(value) };
}

/// Releases a reference; the last one releases the value's contents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_ref_release(r: *mut RefHeader, release: Option<Glue>) {
    if r.is_null() {
        return;
    }
    match unsafe { (*r).rc.fetch_sub(1, Ordering::Release) } {
        1 => {
            fence(Ordering::Acquire);
            let value = ref_payload(r as *mut u8);
            // A live build's code may be older than the value.
            let release = if live::on() { live::forget_object(r as usize) } else { release };
            if let Some(release) = release {
                unsafe { release(value) };
            }
            if live::on() {
                unsafe { libc::free(value as *mut libc::c_void) };
            }
            unsafe { ref_forget(r) };
        }
        0 => runtime_fault("reference released after it was freed"),
        _ => {}
    }
}

/// Marks a reference the cycle collector found to be garbage: its weak
/// references no longer give it.
pub(crate) fn ref_mark_dead(r: *mut u8) {
    unsafe { (*(r as *const RefHeader)).dead.store(true, Ordering::Release) };
}

/// `WeakRef(reference)`: a weak reference to the same object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_weak_new(r: *mut RefHeader) -> *mut RefHeader {
    unsafe { tondo_rt_weak_retain(r) };
    r
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_weak_retain(r: *mut RefHeader) {
    if !r.is_null() {
        unsafe { (*r).weak.fetch_add(1, Ordering::Relaxed) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_weak_release(r: *mut RefHeader) {
    if !r.is_null() {
        unsafe { ref_forget(r) };
    }
}

/// `weak.get()`: a strong reference while the object lives, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_weak_get(r: *mut RefHeader) -> *mut RefHeader {
    let header = unsafe { &*r };
    if header.dead.load(Ordering::Acquire) {
        return std::ptr::null_mut();
    }
    let mut count = header.rc.load(Ordering::Relaxed);
    loop {
        if count == 0 {
            return std::ptr::null_mut();
        }
        match header.rc.compare_exchange_weak(count, count + 1, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return r,
            Err(seen) => count = seen,
        }
    }
}

/// Drops one weak count, freeing the object with the last.
pub(crate) unsafe fn ref_forget(r: *mut RefHeader) {
    unsafe {
        if (*r).weak.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            LIVE_OBJECTS.gone();
            dealloc(r as *mut u8, ref_layout((*r).size));
        }
    }
}
