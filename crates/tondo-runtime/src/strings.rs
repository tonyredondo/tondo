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
/// string is the one kept for good, and a short one is its word, made where
/// it is asked for.
#[inline]
pub fn str_new(bytes: &[u8]) -> *mut Str {
    match bytes.len() {
        0 => EMPTY.get(),
        1..=SHORT_MAX => short_text(bytes),
        _ => str_block(bytes),
    }
}

/// A string of more than `SHORT_MAX` bytes in a block of its own.
#[inline(never)]
fn str_block(bytes: &[u8]) -> *mut Str {
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

/// A copy of `bytes` in `region`, the last reference its region lets go
/// of, or on the heap when there is none: the text an operation gives where
/// the analysis put it in a region. For a place whose text an append may
/// grow (`Region.grows`), with all the room the heap's block would hold
/// (`meta::good`), as an append on the heap takes all of its block. A long
/// one forgets the positions a text its region let go of left at the same
/// address.
#[inline]
pub(crate) fn str_in(region: *mut super::regions::Region, bytes: &[u8]) -> *mut Str {
    if region.is_null() || bytes.len() <= SHORT_MAX {
        return str_new(bytes);
    }
    str_in_region(region, bytes)
}

/// `str_in` of more than `SHORT_MAX` bytes in a region: one too short to
/// keep positions, in a region that does not grow and has room, made with
/// nothing to save; any other apart.
#[inline(never)]
fn str_in_region(region: *mut super::regions::Region, bytes: &[u8]) -> *mut Str {
    let n = bytes.len();
    if n < POSITIONED_FROM
        && !super::regions::grows(region)
        && let Some(at) = unsafe { super::regions::bump(region, HEADER + n) }
    {
        let text = at as *mut Str;
        unsafe {
            text.write(Str { rc: AtomicUsize::new(super::regions::LAST_IN_REGION), len: n, cap: n });
            super::builder::copy_upto_64(bytes, at.add(HEADER));
        }
        return text;
    }
    str_in_region_slowly(region, bytes)
}

#[cold]
#[inline(never)]
fn str_in_region_slowly(region: *mut super::regions::Region, bytes: &[u8]) -> *mut Str {
    unsafe {
        let size = match super::regions::grows(region) {
            true => super::meta::good(HEADER + bytes.len()),
            false => HEADER + bytes.len(),
        };
        let text = super::regions::tondo_rt_region_alloc(region, size) as *mut Str;
        text.write(Str { rc: AtomicUsize::new(super::regions::LAST_IN_REGION), len: bytes.len(), cap: size - HEADER });
        copy_short(bytes, (text as *mut u8).add(HEADER));
        if bytes.len() >= POSITIONED_FROM {
            forget_positions(text);
        }
        text
    }
}

/// Texts in one room of a region, each a text of its own, which the region
/// lets go of with the rest: the pieces `split` makes, as `Pieces` makes
/// them in one block on the heap.
pub(crate) struct RegionPieces {
    at: usize,
    end: usize,
    region: *mut super::regions::Region,
}

impl RegionPieces {
    /// Room in `region` for `count` pieces of `bytes` bytes in all, after
    /// an array of `count` texts for them, in one allocation, as
    /// `Pieces::with_array` makes them in one block on the heap.
    pub(crate) fn with_array(
        region: *mut super::regions::Region,
        count: usize,
        bytes: usize,
    ) -> (RegionPieces, *mut ArrayHeader) {
        let array = super::arrays::ARRAY_HEADER + count * 8;
        let room = count * (HEADER + 8) + bytes;
        unsafe {
            let at = super::regions::tondo_rt_region_alloc(region, array + room);
            let pieces = RegionPieces { at: at as usize + array, end: at as usize + array + room, region };
            (pieces, super::arrays::array_at_in(at, count))
        }
    }

    /// The next piece, in the room, or in its word when it is short: in the
    /// loop of each caller, as a split's pieces are many and short.
    #[inline(always)]
    pub(crate) fn push(&mut self, bytes: &[u8]) -> *mut Str {
        let size = (HEADER + bytes.len()).next_multiple_of(8);
        if bytes.len() <= SHORT_MAX {
            return str_new(bytes);
        }
        // More than it was given room for, which its caller never asks.
        if self.at + size > self.end {
            return str_in(self.region, bytes);
        }
        let text = self.at as *mut Str;
        unsafe {
            text.write(Str {
                rc: AtomicUsize::new(super::regions::PIECE_IN_REGION),
                len: bytes.len(),
                cap: bytes.len(),
            });
            copy_short(bytes, (text as *mut u8).add(HEADER));
        }
        if bytes.len() >= POSITIONED_FROM {
            forget_positions(text);
        }
        self.at += size;
        text
    }
}

/// The bytes of a string. The caller keeps the string alive.
pub unsafe fn str_bytes(s: &impl Word) -> &[u8] {
    let word = s.word();
    if short(word) {
        // In the word itself: its first bytes, as the processor keeps the
        // lowest first.
        return unsafe { std::slice::from_raw_parts(s as *const _ as *const u8, short_len(word)) };
    }
    unsafe { std::slice::from_raw_parts((word as *const u8).add(HEADER), (*word).len) }
}

/// The bytes of a string that is not short, where its header is. The
/// caller keeps the string alive.
#[inline(always)]
pub(crate) unsafe fn long_bytes<'a>(s: *const Str) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts((s as *const u8).add(HEADER), (*s).len) }
}

