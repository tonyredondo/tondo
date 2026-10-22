//! Runtime linked into every native Tondo executable.
//!
//! The compiler's build script compiles this file with `rustc` as a staticlib
//! and embeds it; the generated code calls the `tondo_rt_*` symbols below.
//! That interface is private to the compiler and the runtime: nothing here is
//! a stable ABI.

#![allow(clippy::missing_safety_doc)]

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering, fence};

mod clock;
mod env;
mod fs;
mod io;
mod json;
mod memory;
mod process;
mod shared;
mod sync;
mod tasks;
mod testing;
mod time;

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// Header of every string. The UTF-8 bytes follow the header.
///
/// String literals are emitted by the compiler as static objects with the
/// same layout and `rc == IMMORTAL`, so retain and release ignore them.
#[repr(C)]
pub struct Str {
    rc: AtomicUsize,
    len: usize,
}

const IMMORTAL: usize = usize::MAX;
const HEADER: usize = std::mem::size_of::<Str>();

/// Heap objects (strings and boxes) currently allocated. Checked at exit when
/// `TONDO_RT_CHECK_LEAKS` is set, which the compiler's tests use to verify
/// reference counting.
static LIVE_OBJECTS: AtomicUsize = AtomicUsize::new(0);

fn str_layout(len: usize) -> Layout {
    Layout::from_size_align(HEADER + len, std::mem::align_of::<Str>()).expect("string too large")
}

/// Allocates a string with refcount 1 holding a copy of `bytes`.
pub fn str_new(bytes: &[u8]) -> *mut Str {
    let layout = str_layout(bytes.len());
    unsafe {
        let ptr = alloc(layout) as *mut Str;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(Str { rc: AtomicUsize::new(1), len: bytes.len() });
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), (ptr as *mut u8).add(HEADER), bytes.len());
        ptr
    }
}

/// The bytes of a string. The caller keeps the string alive.
pub unsafe fn str_bytes<'a>(s: *const Str) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts((s as *const u8).add(HEADER), (*s).len) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_retain(s: *const Str) {
    let rc = unsafe { &(*s).rc };
    if rc.load(Ordering::Relaxed) != IMMORTAL {
        rc.fetch_add(1, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_release(s: *mut Str) {
    // Releasing a binding whose value moved away does nothing: it is zero.
    if s.is_null() {
        return;
    }
    let rc = unsafe { &(*s).rc };
    if rc.load(Ordering::Relaxed) == IMMORTAL {
        return;
    }
    match rc.fetch_sub(1, Ordering::Release) {
        1 => {
            fence(Ordering::Acquire);
            LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
            unsafe { dealloc(s as *mut u8, str_layout((*s).len)) }
        }
        0 => runtime_fault("string released after it was freed"),
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_eq(a: *const Str, b: *const Str) -> u8 {
    unsafe { (str_bytes(a) == str_bytes(b)) as u8 }
}

/// Orders strings by Unicode scalar values; for UTF-8 this is byte order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_cmp(a: *const Str, b: *const Str) -> i32 {
    unsafe { str_bytes(a).cmp(str_bytes(b)) as i32 }
}

/// The text of a string; strings always hold valid UTF-8.
unsafe fn str_text<'a>(s: *const Str) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(str_bytes(s)) }
}

/// The number of Unicode scalar values (19.2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_length(s: *const Str) -> i64 {
    // Every scalar has exactly one byte that is not a continuation byte.
    unsafe { str_bytes(s) }.iter().filter(|&&b| (b as i8) >= -0x40).count() as i64
}

/// The scalar at `index`, counting from the end when negative, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_get(s: *const Str, index: i64) -> i64 {
    let text = unsafe { str_text(s) };
    let found = if index >= 0 {
        text.chars().nth(index as usize)
    } else {
        index.checked_neg().and_then(|back| text.chars().nth_back(back as usize - 1))
    };
    found.map_or(-1, |c| c as i64)
}

/// `text[index]`: the scalar at `index`, panicking with `P0001` outside.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_at(s: *const Str, index: i64, location: *const Str) -> u32 {
    match unsafe { tondo_rt_str_get(s, index) } {
        -1 => unsafe { tondo_rt_panic(1, std::ptr::null(), location) },
        c => c as u32,
    }
}

/// `text[start:end:step]` by scalar positions (19.3).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_slice(
    s: *const Str,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    location: *const Str,
) -> *mut Str {
    unsafe {
        let text = str_text(s);
        let chars: Vec<char> = text.chars().collect();
        let (first, count) = slice_range(chars.len(), start, has_start, end, has_end, step, location);
        let selected: String = (0..count as i64).map(|k| chars[(first + k * step) as usize]).collect();
        str_new(selected.as_bytes())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_concat(a: *const Str, b: *const Str) -> *mut Str {
    unsafe { str_new(&[str_bytes(a), str_bytes(b)].concat()) }
}

/// `contains` (0), `startsWith` (1) or `endsWith` (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_test(s: *const Str, other: *const Str, test: u32) -> u8 {
    let (text, other) = unsafe { (str_text(s), str_text(other)) };
    (match test {
        0 => text.contains(other),
        1 => text.starts_with(other),
        _ => text.ends_with(other),
    }) as u8
}

/// The scalar position of the first occurrence of `needle`, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_find(s: *const Str, needle: *const Str) -> i64 {
    let (text, needle) = unsafe { (str_text(s), str_text(needle)) };
    text.find(needle).map_or(-1, |offset| text[..offset].chars().count() as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_replace(s: *const Str, old: *const Str, new: *const Str) -> *mut Str {
    let replaced = unsafe { str_text(s).replace(str_text(old), str_text(new)) };
    str_new(replaced.as_bytes())
}

/// `trim` (0), `toLowerAscii` (1) or `toUpperAscii` (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_transform(s: *const Str, transform: u32) -> *mut Str {
    let text = unsafe { str_text(s) };
    match transform {
        0 => str_new(text.trim().as_bytes()),
        1 => str_new(text.to_ascii_lowercase().as_bytes()),
        _ => str_new(text.to_ascii_uppercase().as_bytes()),
    }
}

/// The checked `slice(start, end)`: null with `error` set to 0
/// (`InvalidIndex`) or 1 (`InvalidBoundary`) when the bounds are invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_slice_checked(s: *const Str, start: i64, end: i64, error: *mut i8) -> *mut Str {
    unsafe {
        let text = str_text(s);
        let length = text.chars().count() as i64;
        let code = if start < 0 || end < 0 || start > length || end > length {
            0
        } else if start > end {
            1
        } else {
            let selected: String = text.chars().skip(start as usize).take((end - start) as usize).collect();
            *error = -1;
            return str_new(selected.as_bytes());
        };
        *error = code;
        std::ptr::null_mut()
    }
}

