//! The string builder (docs/refactor.md, phase 5): what interpolation and
//! `StringBuilder` append to, with room that grows.

use super::*;

/// Text being built, as interpolation builds it: a string with room for
/// more, which `finish` makes the string built without copying it. The code
/// that interpolates keeps it on its stack.
#[repr(C)]
pub struct Builder {
    /// A string whose header's `cap` is its room, or a short text while
    /// what it builds fits in its word.
    text: *mut Str,
    /// Its length; in its word, where it goes once it outgrows it: 0 for
    /// the heap, or a region, marked with `GROWING` for a place whose text
    /// an append grows.
    len: usize,
}

/// The mark of a region that a builder in its word goes to, beside its
/// address, for a place whose text an append grows (`start_growing`).
const GROWING: usize = 1;

impl Builder {
    /// A builder with room for `room` bytes: in its word when they fit,
    /// as a short text needs no block.
    pub fn new(room: usize) -> Builder {
        let mut builder = Builder { text: SHORT as *mut Str, len: 0 };
        if room > SHORT_MAX {
            builder.text = std::ptr::null_mut();
            builder.grow(room);
        }
        builder
    }

    /// The room of its block, which it has unless it builds in its word.
    fn room(&self) -> usize {
        unsafe { (*self.text).cap }
    }

    /// The backing buffer pointer, current length and capacity of a non-short
    /// builder, for tight loops that write directly into the string's storage.
    #[inline(always)]
    pub(crate) fn parts(&self) -> (*mut u8, usize, usize) {
        debug_assert!(!short(self.text));
        unsafe { ((self.text as *mut u8).add(HEADER), self.len, self.room()) }
    }

    /// Updates the length after writing through `parts`.
    #[inline(always)]
    pub(crate) fn set_len(&mut self, len: usize) {
        debug_assert!(!short(self.text) && len <= self.room());
        self.len = len;
    }

    /// Ensures room for `more` bytes past `self.len` in a non-short builder.
    #[inline(always)]
    pub(crate) fn reserve(&mut self, more: usize) {
        debug_assert!(!short(self.text));
        if self.len + more > self.room() {
            self.grow(more);
        }
    }

    #[inline]
    pub fn extend(&mut self, bytes: &[u8]) {
        if short(self.text) {
            return self.extend_word(bytes);
        }
        if self.len + bytes.len() > self.room() {
            self.grow(bytes.len());
        }
        unsafe { copy_short(bytes, (self.text as *mut u8).add(HEADER + self.len)) }
        self.len += bytes.len();
    }

    /// `extend` with the bytes of a short text: one store of its whole word
    /// when the block has room for eight bytes more, as the bytes past the
    /// length are room the next ones write over.
    #[inline]
    pub(crate) fn extend_short(&mut self, s: *const Str) {
        if (self.text as isize) > 0 && self.len + 8 <= self.room() {
            unsafe { (self.text as *mut u8).add(HEADER + self.len).cast::<usize>().write_unaligned(s as usize) };
            self.len += short_len(s);
            return;
        }
        self.extend(unsafe { str_bytes(&s) });
    }

    /// `extend` of a builder in its word: the bytes after its own, and its
    /// length counted on, or out of it when they do not fit.
    #[inline(never)]
    fn extend_word(&mut self, bytes: &[u8]) {
        let len = short_len(self.text);
        if len + bytes.len() <= SHORT_MAX {
            let word = (self.text as usize + (bytes.len() << 56)) | (short_bytes(bytes) << (8 * len));
            self.text = word as *mut Str;
            return;
        }
        self.out_of_word(bytes.len());
        unsafe { copy_short(bytes, (self.text as *mut u8).add(HEADER + self.len)) }
        self.len += bytes.len();
    }

