//! CBOR (RFC 8949) for `std.cbor`: a data item read into the events of
//! `std.serialization`, each with the path and byte it comes from, and a
//! writer of events in the preferred serialization: the shortest heads and
//! floats, definite lengths and, deterministic, map keys in the order of
//! their encoding (section 4.2).
//!
//! Definite and indefinite lengths are read; text must be UTF-8; tags 0 and
//! 1 are dates and times, 2 and 3 integers when they fit; other tags are
//! read as the item they hold. Map keys are text, or integers read as their
//! digits.

use crate::http::bytes_of;
use crate::io::byte_array;
use crate::native::{self, NativeObject};
use crate::{ArrayHeader, Str, str_bytes, str_new};
use std::sync::Mutex;

const END: i64 = 0;
const NULL: i64 = 1;
const TRUE: i64 = 2;
const FALSE: i64 = 3;
const INT: i64 = 4;
const FLOAT: i64 = 5;
const TEXT: i64 = 6;
const NUMBER: i64 = 7;
const BEGIN_LIST: i64 = 8;
const END_LIST: i64 = 9;
const BEGIN_MAP: i64 = 10;
const END_MAP: i64 = 11;
const KEY: i64 = 12;
const BYTES: i64 = 13;
const DATETIME: i64 = 14;
const ERROR: i64 = -1;

/// The deepest nesting read, so that a crafted input cannot exhaust memory
/// with open containers.
const DEPTH: usize = 512;

#[derive(Clone)]
struct Event {
    code: i64,
    text: String,
    int: i64,
    float: f64,
    bytes: Vec<u8>,
    path: String,
}

struct Reader {
    events: Vec<Event>,
    read: usize,
    error: Option<(String, String)>,
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

/// A container being read: how many items are left (`None` until a break
/// when indefinite), and for a map whether the next is a key and the key of
/// the value being read.
struct Frame {
    map: bool,
    left: Option<u64>,
    key: bool,
    index: u64,
    current: String,
}

type Failure = (String, String);

struct Decoder<'a> {
    input: &'a [u8],
    at: usize,
    frames: Vec<Frame>,
    events: Vec<Event>,
}

/// A scalar of a data item.
enum Item {
    Int(i64),
    /// An integer past `Int`, up to 128 bits.
    Big(u128),
    /// The integer `-1 - n` past `Int`, up to 128 bits.
    Negative(u128),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Null,
}