/// A short text: up to `SHORT_MAX` bytes held in its word instead of a
/// block, the lowest first, and in its top byte the mark `SHORT`, its
/// sign, which no address a program is given has, and its length. It
/// counts nothing, is never freed and never changes.
pub(crate) const SHORT: usize = 1 << 63;
pub(crate) const SHORT_MAX: usize = 7;

/// Whether a text is a short one, held in its word.
#[inline(always)]
pub(crate) fn short(s: *const Str) -> bool {
    (s as isize) < 0
}

/// The length of a short text.
#[inline(always)]
pub(crate) fn short_len(s: *const Str) -> usize {
    (s as usize >> 56) & 0x7f
}

/// The short text of `bytes`, at most `SHORT_MAX` of them.
#[inline(always)]
pub(crate) fn short_text(bytes: &[u8]) -> *mut Str {
    (short_bytes(bytes) | SHORT | bytes.len() << 56) as *mut Str
}

/// Up to `SHORT_MAX` bytes as the low bytes of a word, the first lowest:
/// in two loads that may overlap, as `copy_short` copies, rather than a
/// call of `memcpy`.
#[inline(always)]
pub(crate) fn short_bytes(bytes: &[u8]) -> usize {
    debug_assert!(bytes.len() <= SHORT_MAX);
    let (from, n) = (bytes.as_ptr(), bytes.len());
    unsafe {
        match n {
            4..=7 => {
                let low = from.cast::<u32>().read_unaligned() as usize;
                let high = from.add(n - 4).cast::<u32>().read_unaligned() as usize;
                low | high << (8 * (n - 4))
            }
            2..=3 => {
                let low = from.cast::<u16>().read_unaligned() as usize;
                let high = from.add(n - 2).cast::<u16>().read_unaligned() as usize;
                low | high << (8 * (n - 2))
            }
            1 => *from as usize,
            _ => 0,
        }
    }
}

/// A short text's bytes as `change` leaves them, in a short text.
pub(crate) fn short_changed(s: *const Str, change: impl FnOnce(&mut [u8])) -> *mut Str {
    let mut bytes = (s as usize).to_le_bytes();
    let len = short_len(s);
    change(&mut bytes[..len]);
    short_text(&bytes[..len])
}

// A short text's bytes are read where its word is, the lowest first.
const _: () = assert!(cfg!(target_endian = "little"));

/// A text's length in bytes, short or not.
#[inline(always)]
pub(crate) unsafe fn str_len(s: *const Str) -> usize {
    match short(s) {
        true => short_len(s),
        false => unsafe { (*s).len },
    }
}

/// Where a text's word is: what its bytes are read through, so that they
/// live as long as it, as a short text's live in the word itself.
pub trait Word {
    fn word(&self) -> *const Str;
}

impl Word for *const Str {
    #[inline(always)]
    fn word(&self) -> *const Str {
        *self
    }
}

impl Word for *mut Str {
    #[inline(always)]
    fn word(&self) -> *const Str {
        *self
    }
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
    /// Where the pieces start: after the header, or after the array of
    /// them the block holds too (`Pieces::with_array`).
    first: u32,
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
    /// Whether the block holds the array of its pieces.
    array: bool,
}