    /// Out of its word, with room for `more` bytes after its own: on the
    /// heap, or in the region it was started in, as it would have started
    /// there with that room.
    #[cold]
    #[inline(never)]
    fn out_of_word(&mut self, more: usize) {
        let (word, len, goes) = ((self.text as usize).to_le_bytes(), short_len(self.text), self.len);
        let room = (len + more).max(2 * SHORT_MAX);
        let region = (goes & !GROWING) as *mut regions::Region;
        unsafe {
            match (region.is_null(), goes & GROWING != 0) {
                (true, _) => {
                    self.text = std::ptr::null_mut();
                    self.grow(room);
                }
                (false, false) => start_region(self, room, region),
                (false, true) => start_growing(self, room, region),
            }
            std::ptr::copy_nonoverlapping(word.as_ptr(), (self.text as *mut u8).add(HEADER), len);
        }
        self.len = len;
    }

    pub fn push(&mut self, byte: u8) {
        self.extend(&[byte]);
    }

    /// Whether it builds in room a frame keeps (`tondo_rt_sb_start_in`).
    fn framed(&self) -> bool {
        (self.text as isize) > 0 && unsafe { (*self.text).rc.load(Ordering::Relaxed) } == FRAMING
    }

    /// The region it builds in (`tondo_rt_sb_start_region`), whose address
    /// the text's count holds while it builds, marked.
    fn region(&self) -> Option<*mut regions::Region> {
        if (self.text as isize) <= 0 {
            return None;
        }
        let rc = unsafe { (*self.text).rc.load(Ordering::Relaxed) };
        matches!(rc & REGION_MARK_BITS, REGION_MARK | REGION_ASKED | REGION_HELD)
            .then_some((rc & !REGION_MARK_BITS) as *mut regions::Region)
    }

    #[cold]
    fn grow(&mut self, more: usize) {
        let held = if self.text.is_null() { 0 } else { self.room() };
        let room = (self.len + more).max(held * 2);
        // A text a region takes grows in it, whatever its length: what the
        // analysis puts in a region stays a region's, as what holds it lets
        // go of nothing. Its old rooms are the region's until it ends; past
        // a mebibyte it is alone in a chunk of its own, which grows as a
        // block on the heap does, so that a long text holds no more. Each
        // room is what the heap's block would hold, so that it grows by the
        // heap's steps and counts as many allocations (`meta::good`).
        if let Some(region) = self.region() {
            let size = meta::good(HEADER + room);
            let text = unsafe {
                match size > regions::REGION_ROOM_MAX {
                    true => regions::grow_alone(region, self.text as *mut u8, HEADER + self.len, size),
                    false => {
                        let text = regions::tondo_rt_region_alloc(region, size);
                        std::ptr::copy_nonoverlapping((self.text as *const u8).add(HEADER), text.add(HEADER), self.len);
                        (text as *mut Str).write(Str {
                            rc: AtomicUsize::new(region as usize | REGION_HELD),
                            len: 0,
                            cap: 0,
                        });
                        text
                    }
                }
            };
            unsafe {
                (*(text as *mut Str)).cap = size - HEADER;
                (*(text as *mut Str)).rc.store(region as usize | REGION_HELD, Ordering::Relaxed);
            }
            self.text = text as *mut Str;
            return;
        }
        let layout = str_layout(room);
        // A new block holds what was asked, as a guess that is seldom
        // short; one that grows takes all the room its block has. What a
        // frame's room held moves to the heap.
        let framed = self.framed();
        let grown = !self.text.is_null() && !framed;
        let text = unsafe {
            match grown {
                false => alloc(layout),
                true => std::alloc::realloc(self.text as *mut u8, str_layout(self.room()), layout.size()),
            }
        };
        if text.is_null() {
            handle_alloc_error(layout);
        }
        if framed {
            unsafe { std::ptr::copy_nonoverlapping((self.text as *const u8).add(HEADER), text.add(HEADER), self.len) };
        }
        self.text = text as *mut Str;
        unsafe {
            (*self.text).rc = AtomicUsize::new(0);
            (*self.text).cap = if grown { meta::usable(text, layout.size()) - HEADER } else { room };
        }
    }

