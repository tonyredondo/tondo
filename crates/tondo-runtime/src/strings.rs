//! Strings (docs/refactor.md, phase 5): a header and UTF-8 bytes after it, with
//! room a string appended to in place grows into.

use super::*;

/// Header of every string. The UTF-8 bytes follow the header, with room
/// for `cap` of them, which a string appended to in place grows into.
///
/// String literals are emitted by the compiler as static objects with the
/// same layout and `rc == IMMORTAL`, so retain and release ignore them.
#[repr(C)]
pub struct Str {
    pub(crate) rc: AtomicUsize,
    pub(crate) len: usize,
    pub(crate) cap: usize,
}

pub(crate) const IMMORTAL: usize = usize::MAX;
pub(crate) const HEADER: usize = std::mem::size_of::<Str>();

/// Heap objects (strings and boxes) currently allocated. Checked at exit when
/// `TONDO_RT_CHECK_LEAKS` is set, which the compiler's tests use to verify
/// reference counting.
pub(crate) static LIVE_OBJECTS: diagnostics::Objects = diagnostics::Objects;

pub(crate) fn str_layout(len: usize) -> Layout {
    Layout::from_size_align(HEADER + len, std::mem::align_of::<Str>()).expect("string too large")
}

/// A string the runtime keeps for good, as the compiler keeps a literal:
/// retaining and releasing it do nothing, and nothing changes it in place.
#[repr(C)]
pub(crate) struct Kept<const N: usize> {
    header: Str,
    bytes: [u8; N],
}

impl<const N: usize> Kept<N> {
    pub(crate) const fn new(bytes: &[u8; N]) -> Kept<N> {
        Kept { header: Str { rc: AtomicUsize::new(IMMORTAL), len: N, cap: N }, bytes: *bytes }
    }

    pub(crate) fn get(&'static self) -> *mut Str {
        &self.header as *const Str as *mut Str
    }
}

/// Every empty string the runtime makes.
static EMPTY: Kept<0> = Kept::new(b"");

/// Allocates a string with refcount 1 holding a copy of `bytes`; the empty
/// string is the one kept for good.
pub fn str_new(bytes: &[u8]) -> *mut Str {
    if bytes.is_empty() {
        return EMPTY.get();
    }
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
    if s.is_null() {
        return;
    }
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
            // Only a long string's positions are kept, and its room never
            // shrinks, even if it does.
            if unsafe { (*s).cap } >= POSITIONED_FROM {
                forget_positions(s);
            }
            unsafe { dealloc(s as *mut u8, str_layout((*s).cap)) }
        }
        0 => runtime_fault("string released after it was freed"),
        _ => {}
    }
}

/// Where the scalars of the long strings a program indexes are (19.2), a
/// string in each slot: its address and length, whether it is ASCII, its
/// length in scalars, and the last scalar found and its byte. An index in
/// an ASCII string is its byte; in another, the walk starts at the last
/// one found, so a loop over the indices takes a step each. A slot holds a
/// string only while it lives: freeing it, or taking its block to build
/// another, empties its slot first.
pub(crate) struct Positions {
    text: usize,
    len: usize,
    ascii: bool,
    count: usize,
    index: usize,
    offset: usize,
}

pub(crate) const POSITION_SLOTS: usize = 16;
/// The address each slot holds, which freeing a string checks with a load.
pub(crate) static POSITIONED: [AtomicUsize; POSITION_SLOTS] = [const { AtomicUsize::new(0) }; POSITION_SLOTS];
pub(crate) static POSITIONS: [Mutex<Option<Positions>>; POSITION_SLOTS] = [const { Mutex::new(None) }; POSITION_SLOTS];
/// Strings shorter than this are walked from their start.
pub(crate) const POSITIONED_FROM: usize = 64;

pub(crate) fn position_slot(s: *const Str) -> usize {
    (s as usize >> 4) % POSITION_SLOTS
}

/// Empties the slot of a string whose block is freed or taken.
pub(crate) fn forget_positions(s: *const Str) {
    let slot = position_slot(s);
    if POSITIONED[slot].load(Ordering::Relaxed) == s as usize {
        let mut positions = POSITIONS[slot].lock().unwrap_or_else(|e| e.into_inner());
        if positions.as_ref().is_some_and(|p| p.text == s as usize) {
            *positions = None;
            POSITIONED[slot].store(0, Ordering::Relaxed);
        }
    }
}

