//! The named operations of the numbers (language 18.9) that compiled code
//! does not do inline: powers, text, bytes, and parsing. An integer comes
//! as 128 bits, extended by its signedness, with its width and signedness.

use super::{ArrayHeader, Str, raise};

/// The bounds of an integer of `bits` bits.
fn bounds(bits: u32, signed: bool) -> (i128, u128) {
    let unused = 128 - bits.clamp(1, 128);
    match signed {
        true => (i128::MIN >> unused, (i128::MAX >> unused) as u128),
        false => (0, u128::MAX >> unused),
    }
}

/// The low `bits` bits of `value`, extended by the signedness.
fn wrap(value: u128, bits: u32, signed: bool) -> u128 {
    let unused = 128 - bits.clamp(1, 128);
    if signed { ((value << unused) as i128 >> unused) as u128 } else { value << unused >> unused }
}

fn wide(low: u64, high: u64) -> u128 {
    (high as u128) << 64 | low as u128
}

/// `base` to the power `exponent`, an integer of `bits` bits, into `out`:
/// 0 when it fits, 1 when it does not, 2 for a negative exponent. A value
/// that does not fit is its low bits, or the nearest bound when `saturate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int_pow(
    saturate: u8,
    signed: u8,
    bits: u32,
    values: *const [u64; 4],
    out: *mut [u64; 2],
) -> u8 {
    let [base_low, base_high, exponent, _] = unsafe { *values };
    let (base, exponent, signed) = (wide(base_low, base_high), exponent as i64, signed != 0);
    let write = |value: u128| unsafe { *out = [value as u64, (value >> 64) as u64] };
    if exponent < 0 {
        write(0);
        return 2;
    }
    let (min, max) = bounds(bits, signed);
    let fits = exact(base, exponent as u64, signed, min, max).is_some();
    let result = match (fits, saturate != 0) {
        (false, true) if signed && (base as i128) < 0 && exponent % 2 == 1 => min as u128,
        (false, true) => max,
        _ => wrap(power_bits(base, exponent as u64), bits, signed),
    };
    write(result);
    !fits as u8
}

/// `base` to the power `exponent` when it is from `min` to `max`. A base
/// of more than one in magnitude leaves the range within 128 products.
fn exact(base: u128, exponent: u64, signed: bool, min: i128, max: u128) -> Option<u128> {
    let fits = |value: i128| value >= min && (value < 0 || value as u128 <= max);
    if signed {
        let base = base as i128;
        let value = match base {
            0 => i128::from(exponent == 0),
            1 => 1,
            -1 if exponent.is_multiple_of(2) => 1,
            -1 => -1,
            _ => {
                let mut value = 1i128;
                for _ in 0..exponent {
                    value = value.checked_mul(base).filter(|&v| fits(v))?;
                }
                value
            }
        };
        return fits(value).then_some(value as u128);
    }
    let value = match base {
        0 => u128::from(exponent == 0),
        1 => 1,
        _ => {
            let mut value = 1u128;
            for _ in 0..exponent {
                value = value.checked_mul(base).filter(|&v| v <= max)?;
            }
            value
        }
    };
    (value <= max).then_some(value)
}

/// The low 128 bits of `base` to the power `exponent`.
fn power_bits(mut base: u128, mut exponent: u64) -> u128 {
    let mut result = 1u128;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exponent >>= 1;
    }
    result
}

/// The integer of `bits` bits a text writes in `radix` (2 to 36), with an
/// optional sign and `_` between digits, into `out`; whether it is one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int_parse(
    s: *const Str,
    radix: i64,
    bits: u32,
    signed: u8,
    out: *mut [u64; 2],
) -> u8 {
    // A short text's decimal digits, from its word: never through the
    // memory its word was just written to, which a processor reads back a
    // byte at a time only after the write leaves (store-to-load interlocks).
    if radix == 10
        && bits == 64
        && signed != 0
        && super::strings::short(s)
        && let Some(value) = short_decimal(s as u64)
    {
        let wide = i128::from(value) as u128;
        unsafe { *out = [wide as u64, (wide >> 64) as u64] };
        return 1;
    }
    let text = unsafe { super::str_bytes(&s) };
    // An `Int` in decimal of up to 18 digits, as most texts parsed are,
    // fits in 64 bits as it is read and needs no bounds of 128.
    if radix == 10 && bits == 64 && signed != 0 {
        let (negative, digits) = match text.first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        if (1..=18).contains(&digits.len())
            && let Some(value) = decimal(digits)
        {
            let value = if negative { -(value as i64) } else { value as i64 };
            let wide = i128::from(value) as u128;
            unsafe { *out = [wide as u64, (wide >> 64) as u64] };
            return 1;
        }
    }
    let parsed = parse(unsafe { std::str::from_utf8_unchecked(text) }, radix, bits, signed != 0);
    let value = parsed.unwrap_or(0);
    unsafe { *out = [value as u64, (value >> 64) as u64] };
    parsed.is_some() as u8
}