    /// The string built. Its block keeps room it does not use, as much as it
    /// uses at most, which the allocator frees with it and a string appended
    /// to in place grows into. While a budget counts memory, that room is
    /// what was asked for, and counted (`meta::usable`).
    pub fn finish(self) -> *mut Str {
        if short(self.text) {
            return if short_len(self.text) == 0 { str_new(&[]) } else { self.text };
        }
        // Built in a region: the last reference, which its region lets go
        // of. A long one forgets the positions that a text its region let go
        // of left for the same address. One an append may grow keeps all
        // the room the heap's block would hold once built, as an append on
        // the heap takes all of its block: the room it has, or what a
        // smaller block holds where the heap's realloc would move it to
        // one, as mimalloc does when it shrinks a block by half or more.
        let mark = unsafe { (*self.text).rc.load(Ordering::Relaxed) } & REGION_MARK_BITS;
        if mark == REGION_MARK {
            return self.built_in_region();
        }
        if mark == REGION_ASKED || mark == REGION_HELD {
            let (room, len) = (self.room(), self.len);
            let held = if mark == REGION_ASKED { unsafe { (*self.text).len } } else { HEADER + room };
            let kept = match room - len > len.max(16) && 2 * (HEADER + len) < held {
                true => meta::good(HEADER + len),
                false => held,
            };
            unsafe { (*self.text).cap = kept - HEADER };
            return self.built_in_region();
        }
        // Built in a frame's room: a text nothing counts.
        if self.framed() {
            unsafe { (*self.text).rc.store(IMMORTAL, Ordering::Relaxed) };
            unsafe { (*self.text).len = self.len };
            return self.text;
        }
        self.on_heap()
    }