impl Pieces {
    /// Room for `count` pieces of `bytes` bytes in all, after `before`
    /// bytes of the block's header.
    fn after(before: usize, count: usize, bytes: usize) -> Pieces {
        let first = BLOCK + before;
        let size = first + count * piece_room(0) + bytes.next_multiple_of(8) + count * 8;
        let layout = Layout::from_size_align(size, std::mem::align_of::<Block>()).expect("pieces too large");
        let block = unsafe { alloc(layout) } as *mut Block;
        if block.is_null() {
            handle_alloc_error(layout);
        }
        let first = u32::try_from(first).expect("pieces too large");
        unsafe { block.write(Block { count: AtomicUsize::new(0), size, used: first as usize, long: false, first }) };
        Pieces { block, used: first as usize, count: 0, long: false, array: false }
    }

    /// Room for `count` pieces of `bytes` bytes in all, and before them an
    /// `Array[String]` of `count` elements, which the caller writes: one
    /// allocation for the array and its pieces, as `split` makes. The array
    /// is never the only owner of its storage, so a change copies it first,
    /// and while it lives the block does.
    pub(crate) fn with_array(count: usize, bytes: usize) -> (Pieces, *mut ArrayHeader) {
        let mut pieces = Pieces::after(ARRAY_HEADER + count * 8, count, bytes);
        pieces.array = true;
        let array = unsafe { in_block((pieces.block as *mut u8).add(BLOCK), count) };
        (pieces, array)
    }

