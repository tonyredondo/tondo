//! The string builder (docs/refactor.md, phase 5): what interpolation and
//! `StringBuilder` append to, with room that grows.

use super::*;

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
    /// to in place grows into. While a budget or a watch counts memory, that
    /// room is what was asked for, and counted (`meta::usable`).
    pub fn finish(self) -> *mut Str {
        if self.text.is_null() {
            return str_new(&[]);
        }
        let mut text = self.text as *mut u8;
        let mut cap = self.room();
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
            _ => std::ptr::copy_nonoverlapping(from, to, n),
        }
    }
}

pub(crate) unsafe fn append(s: *mut Str, bytes: &[u8]) -> *mut Str {
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