    /// The string built in a block on the heap: apart, so that what is
    /// built in a region or a frame does not keep the registers it takes.
    #[inline(never)]
    fn on_heap(self) -> *mut Str {
        let mut text = self.text as *mut u8;
        let mut cap = self.room();
        if (1..=SHORT_MAX).contains(&self.len) {
            return self.into_word();
        }
        if cap - self.len > self.len.max(16) {
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

impl Builder {
    /// The few bytes built in a block on the heap, in a short text's word;
    /// the block goes.
    #[inline(never)]
    fn into_word(self) -> *mut Str {
        let text = self.text as *mut u8;
        let built = short_text(unsafe { std::slice::from_raw_parts(text.add(HEADER), self.len) });
        unsafe { dealloc(text, str_layout(self.room())) };
        built
    }

    /// The text built in a region, the last reference to it.
    #[inline(always)]
    fn built_in_region(self) -> *mut Str {
        unsafe {
            (*self.text).rc.store(regions::LAST_IN_REGION, Ordering::Relaxed);
            (*self.text).len = self.len;
        }
        if self.len >= POSITIONED_FROM {
            forget_positions(self.text);
        }
        self.text
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

/// What Rust's formatting writes, as a float's shortest form, straight
/// into the builder, rather than through `io::Write`'s adapter.
impl std::fmt::Write for Builder {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.extend(text.as_bytes());
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
    region: *mut regions::Region,
) {
    unsafe {
        let separator = str_bytes(&separator);
        let room = (*a).len * (8 + separator.len());
        // In the region the analysis chose for the text, when it chose one.
        match region.is_null() {
            true => b.write(Builder::new(room)),
            false => tondo_rt_sb_start_region(b, room, region),
        }
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
/// released; in `region`, when not null, as `append_in` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_append(s: *mut Str, t: *const Str, region: *mut regions::Region) -> *mut Str {
    unsafe {
        match region.is_null() {
            true => append(s, str_bytes(&t)),
            false => append_in(s, str_bytes(&t), region),
        }
    }
}

/// The count of the text a builder builds in room a frame keeps.
const FRAMING: usize = usize::MAX - 1;

/// Starts the builder at `b` with room for `room` bytes, in `framed`, the
/// room a frame keeps for a text no one but the frame reads, when it has
/// `framed_room` bytes or more (`stack.rs`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_start_in(b: *mut Builder, room: usize, framed: *mut Str, framed_room: usize) {
    // The positions of a longer text are kept by its address, which the
    // frame's room reuses for the next.
    let framed_room = framed_room.min(POSITIONED_FROM - 1);
    if room > framed_room {
        return unsafe { b.write(Builder::new(room)) };
    }
    unsafe {
        framed.write(Str { rc: AtomicUsize::new(FRAMING), len: 0, cap: framed_room });
        b.write(Builder { text: framed, len: 0 });
    }
}

/// The mark in the count of a text a builder builds in a region, beside the
/// region's address, which a frame aligns to a word: its low bits are free,
/// and none of the counts a text has otherwise ends in them (`FRAMING`,
/// `IMMORTAL`, 0 while a builder builds on the heap).
const REGION_MARK: usize = 0b101;
const REGION_MARK_BITS: usize = 0b111;

/// The mark of a text a builder starts in a region for a place whose text
/// an append may grow (`Region.grows`): its room is what was asked for,
/// as a block's of the heap is until it grows, and its length holds, until
/// it is built, the bytes its block holds, as many as the heap's would.
const REGION_ASKED: usize = 0b011;

/// The mark of a text a builder grows, or takes to grow, in a region: its
/// room is what its block holds, as the heap's block's once it grows.
const REGION_HELD: usize = 0b010;

/// Starts the builder at `b` with room for `room` bytes in `region`, for a
/// text that nothing keeps past it: the room asked for, at least 16 bytes,
/// as the builder knows its region by its text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_start_region(b: *mut Builder, room: usize, region: *mut regions::Region) {
    // In its word while what it builds fits there, as on the heap.
    match room <= SHORT_MAX {
        true => unsafe { b.write(Builder { text: SHORT as *mut Str, len: region as usize }) },
        false => unsafe { start_region(b, room, region) },
    }
}

#[inline]
unsafe fn start_region(b: *mut Builder, room: usize, region: *mut regions::Region) {
    unsafe {
        let room = room.max(16);
        let text = regions::tondo_rt_region_alloc(region, HEADER + room) as *mut Str;
        text.write(Str { rc: AtomicUsize::new(region as usize | REGION_MARK), len: 0, cap: room });
        b.write(Builder { text, len: 0 });
    }
}

/// `tondo_rt_sb_start_region` for a place whose text an append may grow
/// (`Region.grows`): in as many bytes as the heap's block would hold,
/// which the text takes once built.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_start_region_growing(b: *mut Builder, room: usize, region: *mut regions::Region) {
    unsafe { start_growing_or_word(b, room, region) }
}

/// `start_growing`, or in the builder's word while what it builds fits
/// there, as on the heap.
unsafe fn start_growing_or_word(b: *mut Builder, room: usize, region: *mut regions::Region) {
    match room <= SHORT_MAX {
        true => unsafe { b.write(Builder { text: SHORT as *mut Str, len: region as usize | GROWING }) },
        false => unsafe { start_growing(b, room, region) },
    }
}

unsafe fn start_growing(b: *mut Builder, room: usize, region: *mut regions::Region) {
    unsafe {
        let held = meta::good(HEADER + room);
        let text = regions::tondo_rt_region_alloc(region, held) as *mut Str;
        text.write(Str { rc: AtomicUsize::new(region as usize | REGION_ASKED), len: held, cap: room });
        b.write(Builder { text, len: 0 });
    }
}

/// `"{i}"` in room a frame keeps for it, which it always fits, or in its
/// word when it is short.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int_text_into(value: i64, framed: *mut Str) -> *mut Str {
    let mut digits = [0; 21];
    let bytes = signed_decimal(value, &mut digits);
    if bytes.len() <= SHORT_MAX {
        return short_text(bytes);
    }
    unsafe {
        framed.write(Str { rc: AtomicUsize::new(IMMORTAL), len: bytes.len(), cap: bytes.len() });
        copy_short(bytes, (framed as *mut u8).add(HEADER));
    }
    framed
}

/// `"{i}"`: an integer's digits, in a string of their own, a short one
/// from -999999 to 9999999, which takes no allocation.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int_text(value: i64) -> *mut Str {
    let mut digits = [0; 21];
    str_new(signed_decimal(value, &mut digits))
}

/// The decimal digits of `value`, after its sign, at the end of `digits`.
fn signed_decimal(value: i64, digits: &mut [u8; 21]) -> &[u8] {
    let len = decimal(value.unsigned_abs(), (&mut digits[1..]).try_into().unwrap()).len();
    let start = 21 - len - usize::from(value < 0);
    if value < 0 {
        digits[start] = b'-';
    }
    &digits[start..]
}

/// `x = "{x}{i}"`, as `tondo_rt_str_append`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_append_int(s: *mut Str, value: i64, region: *mut regions::Region) -> *mut Str {
    let mut digits = [0; 21];
    let digits = signed_decimal(value, &mut digits);
    unsafe {
        match region.is_null() {
            true => append(s, digits),
            false => append_in(s, digits, region),
        }
    }
}