/// In one pass over the bytes, with nothing made: the digits accumulate as
/// they come, a `_` only between two of them.
fn parse(s: &str, radix: i64, bits: u32, signed: bool) -> Option<u128> {
    let radix = u32::try_from(radix).ok().filter(|r| (2..=36).contains(r))?;
    let bytes = s.as_bytes();
    let (negative, digits) = match bytes.first()? {
        b'-' => (true, &bytes[1..]),
        b'+' => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    // Up to 18 decimal digits, as most numbers are, in a loop that cannot
    // overflow: a `u64` holds what is below 10^18. A `_` or anything else
    // goes the general way.
    let fast = (radix == 10 && (1..=18).contains(&digits.len())).then(|| decimal(digits)).flatten();
    let magnitude = match fast {
        Some(value) => u128::from(value),
        None => general(digits, radix)?,
    };
    let (min, max) = bounds(bits, signed);
    match negative {
        true if magnitude == 0 => Some(0),
        true if signed && magnitude <= min.unsigned_abs() => Some((magnitude as i128).wrapping_neg() as u128),
        true => None,
        false => (magnitude <= max).then_some(magnitude),
    }
}

/// The decimal digits' value, when they are digits alone.
fn decimal(digits: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for &byte in digits {
        let digit = byte.wrapping_sub(b'0');
        if digit >= 10 {
            return None;
        }
        value = value * 10 + u64::from(digit);
    }
    Some(value)
}

/// The value of a short text's word written in decimal, with an optional
/// sign: its digits put at the top of eight, `0`s below them, all checked as
/// digits at once, then joined two, four and eight at a time by three
/// multiplications; anything else, as a `_`, a letter or a lone sign, is
/// left to the general path.
fn short_decimal(word: u64) -> Option<i64> {
    let len = ((word >> 56) & 0x7f) as u32;
    let bytes = word & 0x00ff_ffff_ffff_ffff;
    let (negative, skip) = match bytes & 0xff {
        0x2d => (true, 1),
        0x2b => (false, 1),
        _ => (false, 0),
    };
    let digits = len.checked_sub(skip).filter(|&digits| digits > 0)?;
    let chunk = ((bytes >> (8 * skip)) << (8 * (8 - digits))) | (0x3030_3030_3030_3030u64 >> (8 * digits));
    let high = 0xf0f0_f0f0_f0f0_f0f0;
    let digits_only = chunk & high == 0x3030_3030_3030_3030
        && chunk.wrapping_add(0x0606_0606_0606_0606) & high == 0x3030_3030_3030_3030;
    if !digits_only {
        return None;
    }
    let mut value = chunk - 0x3030_3030_3030_3030;
    value = (value.wrapping_mul(10) + (value >> 8)) & 0x00ff_00ff_00ff_00ff;
    value = (value.wrapping_mul(100) + (value >> 16)) & 0x0000_ffff_0000_ffff;
    value = (value.wrapping_mul(10000) + (value >> 32)) & 0xffff_ffff;
    Some(if negative { -(value as i64) } else { value as i64 })
}

/// The digits' value in `radix`, a `_` only between two of them, in 64 bits
/// while it fits and then in 128.
fn general(digits: &[u8], radix: u32) -> Option<u128> {
    if digits.first().is_none_or(|&b| b == b'_') || digits.last() == Some(&b'_') {
        return None;
    }
    // In 64 bits while the number fits, as most do, then in 128; a
    // decimal digit is its byte less `0`.
    let digit_of = |byte: u8| match radix {
        10 => Some(u32::from(byte.wrapping_sub(b'0'))).filter(|&digit| digit < 10),
        _ => char::from(byte).to_digit(radix),
    };
    let (mut small, mut wide, mut separated) = (0u64, None::<u128>, false);
    for &byte in digits {
        if byte == b'_' {
            if separated {
                return None;
            }
            separated = true;
            continue;
        }
        separated = false;
        let digit = digit_of(byte)?;
        wide = match wide {
            None => match small.checked_mul(u64::from(radix)).and_then(|v| v.checked_add(u64::from(digit))) {
                Some(next) => {
                    small = next;
                    None
                }
                None => Some(u128::from(small).checked_mul(u128::from(radix))?.checked_add(u128::from(digit))?),
            },
            Some(value) => Some(value.checked_mul(u128::from(radix))?.checked_add(u128::from(digit))?),
        };
    }
    Some(wide.unwrap_or(u128::from(small)))
}

/// An integer in `radix`, clamped to 2 to 36, with lowercase digits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int_radix(
    low: u64,
    high: u64,
    signed: u8,
    radix: i64,
    region: *mut super::regions::Region,
) -> *mut Str {
    let value = wide(low, high);
    let radix = radix.clamp(2, 36) as u128;
    let negative = signed != 0 && (value as i128) < 0;
    let mut magnitude = if negative { (value as i128).unsigned_abs() } else { value };
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((magnitude % radix) as u32, radix as u32).unwrap() as u8);
        magnitude /= radix;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        digits.push(b'-');
    }
    digits.reverse();
    super::strings::str_in(region, &digits)
}

