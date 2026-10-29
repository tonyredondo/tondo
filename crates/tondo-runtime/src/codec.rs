//! What `std.encoding`, `std.hash` and `std.random` do in the runtime
//! (standard library 14.20 to 14.22): Base64, hexadecimal and URL text,
//! SHA digests and HMAC, the system's entropy and the mixing function of
//! seeded generators.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, element, str_bytes, str_new};
use crate::io::byte_array;
use hmac::{HmacReset, KeyInit};
use sha2::digest::DynDigest;
use std::sync::Mutex;

fn bytes(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { (0..(*array).len).map(|i| *element(array, 1, i)).collect() }
}

fn text<'a>(s: *const Str) -> &'a [u8] {
    unsafe { str_bytes(s) }
}

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64 of RFC 4648: the standard alphabet with `=` padding, or the URL
/// one without it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_base64_encode(data: *mut ArrayHeader, url: u8) -> *mut Str {
    let (alphabet, pad) = if url != 0 { (URL, false) } else { (STANDARD, true) };
    let data = bytes(data);
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(alphabet[(n >> (18 - 6 * i) & 63) as usize]);
        }
        if pad {
            out.extend(std::iter::repeat_n(b'=', 3 - chunk.len()));
        }
    }
    str_new(&out)
}

/// The bytes of Base64 text, with or without its padding, or the offset
/// where it is wrong: its length when it ends in the middle of a byte.
fn base64_decode(input: &[u8], url: bool) -> Result<Vec<u8>, usize> {
    let alphabet = if url { URL } else { STANDARD };
    let end = input.iter().rposition(|&c| c != b'=').map_or(0, |i| i + 1);
    if input.len() - end > 2 || (input.len() > end && !input.len().is_multiple_of(4)) {
        return Err(end);
    }
    let mut out = Vec::with_capacity(end / 4 * 3 + 2);
    let (mut n, mut count) = (0u32, 0);
    for (offset, &c) in input[..end].iter().enumerate() {
        let value = alphabet.iter().position(|&a| a == c).ok_or(offset)?;
        n = n << 6 | value as u32;
        count += 1;
        if count == 4 {
            out.extend_from_slice(&n.to_be_bytes()[1..]);
            (n, count) = (0, 0);
        }
    }
    match count {
        0 => {}
        2 => out.push((n >> 4) as u8),
        3 => out.extend_from_slice(&((n >> 2) as u16).to_be_bytes()),
        _ => return Err(input.len()),
    }
    Ok(out)
}

/// Where Base64 text is wrong, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_base64_check(input: *const Str, url: u8) -> i64 {
    base64_decode(text(input), url != 0).err().map_or(-1, |offset| offset as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_base64_decode(input: *const Str, url: u8) -> *mut ArrayHeader {
    byte_array(&base64_decode(text(input), url != 0).unwrap_or_default())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hex_encode(data: *mut ArrayHeader) -> *mut Str {
    let out: String = bytes(data).iter().map(|b| format!("{b:02x}")).collect();
    str_new(out.as_bytes())
}

fn hex_decode(input: &[u8]) -> Result<Vec<u8>, usize> {
    let digit = |offset: usize| (input[offset] as char).to_digit(16).ok_or(offset);
    let mut out = Vec::with_capacity(input.len() / 2);
    for offset in (0..input.len()).step_by(2) {
        let high = digit(offset)?;
        if offset + 1 == input.len() {
            return Err(input.len());
        }
        out.push((high << 4 | digit(offset + 1)?) as u8);
    }
    Ok(out)
}

/// Where hexadecimal text is wrong, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hex_check(input: *const Str) -> i64 {
    hex_decode(text(input)).err().map_or(-1, |offset| offset as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hex_decode(input: *const Str) -> *mut ArrayHeader {
    byte_array(&hex_decode(text(input)).unwrap_or_default())
}

/// Percent-encodes the UTF-8 of a text, except the unreserved characters of
/// `encoding.htmlEscape`: `&`, `<`, `>`, `"` and `'` as references, which
/// HTML's content and quoted attributes take.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_html_escape(input: *const Str) -> *mut Str {
    let mut out = Vec::new();
    for &b in text(input) {
        match b {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'"' => out.extend_from_slice(b"&#34;"),
            b'\'' => out.extend_from_slice(b"&#39;"),
            b => out.push(b),
        }
    }
    str_new(&out)
}

/// `encoding.htmlUnescape`: HTML5's named and numeric references read; what
/// is not one stays as it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_html_unescape(input: *const Str) -> *mut Str {
    let input = String::from_utf8_lossy(text(input));
    str_new(html_escape::decode_html_entities(&input).as_bytes())
}

/// RFC 3986: letters, digits, `-`, `.`, `_` and `~`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_url_encode(input: *const Str) -> *mut Str {
    let mut out = Vec::new();
    for &b in text(input) {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02X}").as_bytes());
        }
    }
    str_new(&out)
}