/// A string holding the characters of an `Array[Char]`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_from_chars(chars: *mut ArrayHeader) -> *mut Str {
    unsafe {
        let text: String = (0..(*chars).len)
            .map(|k| char::from_u32((element(chars, 4, k) as *const u32).read()).unwrap_or('\u{FFFD}'))
            .collect();
        str_new(text.as_bytes())
    }
}

/// The scalar that starts at byte `offset`; iteration only visits valid
/// offsets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_decode(s: *const Str, offset: i64) -> u32 {
    unsafe { str_text(s).get_unchecked(offset as usize..) }.chars().next().map_or(0, |c| c as u32)
}

/// The byte offset of the scalar after the one at `offset`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_next(s: *const Str, offset: i64) -> i64 {
    let lead = unsafe { str_bytes(s) }[offset as usize];
    let width = match lead {
        0x00..0x80 => 1,
        0x80..0xE0 => 2,
        0xE0..0xF0 => 3,
        _ => 4,
    };
    offset + width
}

// ---------------------------------------------------------------------------
// Boxes
// ---------------------------------------------------------------------------

/// Header of a box: the indirection the compiler inserts where a recursive
/// type contains itself. The value follows the header.
#[repr(C)]
pub struct BoxHeader {
    rc: AtomicUsize,
    size: usize,
}

const BOX_HEADER: usize = std::mem::size_of::<BoxHeader>();

/// Retains or releases the counted contents of a value in place. Generated by
/// the compiler for each boxed type.
pub type Glue = unsafe extern "C" fn(*mut u8);

fn box_layout(size: usize) -> Layout {
    Layout::from_size_align(BOX_HEADER + size, 8).expect("box too large")
}

/// Allocates a box with refcount 1; the caller initializes the value.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_box_new(size: usize) -> *mut BoxHeader {
    let layout = box_layout(size);
    unsafe {
        let ptr = alloc(layout) as *mut BoxHeader;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(BoxHeader { rc: AtomicUsize::new(1), size });
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        ptr
    }
}

fn box_value(b: *mut BoxHeader) -> *mut u8 {
    unsafe { (b as *mut u8).add(BOX_HEADER) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_box_retain(b: *mut BoxHeader) {
    unsafe { (*b).rc.fetch_add(1, Ordering::Relaxed) };
}

/// Releases a box; the last release releases the value's contents and frees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_box_release(b: *mut BoxHeader, release: Option<Glue>) {
    if b.is_null() {
        return;
    }
    match unsafe { (*b).rc.fetch_sub(1, Ordering::Release) } {
        1 => {
            fence(Ordering::Acquire);
            if let Some(release) = release {
                unsafe { release(box_value(b)) };
            }
            LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
            unsafe { dealloc(b as *mut u8, box_layout((*b).size)) }
        }
        0 => runtime_fault("box released after it was freed"),
        _ => {}
    }
}

/// Makes the box stored in `slot` unique before a write through it: a shared
/// box is replaced by a copy whose counted contents are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_box_unique(slot: *mut *mut BoxHeader, retain: Option<Glue>, release: Option<Glue>) {
    unsafe {
        let old = *slot;
        if (*old).rc.load(Ordering::Acquire) == 1 {
            return;
        }
        let size = (*old).size;
        let new = tondo_rt_box_new(size);
        std::ptr::copy_nonoverlapping(box_value(old), box_value(new), size);
        if let Some(retain) = retain {
            retain(box_value(new));
        }
        tondo_rt_box_release(old, release);
        *slot = new;
    }
}

// ---------------------------------------------------------------------------
// Function values
// ---------------------------------------------------------------------------

/// A `fn(...)` value (11.7): the code to call and, for a closure, its
/// captures, which follow the header. The code receives the address of the
/// captures first. Named functions are static objects with `rc == IMMORTAL`.
#[repr(C)]
pub struct FnObject {
    rc: AtomicUsize,
    release: Option<Glue>,
    code: *const u8,
    size: usize,
}

const FN_HEADER: usize = std::mem::size_of::<FnObject>();

fn fn_layout(size: usize) -> Layout {
    Layout::from_size_align(FN_HEADER + size, 8).expect("closure too large")
}