/// The `count` bytes of an integer, the highest first when `big`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int_to_bytes(low: u64, high: u64, count: i64, big: u8) -> *mut ArrayHeader {
    let bytes = wide(low, high).to_le_bytes();
    let count = count as usize;
    let array = super::array_alloc(1, count, count);
    for k in 0..count {
        let byte = if big != 0 { bytes[count - 1 - k] } else { bytes[k] };
        unsafe { *super::element(array, 1, k) = byte };
    }
    array
}

/// The integer the `count` bytes from `at` of an array write, the highest
/// first when `big`, into `out`; a panic when they are not there.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_int_from_bytes(
    array: *mut ArrayHeader,
    at: i64,
    count: i64,
    big: u8,
    location: *const Str,
    out: *mut [u64; 2],
) {
    let len = unsafe { (*array).len };
    let count = count as usize;
    unsafe { *out = [0, 0] };
    if at < 0 || at as usize + count > len {
        let message = match len.checked_sub(count) {
            Some(last) => {
                format!("{count} bytes from {at} are not in an array of {len}: `at` must be from 0 to {last}")
            }
            None => format!("{count} bytes from {at} are not in an array of {len}, which has fewer"),
        };
        raise(1, &message, location);
        return;
    }
    let mut bytes = [0u8; 16];
    for k in 0..count {
        let byte = unsafe { *super::element(array, 1, at as usize + k) };
        bytes[if big != 0 { count - 1 - k } else { k }] = byte;
    }
    let value = u128::from_le_bytes(bytes);
    unsafe { *out = [value as u64, (value >> 64) as u64] };
}

/// The integer of 128 bits at most a number's digits write, as its sign
/// and magnitude; `None` for a float's digits, or more than 128 bits.
pub(crate) fn integer_digits(text: &str) -> Option<(bool, u128)> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u128>().ok().map(|magnitude| (negative, magnitude))
}

