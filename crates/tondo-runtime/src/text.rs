//! The text operations of the prelude that need the runtime (standard
//! library 14.1): splitting, Unicode case and classes, and numbers to and
//! from text. Positions are scalar indexes, as in the language.

use super::{ArrayHeader, Str, element, str_bytes, str_new, tondo_rt_array_new};

fn text<'a>(s: *const Str) -> &'a str {
    std::str::from_utf8(unsafe { str_bytes(s) }).unwrap_or_default()
}

/// An `Array[String]` of new strings.
fn strings<'a>(parts: impl ExactSizeIterator<Item = &'a str>) -> *mut ArrayHeader {
    let array = tondo_rt_array_new(8, parts.len());
    for (index, part) in parts.enumerate() {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = str_new(part.as_bytes()) };
    }
    array
}

/// The pieces between each `separator`; the characters, when it is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_split(s: *const Str, separator: *const Str) -> *mut ArrayHeader {
    let (s, separator) = (text(s), text(separator));
    if separator.is_empty() {
        let chars: Vec<String> = s.chars().map(String::from).collect();
        return strings(chars.iter().map(String::as_str));
    }
    let parts: Vec<&str> = s.split(separator).collect();
    strings(parts.into_iter())
}

/// The scalar that starts at a byte offset of a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_scalar_at(s: *const Str, offset: i64) -> u32 {
    let rest = text(s).get(offset.max(0) as usize..).unwrap_or_default();
    rest.chars().next().map_or(0xFFFD, |c| c as u32)
}

/// The lines, without their `\n` or `\r\n`; no last empty line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_lines(s: *const Str) -> *mut ArrayHeader {
    let lines: Vec<&str> = text(s).lines().collect();
    strings(lines.into_iter())
}

/// The pieces between runs of Unicode whitespace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_words(s: *const Str) -> *mut ArrayHeader {
    let words: Vec<&str> = text(s).split_whitespace().collect();
    strings(words.into_iter())
}

/// The strings of an `Array[String]` with `separator` between them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_join(array: *mut ArrayHeader, separator: *const Str) -> *mut Str {
    let separator = text(separator);
    let mut out = String::new();
    if !array.is_null() {
        for index in 0..unsafe { (*array).len } {
            if index > 0 {
                out.push_str(separator);
            }
            out.push_str(text(unsafe { *(element(array, 8, index) as *const *const Str) }));
        }
    }
    str_new(out.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_repeat(s: *const Str, count: i64) -> *mut Str {
    str_new(text(s).repeat(count.max(0) as usize).as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_trim_start(s: *const Str) -> *mut Str {
    str_new(text(s).trim_start().as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_trim_end(s: *const Str) -> *mut Str {
    str_new(text(s).trim_end().as_bytes())
}

/// The scalar index of the last `needle`, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_find_last(s: *const Str, needle: *const Str) -> i64 {
    let s = text(s);
    s.rfind(text(needle)).map_or(-1, |at| s[..at].chars().count() as i64)
}

/// How many times `needle` appears, without overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_count(s: *const Str, needle: *const Str) -> i64 {
    let needle = text(needle);
    if needle.is_empty() {
        return text(s).chars().count() as i64 + 1;
    }
    text(s).matches(needle).count() as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_lower(s: *const Str) -> *mut Str {
    str_new(text(s).to_lowercase().as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_upper(s: *const Str) -> *mut Str {
    str_new(text(s).to_uppercase().as_bytes())
}

/// The integer a text writes in `radix` (2 to 36), with an optional sign
/// and `_` between digits; `ok` tells whether it is one that fits.
fn parse_int(s: &str, radix: i64) -> Option<i64> {
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
    let magnitude = u64::from_str_radix(&digits, radix).ok()?;
    if negative { 0i64.checked_sub_unsigned(magnitude) } else { i64::try_from(magnitude).ok() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_is_int(s: *const Str, radix: i64) -> u8 {
    parse_int(text(s), radix).is_some() as u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_parse_int(s: *const Str, radix: i64) -> i64 {
    parse_int(text(s), radix).unwrap_or(0)
}

/// A decimal float, `inf`, `-inf` or `NaN`, as Tondo writes them.
fn parse_float(s: &str) -> Option<f64> {
    match s {
        "inf" | "+inf" => return Some(f64::INFINITY),
        "-inf" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        || digits.contains(|c: char| c.is_alphabetic() && c != 'e' && c != 'E')
    {
        return None;
    }
    s.parse().ok()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_is_float(s: *const Str) -> u8 {
    parse_float(text(s)).is_some() as u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_parse_float(s: *const Str) -> f64 {
    parse_float(text(s)).unwrap_or(0.0)
}

/// An integer in `radix` (2 to 36), with lowercase digits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_int(value: i64, radix: i64) -> *mut Str {
    let radix = radix.clamp(2, 36) as u64;
    let mut magnitude = value.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((magnitude % radix) as u32, radix as u32).unwrap() as u8);
        magnitude /= radix;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        digits.push(b'-');
    }
    digits.reverse();
    str_new(&digits)
}

/// A float with exactly `decimals` digits after the point, rounded half to
/// even as the value is written.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_fixed(value: f64, decimals: i64) -> *mut Str {
    if !value.is_finite() {
        return str_new(format!("{value:?}").as_bytes());
    }
    str_new(format!("{value:.*}", decimals.clamp(0, 64) as usize).as_bytes())
}

fn scalar(c: u32) -> char {
    char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Which class a scalar is in: 0 decimal digit, 1 letter, 2 whitespace,
/// 3 uppercase, 4 lowercase, 5 letter or digit.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_is(c: u32, class: i64) -> u8 {
    let c = scalar(c);
    (match class {
        0 => c.is_ascii_digit(),
        1 => c.is_alphabetic(),
        2 => c.is_whitespace(),
        3 => c.is_uppercase(),
        4 => c.is_lowercase(),
        _ => c.is_alphanumeric(),
    }) as u8
}

/// The scalar in the other case when it is a single one, or the same.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_case(c: u32, upper: u8) -> u32 {
    let c = scalar(c);
    let mut changed: Vec<char> = if upper != 0 { c.to_uppercase().collect() } else { c.to_lowercase().collect() };
    match changed.len() {
        1 => changed.pop().unwrap() as u32,
        _ => c as u32,
    }
}

/// Whether a number is a Unicode scalar value.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_valid(code: i64) -> u8 {
    u32::try_from(code).ok().and_then(char::from_u32).is_some() as u8
}