fn url_decode(input: &[u8]) -> Result<Vec<u8>, usize> {
    let mut out = Vec::with_capacity(input.len());
    let mut offset = 0;
    while offset < input.len() {
        if input[offset] == b'%' {
            let pair = input.get(offset + 1..offset + 3).ok_or(input.len())?;
            let value = std::str::from_utf8(pair).ok().and_then(|p| u8::from_str_radix(p, 16).ok()).ok_or(offset)?;
            out.push(value);
            offset += 3;
        } else {
            out.push(input[offset]);
            offset += 1;
        }
    }
    Ok(out)
}

/// Where percent-encoded text is wrong, `-2` when what it encodes is not
/// UTF-8, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_url_check(input: *const Str) -> i64 {
    match url_decode(text(input)) {
        Ok(decoded) if std::str::from_utf8(&decoded).is_ok() => -1,
        Ok(_) => -2,
        Err(offset) => offset as i64,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_url_decode(input: *const Str) -> *mut Str {
    str_new(&url_decode(text(input)).unwrap_or_default())
}

/// A hasher of `kind`: MD5, SHA-1, SHA-256, SHA-384 or SHA-512 as it is 5,
/// 1, 256, 384 or 512; with a `key`, its HMAC.
fn hasher(kind: i64, key: Option<&[u8]>) -> Box<dyn DynDigest + Send> {
    fn mac<M: DynDigest + KeyInit + Send + 'static>(key: &[u8]) -> Box<dyn DynDigest + Send> {
        Box::new(<M as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length"))
    }
    match (kind, key) {
        (5, None) => Box::new(md5::Md5::default()),
        (1, None) => Box::new(sha1::Sha1::default()),
        (256, None) => Box::new(sha2::Sha256::default()),
        (384, None) => Box::new(sha2::Sha384::default()),
        (512, None) => Box::new(sha2::Sha512::default()),
        (5, Some(key)) => mac::<HmacReset<md5::Md5>>(key),
        (1, Some(key)) => mac::<HmacReset<sha1::Sha1>>(key),
        (256, Some(key)) => mac::<HmacReset<sha2::Sha256>>(key),
        (384, Some(key)) => mac::<HmacReset<sha2::Sha384>>(key),
        (_, None) => Box::new(sha2::Sha512::default()),
        (_, Some(key)) => mac::<HmacReset<sha2::Sha512>>(key),
    }
}

/// The digest of `data` (`hasher`), or with `keyed` its HMAC with `key`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hash_digest(
    kind: i64,
    key: *mut ArrayHeader,
    data: *mut ArrayHeader,
    keyed: u8,
) -> *mut ArrayHeader {
    let key = bytes(key);
    let mut hasher = hasher(kind, (keyed != 0).then_some(&key[..]));
    hasher.update(&bytes(data));
    byte_array(&hasher.finalize_reset())
}

/// A hasher that takes its data in parts (`hasher`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hasher(kind: i64, key: *mut ArrayHeader, keyed: u8) -> *mut NativeObject {
    let key = bytes(key);
    native::new(Mutex::new(hasher(kind, (keyed != 0).then_some(&key[..]))))
}