/// A function value whose `size` bytes of captures the caller initializes.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fn_new(code: *const u8, size: usize, release: Option<Glue>) -> *mut FnObject {
    let layout = fn_layout(size);
    unsafe {
        let ptr = alloc(layout) as *mut FnObject;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(FnObject { rc: AtomicUsize::new(1), release, code, size });
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        ptr
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fn_retain(f: *mut FnObject) {
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
    match rc.fetch_sub(1, Ordering::Release) {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                if let Some(release) = (*f).release {
                    release((f as *mut u8).add(FN_HEADER));
                }
                LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
                dealloc(f as *mut u8, fn_layout((*f).size));
            }
        }
        0 => runtime_fault("function value released after it was freed"),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------

/// Header of an array or a deque. Storage for `cap` elements of the size the
/// compiler passes to every operation follows it, and the `len` elements
/// start at position `head`. Arrays always have `head == 0`, which their
/// generated code relies on; a deque keeps free room at both ends. Both are
/// values: a write first makes the object unique, copying it (and retaining
/// its elements) when shared.
#[repr(C)]
pub struct ArrayHeader {
    rc: AtomicUsize,
    pub(crate) len: usize,
    cap: usize,
    head: usize,
}

const ARRAY_HEADER: usize = std::mem::size_of::<ArrayHeader>();

/// Compares two values in place; generated by the compiler per type.
pub type EqualGlue = unsafe extern "C" fn(*const u8, *const u8) -> u8;
/// Appends the `Display` text of a value to a builder.
pub type DisplayGlue = unsafe extern "C" fn(*mut Builder, *const u8);

fn array_layout(size: usize, cap: usize) -> Layout {
    let bytes = size.checked_mul(cap).and_then(|b| b.checked_add(ARRAY_HEADER)).expect("array too large");
    Layout::from_size_align(bytes, 8).expect("array too large")
}

/// The address of storage position `position`, ignoring `head`.
unsafe fn position(a: *mut ArrayHeader, size: usize, position: usize) -> *mut u8 {
    unsafe { (a as *mut u8).add(ARRAY_HEADER + position * size) }
}

pub(crate) unsafe fn element(a: *mut ArrayHeader, size: usize, index: usize) -> *mut u8 {
    unsafe { position(a, size, (*a).head + index) }
}

/// Allocates an array of `len` uninitialized elements and room for `cap`.
fn array_alloc(size: usize, len: usize, cap: usize) -> *mut ArrayHeader {
    let cap = cap.max(len);
    let layout = array_layout(size, cap);
    unsafe {
        let ptr = alloc(layout) as *mut ArrayHeader;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(ArrayHeader { rc: AtomicUsize::new(1), len, cap, head: 0 });
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        ptr
    }
}

/// A new array with `len` elements for the caller to initialize.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_array_new(size: usize, len: usize) -> *mut ArrayHeader {
    array_alloc(size, len, len)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_retain(a: *mut ArrayHeader) {
    unsafe { (*a).rc.fetch_add(1, Ordering::Relaxed) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_release(a: *mut ArrayHeader, size: usize, release: Option<Glue>) {
    if a.is_null() {
        return;
    }
    match unsafe { (*a).rc.fetch_sub(1, Ordering::Release) } {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                if let Some(release) = release {
                    for index in 0..(*a).len {
                        release(element(a, size, index));
                    }
                }
                LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
                dealloc(a as *mut u8, array_layout(size, (*a).cap));
            }
        }
        0 => runtime_fault("array released after it was freed"),
        _ => {}
    }
}

/// Copies `count` elements starting at `from`, stepping `step` elements,
/// into a new array, retaining each copied element.
unsafe fn array_copy(
    a: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    from: i64,
    step: i64,
    count: usize,
    cap: usize,
) -> *mut ArrayHeader {
    let new = array_alloc(size, count, cap);
    for k in 0..count {
        unsafe {
            let index = (from + k as i64 * step) as usize;
            std::ptr::copy_nonoverlapping(element(a, size, index), element(new, size, k), size);
            if let Some(retain) = retain {
                retain(element(new, size, k));
            }
        }
    }
    new
}

/// Makes the array or deque in `slot` unique, with room for `front` more
/// elements before its first and `back` more after its last. A deque
/// (`center`) moves its elements to the middle of storage that is at least
/// twice as large, so both ends stay amortized constant time.
unsafe fn reserve(
    slot: *mut *mut ArrayHeader,
    size: usize,
    front: usize,
    back: usize,
    center: bool,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe {
        let a = *slot;
        let (len, head, cap) = ((*a).len, (*a).head, (*a).cap);
        let shared = (*a).rc.load(Ordering::Acquire) != 1;
        if !shared && head >= front && head + len + back <= cap {
            return;
        }
        let needed = len + front + back;
        let new_cap = if center {
            if 2 * needed <= cap { cap } else { (2 * needed).max(8) }
        } else if needed > cap {
            needed.max(2 * len).max(4)
        } else {
            cap
        };
        let new_head = if center { front + (new_cap - needed) / 2 } else { 0 };
        if !shared && new_cap == cap {
            // Unique with enough storage: slide the elements into place.
            std::ptr::copy(element(a, size, 0), position(a, size, new_head), len * size);
            (*a).head = new_head;
            return;
        }
        let new = array_alloc(size, len, new_cap);
        (*new).head = new_head;
        std::ptr::copy_nonoverlapping(element(a, size, 0), element(new, size, 0), len * size);
        if shared {
            if let Some(retain) = retain {
                for index in 0..len {
                    retain(element(new, size, index));
                }
            }
            tondo_rt_array_release(a, size, release);
        } else {
            // Unique: the elements moved; nothing is retained or released.
            LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
            dealloc(a as *mut u8, array_layout(size, cap));
        }
        *slot = new;
    }
}

/// Makes the array or deque stored in `slot` unique and able to hold `extra`
/// more elements at its end, before a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_unique(
    slot: *mut *mut ArrayHeader,
    size: usize,
    extra: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe { reserve(slot, size, 0, extra, false, retain, release) }
}

/// Appends the value at `value` (its ownership moves into the array).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_push(
    slot: *mut *mut ArrayHeader,
    value: *const u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe {
        tondo_rt_array_unique(slot, size, 1, retain, release);
        let a = *slot;
        std::ptr::copy_nonoverlapping(value, element(a, size, (*a).len), size);
        (*a).len += 1;
    }
}

/// Moves the last element to `out` and returns 1, or returns 0 when empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_pop(
    slot: *mut *mut ArrayHeader,
    out: *mut u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) -> u8 {
    unsafe {
        if (**slot).len == 0 {
            return 0;
        }
        tondo_rt_array_unique(slot, size, 0, retain, release);
        let a = *slot;
        (*a).len -= 1;
        std::ptr::copy_nonoverlapping(element(a, size, (*a).len), out, size);
        1
    }
}

/// A position counted from the end when negative (10.3).
fn normalize(index: i64, len: usize) -> i64 {
    if index < 0 { index.saturating_add(len as i64) } else { index }
}

/// `insert(at, value)`: shifts the elements from `at` on and stores the value
/// there (its ownership moves in). `at` may equal the length; beyond that it
/// panics with `P0001`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_insert(
    slot: *mut *mut ArrayHeader,
    at: i64,
    value: *const u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    location: *const Str,
) {
    unsafe {
        let len = (**slot).len;
        let at = normalize(at, len);
        if at < 0 || at as usize > len {
            tondo_rt_panic(1, std::ptr::null(), location);
        }
        let at = at as usize;
        tondo_rt_array_unique(slot, size, 1, retain, release);
        let a = *slot;
        std::ptr::copy(element(a, size, at), element(a, size, at + 1), (len - at) * size);
        std::ptr::copy_nonoverlapping(value, element(a, size, at), size);
        (*a).len += 1;
    }
}

/// `removeAt(at)`: moves the element at `at` to `out`, shifts the rest and
/// returns 1, or returns 0 when there is no such element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_remove_at(
    slot: *mut *mut ArrayHeader,
    at: i64,
    out: *mut u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) -> u8 {
    unsafe {
        let len = (**slot).len;
        let at = normalize(at, len);
        if at < 0 || at as usize >= len {
            return 0;
        }
        let at = at as usize;
        tondo_rt_array_unique(slot, size, 0, retain, release);
        let a = *slot;
        std::ptr::copy_nonoverlapping(element(a, size, at), out, size);
        std::ptr::copy(element(a, size, at + 1), element(a, size, at), (len - at - 1) * size);
        (*a).len -= 1;
        1
    }
}

/// Removes every element of the array or deque in `slot`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_clear(slot: *mut *mut ArrayHeader, size: usize, release: Option<Glue>) {
    unsafe {
        let a = *slot;
        if (*a).rc.load(Ordering::Acquire) != 1 {
            *slot = array_alloc(size, 0, 0);
            tondo_rt_array_release(a, size, release);
            return;
        }
        if let Some(release) = release {
            for index in 0..(*a).len {
                release(element(a, size, index));
            }
        }
        (*a).len = 0;
        (*a).head = 0;
    }
}