    /// The next piece, with one reference; the empty string kept for good
    /// when it is empty, and a short one in its word. In the loop of each
    /// caller, as `RegionPieces`'s.
    #[inline(always)]
    pub(crate) fn push(&mut self, bytes: &[u8]) -> *mut Str {
        if bytes.len() <= SHORT_MAX {
            return match bytes.is_empty() {
                true => EMPTY.get(),
                false => short_text(bytes),
            };
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

    /// Done: the block holds its pieces and its array, or goes when none
    /// needs it.
    pub(crate) fn finish(self) {
        unsafe {
            let held = self.count + usize::from(self.array);
            if held == 0 {
                dealloc(self.block as *mut u8, block_layout(self.block));
                return;
            }
            let block = &mut *self.block;
            (*block.count.get_mut(), block.used, block.long) = (held, self.used, self.long);
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
        let block_value =
            Block { count: AtomicUsize::new(UNMADE), size, used: BLOCK, long: false, first: BLOCK as u32 };
        unsafe { block.write(block_value) };
        LIVE_OBJECTS.made();
        OpenPieces { pieces: Pieces { block, used: BLOCK, count: 0, long: false, array: false } }
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
        let Pieces { block, used, count, long, .. } = self.pieces;
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
        let mut at = if (*block).long { (*block).first as usize } else { (*block).used };
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

/// Releases an array a block of strings holds, which no value holds any
/// more (`Pieces::with_array`), and its elements: the references of the
/// block's own pieces and the one it holds for the array go back in one
/// operation.
pub(crate) unsafe fn release_in_block(a: *mut ArrayHeader, size: usize, release: Option<Glue>) {
    unsafe {
        let block = (a as *mut u8).sub(BLOCK) as *mut Block;
        match release {
            Some(release) if std::ptr::fn_addr_eq(release, tondo_rt_str_release_at as Glue) => {
                let mut releasing = Releasing::default();
                for index in 0..(*a).len {
                    releasing.release(*(element(a, size, index) as *mut *mut Str));
                }
                // Last, as the array is in the block.
                match &mut releasing.block {
                    Some((held, run)) if *held == block => *run += 1,
                    _ => {
                        releasing.finish();
                        releasing.block = Some((block, 1));
                    }
                }
                releasing.finish();
            }
            _ => {
                release_elements(a, size, release);
                release_pieces(block, 1);
            }
        }
    }
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
            if (s as isize) <= 0 {
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
    // None and a short text, marked by its sign, count nothing: one
    // comparison.
    if (s as isize) <= 0 {
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
    // Nor does releasing a short text, marked by its sign.
    if (s as isize) <= 0 {
        return;
    }
    let rc = unsafe { &(*s).rc };
    match rc.load(Ordering::Relaxed) {
        IMMORTAL => return,
        piece if piece & PIECE != 0 => return unsafe { release_piece(s, piece) },
        _ => {}
    }
    match dropped(rc) {
        1 => unsafe { tondo_rt_str_free(s) },
        0 => runtime_fault("string released after it was freed"),
        _ => {}
    }
}

/// Frees a string whose last reference went: one `tondo_rt_str_release`
/// let go of, or one whose count the generated code took from one to
/// nothing, as another thread let go of it after the code read it shared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_free(s: *mut Str) {
    fence(Ordering::Acquire);
    LIVE_OBJECTS.gone();
    // Only a long string's positions are kept, and its room never shrinks,
    // even if it does.
    if unsafe { (*s).cap } >= POSITIONED_FROM {
        forget_positions(s);
    }
    unsafe { dealloc(s as *mut u8, str_layout((*s).cap)) }
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
    pub(crate) len: usize,
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

/// The slots, held across a fork so that the copy has them free
/// (`tasks::fork_copy`): a copy that found one taken by a thread it does
/// not have would wait for it forever as it indexes or frees a long text.
/// None when another thread keeps one longer than a fork waits.
pub(crate) fn positions_held() -> Option<[std::sync::MutexGuard<'static, Option<Positions>>; POSITION_SLOTS]> {
    // A `Mutex` that makes its lock the first time it is taken, as on
    // macOS, allocates: each is taken alone first, as the objects' shards.
    for slot in &POSITIONS {
        drop(slot.try_lock());
    }
    let mut held: [Option<std::sync::MutexGuard<'static, Option<Positions>>>; POSITION_SLOTS] =
        [const { None }; POSITION_SLOTS];
    for (guard, slot) in held.iter_mut().zip(&POSITIONS) {
        *guard = Some(super::tasks::soon(slot)?);
    }
    Some(held.map(|guard| guard.expect("every slot is held")))
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

/// Empties the slots of the strings in the memory from `from` to `to`, a
/// region's chunk that goes back to the allocator, whose next strings must
/// not find the positions of those its region let go of.
pub(crate) fn forget_positions_within(from: usize, to: usize) {
    for slot in 0..POSITION_SLOTS {
        if (from..to).contains(&POSITIONED[slot].load(Ordering::Relaxed)) {
            let mut positions = POSITIONS[slot].lock().unwrap_or_else(|e| e.into_inner());
            if positions.as_ref().is_some_and(|p| (from..to).contains(&p.text)) {
                *positions = None;
                POSITIONED[slot].store(0, Ordering::Relaxed);
            }
        }
    }
}

/// The number of scalars of some bytes: every scalar has exactly one byte
/// that is not a continuation byte.
pub(crate) fn scalars(bytes: &[u8]) -> usize {
    // ASCII, as most texts are, whose scalars are its bytes, tested eight
    // bytes at a time.
    if bytes.is_ascii() {
        return bytes.len();
    }
    bytes.iter().filter(|&&b| (b as i8) >= -0x40).count()
}

/// The high bits of the seven bytes a short text holds in its word: none
/// set is ASCII, whose scalars are its bytes.
const SHORT_HIGH: usize = 0x0080_8080_8080_8080;

/// Whether a text is short and ASCII, by one test of its word.
#[inline(always)]
fn short_ascii(s: *const Str) -> bool {
    short(s) && s as usize & SHORT_HIGH == 0
}

/// The byte where the scalar at `index` starts, or `None` past the end.
pub(crate) unsafe fn scalar_offset(s: *const Str, index: usize) -> Option<usize> {
    if short_ascii(s) {
        return (index < short_len(s)).then_some(index);
    }
    let bytes = unsafe { str_bytes(&s) };
    if bytes.len() < POSITIONED_FROM {
        if bytes.is_ascii() {
            return (index < bytes.len()).then_some(index);
        }
        // Where each scalar starts, a byte that does not go on one.
        return bytes.iter().enumerate().filter(|&(_, &b)| (b as i8) >= -0x40).nth(index).map(|(at, _)| at);
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
    if short_ascii(s) {
        return short_len(s);
    }
    let bytes = unsafe { str_bytes(&s) };
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
        // The last reference to a value in a region, which its region lets
        // go of: nothing to count.
        super::regions::LAST_IN_REGION => super::regions::LAST_IN_REGION,
        _ => rc.fetch_sub(1, Ordering::Release),
    }
}

/// Whether two strings hold the same bytes. Two short ones do when their
/// words are the same, as the bytes past a short text's length are zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_eq(a: *const Str, b: *const Str) -> u8 {
    if a == b {
        return 1;
    }
    unsafe {
        match (short(a), short(b)) {
            (false, false) => same(long_bytes(a), long_bytes(b)) as u8,
            (true, true) => 0,
            _ => same(str_bytes(&a), str_bytes(&b)) as u8,
        }
    }
}

/// Orders strings by Unicode scalar values; for UTF-8 this is byte order.
/// Two short ones are ordered by their words, the first byte highest, and
/// then by their lengths, as the bytes past a length are zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_cmp(a: *const Str, b: *const Str) -> i32 {
    let bytes = |s: *const Str| ((s as usize) & !(0xff << 56)).swap_bytes();
    unsafe {
        // Neither short, in one test of both signs.
        if !short((a as usize | b as usize) as *const Str) {
            return order(long_bytes(a), long_bytes(b)) as i32;
        }
        match short(a) && short(b) {
            true => (bytes(a), short_len(a)).cmp(&(bytes(b), short_len(b))) as i32,
            false => order(str_bytes(&a), str_bytes(&b)) as i32,
        }
    }
}

/// The most bytes `same` and `order` compare in registers. The C library's
/// compare of fewer than 32 bytes takes a longer way when either address
/// may lie near the end of a page, and so a program's count of
/// instructions depended on where its texts happened to lie (glibc's
/// `__memcmp_avx2_movbe`); compared here, it does not.
const IN_REGISTERS: usize = 32;

/// `N` bytes of `bytes` from `at`.
#[inline(always)]
fn chunk<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes[at..at + N].try_into().expect("a chunk inside the bytes")
}

/// Whether two byte strings are the same: up to `IN_REGISTERS` bytes by
/// the first and the last word of a size that covers them together, which
/// overlap where the length is not twice the size.
#[inline]
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    let n = a.len();
    if n != b.len() {
        return false;
    }
    match n {
        0 => true,
        1..=3 => a[0] == b[0] && a[n / 2] == b[n / 2] && a[n - 1] == b[n - 1],
        4..=7 => chunk::<4>(a, 0) == chunk::<4>(b, 0) && chunk::<4>(a, n - 4) == chunk::<4>(b, n - 4),
        8..=16 => chunk::<8>(a, 0) == chunk::<8>(b, 0) && chunk::<8>(a, n - 8) == chunk::<8>(b, n - 8),
        17..=IN_REGISTERS => chunk::<16>(a, 0) == chunk::<16>(b, 0) && chunk::<16>(a, n - 16) == chunk::<16>(b, n - 16),
        _ => a == b,
    }
}

/// How two byte strings order, byte by byte and then by length: the bytes
/// both have up to `IN_REGISTERS` as `same` reads them, each word read
/// with its first byte highest. Where the first words are equal, the bytes
/// the last one shares with them are too, so the last one orders the rest.
#[inline]
pub(crate) fn order(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let n = a.len().min(b.len());
    let words = match n {
        0 => std::cmp::Ordering::Equal,
        1..=3 => (a[0], a[n / 2], a[n - 1]).cmp(&(b[0], b[n / 2], b[n - 1])),
        4..=7 => {
            let word = |s: &[u8], at| u32::from_be_bytes(chunk(s, at));
            (word(a, 0), word(a, n - 4)).cmp(&(word(b, 0), word(b, n - 4)))
        }
        8..=16 => {
            let word = |s: &[u8], at| u64::from_be_bytes(chunk(s, at));
            (word(a, 0), word(a, n - 8)).cmp(&(word(b, 0), word(b, n - 8)))
        }
        17..=IN_REGISTERS => {
            let word = |s: &[u8], at| u128::from_be_bytes(chunk(s, at));
            (word(a, 0), word(a, n - 16)).cmp(&(word(b, 0), word(b, n - 16)))
        }
        _ => return a.cmp(b),
    };
    words.then(a.len().cmp(&b.len()))
}

/// The text of a string; strings always hold valid UTF-8.
pub(crate) unsafe fn str_text(s: &impl Word) -> &str {
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
    let text = unsafe { str_text(&s) };
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

/// `text.byteAt(offset)` outside the text: the panic `P0001`, which returns
/// when a `recover` stops it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_byte_outside(s: *const Str, offset: i64, location: *const Str) {
    let length = unsafe { str_text(&s) }.len();
    panics::language_data(vec![("index", panics::Datum::Int(offset)), ("length", panics::Datum::Int(length as i64))]);
    let message = match length {
        0 => format!("offset {offset} is out of bounds: the text is empty"),
        _ => format!(
            "offset {offset} is out of bounds for a text of {length} bytes: valid offsets are 0 to {}",
            length - 1
        ),
    };
    raise(1, &message, location);
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

/// `text[start:end:step]` by scalar positions (19.3), in `region` when it
/// is not null.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_slice(
    s: *const Str,
    start: i64,
    has_start: u8,
    end: i64,
    has_end: u8,
    step: i64,
    location: *const Str,
    region: *mut super::regions::Region,
) -> *mut Str {
    unsafe {
        let text = str_text(&s);
        let (first, count) = slice_range(scalar_count(s), start, has_start, end, has_end, step, location);
        // A run of scalars is the bytes between where they start.
        if step == 1 {
            let from = scalar_offset(s, first as usize).unwrap_or(text.len());
            let to = scalar_offset(s, first as usize + count).unwrap_or(text.len());
            return str_in(region, &text.as_bytes()[from..to]);
        }
        let chars: Vec<char> = text.chars().collect();
        let selected: String = (0..count as i64).map(|k| chars[(first + k * step) as usize]).collect();
        str_in(region, selected.as_bytes())
    }
}

/// `contains` (0), `startsWith` (1) or `endsWith` (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_test(s: *const Str, other: *const Str, test: u32) -> u8 {
    let (text, other) = unsafe { (str_bytes(&s), str_bytes(&other)) };
    (match test {
        0 => find(text, other).is_some(),
        1 => text.len() >= other.len() && same(&text[..other.len()], other),
        _ => text.len() >= other.len() && same(&text[text.len() - other.len()..], other),
    }) as u8
}

/// The scalar position of the first occurrence of `needle`, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_find(s: *const Str, needle: *const Str) -> i64 {
    let (text, needle) = unsafe { (str_text(&s), str_bytes(&needle)) };
    find(text.as_bytes(), needle).map_or(-1, |offset| text[..offset].chars().count() as i64)
}

/// Where `needle` first is in `haystack`: at each place its first byte is,
/// as Go does, and past a few misses with a searcher that is never slow.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let Some(&first) = needle.first() else { return Some(0) };
    let mut at = 0;
    for _ in 0..16 {
        let start = at + memchr::memchr(first, &haystack[at..])?;
        let rest = &haystack[start..];
        if rest.len() >= needle.len() && same(&rest[..needle.len()], needle) {
            return Some(start);
        }
        at = start + 1;
    }
    memchr::memmem::find(&haystack[at..], needle).map(|offset| at + offset)
}

/// `replace`, in `region` when it is not null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_replace(
    s: *const Str,
    old: *const Str,
    new: *const Str,
    region: *mut super::regions::Region,
) -> *mut Str {
    let (s, old, new) = unsafe { (str_text(&s), str_text(&old), str_text(&new)) };
    REPLACING.with_borrow_mut(|out| {
        out.clear();
        let mut last = 0;
        for (at, _) in s.match_indices(old) {
            out.push_str(&s[last..at]);
            out.push_str(new);
            last = at + old.len();
        }
        out.push_str(&s[last..]);
        let made = str_in(region, out.as_bytes());
        if out.capacity() > REPLACING_KEPT {
            *out = String::new();
        }
        made
    })
}

thread_local! {
    /// The text `replace` writes before it is copied where it goes, one for
    /// each thread, as each replacement ends before the next: it allocates
    /// only the text it gives, where a `String` that grows from nothing
    /// took eight steps for a kibibyte.
    static REPLACING: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// The most room `REPLACING` keeps once a replacement is done.
const REPLACING_KEPT: usize = 64 * 1024;

/// `toLowerAscii` (1) or `toUpperAscii` (2), in `region` when it is not
/// null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_transform(
    s: *const Str,
    transform: u32,
    region: *mut super::regions::Region,
) -> *mut Str {
    let change = |bytes: &mut [u8]| match transform {
        1 => bytes.make_ascii_lowercase(),
        _ => bytes.make_ascii_uppercase(),
    };
    // The copy changed in place: no text of Rust's between.
    let copy = str_in(region, unsafe { str_bytes(&s) });
    if short(copy) {
        return short_changed(copy, change);
    }
    let len = unsafe { (*copy).len };
    if len > 0 {
        change(unsafe { std::slice::from_raw_parts_mut((copy as *mut u8).add(HEADER), len) });
    }
    copy
}

/// The checked `slice(start, end)`: null with `error` set to 0
/// (`InvalidIndex`) or 1 (`InvalidBoundary`) when the bounds are invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_slice_checked(
    s: *const Str,
    start: i64,
    end: i64,
    error: *mut i8,
    region: *mut super::regions::Region,
) -> *mut Str {
    unsafe {
        let length = scalar_count(s) as i64;
        let code = if start < 0 || end < 0 || start > length || end > length {
            0
        } else if start > end {
            1
        } else {
            // A run of scalars is the bytes between where they start.
            let bytes = str_bytes(&s);
            let from = scalar_offset(s, start as usize).unwrap_or(bytes.len());
            let to = scalar_offset(s, end as usize).unwrap_or(bytes.len());
            *error = -1;
            return str_in(region, &bytes[from..to]);
        };
        *error = code;
        std::ptr::null_mut()
    }
}