/// The number of scalars of some bytes: every scalar has exactly one byte
/// that is not a continuation byte.
pub(crate) fn scalars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| (b as i8) >= -0x40).count()
}

/// The byte where the scalar at `index` starts, or `None` past the end.
pub(crate) unsafe fn scalar_offset(s: *const Str, index: usize) -> Option<usize> {
    let text = unsafe { str_text(s) };
    let bytes = text.as_bytes();
    if bytes.len() < POSITIONED_FROM {
        return text.char_indices().nth(index).map(|(offset, _)| offset);
    }
    let slot = position_slot(s);
    let mut held = POSITIONS[slot].lock().unwrap_or_else(|e| e.into_inner());
    if !held.as_ref().is_some_and(|p| p.text == s as usize && p.len == bytes.len()) {
        let ascii = bytes.is_ascii();
        let count = if ascii { bytes.len() } else { scalars(bytes) };
        *held = Some(Positions { text: s as usize, len: bytes.len(), ascii, count, index: 0, offset: 0 });
        POSITIONED[slot].store(s as usize, Ordering::Relaxed);
    }
    let positions = held.as_mut().expect("a string's positions");
    if index >= positions.count {
        return None;
    }
    if positions.ascii {
        return Some(index);
    }
    // From the nearest of the start, the end and the last one found.
    let (mut at, mut offset) = (positions.index, positions.offset);
    if index < at.abs_diff(index) {
        (at, offset) = (0, 0);
    }
    if positions.count - index < at.abs_diff(index) {
        (at, offset) = (positions.count, bytes.len());
    }
    while at < index {
        offset += 1;
        while offset < bytes.len() && (bytes[offset] as i8) < -0x40 {
            offset += 1;
        }
        at += 1;
    }
    while at > index {
        offset -= 1;
        while (bytes[offset] as i8) < -0x40 {
            offset -= 1;
        }
        at -= 1;
    }
    (positions.index, positions.offset) = (index, offset);
    Some(offset)
}

/// The number of scalars, from the string's slot when it has one.
pub(crate) unsafe fn scalar_count(s: *const Str) -> usize {
    let bytes = unsafe { str_bytes(s) };
    if bytes.len() >= POSITIONED_FROM && POSITIONED[position_slot(s)].load(Ordering::Relaxed) == s as usize {
        let held = POSITIONS[position_slot(s)].lock().unwrap_or_else(|e| e.into_inner());
        if let Some(positions) = held.as_ref().filter(|p| p.text == s as usize && p.len == bytes.len()) {
            return positions.count;
        }
    }
    scalars(bytes)
}

/// Drops a reference to an object and returns the count it had. The last
/// one goes without the atomic decrement, which costs more than most
/// objects' allocation: no other reference is left to count, nor to take
/// another, as only a `Ref`'s weak ones could.
#[inline(always)]
pub(crate) fn dropped(rc: &AtomicUsize) -> usize {
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
    unsafe { scalar_count(s) as i64 }
}

/// The scalar at `index`, counting from the end when negative, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_get(s: *const Str, index: i64) -> i64 {
    let text = unsafe { str_text(s) };
    let index = if index >= 0 {
        index as usize
    } else {
        match (unsafe { scalar_count(s) } as i64).checked_add(index) {
            Some(from_start) if from_start >= 0 => from_start as usize,
            _ => return -1,
        }
    };
    match unsafe { scalar_offset(s, index) } {
        Some(offset) => text[offset..].chars().next().map_or(-1, |c| c as i64),
        None => -1,
    }
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
        let (first, count) = slice_range(scalar_count(s), start, has_start, end, has_end, step, location);
        // A run of scalars is the bytes between where they start.
        if step == 1 {
            let from = scalar_offset(s, first as usize).unwrap_or(text.len());
            let to = scalar_offset(s, first as usize + count).unwrap_or(text.len());
            return str_new(&text.as_bytes()[from..to]);
        }
        let chars: Vec<char> = text.chars().collect();
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
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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

/// `toLowerAscii` (1) or `toUpperAscii` (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_transform(s: *const Str, transform: u32) -> *mut Str {
    let text = unsafe { str_text(s) };
    match transform {
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