/// Adds the value at `value` to the front of the deque when `front` is 1,
/// otherwise to its back (its ownership moves in).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_deque_push(
    slot: *mut *mut ArrayHeader,
    value: *const u8,
    size: usize,
    front: u8,
    retain: Option<Glue>,
    release: Option<Glue>,
) {
    unsafe {
        let front = front != 0;
        reserve(slot, size, front as usize, !front as usize, true, retain, release);
        let a = *slot;
        if front {
            (*a).head -= 1;
            std::ptr::copy_nonoverlapping(value, element(a, size, 0), size);
        } else {
            std::ptr::copy_nonoverlapping(value, element(a, size, (*a).len), size);
        }
        (*a).len += 1;
    }
}

/// Moves the first element to `out` and returns 1, or returns 0 when empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_deque_pop_front(
    slot: *mut *mut ArrayHeader,
    out: *mut u8,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
) -> u8 {
    unsafe {
        if (**slot).len == 0 {
            return 0;
        }
        tondo_rt_array_unique(slot, size, 0, retain, release);
        let a = *slot;
        std::ptr::copy_nonoverlapping(element(a, size, 0), out, size);
        (*a).head += 1;
        (*a).len -= 1;
        1
    }
}

/// The first position and the number of elements that `[start:end:step]`
/// selects from `len` elements, with the normalization of spec 10.4. Absent
/// bounds are passed as `has_start`/`has_end` equal to 0; a zero step
/// panics with `P0002`.
#[allow(clippy::too_many_arguments)]
unsafe fn slice_range(
    len: usize,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    location: *const Str,
) -> (i64, usize) {
    let n = len as i128;
    if step == 0 {
        unsafe { tondo_rt_panic(2, std::ptr::null(), location) };
    }
    let step_wide = step as i128;
    let explicit = |value: i64| if value < 0 { value as i128 + n } else { value as i128 };
    let (first, last) = if step > 0 {
        let first = if has_start != 0 { explicit(start).clamp(0, n) } else { 0 };
        let last = if has_end != 0 { explicit(end).clamp(0, n) } else { n };
        (first, last)
    } else {
        let first = if has_start != 0 { explicit(start).clamp(-1, n - 1) } else { n - 1 };
        let last = if has_end != 0 { explicit(end).clamp(-1, n - 1) } else { -1 };
        (first, last)
    };
    let count = if step > 0 {
        if last > first { (last - first + step_wide - 1) / step_wide } else { 0 }
    } else if first > last {
        (first - last + (-step_wide) - 1) / (-step_wide)
    } else {
        0
    };
    (first as i64, count as usize)
}

/// Two loans of one call must not share an element (16.4): a region
/// `[start:end:step]` of an array of `len` elements, and either another
/// region or, when `index` is set, the element at `start2`. Panics with
/// `P0004` when they do.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_check_region(
    len: usize,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    index: u8,
    start2: i64,
    has_start2: u8,
    end2: i64,
    has_end2: u8,
    step2: i64,
    location: *const Str,
) {
    unsafe {
        let (first, count) = slice_range(len, start, has_start, end, has_end, step, location);
        let (first2, count2, step2) = if index != 0 {
            let at = normalize(start2, len);
            if at < 0 || at as usize >= len {
                return;
            }
            (at, 1, 1)
        } else {
            let (first2, count2) = slice_range(len, start2, has_start2, end2, has_end2, step2, location);
            (first2, count2, step2)
        };
        // Walk the shorter progression and test membership in the other.
        let ((a, na, sa), (b, nb, sb)) = if count <= count2 {
            ((first, count, step), (first2, count2, step2))
        } else {
            ((first2, count2, step2), (first, count, step))
        };
        let (b, sb) = (b as i128, sb as i128);
        for k in 0..na {
            let offset = (a as i128 + k as i128 * sa as i128) - b;
            if offset % sb == 0 && (0..nb as i128).contains(&(offset / sb)) {
                tondo_rt_panic(4, std::ptr::null(), location);
            }
        }
    }
}

/// `values[start:end:step]` (10.4).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_slice(
    a: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    location: *const Str,
) -> *mut ArrayHeader {
    unsafe {
        let (first, count) = slice_range((*a).len, start, has_start, end, has_end, step, location);
        array_copy(a, size, retain, first, step, count, count)
    }
}

/// `values[start:end:step] = source`: replaces the selected elements with
/// those of `source`, which must have as many (`P0006`). The source acts as
/// a snapshot, so it may share elements or storage with the target.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_assign_slice(
    slot: *mut *mut ArrayHeader,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    source: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    location: *const Str,
) {
    unsafe {
        let (first, count) = slice_range((**slot).len, start, has_start, end, has_end, step, location);
        if count != (*source).len {
            tondo_rt_panic(6, std::ptr::null(), location);
        }
        let mut snapshot = vec![0u8; count * size];
        for k in 0..count {
            let value = element(source, size, k);
            if let Some(retain) = retain {
                retain(value);
            }
            std::ptr::copy_nonoverlapping(value, snapshot.as_mut_ptr().add(k * size), size);
        }
        tondo_rt_array_unique(slot, size, 0, retain, release);
        let a = *slot;
        for k in 0..count {
            let target = element(a, size, (first + k as i64 * step) as usize);
            if let Some(release) = release {
                release(target);
            }
            std::ptr::copy_nonoverlapping(snapshot.as_ptr().add(k * size), target, size);
        }
    }
}

/// A number type of lifted arithmetic (10.6) with checked operations; the
/// operation codes are `+ - * / %` in that order.
trait Number: Copy {
    unsafe fn apply(op: u32, a: Self, b: Self, location: *const Str) -> Self;
}

macro_rules! integer_numbers {
    ($($t:ty),*) => {$(
        impl Number for $t {
            unsafe fn apply(op: u32, a: Self, b: Self, location: *const Str) -> Self {
                let result = match op {
                    0 => a.checked_add(b),
                    1 => a.checked_sub(b),
                    2 => a.checked_mul(b),
                    _ if b == 0 => unsafe { tondo_rt_panic(3, std::ptr::null(), location) },
                    3 => a.checked_div(b),
                    // `MIN % -1` is 0 (18.3).
                    _ => Some(a.wrapping_rem(b)),
                };
                result.unwrap_or_else(|| unsafe { tondo_rt_panic(5, std::ptr::null(), location) })
            }
        }
    )*};
}

integer_numbers!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! float_numbers {
    ($($t:ty),*) => {$(
        impl Number for $t {
            unsafe fn apply(op: u32, a: Self, b: Self, _: *const Str) -> Self {
                match op {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    _ => a / b,
                }
            }
        }
    )*};
}

float_numbers!(f32, f64);