/// Gives a hasher the next part of its data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hasher_update(object: *mut NativeObject, data: *mut ArrayHeader) {
    if let Some(hasher) = native::get::<Mutex<Box<dyn DynDigest + Send>>>(object) {
        hasher.lock().unwrap_or_else(|e| e.into_inner()).update(&bytes(data));
    }
}

/// The digest of the data a hasher has had so far; it can take more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_hasher_finish(object: *mut NativeObject) -> *mut ArrayHeader {
    let Some(hasher) = native::get::<Mutex<Box<dyn DynDigest + Send>>>(object) else { return byte_array(&[]) };
    let digest = hasher.lock().unwrap_or_else(|e| e.into_inner()).box_clone().finalize_reset();
    byte_array(&digest)
}

/// `count` bytes from the system's source of entropy.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_entropy(count: i64) -> *mut ArrayHeader {
    byte_array(&entropy(count.max(0) as usize))
}

/// `count` bytes from the system's source of entropy.
pub(crate) fn entropy(count: usize) -> Vec<u8> {
    use std::io::Read;
    let mut out = vec![0u8; count];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut out));
    if read.is_err() {
        super::runtime_fault("the system has no source of entropy at /dev/urandom");
    }
    out
}

/// Milliseconds since the Unix epoch, for UUIDv7.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unix_millis() -> i64 {
    super::time::tondo_rt_time_unix().div_euclid(1_000_000)
}

/// The `counter`-th value of the SplitMix64 sequence that starts at `seed`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_random_mix(seed: i64, counter: i64) -> i64 {
    let mut z = (seed as u64).wrapping_add((counter as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as i64
}

/// `count` bytes from the values of the sequence from the `counter`-th on,
/// eight from each, in little-endian order.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_random_bytes(seed: i64, counter: i64, count: i64) -> *mut ArrayHeader {
    let count = count.max(0) as usize;
    let out: Vec<u8> = (0..count.div_ceil(8) as i64)
        .flat_map(|i| tondo_rt_random_mix(seed, counter + i).to_le_bytes())
        .take(count)
        .collect();
    byte_array(&out)
}

/// The fields of one CSV record (RFC 4180) that may span lines, or where it
/// is wrong: `Err(None)` while a quoted field has not ended, or the scalar
/// index of a character out of place.
fn csv_record(record: &str, separator: char) -> Result<Vec<String>, Option<usize>> {
    let (mut fields, mut field) = (Vec::new(), String::new());
    // Inside quotes; after a closing quote; at the start of a field.
    let (mut quoted, mut closed, mut start) = (false, false, true);
    let mut chars = record.chars().enumerate().peekable();
    while let Some((index, c)) = chars.next() {
        if quoted {
            if c != '"' {
                field.push(c);
            } else if chars.next_if(|&(_, next)| next == '"').is_some() {
                field.push('"');
            } else {
                (quoted, closed) = (false, true);
            }
        } else if c == separator {
            fields.push(std::mem::take(&mut field));
            (closed, start) = (false, true);
        } else if c == '"' && start {
            (quoted, start) = (true, false);
        } else if closed || c == '"' {
            return Err(Some(index));
        } else {
            field.push(c);
            start = false;
        }
    }
    if quoted {
        return Err(None);
    }
    fields.push(field);
    Ok(fields)
}

/// `-1` for a whole record, `-2` while a quoted field has not ended, or the
/// scalar index of a character out of place.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_csv_check(record: *const Str, separator: u32) -> i64 {
    let separator = char::from_u32(separator).unwrap_or(',');
    match csv_record(&String::from_utf8_lossy(text(record)), separator) {
        Ok(_) => -1,
        Err(None) => -2,
        Err(Some(index)) => index as i64,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_csv_fields(record: *const Str, separator: u32) -> *mut ArrayHeader {
    let separator = char::from_u32(separator).unwrap_or(',');
    let fields = csv_record(&String::from_utf8_lossy(text(record)), separator).unwrap_or_default();
    let array = super::tondo_rt_array_new(8, fields.len());
    for (index, field) in fields.iter().enumerate() {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = str_new(field.as_bytes()) };
    }
    array
}