impl Decoder<'_> {
    fn path(&self) -> String {
        let mut path = String::from("$");
        for frame in &self.frames {
            if frame.map {
                path.push_str(&format!(".{}", frame.current));
            } else {
                path.push_str(&format!("[{}]", frame.index));
            }
        }
        path
    }

    fn fail<T>(&self, message: impl Into<String>) -> Result<T, Failure> {
        Err((message.into(), self.path()))
    }

    fn take(&mut self, count: usize) -> Result<&[u8], Failure> {
        if self.input.len() - self.at < count {
            let at = self.at;
            return self.fail(format!("the input ends inside a data item (byte {at})"));
        }
        let taken = &self.input[self.at..self.at + count];
        self.at += count;
        Ok(taken)
    }

    fn number(&mut self, size: usize) -> Result<u64, Failure> {
        Ok(self.take(size)?.iter().fold(0, |value, &byte| value << 8 | byte as u64))
    }

    /// The argument of a head with additional information `info`; `None`
    /// for an indefinite length.
    fn argument(&mut self, info: u8, at: usize) -> Result<Option<u64>, Failure> {
        match info {
            0..=23 => Ok(Some(info as u64)),
            24..=27 => Ok(Some(self.number(1 << (info - 24))?)),
            31 => Ok(None),
            _ => self.fail(format!("additional information {info} is reserved (byte {at})")),
        }
    }

    fn push(&mut self, code: i64) -> &mut Event {
        let path = self.path();
        self.events.push(Event { code, text: String::new(), int: 0, float: 0.0, bytes: Vec::new(), path });
        self.events.last_mut().unwrap()
    }

    fn at_key(&self) -> bool {
        self.frames.last().is_some_and(|f| f.map && f.key)
    }

    /// After an item: its container counts it, and closes when full.
    fn done(&mut self) {
        let Some(frame) = self.frames.last_mut() else { return };
        if frame.map && !frame.key {
            frame.key = true;
        } else if frame.map {
            return;
        }
        frame.index += 1;
        if let Some(left) = &mut frame.left {
            *left -= 1;
            if *left == 0 {
                self.close();
            }
        }
    }

    fn close(&mut self) {
        let frame = self.frames.pop().unwrap();
        self.push(if frame.map { END_MAP } else { END_LIST });
        self.done();
    }

    /// A string's content: definite, or chunks of the same major type up to
    /// a break.
    fn chunks(&mut self, major: u8, length: Option<u64>, at: usize) -> Result<Vec<u8>, Failure> {
        if let Some(length) = length {
            return Ok(self.take(length as usize)?.to_vec());
        }
        let mut content = Vec::new();
        loop {
            let head = self.take(1)?[0];
            if head == 0xff {
                return Ok(content);
            }
            if head >> 5 != major || head & 0x1f == 31 {
                return self.fail(format!("an indefinite string has a chunk of another kind (byte {at})"));
            }
            let length = self.argument(head & 0x1f, at)?.unwrap_or(0);
            content.extend_from_slice(self.take(length as usize)?);
        }
    }

    /// Reads the next scalar, or opens a container; `None` after a break.
    fn item(&mut self) -> Result<Option<Item>, Failure> {
        let at = self.at;
        let head = self.take(1)?[0];
        let (major, info) = (head >> 5, head & 0x1f);
        if head == 0xff {
            return match self.frames.last() {
                Some(frame) if frame.left.is_none() && (!frame.map || frame.key) => {
                    self.close();
                    Ok(None)
                }
                _ => self.fail(format!("a break outside an indefinite container (byte {at})")),
            };
        }
        let argument = if major == 7 { None } else { self.argument(info, at)? };
        let definite = |this: &Self, argument: Option<u64>| match argument {
            Some(value) => Ok(value),
            None => this.fail(format!("an integer or tag has an indefinite length (byte {at})")),
        };
        Ok(Some(match major {
            0 => {
                let value = definite(self, argument)?;
                i64::try_from(value).map_or(Item::Big(value.into()), Item::Int)
            }
            1 => {
                let value = definite(self, argument)?;
                i64::try_from(value).map_or(Item::Negative(value.into()), |v| Item::Int(-1 - v))
            }
            2 => Item::Bytes(self.chunks(2, argument, at)?),
            3 => match String::from_utf8(self.chunks(3, argument, at)?) {
                Ok(text) => Item::Text(text),
                Err(_) => return self.fail(format!("a text string is not UTF-8 (byte {at})")),
            },
            4 | 5 => {
                if self.at_key() {
                    return self.fail(format!("a map key is not text or an integer (byte {at})"));
                }
                if self.frames.len() >= DEPTH {
                    return self.fail(format!("the data nests deeper than {DEPTH} levels (byte {at})"));
                }
                let map = major == 5;
                self.push(if map { BEGIN_MAP } else { BEGIN_LIST });
                if argument == Some(0) {
                    self.push(if map { END_MAP } else { END_LIST });
                    self.done();
                } else {
                    self.frames.push(Frame { map, left: argument, key: map, index: 0, current: String::new() });
                }
                // A container has no scalar to place.
                return Ok(None);
            }
            6 => return self.tagged(definite(self, argument)?, at),
            _ => match info {
                20 => Item::Bool(false),
                21 => Item::Bool(true),
                22 | 23 => Item::Null,
                25 => Item::Float(half(self.number(2)? as u16)),
                26 => Item::Float(f32::from_bits(self.number(4)? as u32) as f64),
                27 => Item::Float(f64::from_bits(self.number(8)?)),
                _ => return self.fail(format!("the simple value {info} is not one this reads (byte {at})")),
            },
        }))
    }

    /// A tagged item: dates and times, integers in bytes, or the item.
    fn tagged(&mut self, tag: u64, at: usize) -> Result<Option<Item>, Failure> {
        match tag {
            0 => match self.item()? {
                Some(Item::Text(text)) if crate::civil::parse(&text).is_some() => {
                    self.scalar_datetime(text);
                    Ok(None)
                }
                _ => self.fail(format!("a date and time (tag 0) is not RFC 3339 text (byte {at})")),
            },
            1 => {
                let seconds = match self.item()? {
                    Some(Item::Int(value)) => (value, 0),
                    Some(Item::Float(value)) if value.is_finite() => {
                        (value.floor() as i64, ((value - value.floor()) * 1e9).round() as u32)
                    }
                    _ => return self.fail(format!("a time (tag 1) is not a number of seconds (byte {at})")),
                };
                self.scalar_datetime(crate::civil::format(seconds.0, seconds.1));
                Ok(None)
            }
            2 | 3 => match self.item()? {
                Some(Item::Bytes(bytes)) => {
                    let significant: Vec<u8> = bytes.iter().copied().skip_while(|&b| b == 0).collect();
                    if significant.len() > 16 {
                        return self.fail(format!("an integer (tag {tag}) is larger than 128 bits (byte {at})"));
                    }
                    let value = significant.iter().fold(0u128, |value, &byte| value << 8 | byte as u128);
                    Ok(Some(match (tag, i64::try_from(value)) {
                        (2, Ok(value)) => Item::Int(value),
                        (2, Err(_)) => Item::Big(value),
                        (_, Ok(value)) => Item::Int(-1 - value),
                        (_, Err(_)) => Item::Negative(value),
                    }))
                }
                _ => self.fail(format!("an integer (tag {tag}) is not a byte string (byte {at})")),
            },
            _ => self.item(),
        }
    }

    fn scalar_datetime(&mut self, text: String) {
        if self.at_key() {
            self.push(KEY).text = text.clone();
            self.key(text);
        } else {
            self.push(DATETIME).text = text;
            self.done();
        }
    }

    fn key(&mut self, key: String) {
        if let Some(frame) = self.frames.last_mut() {
            frame.key = false;
            frame.current = key;
        }
    }

    /// Reads one item and the containers it opens, until it is complete.
    fn value(&mut self) -> Result<(), Failure> {
        let depth = self.frames.len();
        loop {
            let at = self.at;
            if let Some(item) = self.item()? {
                if self.at_key() {
                    let key = match item {
                        Item::Text(text) => text,
                        Item::Int(value) => value.to_string(),
                        Item::Big(value) => value.to_string(),
                        Item::Negative(value) => negative(value),
                        _ => return self.fail(format!("a map key is not text or an integer (byte {at})")),
                    };
                    self.push(KEY).text = key.clone();
                    self.key(key);
                } else {
                    match item {
                        Item::Int(value) => self.push(INT).int = value,
                        Item::Big(value) => self.push(NUMBER).text = value.to_string(),
                        Item::Negative(value) => self.push(NUMBER).text = negative(value),
                        Item::Float(value) => self.push(FLOAT).float = value,
                        Item::Text(text) => self.push(TEXT).text = text,
                        Item::Bytes(bytes) => self.push(BYTES).bytes = bytes,
                        Item::Bool(value) => {
                            self.push(if value { TRUE } else { FALSE });
                        }
                        Item::Null => {
                            self.push(NULL);
                        }
                    }
                    self.done();
                }
            }
            if self.frames.len() <= depth {
                return Ok(());
            }
        }
    }
}

