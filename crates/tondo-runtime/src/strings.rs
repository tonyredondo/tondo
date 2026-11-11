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

/// The `rc` of a piece of a block, beside how far before it the block
/// starts: the block counts the references to all its pieces together.
pub(crate) const PIECE: usize = 1 << 63;

/// The header of a block of pieces of one text, which `split` and the like
/// make in one allocation instead of one for each: the references to all
/// its pieces, its size, and how much of it the pieces fill, one after
/// another, each a string's header and its bytes. A piece is never the
/// only owner of its block, so a change copies it first, as one of any
/// shared string; and while one of them is held, the block is.
#[repr(C)]
struct Block {
    count: AtomicUsize,
    size: usize,
    used: usize,
    /// Whether a piece is long enough for its positions to be kept.
    long: bool,
}

const BLOCK: usize = std::mem::size_of::<Block>();

/// The room a piece of `len` bytes takes in a block.
fn piece_room(len: usize) -> usize {
    (HEADER + len).next_multiple_of(std::mem::align_of::<Str>())
}

/// The pieces of one text, written into a block as they come; what the
/// block's header says of them goes there once they are all in.
pub(crate) struct Pieces {
    block: *mut Block,
    used: usize,
    count: usize,
    long: bool,
}

impl Pieces {
    /// Room for `count` pieces of `bytes` bytes in all.
    pub(crate) fn new(count: usize, bytes: usize) -> Pieces {
        let size = BLOCK + count * piece_room(0) + bytes.next_multiple_of(8) + count * 8;
        let layout = Layout::from_size_align(size, std::mem::align_of::<Block>()).expect("pieces too large");
        let block = unsafe { alloc(layout) } as *mut Block;
        if block.is_null() {
            handle_alloc_error(layout);
        }
        unsafe { block.write(Block { count: AtomicUsize::new(0), size, used: BLOCK, long: false }) };
        Pieces { block, used: BLOCK, count: 0, long: false }
    }

    /// The next piece, with one reference; the empty string kept for good
    /// when it is empty.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> *mut Str {
        if bytes.is_empty() {
            return EMPTY.get();
        }
        unsafe {
            let at = self.used;
            debug_assert!(at + piece_room(bytes.len()) <= (*self.block).size, "a piece past its block's room");
            let piece = (self.block as *mut u8).add(at) as *mut Str;
            piece.write(Str { rc: AtomicUsize::new(PIECE | at), len: bytes.len(), cap: bytes.len() });
            copy_short(bytes, (piece as *mut u8).add(HEADER));
            self.used = at + piece_room(bytes.len());
            self.long |= bytes.len() >= POSITIONED_FROM;
            self.count += 1;
            piece
        }
    }

    /// Done: the block holds its pieces, or goes when none needs it.
    pub(crate) fn finish(self) {
        unsafe {
            if self.count == 0 {
                dealloc(self.block as *mut u8, block_layout(self.block));
                return;
            }
            let block = &mut *self.block;
            (*block.count.get_mut(), block.used, block.long) = (self.count, self.used, self.long);
            LIVE_OBJECTS.made();
        }
    }
}

/// A block whose pieces go out as they are made, before it is done, as a
/// reader makes the texts of a document: it holds a reference for each
/// piece it may yet make, which it gives back when it closes, so that a
/// piece let go meanwhile does not free it. It closes when it is dropped.
pub(crate) struct OpenPieces {
    pieces: Pieces,
}

/// The references an open block holds for the pieces it may yet make.
const UNMADE: usize = 1 << 40;

// The block's owner alone writes it; the pieces it gave out change only
// its count, atomically.
unsafe impl Send for OpenPieces {}

impl OpenPieces {
    /// A block of `size` bytes, its header's included.
    pub(crate) fn new(size: usize) -> OpenPieces {
        let size = size.next_multiple_of(8).max(BLOCK);
        let layout = Layout::from_size_align(size, std::mem::align_of::<Block>()).expect("pieces too large");
        let block = unsafe { alloc(layout) } as *mut Block;
        if block.is_null() {
            handle_alloc_error(layout);
        }
        unsafe { block.write(Block { count: AtomicUsize::new(UNMADE), size, used: BLOCK, long: false }) };
        LIVE_OBJECTS.made();
        OpenPieces { pieces: Pieces { block, used: BLOCK, count: 0, long: false } }
    }