unsafe fn apply_number<T: Number>(op: u32, a: *const u8, b: *const u8, out: *mut u8, location: *const Str) {
    unsafe {
        let value = T::apply(op, (a as *const T).read_unaligned(), (b as *const T).read_unaligned(), location);
        (out as *mut T).write_unaligned(value);
    }
}

/// Number types are coded as Int8, Int16, Int32, Int64, UInt8, UInt16,
/// UInt32, UInt64, Float32 and Float64, from 0.
fn number_size(number: u32) -> usize {
    match number {
        0 | 4 => 1,
        1 | 5 => 2,
        2 | 6 | 8 => 4,
        _ => 8,
    }
}

/// The size of a value nested `depth` arrays deep around numbers.
fn nested_size(depth: u32, number: u32) -> usize {
    if depth == 0 { number_size(number) } else { 8 }
}

/// The array stored at `value`.
unsafe fn array_at(value: *const u8) -> *mut ArrayHeader {
    unsafe { (value as *const *mut ArrayHeader).read() }
}

/// Paired arrays must have the same length at every level (`P0006`).
unsafe fn check_shape(a: *const u8, da: u32, b: *const u8, db: u32, location: *const Str) {
    if da == 0 || db == 0 {
        return;
    }
    unsafe {
        let (x, y) = (array_at(a), array_at(b));
        if (*x).len != (*y).len {
            tondo_rt_panic(6, std::ptr::null(), location);
        }
        if da > 1 && db > 1 {
            for k in 0..(*x).len {
                check_shape(element(x, 8, k), da - 1, element(y, 8, k), db - 1, location);
            }
        }
    }
}

/// Writes `a op b` to `out`, where each operand is a number or an array
/// nested `da`/`db` deep. Arrays are paired element by element and a
/// number pairs with every element of an array.
#[allow(clippy::too_many_arguments)]
unsafe fn lifted(
    a: *const u8,
    da: u32,
    b: *const u8,
    db: u32,
    op: u32,
    number: u32,
    out: *mut u8,
    location: *const Str,
) {
    unsafe {
        if da == 0 && db == 0 {
            let apply = match number {
                0 => apply_number::<i8>,
                1 => apply_number::<i16>,
                2 => apply_number::<i32>,
                3 => apply_number::<i64>,
                4 => apply_number::<u8>,
                5 => apply_number::<u16>,
                6 => apply_number::<u32>,
                7 => apply_number::<u64>,
                8 => apply_number::<f32>,
                _ => apply_number::<f64>,
            };
            return apply(op, a, b, out, location);
        }
        let len = if da > 0 { (*array_at(a)).len } else { (*array_at(b)).len };
        let size = nested_size(da.max(db) - 1, number);
        let result = array_alloc(size, len, len);
        for k in 0..len {
            let x = if da > 0 { element(array_at(a), nested_size(da - 1, number), k) } else { a as *mut u8 };
            let y = if db > 0 { element(array_at(b), nested_size(db - 1, number), k) } else { b as *mut u8 };
            lifted(x, da.saturating_sub(1), y, db.saturating_sub(1), op, number, element(result, size, k), location);
        }
        (out as *mut *mut ArrayHeader).write(result);
    }
}

/// Lifted arithmetic over arrays (10.6). `a` and `b` point to the operands;
/// the whole shape is checked before the first operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_arith(
    a: *const u8,
    da: u32,
    b: *const u8,
    db: u32,
    op: u32,
    number: u32,
    location: *const Str,
) -> *mut ArrayHeader {
    unsafe {
        check_shape(a, da, b, db, location);
        let mut result: *mut ArrayHeader = std::ptr::null_mut();
        lifted(a, da, b, db, op, number, &mut result as *mut _ as *mut u8, location);
        result
    }
}