/// A half-precision float.
fn half(bits: u16) -> f64 {
    let (sign, exponent, fraction) = ((bits >> 15) as i32, ((bits >> 10) & 0x1f) as i32, (bits & 0x3ff) as f64);
    let magnitude = match exponent {
        0 => fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        _ => (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    };
    if sign == 1 { -magnitude } else { magnitude }
}

fn reader(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Reader>> {
    native::get::<Mutex<Reader>>(object).map(|r| r.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The events of one data item in `input`, or the first failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_parse(input: *mut ArrayHeader) -> *mut NativeObject {
    let input = bytes_of(input);
    let mut decoder = Decoder { input: &input, at: 0, frames: Vec::new(), events: Vec::new() };
    let mut error = if input.is_empty() { Some(("the input is empty".to_string(), "$".to_string())) } else { None };
    if error.is_none()
        && let Err(failure) = decoder.value()
    {
        error = Some(failure);
    }
    if error.is_none() && decoder.at < input.len() {
        error = Some((format!("unexpected data after the item (byte {})", decoder.at), "$".to_string()));
    }
    native::new(Mutex::new(Reader { events: decoder.events, read: 0, error }))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_next(object: *mut NativeObject) -> i64 {
    let Some(mut reader) = reader(object) else { return ERROR };
    if reader.error.is_some() {
        return ERROR;
    }
    if reader.read == reader.events.len() {
        return END;
    }
    reader.read += 1;
    reader.events[reader.read - 1].code
}

fn current<T>(object: *mut NativeObject, part: impl Fn(&Event) -> T, empty: T) -> T {
    let Some(reader) = reader(object) else { return empty };
    reader.read.checked_sub(1).and_then(|index| reader.events.get(index)).map_or(empty, part)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_text(object: *mut NativeObject) -> *mut Str {
    str_new(current(object, |e| e.text.clone(), String::new()).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_int(object: *mut NativeObject) -> i64 {
    current(object, |e| e.int, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_float(object: *mut NativeObject) -> f64 {
    current(object, |e| e.float, 0.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_bytes(object: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(&current(object, |e| e.bytes.clone(), Vec::new()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_skip(object: *mut NativeObject, code: i64) -> i64 {
    if code != BEGIN_LIST && code != BEGIN_MAP {
        return 0;
    }
    let Some(mut reader) = reader(object) else { return ERROR };
    let mut depth = 1;
    while depth > 0 && reader.read < reader.events.len() {
        match reader.events[reader.read].code {
            BEGIN_LIST | BEGIN_MAP => depth += 1,
            END_LIST | END_MAP => depth -= 1,
            _ => {}
        }
        reader.read += 1;
    }
    if depth == 0 { 0 } else { ERROR }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cbor_fail(object: *mut NativeObject, message: *const Str) {
    let path = current(object, |e| e.path.clone(), "$".to_string());
    if let Some(mut reader) = reader(object) {
        reader.error = Some((text(message), path));
    }
}

/// The failure's message (0) or path (1).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_where(object: *mut NativeObject, part: i64) -> *mut Str {
    let failure = reader(object).and_then(|r| r.error.clone()).unwrap_or_default();
    str_new(if part == 0 { failure.0 } else { failure.1 }.as_bytes())
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The head of a major type with its argument, in its shortest form.
/// The digits of `-1 - n`, which may be past 128 bits.
fn negative(n: u128) -> String {
    match n.checked_add(1) {
        Some(magnitude) => format!("-{magnitude}"),
        None => "-340282366920938463463374607431768211456".into(),
    }
}

/// An integer its digits write, as the shortest major type 0 or 1, or past
/// 64 bits as tag 2 or 3 with its bytes (RFC 8949, 3.4.3); a float when the
/// digits are not an integer's. Whether it could.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cbor_write_number(object: *mut NativeObject, digits: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(digits) }).into_owned();
    let Some((negative, magnitude)) = crate::numbers::integer_digits(&text) else {
        let value = crate::text::parse_float(&text).unwrap_or(0.0);
        return write(object, |out| float(out, value));
    };
    // `-n` is `-1 - (n - 1)`.
    let (major, tag, argument) = match negative && magnitude > 0 {
        true => (1, 3, magnitude - 1),
        false => (0, 2, magnitude),
    };
    write(object, |out| match u64::try_from(argument) {
        Ok(argument) => head(out, major, argument),
        Err(_) => {
            let bytes = argument.to_be_bytes();
            let significant: Vec<u8> = bytes.iter().copied().skip_while(|&b| b == 0).collect();
            head(out, 6, tag);
            head(out, 2, significant.len() as u64);
            out.extend(significant);
        }
    });
}

fn head(out: &mut Vec<u8>, major: u8, argument: u64) {
    let major = major << 5;
    match argument {
        0..=23 => out.push(major | argument as u8),
        24..=0xff => out.extend([major | 24, argument as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend((argument as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend((argument as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend(argument.to_be_bytes());
        }
    }
}

/// The shortest float that holds `value` exactly; NaN as half precision.
fn float(out: &mut Vec<u8>, value: f64) {
    if value.is_nan() {
        return out.extend([0xf9, 0x7e, 0x00]);
    }
    let single = value as f32;
    if single as f64 != value {
        out.push(0xfb);
        return out.extend(value.to_bits().to_be_bytes());
    }
    if let Some(bits) = to_half(single) {
        out.push(0xf9);
        return out.extend(bits.to_be_bytes());
    }
    out.push(0xfa);
    out.extend(single.to_bits().to_be_bytes());
}

/// The half-precision bits of a float that one holds exactly.
fn to_half(value: f32) -> Option<u16> {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    if value == 0.0 {
        return Some(sign);
    }
    if value.is_infinite() {
        return Some(sign | 0x7c00);
    }
    let exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let fraction = bits & 0x7f_ffff;
    if (-14..=15).contains(&exponent) {
        // Normal: ten bits of fraction, the rest zero.
        if fraction & 0x1fff != 0 {
            return None;
        }
        return Some(sign | (((exponent + 15) as u16) << 10) | (fraction >> 13) as u16);
    }
    if (-24..-14).contains(&exponent) {
        // Subnormal: the significand shifted, with nothing lost.
        let significand = fraction | 0x80_0000;
        let shift = 13 + (-14 - exponent) as u32;
        if significand & ((1 << shift) - 1) != 0 {
            return None;
        }
        return Some(sign | (significand >> shift) as u16);
    }
    None
}

/// A container being written: its items, each a key's encoding and the
/// value's, the count it declared, and for a map the key written last.
struct Open {
    map: bool,
    declared: u64,
    items: Vec<(Vec<u8>, Vec<u8>)>,
    key: Option<Vec<u8>>,
}

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    open: Vec<Open>,
    deterministic: bool,
    error: Option<String>,
}

impl Writer {
    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    /// Places the encoding of a finished item.
    fn add(&mut self, encoded: Vec<u8>) {
        match self.open.last_mut() {
            None => self.out.extend(encoded),
            Some(open) if open.map => match open.key.take() {
                Some(key) => open.items.push((key, encoded)),
                None => self.fail("a value of a map came without its key".into()),
            },
            Some(open) => open.items.push((Vec::new(), encoded)),
        }
    }
}

fn writer(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Writer>> {
    native::get::<Mutex<Writer>>(object).map(|w| w.lock().unwrap_or_else(|e| e.into_inner()))
}

fn write(object: *mut NativeObject, encode: impl FnOnce(&mut Vec<u8>)) {
    if let Some(mut writer) = writer(object) {
        let mut encoded = Vec::new();
        encode(&mut encoded);
        writer.add(encoded);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_writer(deterministic: u8) -> *mut NativeObject {
    native::new(Mutex::new(Writer { deterministic: deterministic != 0, ..Writer::default() }))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_null(object: *mut NativeObject) {
    write(object, |out| out.push(0xf6));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_bool(object: *mut NativeObject, value: u8) {
    write(object, |out| out.push(if value != 0 { 0xf5 } else { 0xf4 }));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_int(object: *mut NativeObject, value: i64) {
    write(object, |out| {
        if value >= 0 {
            head(out, 0, value as u64);
        } else {
            head(out, 1, (-1 - value) as u64);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_float(object: *mut NativeObject, value: f64) {
    write(object, |out| float(out, value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cbor_write_text(object: *mut NativeObject, value: *const Str) {
    let value = unsafe { str_bytes(value) }.to_vec();
    write(object, |out| {
        head(out, 3, value.len() as u64);
        out.extend(value);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_bytes(object: *mut NativeObject, value: *mut ArrayHeader) {
    let value = bytes_of(value);
    write(object, |out| {
        head(out, 2, value.len() as u64);
        out.extend(value);
    });
}

/// A date and time as tag 0 and its RFC 3339 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cbor_write_datetime(object: *mut NativeObject, value: *const Str) {
    let value = text(value);
    let Some((seconds, nanos)) = crate::civil::parse(&value) else {
        if let Some(mut writer) = writer(object) {
            writer.fail(format!("`{value}` is not a date and time"));
        }
        return;
    };
    let shown = crate::civil::format(seconds, nanos);
    write(object, |out| {
        out.push(0xc0);
        head(out, 3, shown.len() as u64);
        out.extend(shown.as_bytes());
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cbor_write_key(object: *mut NativeObject, key: *const Str) {
    let key = unsafe { str_bytes(key) };
    let Some(mut writer) = writer(object) else { return };
    let mut encoded = Vec::new();
    head(&mut encoded, 3, key.len() as u64);
    encoded.extend(key);
    match writer.open.last_mut() {
        Some(open) if open.map && open.key.is_none() => open.key = Some(encoded),
        _ => writer.fail("a key is outside a map, or follows another key".into()),
    }
}

/// Opens a map (`map`) or an array of `count` items.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_open(object: *mut NativeObject, map: u8, count: i64) {
    if let Some(mut writer) = writer(object) {
        writer.open.push(Open { map: map != 0, declared: count.max(0) as u64, items: Vec::new(), key: None });
    }
}

/// Closes the container opened last, with a definite length; a
/// deterministic map orders its keys by their encodings.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_write_close(object: *mut NativeObject) {
    let Some(mut writer) = writer(object) else { return };
    let Some(mut open) = writer.open.pop() else { return };
    if open.items.len() as u64 != open.declared {
        let what = if open.map { "a map" } else { "an array" };
        writer.fail(format!("{what} declared {} items and has {}", open.declared, open.items.len()));
    }
    if open.map && writer.deterministic {
        open.items.sort_by(|a, b| a.0.cmp(&b.0));
        if open.items.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            writer.fail("a map has the same key twice".into());
        }
    }
    let mut encoded = Vec::new();
    head(&mut encoded, if open.map { 5 } else { 4 }, open.items.len() as u64);
    for (key, value) in open.items {
        encoded.extend(key);
        encoded.extend(value);
    }
    writer.add(encoded);
}

/// The bytes written, or none after a failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_finish(object: *mut NativeObject) -> *mut ArrayHeader {
    let out = writer(object).filter(|w| w.error.is_none()).map(|w| w.out.clone()).unwrap_or_default();
    byte_array(&out)
}

/// The first value that could not be written, or empty.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cbor_failure(object: *mut NativeObject) -> *mut Str {
    let message = writer(object).and_then(|w| w.error.clone()).unwrap_or_default();
    str_new(message.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_take_their_shortest_form() {
        let encoded = |value: f64| {
            let mut out = Vec::new();
            float(&mut out, value);
            out
        };
        assert_eq!(encoded(1.5), [0xf9, 0x3e, 0x00]);
        assert_eq!(encoded(0.0), [0xf9, 0x00, 0x00]);
        assert_eq!(encoded(-0.0), [0xf9, 0x80, 0x00]);
        assert_eq!(encoded(65504.0), [0xf9, 0x7b, 0xff]);
        assert_eq!(encoded(5.960464477539063e-8), [0xf9, 0x00, 0x01]);
        assert_eq!(encoded(100000.0), [0xfa, 0x47, 0xc3, 0x50, 0x00]);
        assert_eq!(encoded(1.1), [0xfb, 0x3f, 0xf1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9a]);
        assert_eq!(encoded(f64::INFINITY), [0xf9, 0x7c, 0x00]);
        assert_eq!(half(0x3e00), 1.5);
        assert_eq!(half(0x0001), 5.960464477539063e-8);
    }
}
