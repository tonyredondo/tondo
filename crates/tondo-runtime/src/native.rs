//! Values of the runtime that Tondo holds through one handle,
//! `std.internal`'s `Native`: an HTTP client, a response, a request body.
//! Each is a reference count and a value of any type, in one allocation,
//! dropped with its last reference. One a region made is the region's
//! memory: its last reference drops its value, which lets go of what the
//! value holds outside, as a descriptor, and leaves the memory to the
//! region (`objects`).

use super::LIVE_OBJECTS;
use std::any::{Any, TypeId};
use std::sync::atomic::AtomicUsize;

/// The header of an object, which its value follows.
#[repr(C)]
pub struct NativeObject {
    rc: AtomicUsize,
    /// Drops the value, and frees the object when it is the heap's.
    drop: unsafe fn(*mut NativeObject, bool),
    id: TypeId,
}

#[repr(C)]
struct Inner<T> {
    header: NativeObject,
    value: T,
}

unsafe fn drop_inner<T>(object: *mut NativeObject, free: bool) {
    let inner = object as *mut Inner<T>;
    match free {
        true => drop(unsafe { Box::from_raw(inner) }),
        false => unsafe { std::ptr::drop_in_place(&raw mut (*inner).value) },
    }
}

/// A new object holding `value`, with one reference.
pub(crate) fn new<T: Any + Send + Sync>(value: T) -> *mut NativeObject {
    new_in(std::ptr::null_mut(), value)
}

/// A new object holding `value`, with one reference, in `region` when it
/// is given, or on the heap: the region the compiled code passed a native
/// that makes its object in one, or null.
pub(crate) fn new_in<T: Any + Send + Sync>(region: *mut super::regions::Region, value: T) -> *mut NativeObject {
    let header = NativeObject { rc: AtomicUsize::new(1), drop: drop_inner::<T>, id: TypeId::of::<T>() };
    unsafe { super::objects::make(region, Inner { header, value }) as *mut NativeObject }
}

/// The value of an object, when it is a `T`.
pub(crate) fn get<'a, T: Any>(object: *mut NativeObject) -> Option<&'a T> {
    if object.is_null() || unsafe { (*object).id } != TypeId::of::<T>() {
        return None;
    }
    unsafe { Some(&(*(object as *mut Inner<T>)).value) }
}

/// One more reference to an object of the heap, which the runtime keeps.
pub(crate) unsafe fn retain(object: *mut NativeObject) {
    if !object.is_null() {
        unsafe { (*object).rc.fetch_add(1, std::sync::atomic::Ordering::Relaxed) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_native_release(object: *mut NativeObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        if let Some(free) = super::objects::dropped(&(*object).rc) {
            ((*object).drop)(object, free);
            if free {
                LIVE_OBJECTS.gone();
            }
        }
    }
}