    /// The room a block needs for a piece of `len` bytes.
    pub(crate) fn room(len: usize) -> usize {
        BLOCK + piece_room(len)
    }

    /// Whether a piece of `len` bytes fits in what is left of the block.
    pub(crate) fn fits(&self, len: usize) -> bool {
        self.pieces.used + piece_room(len) <= unsafe { (*self.pieces.block).size }
    }

    /// The next piece, with one reference, which must fit.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> *mut Str {
        assert!(self.pieces.count < UNMADE && self.fits(bytes.len()), "a piece past its block's room");
        self.pieces.push(bytes)
    }
}

impl Drop for OpenPieces {
    fn drop(&mut self) {
        let Pieces { block, used, count, long } = self.pieces;
        unsafe {
            // Before the references go back, as the piece released last
            // reads them to free the block.
            ((*block).used, (*block).long) = (used, long);
            release_pieces(block, UNMADE - count);
        }
    }
}

unsafe fn block_layout(block: *const Block) -> Layout {
    unsafe { Layout::from_size_align_unchecked((*block).size, std::mem::align_of::<Block>()) }
}

/// The block of a piece whose `rc` is `rc`.
unsafe fn block_of(s: *const Str, rc: usize) -> *mut Block {
    unsafe { (s as *mut u8).sub(rc & !PIECE) as *mut Block }
}

/// Drops `count` references to the pieces of a block, which goes with the
/// last one.
unsafe fn release_pieces(block: *mut Block, count: usize) {
    unsafe {
        let counted = &(*block).count;
        // The last references go without the atomic operation, as an
        // object's last one does (`dropped`).
        let before = match counted.load(Ordering::Relaxed) {
            held if held == count => held,
            _ => counted.fetch_sub(count, Ordering::Release),
        };
        if before < count {
            runtime_fault("string released after it was freed");
        }
        if before > count {
            return;
        }
        fence(Ordering::Acquire);
        LIVE_OBJECTS.gone();
        // The long pieces' positions go first, as a string's do.
        let mut at = if (*block).long { BLOCK } else { (*block).used };
        while at < (*block).used {
            let piece = (block as *const u8).add(at) as *const Str;
            if (*piece).len >= POSITIONED_FROM {
                forget_positions(piece);
            }
            at += piece_room((*piece).len);
        }
        dealloc(block as *mut u8, block_layout(block));
    }
}

unsafe fn release_piece(s: *const Str, rc: usize) {
    unsafe { release_pieces(block_of(s, rc), 1) }
}

/// `release` of the string at an address, the glue of `String` elements,
/// which an array's release knows to release them a run at a time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_release_at(at: *mut u8) {
    unsafe { tondo_rt_str_release(*(at as *mut *mut Str)) }
}

/// Strings released one after another, as an array's elements are: the
/// pieces of one block in a row drop their references with one operation.
#[derive(Default)]
pub(crate) struct Releasing {
    block: Option<(*mut Block, usize)>,
}

impl Releasing {
    pub(crate) unsafe fn release(&mut self, s: *mut Str) {
        unsafe {
            if s.is_null() {
                return;
            }
            let rc = (*s).rc.load(Ordering::Relaxed);
            if rc != IMMORTAL && rc & PIECE != 0 {
                let block = block_of(s, rc);
                match &mut self.block {
                    Some((held, run)) if *held == block => *run += 1,
                    _ => {
                        self.finish();
                        self.block = Some((block, 1));
                    }
                }
                return;
            }
            tondo_rt_str_release(s);
        }
    }

    pub(crate) unsafe fn finish(&mut self) {
        if let Some((block, run)) = self.block.take() {
            unsafe { release_pieces(block, run) };
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_retain(s: *const Str) {
    if s.is_null() {
        return;
    }
    let rc = unsafe { &(*s).rc };
    let counted = match rc.load(Ordering::Relaxed) {
        IMMORTAL => return,
        piece if piece & PIECE != 0 => unsafe { &(*block_of(s, piece)).count },
        _ => rc,
    };
    counted.fetch_add(1, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_release(s: *mut Str) {
    // Releasing a binding whose value moved away does nothing: it is zero.
    if s.is_null() {
        return;
    }
    let rc = unsafe { &(*s).rc };
    match rc.load(Ordering::Relaxed) {
        IMMORTAL => return,
        piece if piece & PIECE != 0 => return unsafe { release_piece(s, piece) },
        _ => {}
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