/// The checked `slice(start, end)` method: null when the bounds are invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_slice_checked(
    a: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    start: i64,
    end: i64,
) -> *mut ArrayHeader {
    let len = unsafe { (*a).len } as i64;
    if start < 0 || start > end || end > len {
        return std::ptr::null_mut();
    }
    unsafe { array_copy(a, size, retain, start, 1, (end - start) as usize, (end - start) as usize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_concat(
    a: *mut ArrayHeader,
    b: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    location: *const Str,
) -> *mut ArrayHeader {
    unsafe {
        let (la, lb) = ((*a).len, (*b).len);
        if (la as i128 + lb as i128) > i64::MAX as i128 {
            tondo_rt_panic(5, std::ptr::null(), location);
        }
        let new = array_copy(a, size, retain, 0, 1, la, la + lb);
        for k in 0..lb {
            std::ptr::copy_nonoverlapping(element(b, size, k), element(new, size, la + k), size);
            if let Some(retain) = retain {
                retain(element(new, size, la + k));
            }
        }
        (*new).len = la + lb;
        new
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_repeat(
    a: *mut ArrayHeader,
    count: i64,
    size: usize,
    retain: Option<Glue>,
    location: *const Str,
) -> *mut ArrayHeader {
    unsafe {
        if count < 0 {
            tondo_rt_panic(11, std::ptr::null(), location);
        }
        let len = (*a).len;
        let total = len as i128 * count as i128;
        if total > i64::MAX as i128 {
            tondo_rt_panic(5, std::ptr::null(), location);
        }
        let total = total as usize;
        let new = array_alloc(size, total, total);
        for k in 0..total {
            std::ptr::copy_nonoverlapping(element(a, size, k % len.max(1)), element(new, size, k), size);
            if let Some(retain) = retain {
                retain(element(new, size, k));
            }
        }
        new
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_eq(a: *mut ArrayHeader, b: *mut ArrayHeader, size: usize, eq: EqualGlue) -> u8 {
    unsafe {
        if (*a).len != (*b).len {
            return 0;
        }
        (0..(*a).len).all(|k| eq(element(a, size, k), element(b, size, k)) != 0) as u8
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_contains(
    a: *mut ArrayHeader,
    value: *const u8,
    size: usize,
    eq: EqualGlue,
) -> u8 {
    unsafe { (0..(*a).len).any(|k| eq(element(a, size, k), value) != 0) as u8 }
}

/// Appends `[a, b, c]` using the elements' `Display`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_array(b: *mut Builder, a: *mut ArrayHeader, size: usize, display: DisplayGlue) {
    unsafe {
        (*b).0.push(b'[');
        for k in 0..(*a).len {
            if k > 0 {
                (*b).0.extend_from_slice(b", ");
            }
            display(b, element(a, size, k));
        }
        (*b).0.push(b']');
    }
}

// ---------------------------------------------------------------------------
// Maps and sets
// ---------------------------------------------------------------------------

/// Hashes a key in place; generated by the compiler per key type.
pub type HashGlue = unsafe extern "C" fn(*const u8) -> u64;

/// An insertion-ordered hash map (10.9). Entries are stored contiguously in
/// insertion order, each a key followed by its value at `value_offset`; an
/// open-addressing table maps hashes to entry positions. A set is a map whose
/// values have size zero.
#[repr(C)]
pub struct MapObject {
    rc: AtomicUsize,
    entry_size: usize,
    value_offset: usize,
    entries: Vec<u8>,
    hashes: Vec<u64>,
    /// Entry position plus one; zero marks an empty slot. Its length is a
    /// power of two at least twice the number of entries.
    table: Vec<usize>,
}

/// The glue functions of a map's key and value types.
#[repr(C)]
pub struct MapGlue {
    pub(crate) hash: HashGlue,
    pub(crate) key_eq: EqualGlue,
    pub(crate) key_retain: Option<Glue>,
    pub(crate) key_release: Option<Glue>,
    pub(crate) value_retain: Option<Glue>,
    pub(crate) value_release: Option<Glue>,
}

impl MapObject {
    fn len(&self) -> usize {
        self.hashes.len()
    }

    fn entry(&self, index: usize) -> *const u8 {
        unsafe { self.entries.as_ptr().add(index * self.entry_size) }
    }

    fn entry_mut(&mut self, index: usize) -> *mut u8 {
        unsafe { self.entries.as_mut_ptr().add(index * self.entry_size) }
    }

    /// The entry position holding `key`, if any.
    unsafe fn find(&self, key: *const u8, hash: u64, glue: &MapGlue) -> Option<usize> {
        if self.table.is_empty() {
            return None;
        }
        let mask = self.table.len() - 1;
        let mut slot = hash as usize & mask;
        loop {
            match self.table[slot] {
                0 => return None,
                position => {
                    let index = position - 1;
                    if self.hashes[index] == hash && unsafe { (glue.key_eq)(self.entry(index), key) } != 0 {
                        return Some(index);
                    }
                }
            }
            slot = (slot + 1) & mask;
        }
    }

    fn rebuild(&mut self) {
        let size = (self.len() * 2).next_power_of_two().max(8);
        self.table = vec![0; size];
        let mask = size - 1;
        for (index, &hash) in self.hashes.iter().enumerate() {
            let mut slot = hash as usize & mask;
            while self.table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            self.table[slot] = index + 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_map_new(entry_size: usize, value_offset: usize) -> *mut MapObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(MapObject {
        rc: AtomicUsize::new(1),
        entry_size,
        value_offset,
        entries: Vec::new(),
        hashes: Vec::new(),
        table: Vec::new(),
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_retain(m: *mut MapObject) {
    unsafe { (*m).rc.fetch_add(1, Ordering::Relaxed) };
}

unsafe fn release_entries(m: &mut MapObject, glue: &MapGlue) {
    for index in 0..m.len() {
        let entry = m.entry_mut(index);
        unsafe {
            if let Some(release) = glue.key_release {
                release(entry);
            }
            if let Some(release) = glue.value_release {
                release(entry.add(m.value_offset));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_release(m: *mut MapObject, glue: *const MapGlue) {
    if m.is_null() {
        return;
    }
    match unsafe { (*m).rc.fetch_sub(1, Ordering::Release) } {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                let mut object = Box::from_raw(m);
                release_entries(&mut object, &*glue);
            }
            LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
        }
        0 => runtime_fault("map released after it was freed"),
        _ => {}
    }
}

/// Makes the map in `slot` unique before a write, copying a shared map and
/// retaining its keys and values.
unsafe fn map_unique(slot: *mut *mut MapObject, glue: &MapGlue) -> &'static mut MapObject {
    unsafe {
        let m = *slot;
        if (*m).rc.load(Ordering::Acquire) != 1 {
            let copy = tondo_rt_map_new((*m).entry_size, (*m).value_offset);
            (*copy).entries = (*m).entries.clone();
            (*copy).hashes = (*m).hashes.clone();
            (*copy).table = (*m).table.clone();
            for index in 0..(*copy).len() {
                let entry = (*copy).entry_mut(index);
                if let Some(retain) = glue.key_retain {
                    retain(entry);
                }
                if let Some(retain) = glue.value_retain {
                    retain(entry.add((*copy).value_offset));
                }
            }
            tondo_rt_map_release(m, glue);
            *slot = copy;
        }
        &mut **slot
    }
}

/// The address of the value stored under `key`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_find(m: *mut MapObject, key: *const u8, glue: *const MapGlue) -> *const u8 {
    unsafe {
        let glue = &*glue;
        let hash = (glue.hash)(key);
        match (*m).find(key, hash, glue) {
            Some(index) => (*m).entry(index).add((*m).value_offset),
            None => std::ptr::null(),
        }
    }
}

/// Inserts or replaces; the key and value move into the map. Returns 1 and
/// moves the previous value to `old` when the key was present (its position
/// is kept and the new key is released), 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_insert(
    slot: *mut *mut MapObject,
    key: *const u8,
    value: *const u8,
    old: *mut u8,
    glue: *const MapGlue,
) -> u8 {
    unsafe {
        let glue = &*glue;
        let m = map_unique(slot, glue);
        let hash = (glue.hash)(key);
        let value_size = m.entry_size - m.value_offset;
        if let Some(index) = m.find(key, hash, glue) {
            let stored = m.entry_mut(index).add(m.value_offset);
            std::ptr::copy_nonoverlapping(stored, old, value_size);
            std::ptr::copy_nonoverlapping(value, stored, value_size);
            if let Some(release) = glue.key_release {
                release(key as *mut u8);
            }
            return 1;
        }
        let index = m.len();
        m.entries.resize((index + 1) * m.entry_size, 0);
        let entry = m.entry_mut(index);
        std::ptr::copy_nonoverlapping(key, entry, m.value_offset);
        std::ptr::copy_nonoverlapping(value, entry.add(m.value_offset), value_size);
        m.hashes.push(hash);
        if m.table.len() < (index + 1) * 2 {
            m.rebuild();
        } else {
            let mask = m.table.len() - 1;
            let mut slot = hash as usize & mask;
            while m.table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            m.table[slot] = index + 1;
        }
        0
    }
}

/// Removes `key`, moving its value to `out`; keeps the order of the other
/// entries. Returns 1 when the key was present.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_remove(
    slot: *mut *mut MapObject,
    key: *const u8,
    out: *mut u8,
    glue: *const MapGlue,
) -> u8 {
    unsafe {
        let glue = &*glue;
        let hash = (glue.hash)(key);
        if (**slot).find(key, hash, glue).is_none() {
            return 0;
        }
        let m = map_unique(slot, glue);
        let index = m.find(key, hash, glue).unwrap();
        let entry = m.entry_mut(index);
        std::ptr::copy_nonoverlapping(entry.add(m.value_offset), out, m.entry_size - m.value_offset);
        if let Some(release) = glue.key_release {
            release(entry);
        }
        let size = m.entry_size;
        m.entries.drain(index * size..(index + 1) * size);
        m.hashes.remove(index);
        m.rebuild();
        1
    }
}

/// Removes every entry of the map or set in `slot`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_clear(slot: *mut *mut MapObject, glue: *const MapGlue) {
    unsafe {
        let m = *slot;
        if (*m).rc.load(Ordering::Acquire) != 1 {
            *slot = tondo_rt_map_new((*m).entry_size, (*m).value_offset);
            tondo_rt_map_release(m, glue);
            return;
        }
        release_entries(&mut *m, &*glue);
        (*m).entries.clear();
        (*m).hashes.clear();
        (*m).table.clear();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_len(m: *mut MapObject) -> usize {
    unsafe { (*m).len() }
}

/// The address of the entry at `index` in insertion order.
/// Gives the map in the slot its own storage before an entry changes in
/// place.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_unique(slot: *mut *mut MapObject, glue: *const MapGlue) {
    unsafe {
        map_unique(slot, &*glue);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_entry(m: *mut MapObject, index: usize) -> *const u8 {
    unsafe { (*m).entry(index) }
}

/// Content equality, ignoring order (10.14). `value_eq` is null for sets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_map_eq(
    a: *mut MapObject,
    b: *mut MapObject,
    glue: *const MapGlue,
    value_eq: Option<EqualGlue>,
) -> u8 {
    unsafe {
        let (glue, a, b) = (&*glue, &*a, &*b);
        if a.len() != b.len() {
            return 0;
        }
        for index in 0..a.len() {
            let entry = a.entry(index);
            let Some(other) = b.find(entry, a.hashes[index], glue) else { return 0 };
            if let Some(value_eq) = value_eq
                && value_eq(entry.add(a.value_offset), b.entry(other).add(a.value_offset)) == 0
            {
                return 0;
            }
        }
        1
    }
}

/// Appends `[key: value, ...]` (`[:]` when empty), or `Set[a, b]` for a set
/// (`value` null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_map(
    b: *mut Builder,
    m: *mut MapObject,
    key: DisplayGlue,
    value: Option<DisplayGlue>,
) {
    unsafe {
        let out = &mut (*b).0;
        if value.is_none() {
            out.extend_from_slice(b"Set");
        }
        out.push(b'[');
        if (*m).len() == 0 && value.is_some() {
            out.push(b':');
        }
        for index in 0..(*m).len() {
            if index > 0 {
                (*b).0.extend_from_slice(b", ");
            }
            let entry = (*m).entry(index);
            key(b, entry);
            if let Some(value) = value {
                (*b).0.extend_from_slice(b": ");
                value(b, entry.add((*m).value_offset));
            }
        }
        (*b).0.push(b']');
    }
}

/// FNV-1a over the bytes of a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_hash(s: *const Str) -> u64 {
    unsafe { str_bytes(s) }.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

// ---------------------------------------------------------------------------
// String builder (interpolation)
// ---------------------------------------------------------------------------

pub struct Builder(Vec<u8>);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sb_new() -> *mut Builder {
    Box::into_raw(Box::new(Builder(Vec::new())))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_str(b: *mut Builder, s: *const Str) {
    unsafe { (*b).0.extend_from_slice(str_bytes(s)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_i64(b: *mut Builder, value: i64) {
    unsafe { write!((*b).0, "{value}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_u64(b: *mut Builder, value: u64) {
    unsafe { write!((*b).0, "{value}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_bool(b: *mut Builder, value: u8) {
    let text: &[u8] = if value != 0 { b"true" } else { b"false" };
    unsafe { (*b).0.extend_from_slice(text) }
}

/// Floats use the shortest decimal form that reads back as the same value,
/// always distinguishable from an integer: `1.0`, `0.1`, `1e-7`, `NaN`, `inf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_f64(b: *mut Builder, value: f64) {
    unsafe { write!((*b).0, "{value:?}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_f32(b: *mut Builder, value: f32) {
    unsafe { write!((*b).0, "{value:?}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_char(b: *mut Builder, value: u32) {
    let c = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
    unsafe { write!((*b).0, "{c}").unwrap() }
}

/// Consumes the builder and returns an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_finish(b: *mut Builder) -> *mut Str {
    let builder = unsafe { Box::from_raw(b) };
    str_new(&builder.0)
}

// ---------------------------------------------------------------------------
// Console
// ---------------------------------------------------------------------------

/// Standard output. `print` and `println` append to this ordered buffer
/// (std.console); it is delivered when it fills, at every line when stdout
/// is a terminal, by `flush`, and when the program ends, so programs never
/// need to flush.
struct Output {
    buffer: Vec<u8>,
    /// Whether stdout is a terminal, decided on first use.
    terminal: Option<bool>,
    /// The first delivery failure nobody has observed yet. Output written
    /// after a failure is dropped.
    failed: Option<std::io::ErrorKind>,
}

static STDOUT: Mutex<Output> = Mutex::new(Output { buffer: Vec::new(), terminal: None, failed: None });
const STDOUT_FLUSH_THRESHOLD: usize = 64 * 1024;

impl Output {
    fn deliver(&mut self) {
        if self.failed.is_none() && !self.buffer.is_empty() {
            let mut stdout = std::io::stdout().lock();
            if let Err(error) = stdout.write_all(&self.buffer).and_then(|()| stdout.flush()) {
                self.failed = Some(error.kind());
            }
        }
        self.buffer.clear();
    }
}

fn stdout() -> std::sync::MutexGuard<'static, Output> {
    STDOUT.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn console_append(bytes: &[u8], newline: bool) {
    let mut out = stdout();
    out.buffer.extend_from_slice(bytes);
    if newline {
        out.buffer.push(b'\n');
    }
    let terminal = *out.terminal.get_or_insert_with(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
    let line = newline || bytes.contains(&b'\n');
    // A test's output must survive a crash, so a test build delivers it at once.
    if out.buffer.len() >= STDOUT_FLUSH_THRESHOLD || (terminal && line) || testing::ACTIVE.load(Ordering::Relaxed) {
        out.deliver();
    }
}

/// Delivers pending output before the process reports a failure itself.
pub fn flush_stdout() {
    stdout().deliver();
}

/// The first failure to deliver standard output since the last call.
pub(crate) fn stdout_failure() -> std::io::Result<()> {
    stdout().failed.take().map_or(Ok(()), |kind| Err(kind.into()))
}

/// Delivers pending output when the program ends. A failure nobody observed
/// is reported on stderr and makes the process exit with status 1.
pub fn finish_stdout() -> Option<i32> {
    let failed = io::finish();
    let mut out = stdout();
    out.deliver();
    let kind = failed.or(out.failed.take())?;
    let variant = match kind {
        std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => "Closed",
        std::io::ErrorKind::InvalidData => "InvalidData",
        _ => "Host",
    };
    let _ = writeln!(std::io::stderr(), "error: standard output: io.IoError.{variant}");
    Some(1)
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Process exit status of a panic (spec 20.10: distinct from error exits).
pub const PANIC_EXIT_CODE: i32 = 101;

fn panic_name(code: u32) -> &'static str {
    match code {
        1 => "bounds",
        2 => "zero-slice-step",
        3 => "integer-division-by-zero",
        4 => "overlapping-borrow",
        5 => "checked-overflow",
        6 => "array-shape-mismatch",
        7 => "assertion-failed",
        8 => "explicit-panic",
        9 => "duplicate-dynamic-map-key",
        10 => "invalid-shift-count",
        11 => "invalid-repeat-count",
        12 => "closed-channel",
        13 => "deadlock",
        14 => "reentrant-lock",
        15 => "invalid-environment-name",
        16 => "nul-in-c-string",
        2001 => "test-skip-during-cleanup",
        2002 => "test-tag-conflict",
        2003 => "test-virtual-time-deadlock",
        2004 => "overlapping-test-virtual-time",
        2005 => "test-virtual-time-range",
        2006 => "test-artifact-conflict",
        2007 => "test-snapshot-mismatch",
        2008 => "test-snapshot-conflict",
        _ => "unknown",
    }
}

/// Each task keeps the cleanups of the scopes it executes, innermost last
/// (13.7). Generated code pushes one when a `defer` registers and pops it
/// before running it on a normal exit; a panic runs the rest. Frames stay
/// alive meanwhile: a panic ends the process, so it never unwinds the
/// native stack.
static PANICKING: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_push(code: tasks::Cleanup, closure: *mut u8) {
    tasks::push_cleanup(code, closure);
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_defer_pop() {
    tasks::pop_cleanup();
}

fn run_cleanups() {
    tasks::run_cleanups();
}

/// Reports a language panic (spec 15.7, codes in 22.2), runs the pending
/// `defer` cleanups and terminates. A panic inside a cleanup is reported
/// as suppressed and the remaining cleanups still run. `message` may be
/// null; `location` is a static `file:line:column` string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic(mut code: u32, message: *const Str, location: *const Str) -> ! {
    flush_stdout();
    let message = (!message.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned());
    let location = String::from_utf8_lossy(unsafe { str_bytes(location) });
    let suppressed = PANICKING.fetch_add(1, Ordering::SeqCst) > 0;
    // `testing.skip(reason)` ends a test as skipped, unless cleanup runs.
    if code == testing::SKIP && suppressed {
        code = 2001;
    } else if code == testing::SKIP {
        testing::terminal(code, &format!("{}\n --> {location}", message.unwrap_or_default()));
        run_cleanups();
        tasks::end_all(0)
    }
    let mut text = format!("panic[P{code:04}]: {}", panic_name(code));
    if let Some(message) = message {
        text.push_str(": ");
        text.push_str(&message);
    }
    let prefix = if suppressed { "suppressed while unwinding: " } else { "" };
    let report = format!("{prefix}{text}\n --> {location}");
    // A test build reports it to `tondo test` instead.
    if !testing::terminal(code, &report) {
        let _ = writeln!(std::io::stderr(), "{report}");
    }
    run_cleanups();
    // The other tasks run their cleanups too, before the process ends.
    tasks::end_all(PANIC_EXIT_CODE)
}

/// Reports the error a fallible `main` returned (spec 20.10).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_report_error(description: *const Str) {
    flush_stdout();
    let text = String::from_utf8_lossy(unsafe { str_bytes(description) });
    let _ = writeln!(std::io::stderr(), "error: {text}");
}

/// An internal invariant of the runtime or the generated code was broken.
fn runtime_fault(message: &str) -> ! {
    flush_stdout();
    let _ = writeln!(std::io::stderr(), "tondo runtime fault: {message}");
    std::process::abort()
}

/// Exit status when `TONDO_RT_CHECK_LEAKS` finds live strings at exit.
pub const LEAK_EXIT_CODE: i32 = 99;

pub fn check_leaks() -> Option<usize> {
    std::env::var_os("TONDO_RT_CHECK_LEAKS")?;
    tasks::wait_foreign_calls();
    let live = LIVE_OBJECTS.load(Ordering::Relaxed);
    (live != 0).then_some(live)
}

// ---------------------------------------------------------------------------
// Process entry
// ---------------------------------------------------------------------------

#[cfg(tondo_staticlib)]
unsafe extern "C" {
    /// Generated by the compiler: runs the program's `main` and returns the
    /// process exit status.
    fn tondo_main() -> i32;
}

#[cfg(tondo_staticlib)]
#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    let mut status = tasks::run_main(tondo_main);
    if let Some(status) = tasks::ending() {
        process::cancel_live();
        flush_stdout();
        return status;
    }
    // Memory the shared collections retired, kept while readers might see it.
    shared::flush();
    if let Some(failure) = finish_stdout()
        && status == 0
    {
        status = failure;
    }
    if let Some(live) = check_leaks() {
        let _ = writeln!(std::io::stderr(), "tondo runtime: {live} object(s) leaked");
        return LEAK_EXIT_CODE;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_compare_by_scalar_order_and_free_at_zero() {
        let a = str_new("abc".as_bytes());
        let b = str_new("abd".as_bytes());
        unsafe {
            assert_eq!(tondo_rt_str_cmp(a, b), -1);
            assert_eq!(tondo_rt_str_eq(a, a), 1);
            tondo_rt_str_retain(a);
            tondo_rt_str_release(a);
            assert_eq!((*a).rc.load(Ordering::Relaxed), 1);
            tondo_rt_str_release(a);
            tondo_rt_str_release(b);
        }
    }

    #[test]
    fn builder_formats_scalars() {
        let b = tondo_rt_sb_new();
        let s = str_new("x=".as_bytes());
        unsafe {
            tondo_rt_sb_str(b, s);
            tondo_rt_sb_i64(b, -42);
            tondo_rt_sb_bool(b, 1);
            let out = tondo_rt_sb_finish(b);
            assert_eq!(str_bytes(out), b"x=-42true");
            tondo_rt_str_release(out);
            tondo_rt_str_release(s);
        }
    }
}
