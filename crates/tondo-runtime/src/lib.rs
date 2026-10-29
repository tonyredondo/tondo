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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};

mod bench;
mod cbor;
mod civil;
mod clock;
mod codec;
mod compress;
mod contexts;
mod cycles;
mod debugger;
mod diagnostics;
mod env;
mod files;
mod fs;
mod fuzz;
mod heap;
mod http;
mod io;
mod json;
mod labels;
mod live;
mod math;
mod memory;
mod meta;
mod msgpack;
mod native;
mod net;
mod numbers;
mod panics;
mod pipes;
mod poll;
mod pprof;
mod process;
mod profiling;
mod recorder;
mod reflect;
mod regex;
mod server;
mod shared;
mod shown;
mod signals;
mod snapshot;
#[cfg(all(tondo_staticlib, target_os = "linux"))]
mod start;
mod symbols;
mod sync;
mod tasks;
mod testing;
mod text;
mod time;
mod toml;
mod trace;
mod yaml;
mod zones;

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// Header of every string. The UTF-8 bytes follow the header, with room
/// for `cap` of them, which a string appended to in place grows into.
///
/// String literals are emitted by the compiler as static objects with the
/// same layout and `rc == IMMORTAL`, so retain and release ignore them.
#[repr(C)]
pub struct Str {
    rc: AtomicUsize,
    len: usize,
    cap: usize,
}

const IMMORTAL: usize = usize::MAX;
pub(crate) const HEADER: usize = std::mem::size_of::<Str>();

/// Heap objects (strings and boxes) currently allocated. Checked at exit when
/// `TONDO_RT_CHECK_LEAKS` is set, which the compiler's tests use to verify
/// reference counting.
static LIVE_OBJECTS: diagnostics::Objects = diagnostics::Objects;

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
        ptr.write(Str { rc: AtomicUsize::new(1), len: bytes.len(), cap: bytes.len() });
        LIVE_OBJECTS.made();
        copy_short(bytes, (ptr as *mut u8).add(HEADER));
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
    match dropped(rc) {
        1 => {
            fence(Ordering::Acquire);
            LIVE_OBJECTS.gone();
            unsafe { dealloc(s as *mut u8, str_layout((*s).cap)) }
        }
        0 => runtime_fault("string released after it was freed"),
        _ => {}
    }
}

/// Drops a reference to an object and returns the count it had. The last
/// one goes without the atomic decrement, which costs more than most
/// objects' allocation: no other reference is left to count, nor to take
/// another, as only a `Ref`'s weak ones could.
#[inline(always)]
fn dropped(rc: &AtomicUsize) -> usize {
    match rc.load(Ordering::Relaxed) {
        1 => 1,
        _ => rc.fetch_sub(1, Ordering::Release),
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
pub(crate) unsafe fn str_text<'a>(s: *const Str) -> &'a str {
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
        -1 => {
            let length = unsafe { tondo_rt_str_length(s) } as usize;
            raise(1, &out_of_bounds(index, length, "string"), location);
            0
        }
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
    let (text, other) = unsafe { (str_bytes(s), str_bytes(other)) };
    (match test {
        0 => find(text, other).is_some(),
        1 => text.starts_with(other),
        _ => text.ends_with(other),
    }) as u8
}

/// The scalar position of the first occurrence of `needle`, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_find(s: *const Str, needle: *const Str) -> i64 {
    let (text, needle) = unsafe { (str_text(s), str_bytes(needle)) };
    find(text.as_bytes(), needle).map_or(-1, |offset| text[..offset].chars().count() as i64)
}

/// Where `needle` first is in `haystack`: at each place its first byte is,
/// as Go does, and past a few misses with a searcher that is never slow.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let Some(&first) = needle.first() else { return Some(0) };
    let mut at = 0;
    for _ in 0..16 {
        let start = at + memchr::memchr(first, &haystack[at..])?;
        if haystack[start..].starts_with(needle) {
            return Some(start);
        }
        at = start + 1;
    }
    memchr::memmem::find(&haystack[at..], needle).map(|offset| at + offset)
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
        LIVE_OBJECTS.made();
        ptr
    }
}

fn box_value(b: *mut BoxHeader) -> *mut u8 {
    unsafe { (b as *mut u8).add(BOX_HEADER) }
}

