//! Values of the runtime that Tondo holds through one handle,
//! `std.internal`'s `Native`: an HTTP client, a response, a request body.
//! Each is a reference count and a value of any type, dropped with its
//! last reference.

use super::LIVE_OBJECTS;
use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
pub struct NativeObject {
    rc: AtomicUsize,
    value: Box<dyn Any + Send + Sync>,
}

/// A new object holding `value`, with one reference.
pub(crate) fn new<T: Any + Send + Sync>(value: T) -> *mut NativeObject {
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(NativeObject { rc: AtomicUsize::new(1), value: Box::new(value) }))
}

/// The value of an object, when it is a `T`.
pub(crate) fn get<'a, T: Any>(object: *mut NativeObject) -> Option<&'a T> {
    if object.is_null() {
        return None;
    }
    unsafe { (*object).value.downcast_ref::<T>() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_native_release(object: *mut NativeObject) {
    if object.is_null() || unsafe { (*object).rc.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    std::sync::atomic::fence(Ordering::Acquire);
    drop(unsafe { Box::from_raw(object) });
    LIVE_OBJECTS.gone();
}