/// Copies `bytes` to `to`: a few bytes, as a string's usual piece, in two
/// moves of a word that may overlap rather than a call of `memcpy`; up to 64
/// by a call of `copy_medium`, whose moves inlined everywhere would keep the
/// callers of this one from being inlined in turn.
#[inline(always)]
pub(crate) unsafe fn copy_short(bytes: &[u8], to: *mut u8) {
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
            17..=64 => copy_medium(bytes, to),
            _ => std::ptr::copy_nonoverlapping(from, to, n),
        }
    }
}

/// `copy_upto_64` of 17 to 64 bytes, out of line.
#[inline(never)]
unsafe fn copy_medium(bytes: &[u8], to: *mut u8) {
    unsafe { copy_upto_64(bytes, to) }
}

/// Copies up to 64 bytes to `to` in moves of 16 bytes or of a word that may
/// overlap: `memcpy`'s dispatch costs more than the copy of so few. Inlined
/// where a leaf makes a text, as `str_in_region`; elsewhere through
/// `copy_short`.
#[inline(always)]
pub(crate) unsafe fn copy_upto_64(bytes: &[u8], to: *mut u8) {
    let (from, n) = (bytes.as_ptr(), bytes.len());
    let move16 = |at: usize| unsafe {
        to.add(at).cast::<u128>().write_unaligned(from.add(at).cast::<u128>().read_unaligned());
    };
    unsafe {
        match n {
            33..=64 => {
                move16(0);
                move16(16);
                move16(n - 32);
                move16(n - 16);
            }
            17..=32 => {
                move16(0);
                move16(n - 16);
            }
            _ => copy_short(bytes, to),
        }
    }
}

