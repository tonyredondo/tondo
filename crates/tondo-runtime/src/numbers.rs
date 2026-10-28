//! The named operations of the numbers (language 18.9) that compiled code
//! does not do inline: powers, text, bytes, and parsing. An integer comes
//! as 128 bits, extended by its signedness, with its width and signedness.

use super::{ArrayHeader, Str, raise, str_new};

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
    let text = String::from_utf8_lossy(unsafe { super::str_bytes(s) });
    let parsed = parse(&text, radix, bits, signed != 0);
    let value = parsed.unwrap_or(0);
    unsafe { *out = [value as u64, (value >> 64) as u64] };
    parsed.is_some() as u8
}

fn parse(s: &str, radix: i64, bits: u32, signed: bool) -> Option<u128> {
    let radix = u32::try_from(radix).ok().filter(|r| (2..=36).contains(r))?;
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let digits: String = digits.chars().filter(|&c| c != '_').collect();
    let magnitude = u128::from_str_radix(&digits, radix).ok()?;
    let (min, max) = bounds(bits, signed);
    match negative {
        true if magnitude == 0 => Some(0),
        true if signed && magnitude <= min.unsigned_abs() => Some((magnitude as i128).wrapping_neg() as u128),
        true => None,
        false => (magnitude <= max).then_some(magnitude),
    }
}

/// An integer in `radix`, clamped to 2 to 36, with lowercase digits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_int_radix(low: u64, high: u64, signed: u8, radix: i64) -> *mut Str {
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
    str_new(&digits)
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
    let text = String::from_utf8_lossy(unsafe { super::str_bytes(s) });
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
        assert_eq!(parse("-1", 10, 32, false), None);
    }
}