/// The value of a box, for the cycle collector.
pub(crate) fn box_payload(b: *mut u8) -> *mut u8 {
    box_value(b as *mut BoxHeader)
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
    match dropped(unsafe { &(*b).rc }) {
        1 => {
            fence(Ordering::Acquire);
            if let Some(release) = release {
                unsafe { release(box_value(b)) };
            }
            LIVE_OBJECTS.gone();
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
// References
// ---------------------------------------------------------------------------

/// Header of the object of a `Ref[T]` (16.10), its identity: the value
/// follows. `weak` counts the weak references, plus one while any strong
/// one lives; the value goes with the last strong reference and the
/// memory with the last of either. `dead` is set by the cycle collector on
/// garbage whose counts have not reached zero yet.
#[repr(C)]
pub struct RefHeader {
    rc: AtomicUsize,
    weak: AtomicUsize,
    size: usize,
    dead: AtomicBool,
}

const REF_HEADER: usize = std::mem::size_of::<RefHeader>();

fn ref_layout(size: usize) -> Layout {
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
    unsafe { (*r).weak.fetch_add(1, Ordering::Relaxed) };
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
unsafe fn ref_forget(r: *mut RefHeader) {
    unsafe {
        if (*r).weak.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            LIVE_OBJECTS.gone();
            dealloc(r as *mut u8, ref_layout((*r).size));
        }
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
    /// Visits the captures for the cycle collector, when they may refer
    /// to a cycle.
    visit: Option<cycles::Visit>,
}

const FN_HEADER: usize = std::mem::size_of::<FnObject>();

fn fn_layout(size: usize) -> Layout {
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

/// Visits an array's elements for the cycle collector.
pub(crate) unsafe fn visit_array(visitor: &mut cycles::Visitor, a: *mut u8, size: usize, visit: Option<cycles::Visit>) {
    let a = a as *mut ArrayHeader;
    for index in 0..unsafe { (*a).len } {
        unsafe { visitor.visit(visit, element(a, size, index)) };
    }
}

/// Allocates an array of `len` uninitialized elements and room for `cap`.
pub(crate) fn array_alloc(size: usize, len: usize, cap: usize) -> *mut ArrayHeader {
    let cap = cap.max(len);
    let layout = array_layout(size, cap);
    unsafe {
        let ptr = alloc(layout) as *mut ArrayHeader;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr.write(ArrayHeader { rc: AtomicUsize::new(1), len, cap, head: 0 });
        LIVE_OBJECTS.made();
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
    match dropped(unsafe { &(*a).rc }) {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                if let Some(release) = release {
                    for index in 0..(*a).len {
                        release(element(a, size, index));
                    }
                }
                LIVE_OBJECTS.gone();
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
            LIVE_OBJECTS.gone();
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
        let position = normalize(at, len);
        if position < 0 || position as usize > len {
            let message =
                format!("cannot insert at index {at} of an array of length {len}: {}", valid_indices(len, len));
            raise(1, &message, location);
            return;
        }
        let at = position;
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
        raise(
            2,
            "the step of a slice is 0: a slice steps forwards with a positive step and backwards with a negative one",
            location,
        );
        return (0, 0);
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
                let at = a as i128 + k as i128 * sa as i128;
                let message = if index != 0 {
                    format!("one call lends the element at index {at} of an array twice: alone and inside a slice")
                } else {
                    format!("one call lends two slices of an array that share the element at index {at}")
                };
                raise(4, &message, location);
                return;
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
            let message = format!(
                "the slice selects {count} elements and the array assigned to it has {}: they must have the same length",
                (*source).len
            );
            raise(6, &message, location);
            return;
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
    ($($t:ty: $name:literal),*) => {$(
        impl Number for $t {
            unsafe fn apply(op: u32, a: Self, b: Self, location: *const Str) -> Self {
                let operator = ["+", "-", "*", "/", "%"][op as usize];
                let result = match op {
                    0 => a.checked_add(b),
                    1 => a.checked_sub(b),
                    2 => a.checked_mul(b),
                    _ if b == 0 => {
                        raise(3, &format!("{a} {operator} 0 in arithmetic on arrays"), location);
                        Some(0)
                    }
                    3 => a.checked_div(b),
                    // `MIN % -1` is 0 (18.3).
                    _ => Some(a.wrapping_rem(b)),
                };
                result.unwrap_or_else(|| {
                    let message = format!(
                        "{a} {operator} {b} in arithmetic on arrays does not fit in {} ({} to {})",
                        $name, <$t>::MIN, <$t>::MAX
                    );
                    raise(5, &message, location);
                    0
                })
            }
        }
    )*};
}

integer_numbers!(
    i8: "Int8", i16: "Int16", i32: "Int32", i64: "Int", u8: "UInt8", u16: "UInt16", u32: "UInt32", u64: "UInt",
    i128: "Int128", u128: "UInt128"
);

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
/// UInt32, UInt64, Float32, Float64, Int128 and UInt128, from 0.
fn number_size(number: u32) -> usize {
    match number {
        0 | 4 => 1,
        1 | 5 => 2,
        2 | 6 | 8 => 4,
        10 | 11 => 16,
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
            let message = format!(
                "arrays of lengths {} and {} cannot go element by element: they must have the same length",
                (*x).len,
                (*y).len
            );
            raise(6, &message, location);
            return;
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
                10 => apply_number::<i128>,
                11 => apply_number::<u128>,
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
        if panics::unwinding() {
            return std::ptr::null_mut();
        }
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

/// `Array[Int].sorted()`: the integers in order, in a new array. Equal
/// integers cannot be told apart, so the sort need not keep them as they
/// were, and pattern-defeating quicksort is faster than a merge.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_sorted_ints(a: *mut ArrayHeader) -> *mut ArrayHeader {
    unsafe {
        let len = (*a).len;
        let new = array_copy(a, 8, None, 0, 1, len, len);
        std::slice::from_raw_parts_mut(element(new, 8, 0) as *mut i64, len).sort_unstable();
        new
    }
}

/// `Array[String].sorted()`: the strings in order, in a new array that
/// holds them too; equal strings cannot be told apart either.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_array_sorted_strings(a: *mut ArrayHeader) -> *mut ArrayHeader {
    unsafe {
        let len = (*a).len;
        let new = array_copy(a, 8, Some(tondo_rt_str_retain_glue), 0, 1, len, len);
        let strings = std::slice::from_raw_parts_mut(element(new, 8, 0) as *mut *const Str, len);
        strings.sort_unstable_by(|a, b| str_bytes(*a).cmp(str_bytes(*b)));
        new
    }
}

unsafe extern "C" fn tondo_rt_str_retain_glue(slot: *mut u8) {
    unsafe { tondo_rt_str_retain(*(slot as *const *const Str)) }
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
            let message = format!("arrays of lengths {la} and {lb} together are longer than an array can be");
            raise(5, &message, location);
            return std::ptr::null_mut();
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
            raise(11, &format!("an array cannot repeat {count} times: the count must not be negative"), location);
            return std::ptr::null_mut();
        }
        let len = (*a).len;
        let total = len as i128 * count as i128;
        if total > i64::MAX as i128 {
            let message = format!("an array of length {len} repeated {count} times is longer than an array can be");
            raise(5, &message, location);
            return std::ptr::null_mut();
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
        (*b).push(b'[');
        for k in 0..(*a).len {
            if k > 0 {
                (*b).extend(b", ");
            }
            display(b, element(a, size, k));
        }
        (*b).push(b']');
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

/// Visits a map's keys and values for the cycle collector.
pub(crate) unsafe fn visit_map(
    visitor: &mut cycles::Visitor,
    map: *mut u8,
    key: Option<cycles::Visit>,
    value: Option<cycles::Visit>,
) {
    let map = unsafe { &*(map as *const MapObject) };
    for index in 0..map.len() {
        let entry = map.entry(index) as *mut u8;
        unsafe {
            visitor.visit(key, entry);
            visitor.visit(value, entry.add(map.value_offset));
        }
    }
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
    /// For the cycle collector, when keys or values may refer to a cycle.
    pub(crate) key_visit: Option<cycles::Visit>,
    pub(crate) value_visit: Option<cycles::Visit>,
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
    LIVE_OBJECTS.made();
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
    match dropped(unsafe { &(*m).rc }) {
        1 => {
            fence(Ordering::Acquire);
            unsafe {
                let mut object = Box::from_raw(m);
                release_entries(&mut object, &*glue);
            }
            LIVE_OBJECTS.gone();
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
        let out = &mut *b;
        if value.is_none() {
            out.extend(b"Set");
        }
        out.push(b'[');
        if (*m).len() == 0 && value.is_some() {
            out.push(b':');
        }
        for index in 0..(*m).len() {
            if index > 0 {
                (*b).extend(b", ");
            }
            let entry = (*m).entry(index);
            key(b, entry);
            if let Some(value) = value {
                (*b).extend(b": ");
                value(b, entry.add((*m).value_offset));
            }
        }
        (*b).push(b']');
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

/// Text being built, as interpolation builds it: a string with room for
/// more, which `finish` makes the string built without copying it. The code
/// that interpolates keeps it on its stack.
#[repr(C)]
pub struct Builder {
    /// A string whose header's `cap` is its room; null while it has none.
    text: *mut Str,
    len: usize,
}

impl Builder {
    pub fn new(room: usize) -> Builder {
        let mut builder = Builder { text: std::ptr::null_mut(), len: 0 };
        if room > 0 {
            builder.grow(room);
        }
        builder
    }

    fn room(&self) -> usize {
        if self.text.is_null() { 0 } else { unsafe { (*self.text).cap } }
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        if self.len + bytes.len() > self.room() {
            self.grow(bytes.len());
        }
        unsafe { copy_short(bytes, (self.text as *mut u8).add(HEADER + self.len)) }
        self.len += bytes.len();
    }

    pub fn push(&mut self, byte: u8) {
        self.extend(&[byte]);
    }

    #[cold]
    fn grow(&mut self, more: usize) {
        let room = (self.len + more).max(self.room() * 2);
        let layout = str_layout(room);
        // A new block holds what was asked, as a guess that is seldom
        // short; one that grows takes all the room its block has.
        let grown = !self.text.is_null();
        let text = unsafe {
            match grown {
                false => alloc(layout),
                true => std::alloc::realloc(self.text as *mut u8, str_layout(self.room()), layout.size()),
            }
        };
        if text.is_null() {
            handle_alloc_error(layout);
        }
        self.text = text as *mut Str;
        unsafe { (*self.text).cap = if grown { meta::usable(text, layout.size()) - HEADER } else { room } }
    }

    /// The string built. Its block keeps room it does not use, as much as it
    /// uses at most, which the allocator frees with it and a string appended
    /// to in place grows into, unless a budget or a watch counts the memory
    /// a string holds by its length.
    pub fn finish(self) -> *mut Str {
        if self.text.is_null() {
            return str_new(&[]);
        }
        let mut text = self.text as *mut u8;
        let mut cap = self.room();
        let counted = diagnostics::hooks() & (diagnostics::BUDGET | diagnostics::WATCHED) != 0;
        if cap - self.len > self.len.max(16) || (counted && cap != self.len) {
            let layout = str_layout(self.len);
            text = unsafe { std::alloc::realloc(text, str_layout(cap), layout.size()) };
            if text.is_null() {
                handle_alloc_error(layout);
            }
            cap = self.len;
        }
        let text = text as *mut Str;
        unsafe { text.write(Str { rc: AtomicUsize::new(1), len: self.len, cap }) };
        LIVE_OBJECTS.made();
        text
    }
}

impl Write for Builder {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Starts the builder at `b` with the elements of an array, as their
/// `Display` writes them, with `separator` between them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_join(
    b: *mut Builder,
    a: *mut ArrayHeader,
    size: usize,
    display: DisplayGlue,
    separator: *const Str,
) {
    unsafe {
        let separator = str_bytes(separator);
        b.write(Builder::new((*a).len * (8 + separator.len())));
        for k in 0..(*a).len {
            if k > 0 {
                (*b).extend(separator);
            }
            display(b, element(a, size, k));
        }
    }
}

/// `x = "{x}{t}"`: `s` with `t` after it, in `s`'s own block when nothing
/// else holds it and the block has room, or else in a new one, `s` then
/// released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_append(s: *mut Str, t: *const Str) -> *mut Str {
    unsafe { append(s, str_bytes(t)) }
}

/// `x = "{x}{i}"`, as `tondo_rt_str_append`.
/// `"{i}"`: an integer's digits, in a string of their own; those of 0 to
/// 99, as Go's, from strings the program keeps, which take no allocation.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int_text(value: i64) -> *mut Str {
    #[repr(C)]
    struct Small {
        header: Str,
        digits: [u8; 8],
    }
    static SMALL: [Small; 100] = {
        let mut table =
            [const { Small { header: Str { rc: AtomicUsize::new(IMMORTAL), len: 1, cap: 1 }, digits: [0; 8] } }; 100];
        let mut n = 0;
        while n < 100 {
            match n < 10 {
                true => table[n].digits[0] = b'0' + n as u8,
                false => {
                    table[n].header.len = 2;
                    table[n].header.cap = 2;
                    table[n].digits[0] = b'0' + (n / 10) as u8;
                    table[n].digits[1] = b'0' + (n % 10) as u8;
                }
            }
            n += 1;
        }
        table
    };
    if (0..100).contains(&value) {
        return &SMALL[value as usize] as *const Small as *mut Str;
    }
    let mut digits = [0; 21];
    let text = decimal(value.unsigned_abs(), (&mut digits[1..]).try_into().unwrap());
    let start = 21 - text.len() - usize::from(value < 0);
    if value < 0 {
        digits[start] = b'-';
    }
    str_new(&digits[start..])
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_append_int(s: *mut Str, value: i64) -> *mut Str {
    let mut digits = [0; 21];
    let text = decimal(value.unsigned_abs(), (&mut digits[1..]).try_into().unwrap());
    let start = 21 - text.len() - usize::from(value < 0);
    if value < 0 {
        digits[start] = b'-';
    }
    unsafe { append(s, &digits[start..]) }
}

/// Copies `bytes` to `to`: a few bytes, as a string's usual piece, in two
/// moves of a word that may overlap rather than a call of `memcpy`.
#[inline(always)]
unsafe fn copy_short(bytes: &[u8], to: *mut u8) {
    let (from, n) = (bytes.as_ptr(), bytes.len());
    unsafe {
        match n {
            8..=16 => {
                to.cast::<u64>().write_unaligned(from.cast::<u64>().read_unaligned());
                to.add(n - 8).cast::<u64>().write_unaligned(from.add(n - 8).cast::<u64>().read_unaligned());
            }
            4..=7 => {
                to.cast::<u32>().write_unaligned(from.cast::<u32>().read_unaligned());
                to.add(n - 4).cast::<u32>().write_unaligned(from.add(n - 4).cast::<u32>().read_unaligned());
            }
            1..=3 => {
                *to = *from;
                *to.add(n / 2) = *from.add(n / 2);
                *to.add(n - 1) = *from.add(n - 1);
            }
            0 => {}
            _ => std::ptr::copy_nonoverlapping(from, to, n),
        }
    }
}

unsafe fn append(s: *mut Str, bytes: &[u8]) -> *mut Str {
    unsafe {
        let len = (*s).len;
        if (*s).rc.load(Ordering::Acquire) == 1 {
            // The room its header knows, and once that is used, the room
            // its block has besides, which the allocator may have given.
            if len + bytes.len() > (*s).cap {
                (*s).cap = (*s).cap.max(meta::usable(s as *const u8, str_layout((*s).cap).size()) - HEADER);
            }
            if len + bytes.len() <= (*s).cap {
                copy_short(bytes, (s as *mut u8).add(HEADER + len));
                (*s).len = len + bytes.len();
                return s;
            }
        }
        let mut builder = Builder::new(0);
        if tondo_rt_sb_adopt(&mut builder, s, bytes.len()) == 0 {
            tondo_rt_str_release(s);
        }
        builder.extend(bytes);
        builder.finish()
    }
}

/// Starts the builder at `b` with the string `s` and room for `room` bytes
/// more: in the string's own block when nothing else holds it, which is the
/// builder's then (1), or else in a new one with a copy of it (0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_adopt(b: *mut Builder, s: *mut Str, room: usize) -> u8 {
    unsafe {
        if (*s).rc.load(Ordering::Acquire) == 1 {
            // The block may hold more than the string: room it grows into.
            let len = (*s).len;
            (*s).cap = (*s).cap.max(meta::usable(s as *const u8, str_layout((*s).cap).size()) - HEADER);
            b.write(Builder { text: s, len });
            if len + room > (*b).room() {
                (*b).grow(room);
            }
            LIVE_OBJECTS.gone();
            return 1;
        }
        b.write(Builder::new((*s).len + room));
        (*b).extend(str_bytes(s));
        0
    }
}

/// Starts the builder at `b`, with room for `room` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_start(b: *mut Builder, room: usize) {
    unsafe { b.write(Builder::new(room)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_str(b: *mut Builder, s: *const Str) {
    unsafe { (*b).extend(str_bytes(s)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_i64(b: *mut Builder, value: i64) {
    let mut digits = [0; 20];
    let text = decimal(value.unsigned_abs(), &mut digits);
    unsafe {
        if value < 0 {
            (*b).push(b'-');
        }
        (*b).extend(text);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_u64(b: *mut Builder, value: u64) {
    let mut digits = [0; 20];
    unsafe { (*b).extend(decimal(value, &mut digits)) }
}

/// An integer of 128 bits, in two halves, as `"{value}"` writes it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_int128(b: *mut Builder, low: u64, high: u64, signed: u8) {
    let value = (high as u128) << 64 | low as u128;
    let text = if signed != 0 { (value as i128).to_string() } else { value.to_string() };
    unsafe { (*b).extend(text.as_bytes()) }
}

/// `a * b` of 128 bits, in halves: its low bits into `out`, and whether
/// it does not fit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int128_multiply(signed: u8, values: *const [u64; 4], out: *mut [u64; 2]) -> u8 {
    let [a_low, a_high, b_low, b_high] = unsafe { *values };
    let (a, b) = ((a_high as u128) << 64 | a_low as u128, (b_high as u128) << 64 | b_low as u128);
    let overflow = match signed != 0 {
        true => (a as i128).checked_mul(b as i128).is_none(),
        false => a.checked_mul(b).is_none(),
    };
    let result = a.wrapping_mul(b);
    unsafe { *out = [result as u64, (result >> 64) as u64] };
    overflow as u8
}

/// `a / b`, or `a % b` when `remainder`, of 128 bits, in halves, into
/// `out`; the compiled code has ruled out a zero divisor and `MIN / -1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int128_divide(
    remainder: u8,
    signed: u8,
    values: *const [u64; 4],
    out: *mut [u64; 2],
) {
    let [a_low, a_high, b_low, b_high] = unsafe { *values };
    let (a, b) = ((a_high as u128) << 64 | a_low as u128, (b_high as u128) << 64 | b_low as u128);
    let result = match (signed != 0, remainder != 0) {
        (true, false) => (a as i128).wrapping_div(b as i128) as u128,
        (true, true) => (a as i128).wrapping_rem(b as i128) as u128,
        (false, false) => a / b,
        (false, true) => a % b,
    };
    unsafe { *out = [result as u64, (result >> 64) as u64] };
}

/// An integer of 128 bits, in halves, as the nearest `Float`, or the
/// nearest `Float32` when `single`, which a `Float` holds exactly.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int128_to_float(low: u64, high: u64, signed: u8, single: u8) -> f64 {
    let value = (high as u128) << 64 | low as u128;
    match (signed != 0, single != 0) {
        (true, false) => value as i128 as f64,
        (true, true) => value as i128 as f32 as f64,
        (false, false) => value as f64,
        (false, true) => value as f32 as f64,
    }
}

/// A float that the compiled code has found integral and in range, as an
/// integer of 128 bits, in halves, into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_float_to_int128(value: f64, signed: u8, out: *mut [u64; 2]) {
    let result = if signed != 0 { value as i128 as u128 } else { value as u128 };
    unsafe { *out = [result as u64, (result >> 64) as u64] };
}

/// The decimal digits of `value`, two at a time, at the end of `digits`.
fn decimal(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
    const PAIRS: &[u8; 200] = b"0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";
    let mut at = digits.len();
    while value >= 100 {
        let pair = (value % 100) as usize * 2;
        value /= 100;
        at -= 2;
        digits[at..at + 2].copy_from_slice(&PAIRS[pair..pair + 2]);
    }
    if value >= 10 {
        at -= 2;
        digits[at..at + 2].copy_from_slice(&PAIRS[value as usize * 2..value as usize * 2 + 2]);
    } else {
        at -= 1;
        digits[at] = b'0' + value as u8;
    }
    &digits[at..]
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_bool(b: *mut Builder, value: u8) {
    let text: &[u8] = if value != 0 { b"true" } else { b"false" };
    unsafe { (*b).extend(text) }
}

/// Floats use the shortest decimal form that reads back as the same value,
/// always distinguishable from an integer: `1.0`, `0.1`, `1e-7`, `NaN`, `inf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_f64(b: *mut Builder, value: f64) {
    unsafe { write!(*b, "{value:?}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_f32(b: *mut Builder, value: f32) {
    unsafe { write!(*b, "{value:?}").unwrap() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_char(b: *mut Builder, value: u32) {
    let c = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
    unsafe { write!(*b, "{c}").unwrap() }
}

/// A text in quotes, escaped as JSON escapes it, as a debugger shows it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_quoted(b: *mut Builder, s: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(s) });
    unsafe { write!(*b, "{text:?}").unwrap() }
}

/// Consumes the builder and returns an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_finish(b: *mut Builder) -> *mut Str {
    unsafe { b.read() }.finish()
}

// ---------------------------------------------------------------------------
// Console
// ---------------------------------------------------------------------------

/// Standard output. `print` and `println` append to this ordered buffer
/// (std.console); it is delivered when it fills, at every line when stdout
/// is a terminal or the program a debug build, by `flush`, and when the
/// program ends, so programs never need to flush.
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

/// The standard output, unless another holds it.
fn try_stdout() -> Option<std::sync::MutexGuard<'static, Output>> {
    match STDOUT.try_lock() {
        Ok(out) => Some(out),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub(crate) fn console_append(bytes: &[u8], newline: bool) {
    let mut out = stdout();
    out.buffer.extend_from_slice(bytes);
    if newline {
        out.buffer.push(b'\n');
    }
    let terminal = *out.terminal.get_or_insert_with(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
    let line = newline || bytes.contains(&b'\n');
    // A test's output must survive a crash, so a test build delivers it at
    // once; a debug build's comes at every line, as to a terminal, for the
    // tool that reads it.
    let lines = terminal || debugger::built();
    if out.buffer.len() >= STDOUT_FLUSH_THRESHOLD || (lines && line) || testing::ACTIVE.load(Ordering::Relaxed) {
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

pub(crate) fn panic_name(code: u32) -> &'static str {
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
        17 => "invalid-panic-code",
        18 => "negative-exponent",
        2001 => "test-skip-during-cleanup",
        2002 => "test-tag-conflict",
        2003 => "test-virtual-time-deadlock",
        2004 => "overlapping-test-virtual-time",
        2005 => "test-virtual-time-range",
        2006 => "test-artifact-conflict",
        2007 => "test-snapshot-mismatch",
        2008 => "test-snapshot-conflict",
        2009 => "test-bench-conflict",
        2010 => "test-fuzz-conflict",
        2011 => "test-fuzz-input-invalid",
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
    // A `defer` that ran before this one and panicked while a panic
    // unwinds was reported as suppressed, and is done (language 15.7).
    panics::defer_runs();
}

fn run_cleanups() {
    tasks::run_cleanups();
}

/// Reports a language panic (spec 15.7, codes in 22.2), runs the pending
/// `defer` cleanups and terminates. A panic inside a cleanup is reported
/// as suppressed and the remaining cleanups still run. `message` may be
/// null; `location` is a static `file:line:column` string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic(code: u32, message: *const Str, location: *const Str) -> ! {
    unsafe { end_in_panic(code, message, location) }
}

/// A panic where compiled code unwinds when the call returns: one that a
/// `recover` of the running task stops is recorded, and the call returns
/// (language 15.9); any other ends the program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_unwind(code: u32, message: *const Str, location: *const Str) {
    if unsafe { panics::record(code, message, location) } {
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// A panic of the runtime: recorded when a `recover` of the running task
/// stops it, and the caller returns at once with a value nobody uses;
/// otherwise the program ends here.
pub(crate) fn raise(code: u32, message: &str, location: *const Str) {
    let message = str_new(message.as_bytes());
    if unsafe { panics::record(code, message, location) } {
        unsafe { tondo_rt_str_release(message) };
        return;
    }
    unsafe { end_in_panic(code, message, location) }
}

/// Reports a panic nothing recovers and ends the program.
unsafe fn end_in_panic(mut code: u32, message: *const Str, location: *const Str) -> ! {
    // `testing.failNow` reports as a failed assertion, which no `recover` stops.
    if code == panics::FAIL_NOW {
        code = 7;
    }
    flush_stdout();
    let message = (!message.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned());
    let mut location = String::from_utf8_lossy(unsafe { str_bytes(location) }).into_owned();
    // A panic inside the standard library points at the program's call.
    if location.starts_with("<std>/") {
        location = trace::caller().unwrap_or(location);
    }
    let suppressed = PANICKING.fetch_add(1, Ordering::SeqCst) > 0;
    // `testing.skip(reason)` ends a test as skipped, unless cleanup runs.
    if code == testing::SKIP && suppressed {
        code = 2001;
    } else if code == testing::SKIP {
        testing::terminal(code, &format!("{}\n --> {location}", message.unwrap_or_default()));
        run_cleanups();
        tasks::end_all(0)
    }
    // An assertion's data are its condition and the sides it compared.
    if code == 7
        && let Some(message) = &message
    {
        let line = |prefix: &str| message.lines().find_map(|l| l.strip_prefix(prefix)).map(str::to_string);
        let condition = line("  condition: ")
            .or_else(|| line("condition: "))
            .unwrap_or_else(|| message.lines().next().unwrap_or_default().to_string());
        let mut data = vec![("condition", panics::Datum::Text(condition))];
        if let (Some(left), Some(right)) = (line("  left:  "), line("  right: ")) {
            data.push(("left", panics::Datum::Text(left)));
            data.push(("right", panics::Datum::Text(right)));
        }
        panics::language_data(data);
    }
    let resumed = panics::resumed_stack();
    let panic = panics::Panic::new(code, panic_name(code), message.unwrap_or_default(), location);
    let text = panic.headline();
    let location = &panic.location;
    let prefix = if suppressed { "suppressed while unwinding: " } else { "" };
    if recorder::on() {
        let task = tasks::current_id();
        recorder::record(recorder::Event::Panic { task, message: format!("{text}\n --> {location}") });
    }
    // A debugger sees where the program's code panicked first.
    if !suppressed {
        debugger::panicked(&text);
    }
    let frames = resumed.as_deref().map(panics::frames_of);
    let stack = resumed.unwrap_or_else(trace::report);
    let report = format!("{prefix}{text}{}{stack}", panic.place());
    // A test build reports it to `tondo test` instead, with its data.
    testing::panic_data(&panic);
    if !testing::terminal(code, &report) {
        match panics::json_reports() {
            true => {
                let frames = frames.unwrap_or_else(trace::report_json);
                let _ = writeln!(std::io::stderr(), "{}", panic.json(&frames, false, suppressed));
            }
            false => {
                let _ = writeln!(std::io::stderr(), "{report}");
            }
        }
        if !suppressed {
            diagnostics::crash_dump("panic", &format!("{text}\n --> {location}"), trace::here_frames(), true);
        }
    }
    run_cleanups();
    // The other tasks run their cleanups too, before the process ends.
    tasks::end_all(PANIC_EXIT_CODE)
}

/// Panics with `code` and a message the runtime writes.
pub(crate) fn panic_with(code: u32, message: &str, location: *const Str) -> ! {
    let message = str_new(message.as_bytes());
    unsafe { tondo_rt_panic(code, message, location) }
}

/// The message of `index` outside the `len` elements of a `noun` (`P0001`).
pub(crate) fn out_of_bounds(index: i64, len: usize, noun: &str) -> String {
    if len == 0 {
        return format!("index {index} is out of bounds: the {noun} is empty");
    }
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("index {index} is out of bounds for {article} {noun} of length {len}: {}", valid_indices(len - 1, len))
}

/// The indices from 0 to `last`, and those from the end of `len` elements.
fn valid_indices(last: usize, len: usize) -> String {
    match (last, len) {
        (0, 0) => "the only valid index is 0".into(),
        (0, 1) => "the only valid indices are 0 and -1".into(),
        (_, 0) => format!("valid indices are 0 to {last}"),
        (_, 1) => format!("valid indices are 0 to {last}, and -1 from the end"),
        _ => format!("valid indices are 0 to {last}, and -{len} to -1 from the end"),
    }
}

/// A panic of compiled code whose message the runtime writes from the
/// values involved. `detail` holds the operation from bit 16, the
/// signedness of the right operand in bit 9 and of the left one in bit 8,
/// and the width of the left one in bits below; `ty` names its type.
/// Operations: `+ - * / %`, negation, `<< >>` from 0; for `P0001`, 0
/// indexes an array, 1 a C array and 2 a deque.
/// The values come in memory, `[a, b]`: as arguments, they would ask the
/// code that checks to keep them in the registers of a call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) -> ! {
    let message = unsafe { values_message(code, detail, values, ty) };
    panic_with(code, &message, location)
}

/// `tondo_rt_panic_values` where compiled code unwinds when it returns
/// (language 15.9).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_values_unwind(
    code: u32,
    detail: u32,
    values: *const [u64; 4],
    ty: *const Str,
    location: *const Str,
) {
    let message = unsafe { values_message(code, detail, values, ty) };
    raise(code, &message, location)
}

/// The message of a panic of compiled code, from its values, whose data it
/// stages.
unsafe fn values_message(code: u32, detail: u32, values: *const [u64; 4], ty: *const Str) -> String {
    // Two values of 128 bits, each extended by its signedness.
    let [a_low, a_high, b_low, b_high] = unsafe { *values };
    let (a, b) = ((a_high as i128) << 64 | a_low as i128, (b_high as i128) << 64 | b_low as i128);
    let (bits, op) = (detail & 0xff, detail >> 16);
    let show = |value: i128, signed: bool| if signed { value.to_string() } else { (value as u128).to_string() };
    let (left, right) = (show(a, detail & 0x100 != 0), show(b, detail & 0x200 != 0));
    let (a, b) = (a as i64, b as i64);
    let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
    let operator =
        ["+", "-", "*", "/", "%", "-", "<<", ">>", "pow", "nextPowerOfTwo"].get(op as usize).copied().unwrap_or("?");
    let range = || {
        let unused = 128 - bits.clamp(1, 128);
        match detail & 0x100 != 0 {
            true => format!("{} to {}", i128::MIN >> unused, i128::MAX >> unused),
            false => format!("0 to {}", u128::MAX >> unused),
        }
    };
    let int = |value: &str| value.parse::<i64>().map_or(panics::Datum::Text(value.to_string()), panics::Datum::Int);
    let data = match code {
        1 => vec![("index", panics::Datum::Int(a)), ("length", panics::Datum::Int(b))],
        3 => vec![("left", int(&left))],
        5 => vec![
            ("operation", panics::Datum::Text(operator.to_string())),
            ("left", int(&left)),
            ("right", int(&right)),
            ("type", panics::Datum::Text(ty.to_string())),
        ],
        6 => vec![("left", panics::Datum::Int(a)), ("right", panics::Datum::Int(b))],
        10 => vec![("count", int(&right)), ("width", panics::Datum::Int(i64::from(bits)))],
        18 => vec![("exponent", int(&right))],
        _ => Vec::new(),
    };
    panics::language_data(data);
    match code {
        1 if op == 1 => {
            format!("index {a} is out of bounds for the C array {ty}: the index must be from 0 to {}", b - 1)
        }
        1 if op == 2 => out_of_bounds(a, b as usize, "deque"),
        1 => out_of_bounds(a, b as usize, "array"),
        3 => format!("{left} {operator} 0"),
        5 if op == 5 => format!("-({left}) does not fit in {ty} ({})", range()),
        5 if op == 8 => format!("{left}.pow({right}) does not fit in {ty} ({})", range()),
        5 if op == 9 => format!("{left}.nextPowerOfTwo() does not fit in {ty} ({})", range()),
        5 if op == 10 => format!("{left} does not fit in {ty}"),
        18 => format!("{left}.pow({right}): the exponent must be 0 or more"),
        5 => format!("{left} {operator} {right} does not fit in {ty} ({})", range()),
        6 => format!("an array of length {a} cannot become the C array {ty}: it needs exactly {b} elements"),
        10 => format!("{left} {operator} {right}: a shift of {ty} must be from 0 to {}", bits - 1),
        _ => String::new(),
    }
}

/// Reports the error a fallible `main` returned (spec 20.10), of the type
/// `ty`: its text, or with `TONDO_PANIC_FORMAT=json` a line of
/// `tondo-panic-0.1/1` (toolchain 7.5).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_report_error(description: *const Str, ty: *const Str) {
    flush_stdout();
    let text = String::from_utf8_lossy(unsafe { str_bytes(description) });
    if panics::json_reports() {
        let ty = String::from_utf8_lossy(unsafe { str_bytes(ty) });
        let _ = writeln!(
            std::io::stderr(),
            "{{\"code\":null,\"data\":null,\"error_type\":{},\"format\":\"tondo-panic-0.1/1\",\"kind\":\"error\",\"message\":{},\"name\":null,\"recovered\":false,\"source\":null,\"stack\":[]}}",
            diagnostics::quoted(&ty),
            diagnostics::quoted(&text),
        );
        return;
    }
    let _ = writeln!(std::io::stderr(), "error: {text}");
}

/// An internal invariant of the runtime or the generated code was broken.
fn runtime_fault(message: &str) -> ! {
    flush_stdout();
    let _ = writeln!(std::io::stderr(), "tondo runtime fault: {message}{}", trace::report());
    std::process::abort()
}

/// Exit status when `TONDO_RT_CHECK_LEAKS` finds live strings at exit.
pub const LEAK_EXIT_CODE: i32 = 99;

pub fn check_leaks() -> Option<usize> {
    std::env::var_os("TONDO_RT_CHECK_LEAKS")?;
    tasks::wait_foreign_calls();
    // The contexts' initial values live as long as the program, and so do
    // the channels of signals.
    contexts::end();
    signals::end();
    // Garbage cycles are not leaks: the collector frees them.
    cycles::collect_all();
    let live = LIVE_OBJECTS.live();
    (live != 0).then_some(live)
}

// ---------------------------------------------------------------------------
// Process entry
// ---------------------------------------------------------------------------

#[cfg(tondo_staticlib)]
#[global_allocator]
static ALLOCATOR: meta::Counting = meta::Counting;

/// `count` values of `T` in memory of their own, zeroed, which the system
/// gives a page at a time as they are written; null if it has none.
pub(crate) fn zeroed<T>(count: usize) -> *mut T {
    let size = count * std::mem::size_of::<T>();
    let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
    let memory = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0) };
    if memory == libc::MAP_FAILED { std::ptr::null_mut() } else { memory.cast() }
}

/// Set in a library, whose host has its own `main`.
static LIBRARY: AtomicBool = AtomicBool::new(false);

pub(crate) fn library() -> bool {
    LIBRARY.load(Ordering::Relaxed)
}

/// Starts a library at the first call from C: the runtime reads its
/// settings and the compiled code hands it its tables. What the library
/// wrote and did not deliver yet goes out when the process ends.
pub(crate) fn start_library(start: unsafe extern "C" fn()) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        LIBRARY.store(true, Ordering::Relaxed);
        #[cfg(tondo_staticlib)]
        meta::start();
        diagnostics::start();
        unsafe { start() };
        unsafe extern "C" {
            fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
        }
        extern "C" fn flush(_: *mut u8) {
            flush_stdout();
        }
        unsafe { __cxa_atexit(flush, std::ptr::null_mut(), std::ptr::null_mut()) };
    });
}

/// Runs a program: the `main` the compiler generates for an executable
/// calls it with `tondo_main`, which runs the program's `main` and returns
/// the process exit status. A library has neither.
#[cfg(tondo_staticlib)]
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_main(tondo_main: unsafe extern "C" fn() -> i32) -> i32 {
    meta::start();
    diagnostics::start();
    signals::start();
    debugger::start_stopped();
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
        let mut builder = Builder::new(2);
        let b = &mut builder as *mut Builder;
        let s = str_new("x=".as_bytes());
        unsafe {
            tondo_rt_sb_str(b, s);
            tondo_rt_sb_i64(b, -42);
            tondo_rt_sb_i64(b, i64::MIN);
            tondo_rt_sb_u64(b, 1_000_000);
            tondo_rt_sb_bool(b, 1);
            let out = tondo_rt_sb_finish(b);
            assert_eq!(str_bytes(out), b"x=-42-92233720368547758081000000true");
            tondo_rt_str_release(out);
            tondo_rt_str_release(s);
        }
    }
}
