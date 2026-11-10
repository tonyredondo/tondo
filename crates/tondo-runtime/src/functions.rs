//! Function values (docs/refactor.md, phase 5): a `fn(...)` value, the code to
//! call and the captures of a closure.

use super::*;

/// A `fn(...)` value (11.7): the code to call and, for a closure, its
/// captures, which follow the header. The code receives the address of the
/// captures first. Named functions are static objects with `rc == IMMORTAL`.
#[repr(C)]
pub struct FnObject {
    rc: AtomicUsize,
    release: Option<Glue>,
    code: *const u8,
    size: usize,
    /// Visits the captures for the cycle collector, when they may refer
    /// to a cycle.
    visit: Option<cycles::Visit>,
}

pub(crate) const FN_HEADER: usize = std::mem::size_of::<FnObject>();

pub(crate) fn fn_layout(size: usize) -> Layout {
    Layout::from_size_align(FN_HEADER + size, 8).expect("closure too large")
}

/// A function value whose `size` bytes of captures the caller initializes.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fn_new(
    code: *const u8,
    size: usize,
    release: Option<Glue>,
    visit: Option<cycles::Visit>,
) -> *mut FnObject {
    let layout = fn_layout(size);
    unsafe {
        let ptr = alloc(layout) as *mut FnObject;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(FnObject { rc: AtomicUsize::new(1), release, code, size, visit });
        LIVE_OBJECTS.made();
        ptr
    }
}

/// A function value in room `room` of a frame, which only the frame reads
/// (`stack.rs`): nothing counts it, as a named function's value, and the
/// frame lets go of its captures (`tondo_rt_fn_let_go`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fn_in(
    room: *mut FnObject,
    code: *const u8,
    size: usize,
    release: Option<Glue>,
    visit: Option<cycles::Visit>,
) -> *mut FnObject {
    unsafe { room.write(FnObject { rc: AtomicUsize::new(IMMORTAL), release, code, size, visit }) };
    room
}

/// Lets go of the captures of a function value a frame made, whose room is
/// the frame's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fn_let_go(f: *mut FnObject) {
    unsafe {
        if let Some(release) = (*f).release {
            release((f as *mut u8).add(FN_HEADER));
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fn_retain(f: *mut FnObject) {
    if f.is_null() {
        return;
    }
    let rc = unsafe { &(*f).rc };
    if rc.load(Ordering::Relaxed) != IMMORTAL {
        rc.fetch_add(1, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fn_release(f: *mut FnObject) {
    if f.is_null() {
        return;
    }
    let rc = unsafe { &(*f).rc };
    if rc.load(Ordering::Relaxed) == IMMORTAL {
        return;
    }
    match dropped(rc) {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                if let Some(release) = (*f).release {
                    release((f as *mut u8).add(FN_HEADER));
                }
                LIVE_OBJECTS.gone();
                dealloc(f as *mut u8, fn_layout((*f).size));
            }
        }
        0 => runtime_fault("function value released after it was freed"),
        _ => {}
    }
}

/// Visits a closure's captures for the cycle collector.
pub(crate) unsafe fn visit_fn(visitor: &mut cycles::Visitor, f: *mut u8) {
    unsafe {
        let f = f as *mut FnObject;
        visitor.visit((*f).visit, (f as *mut u8).add(FN_HEADER));
    }
}