/// The text a function returns where it could be a part of a value the
/// function made: a copy, in `region` when it is not null, and `s` let go
/// of, so that the value it is a part of stays in the function's region.
/// One kept for good is given back as it is, and none, as a return a panic
/// unwinds gives, stays none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_copy_out(s: *mut Str, region: *mut super::regions::Region) -> *mut Str {
    if (s as isize) <= 0 || unsafe { (*s).rc.load(Ordering::Relaxed) } == IMMORTAL {
        return s;
    }
    let copy = str_in(region, unsafe { str_bytes(&s) });
    unsafe { tondo_rt_str_release(s) };
    copy
}

/// A string holding the characters of an `Array[Char]`, in `region` when it
/// is not null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_from_chars(
    chars: *mut ArrayHeader,
    region: *mut super::regions::Region,
) -> *mut Str {
    unsafe {
        let text: String = (0..(*chars).len)
            .map(|k| char::from_u32((element(chars, 4, k) as *const u32).read()).unwrap_or('\u{FFFD}'))
            .collect();
        str_in(region, text.as_bytes())
    }
}

/// The scalar that starts at byte `offset`; iteration only visits valid
/// offsets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_decode(s: *const Str, offset: i64) -> u32 {
    unsafe { str_text(&s).get_unchecked(offset as usize..) }.chars().next().map_or(0, |c| c as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `same` and `order` answer as the slices' own compares do, at every
    /// length to past the ones compared in registers, with one byte higher
    /// or lower at each place, and with lengths that differ.
    #[test]
    fn compares_in_registers_as_the_slices_do() {
        let check = |a: &[u8], b: &[u8]| {
            assert_eq!(same(a, b), a == b, "same {a:?} {b:?}");
            assert_eq!(order(a, b), a.cmp(b), "order {a:?} {b:?}");
        };
        for n in 0..=IN_REGISTERS + 8 {
            let a: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            check(&a, &a);
            for at in 0..n {
                for step in [1u8, 255, 128] {
                    let mut b = a.clone();
                    b[at] = b[at].wrapping_add(step);
                    check(&a, &b);
                    check(&b, &a);
                }
            }
            for m in 0..=n {
                check(&a[..m], &a);
                check(&a, &a[..m]);
            }
        }
    }

    /// A text's count of scalars and where each starts, short or not, ASCII
    /// or not, below and past where positions are kept, are Rust's.
    #[test]
    fn counts_and_finds_scalars_as_rust_does() {
        let long = "word ".repeat(20);
        let accented = format!("{}é{}", "a".repeat(40), "ñ".repeat(30));
        for text in ["", "abc", "abcdefg", "é", "aé", "日本", "a😀b", "abcdefgh", "naïve café", &long, &accented]
        {
            let s = str_new(text.as_bytes());
            let count = text.chars().count();
            assert_eq!(unsafe { scalar_count(s) }, count, "{text:?}");
            for index in 0..=count + 1 {
                let expected = text.char_indices().nth(index).map(|(at, _)| at);
                assert_eq!(unsafe { scalar_offset(s, index) }, expected, "{text:?} at {index}");
            }
            unsafe { tondo_rt_str_release(s) };
        }
    }
}
