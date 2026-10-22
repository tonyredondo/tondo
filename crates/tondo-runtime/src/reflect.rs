//! The box a `reflect.Value` keeps its value in (standard library 14.8):
//! the value's bytes, how to release them and the identity of its type.
//! Everything else reflection does is code the compiler generates for each
//! type.

use super::{Glue, LIVE_OBJECTS, Str, str_bytes, str_new};
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::sync::atomic::{AtomicUsize, Ordering, fence};

/// A type's identity: a static the compiler emits once per type, holding
/// its name.
#[repr(C)]
pub struct Identity {
    name: *const Str,
}

#[repr(C)]
pub struct BoxObject {
    rc: AtomicUsize,
    size: usize,
    release: Option<Glue>,
    identity: *const Identity,
}

/// Where the value starts, after the header.
pub const VALUE_OFFSET: usize = std::mem::size_of::<BoxObject>();

fn layout(size: usize) -> Layout {
    Layout::from_size_align(VALUE_OFFSET + size, 8).expect("reflected value too large")
}

/// A box for `size` bytes of a value of the type `identity`, which the
/// caller stores; `release` releases what it counts.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_reflect_box(
    size: usize,
    release: Option<Glue>,
    identity: *const Identity,
) -> *mut BoxObject {
    let layout = layout(size);
    unsafe {
        let object = alloc(layout) as *mut BoxObject;
        if object.is_null() {
            handle_alloc_error(layout);
        }
        object.write(BoxObject { rc: AtomicUsize::new(1), size, release, identity });
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        object
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_reflect_release(object: *mut BoxObject) {
    if object.is_null() || unsafe { (*object).rc.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    fence(Ordering::Acquire);
    unsafe {
        if let Some(release) = (*object).release {
            release((object as *mut u8).add(VALUE_OFFSET));
        }
        dealloc(object as *mut u8, layout((*object).size));
    }
    LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
}

/// Whether the box holds a value of the type `identity`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_reflect_is(object: *const BoxObject, identity: *const Identity) -> u8 {
    (unsafe { (*object).identity } == identity) as u8
}

/// The name of the type of the boxed value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_reflect_name(object: *const BoxObject) -> *mut Str {
    str_new(unsafe { str_bytes((*(*object).identity).name) })
}

/// The index a path segment names, or `-1` when it is not one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_reflect_index(text: *const Str) -> i64 {
    let text = unsafe { str_bytes(text) };
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) || (text.len() > 1 && text[0] == b'0') {
        return -1;
    }
    std::str::from_utf8(text).ok().and_then(|t| t.parse().ok()).unwrap_or(-1)
}

/// A string as a quoted literal, for a string inside a composite value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_reflect_quote(text: *const Str) -> *mut Str {
    let text = String::from_utf8_lossy(unsafe { str_bytes(text) });
    str_new(format!("{text:?}").as_bytes())
}