pub(crate) unsafe fn append(s: *mut Str, bytes: &[u8]) -> *mut Str {
    unsafe {
        let len = str_len(s);
        if !short(s) && (*s).rc.load(Ordering::Acquire) == 1 {
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

/// `append` in `region`: in `s`'s own room when it is a region's and
/// nothing else holds it, which the analysis lets live as long as what it
/// makes, or else by the builder, in the region, as
/// `tondo_rt_sb_adopt_region` starts it.
unsafe fn append_in(s: *mut Str, bytes: &[u8], region: *mut regions::Region) -> *mut Str {
    unsafe {
        let len = str_len(s);
        if !short(s) && (*s).rc.load(Ordering::Acquire) == regions::LAST_IN_REGION && len + bytes.len() <= (*s).cap {
            copy_short(bytes, (s as *mut u8).add(HEADER + len));
            (*s).len = len + bytes.len();
            return s;
        }
        let mut builder = std::mem::MaybeUninit::<Builder>::uninit();
        let adopted = tondo_rt_sb_adopt_region(builder.as_mut_ptr(), s, bytes.len(), region);
        let mut builder = builder.assume_init();
        builder.extend(bytes);
        if adopted == 0 {
            tondo_rt_str_release(s);
        }
        builder.finish()
    }
}

/// Starts the builder at `b` with the string `s` and room for `room` bytes
/// more: in the string's own block when nothing else holds it, which is the
/// builder's then (1), or else in a new one with a copy of it (0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_adopt(b: *mut Builder, s: *mut Str, room: usize) -> u8 {
    unsafe {
        if !short(s) && (*s).rc.load(Ordering::Acquire) == 1 {
            // Its block becomes the builder's, which may move it.
            forget_positions(s);
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
        b.write(Builder::new(str_len(s) + room));
        (*b).extend(str_bytes(&s));
        0
    }
}

/// Starts the builder at `b` with the string `s` and room for `room` bytes
/// more, in `region`: in the string's own room when it is a region's and
/// nothing else holds it, which the builder grows into `region` from then
/// on (1), or else in the region with a copy of it (0). `x = "{x}..."`
/// so grows a text a region takes, its room doubling, the copies it leaves
/// the region's until it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_adopt_region(
    b: *mut Builder,
    s: *mut Str,
    room: usize,
    region: *mut regions::Region,
) -> u8 {
    unsafe {
        // A text of another region too, as one a caller made and gave: the
        // analysis has its region live as long as what the append makes,
        // and the heap takes the text it is given as it is.
        if !short(s) && (*s).rc.load(Ordering::Acquire) == regions::LAST_IN_REGION {
            forget_positions(s);
            let len = (*s).len;
            (*s).rc.store(region as usize | REGION_HELD, Ordering::Relaxed);
            b.write(Builder { text: s, len });
            if len + room > (*b).room() {
                (*b).grow(room);
            }
            return 1;
        }
        // A copy, as the heap makes one, which the append grows.
        start_growing_or_word(b, str_len(s) + room, region);
        (*b).extend(str_bytes(&s));
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
    unsafe {
        match short(s) {
            true => (*b).extend_short(s),
            false => (*b).extend(long_bytes(s)),
        }
    }
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
pub(crate) fn decimal(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
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
    let _ = std::fmt::Write::write_fmt(unsafe { &mut *b }, format_args!("{value:?}"));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_f32(b: *mut Builder, value: f32) {
    let _ = std::fmt::Write::write_fmt(unsafe { &mut *b }, format_args!("{value:?}"));
}

/// A character's UTF-8 bytes, the replacement character's for a code that
/// is not a scalar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_char(b: *mut Builder, value: u32) {
    let c = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
    unsafe { (*b).extend(c.encode_utf8(&mut [0; 4]).as_bytes()) }
}

/// A text in quotes, escaped as JSON escapes it, as a debugger shows it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_quoted(b: *mut Builder, s: *const Str) {
    let text = unsafe { super::strings::str_text(&s) };
    let _ = std::fmt::Write::write_fmt(unsafe { &mut *b }, format_args!("{text:?}"));
}

/// Consumes the builder and returns an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_finish(b: *mut Builder) -> *mut Str {
    unsafe { b.read() }.finish()
}

#[cfg(test)]
mod tests {
    use super::{copy_short, copy_upto_64};

    /// Every length up to past the moves' edges, inline or by a call: the
    /// bytes land where they go, all of them, and none before or after.
    #[test]
    fn a_short_copy_writes_its_bytes_and_no_others() {
        let from: Vec<u8> = (0..80u8).map(|b| b.wrapping_mul(7).wrapping_add(1)).collect();
        for n in [0, 1, 3, 7, 8, 15, 16, 17, 31, 32, 33, 48, 63, 64, 65, 70] {
            for offset in [0, 1, 7] {
                let copies: [unsafe fn(&[u8], *mut u8); 2] = [copy_short, copy_upto_64];
                for copy in copies.into_iter().filter(|_| n <= 64 || offset == 0) {
                    let mut to = [0xAAu8; 96];
                    unsafe { copy(&from[offset..offset + n], to.as_mut_ptr().add(8)) };
                    assert_eq!(&to[8..8 + n], &from[offset..offset + n], "{n} bytes from {offset}");
                    assert!(to[..8].iter().chain(&to[8 + n..]).all(|&b| b == 0xAA), "{n} bytes from {offset}");
                }
            }
        }
    }
}