/// The float a text writes, as a `Float32` when `single`, into `out`;
/// whether it is one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_float_parse(s: *const Str, single: u8, out: *mut f64) -> u8 {
    let text = String::from_utf8_lossy(unsafe { super::str_bytes(&s) });
    let parsed = super::text::parse_float(&text).and_then(|value| match single != 0 {
        // Read again as 32 bits, so that it is rounded once.
        true if value.is_finite() => text.parse::<f32>().ok().map(f64::from),
        _ => Some(value),
    });
    unsafe { *out = parsed.unwrap_or(0.0) };
    parsed.is_some() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `tondo_rt_int_parse` gave a text before short texts took a path
    /// of their own: its bytes through the general one, kept as the
    /// reference.
    fn parsed_before(bytes: &[u8]) -> Option<i64> {
        let (negative, digits) = match bytes.first() {
            Some(b'-') => (true, &bytes[1..]),
            Some(b'+') => (false, &bytes[1..]),
            _ => (false, bytes),
        };
        if (1..=18).contains(&digits.len())
            && let Some(value) = decimal(digits)
        {
            return Some(if negative { -(value as i64) } else { value as i64 });
        }
        parse(unsafe { std::str::from_utf8_unchecked(bytes) }, 10, 64, true).map(|value| value as i64)
    }

    /// What it gives the short text of `bytes` now.
    fn parsed_now(bytes: &[u8]) -> Option<i64> {
        let text = crate::strings::short_text(bytes);
        let mut out = [0u64; 2];
        let parsed = unsafe { tondo_rt_int_parse(text, 10, 64, 1, &mut out) };
        (parsed == 1).then_some(out[0] as i64)
    }

    /// Every text of one to seven digits, leading zeros too, and of a sign
    /// and up to six, parses to its value; and every byte at each place of
    /// each length, with digits around it, gives what the general path did.
    #[test]
    fn short_decimals_parse_as_before() {
        let mut text = [0u8; 7];
        for len in 1..=7usize {
            for n in 0..10u64.pow(len as u32) {
                let mut rest = n;
                for at in (0..len).rev() {
                    text[at] = b'0' + (rest % 10) as u8;
                    rest /= 10;
                }
                assert_eq!(parsed_now(&text[..len]), Some(n as i64), "{:?}", &text[..len]);
                if len < 7 {
                    let mut signed = [0u8; 7];
                    signed[1..=len].copy_from_slice(&text[..len]);
                    for (sign, value) in [(b'-', -(n as i64)), (b'+', n as i64)] {
                        signed[0] = sign;
                        assert_eq!(parsed_now(&signed[..=len]), Some(value), "{:?}", &signed[..=len]);
                    }
                }
            }
        }
        for len in 1..=7usize {
            for at in 0..len {
                for byte in 0..=255u8 {
                    let mut text: Vec<u8> = (0..len).map(|i| b'1' + (i % 9) as u8).collect();
                    text[at] = byte;
                    assert_eq!(parsed_now(&text), parsed_before(&text), "{text:?}");
                }
            }
        }
        assert_eq!(parsed_now(b""), parsed_before(b""));
    }

    fn power(base: i128, exponent: i64, bits: u32, signed: bool, saturate: bool) -> (i128, u8) {
        let base = base as u128;
        let values = [base as u64, (base >> 64) as u64, exponent as u64, 0];
        let mut out = [0u64; 2];
        let status = unsafe { tondo_rt_int_pow(saturate as u8, signed as u8, bits, &values, &mut out) };
        (wide(out[0], out[1]) as i128, status)
    }

    #[test]
    fn powers() {
        assert_eq!(power(3, 4, 64, true, false), (81, 0));
        assert_eq!(power(-2, 7, 8, true, false), (-128, 0));
        assert_eq!(power(-2, 8, 8, true, false), (0, 1));
        assert_eq!(power(-2, 9, 8, true, true), (-128, 1));
        assert_eq!(power(2, 8, 8, false, false), (0, 1));
        assert_eq!(power(2, 8, 8, false, true), (255, 1));
        assert_eq!(power(3, 200, 64, false, false).1, 1);
        assert_eq!(power(2, -1, 64, true, false).1, 2);
        assert_eq!(power(0, 0, 32, true, false), (1, 0));
        assert_eq!(power(2, 127, 128, false, false), (1 << 127, 0));
    }

    #[test]
    fn parses() {
        assert_eq!(parse("-128", 10, 8, true), Some((-128i128) as u128));
        assert_eq!(parse("128", 10, 8, true), None);
        assert_eq!(parse("ff", 16, 8, false), Some(255));
        assert_eq!(parse("340282366920938463463374607431768211455", 10, 128, false), Some(u128::MAX));
        assert_eq!(parse("1_000", 10, 64, true), Some(1000));
        assert_eq!(parse("+7", 10, 64, true), Some(7));
        assert_eq!(parse("Zz", 36, 64, true), Some(35 * 36 + 35));
        for wrong in ["", "-", "_1", "1_", "1__0", "-+5", "1 2", "١٢"] {
            assert_eq!(parse(wrong, 10, 64, true), None, "{wrong}");
        }
        assert_eq!(parse("18446744073709551616", 10, 64, false), None);
        assert_eq!(parse("3402823669209384634633746074317682114560", 10, 128, false), None);
        assert_eq!(parse("-1", 10, 32, false), None);
    }

    /// An `Int` in decimal, which takes a way of its own, reads as any
    /// other integer does, at its ends and past them.
    #[test]
    fn a_decimal_int_reads_as_the_others() {
        let texts = ["0", "-0", "+7", "-7", "7919", "-", "+", "", "12a", "1_000", "١٢", "-+5"];
        let ends = ["999999999999999999", "-999999999999999999", "1000000000000000000", "-1000000000000000000"];
        let past = ["9223372036854775807", "-9223372036854775808", "9223372036854775808", "-9223372036854775809"];
        for text in texts.into_iter().chain(ends).chain(past) {
            let s = super::super::str_new(text.as_bytes());
            let mut out = [0u64; 2];
            let read = unsafe { tondo_rt_int_parse(s, 10, 64, 1, &mut out) };
            unsafe { super::super::tondo_rt_str_release(s) };
            let expected = parse(text, 10, 64, true);
            assert_eq!(read != 0, expected.is_some(), "{text}");
            assert_eq!(wide(out[0], out[1]), expected.unwrap_or(0), "{text}");
        }
    }
}
