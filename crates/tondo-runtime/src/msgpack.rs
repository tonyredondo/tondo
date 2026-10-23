//! MessagePack for `std.messagepack`: input read into the events of
//! `std.serialization`, each with the path and byte it comes from, and a
//! writer of events that uses the shortest form of every integer and
//! length, checking each list and map against the count it declared.
//!
//! `str` is text and must be UTF-8, `bin` is bytes, and the timestamp
//! extension is a date and time. Map keys are text, or integers read as
//! their digits.

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

/// The type of the timestamp extension.
const TIMESTAMP: i8 = -1;

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
    /// A failure: its message and path.
    error: Option<(String, String)>,
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

/// A list or map being read: how many items are left, and for a map
/// whether the next is a key and the key of the value being read.
struct Frame {
    map: bool,
    left: u64,
    key: bool,
    index: u64,
    current: String,
}

struct Decoder<'a> {
    input: &'a [u8],
    at: usize,
    frames: Vec<Frame>,
    events: Vec<Event>,
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

    fn fail<T>(&self, message: impl Into<String>) -> Result<T, (String, String)> {
        Err((message.into(), self.path()))
    }

    fn take(&mut self, count: usize) -> Result<&[u8], (String, String)> {
        if self.input.len() - self.at < count {
            let at = self.at;
            return self.fail(format!("the input ends inside a value (byte {at})"));
        }
        let taken = &self.input[self.at..self.at + count];
        self.at += count;
        Ok(taken)
    }

    fn uint(&mut self, size: usize) -> Result<u64, (String, String)> {
        Ok(self.take(size)?.iter().fold(0, |value, &byte| value << 8 | byte as u64))
    }

    fn push(&mut self, code: i64) -> &mut Event {
        let path = self.path();
        self.events.push(Event { code, text: String::new(), int: 0, float: 0.0, bytes: Vec::new(), path });
        self.events.last_mut().unwrap()
    }

    /// Whether the next item of the innermost map is a key.
    fn at_key(&self) -> bool {
        self.frames.last().is_some_and(|f| f.map && f.key)
    }

    /// After a value: a list counts it, a map waits for a key; a finished
    /// collection closes.
    fn done(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.left -= 1;
            frame.index += 1;
            frame.key = true;
        }
        while self.frames.last().is_some_and(|f| f.left == 0) {
            let frame = self.frames.pop().unwrap();
            self.push(if frame.map { END_MAP } else { END_LIST });
            if let Some(outer) = self.frames.last_mut() {
                outer.left -= 1;
                outer.index += 1;
                outer.key = true;
            }
        }
    }

    fn open(&mut self, map: bool, count: u64) {
        self.push(if map { BEGIN_MAP } else { BEGIN_LIST });
        if count == 0 {
            self.push(if map { END_MAP } else { END_LIST });
            self.done();
        } else {
            self.frames.push(Frame { map, left: count, key: map, index: 0, current: String::new() });
        }
    }

    fn string(&mut self, size: usize) -> Result<String, (String, String)> {
        let at = self.at;
        let raw = self.take(size)?.to_vec();
        match String::from_utf8(raw) {
            Ok(text) => Ok(text),
            Err(_) => self.fail(format!("a `str` is not UTF-8 (byte {at}); binary data is `bin`")),
        }
    }

    /// A key of the innermost map: text, or an integer as its digits.
    fn key(&mut self, marker: u8) -> Result<(), (String, String)> {
        let key = match marker {
            0xa0..=0xbf => self.string((marker & 0x1f) as usize)?,
            0xd9 => {
                let size = self.uint(1)? as usize;
                self.string(size)?
            }
            0xda => {
                let size = self.uint(2)? as usize;
                self.string(size)?
            }
            0xdb => {
                let size = self.uint(4)? as usize;
                self.string(size)?
            }
            0x00..=0x7f => marker.to_string(),
            0xe0..=0xff => (marker as i8).to_string(),
            0xcc..=0xcf => self.uint(1 << (marker - 0xcc))?.to_string(),
            0xd0..=0xd3 => {
                let size = 1 << (marker - 0xd0);
                let raw = self.uint(size)?;
                let shift = 64 - 8 * size as u32;
                (((raw << shift) as i64) >> shift).to_string()
            }
            _ => return self.fail(format!("a map key is not text or an integer (byte {})", self.at - 1)),
        };
        if let Some(frame) = self.frames.last_mut() {
            frame.key = false;
            frame.current = key.clone();
        }
        self.push(KEY).text = key;
        Ok(())
    }

    fn value(&mut self) -> Result<(), (String, String)> {
        let at = self.at;
        let marker = self.take(1)?[0];
        if self.at_key() {
            return self.key(marker);
        }
        match marker {
            0x00..=0x7f => self.push(INT).int = marker as i64,
            0xe0..=0xff => self.push(INT).int = marker as i8 as i64,
            0xc0 => {
                self.push(NULL);
            }
            0xc2 | 0xc3 => {
                self.push(if marker == 0xc3 { TRUE } else { FALSE });
            }
            0xcc..=0xcf => {
                let value = self.uint(1 << (marker - 0xcc))?;
                match i64::try_from(value) {
                    Ok(value) => self.push(INT).int = value,
                    Err(_) => self.push(NUMBER).text = value.to_string(),
                }
            }
            0xd0..=0xd3 => {
                let size = 1 << (marker - 0xd0);
                let raw = self.uint(size)?;
                let shift = 64 - 8 * size as u32;
                self.push(INT).int = ((raw << shift) as i64) >> shift;
            }
            0xca => {
                let bits = self.uint(4)? as u32;
                self.push(FLOAT).float = f32::from_bits(bits) as f64;
            }
            0xcb => {
                let bits = self.uint(8)?;
                self.push(FLOAT).float = f64::from_bits(bits);
            }
            0xa0..=0xbf | 0xd9..=0xdb => {
                let size = match marker {
                    0xd9 => self.uint(1)?,
                    0xda => self.uint(2)?,
                    0xdb => self.uint(4)?,
                    _ => (marker & 0x1f) as u64,
                };
                let value = self.string(size as usize)?;
                self.push(TEXT).text = value;
            }
            0xc4..=0xc6 => {
                let size = self.uint(1 << (marker - 0xc4))? as usize;
                let value = self.take(size)?.to_vec();
                self.push(BYTES).bytes = value;
            }
            0x90..=0x9f | 0xdc | 0xdd => {
                let count = match marker {
                    0xdc => self.uint(2)?,
                    0xdd => self.uint(4)?,
                    _ => (marker & 0x0f) as u64,
                };
                self.open(false, count);
                return Ok(());
            }
            0x80..=0x8f | 0xde | 0xdf => {
                let count = match marker {
                    0xde => self.uint(2)?,
                    0xdf => self.uint(4)?,
                    _ => (marker & 0x0f) as u64,
                };
                self.open(true, count);
                return Ok(());
            }
            0xd4..=0xd8 | 0xc7..=0xc9 => {
                let size = match marker {
                    0xd4..=0xd8 => 1 << (marker - 0xd4),
                    _ => self.uint(1 << (marker - 0xc7))? as usize,
                };
                let kind = self.take(1)?[0] as i8;
                let data = self.take(size)?.to_vec();
                if kind != TIMESTAMP {
                    return self.fail(format!("an extension of type {kind} is not one this reads (byte {at})"));
                }
                let Some((seconds, nanos)) = timestamp(&data) else {
                    return self.fail(format!("a timestamp is not 4, 8 or 12 bytes long (byte {at})"));
                };
                self.push(DATETIME).text = crate::civil::format(seconds, nanos);
            }
            _ => return self.fail(format!("0xc1 is not a MessagePack value (byte {at})")),
        }
        self.done();
        Ok(())
    }
}

/// The seconds and nanoseconds of a timestamp extension's data.
fn timestamp(data: &[u8]) -> Option<(i64, u32)> {
    let number = |bytes: &[u8]| bytes.iter().fold(0u64, |value, &byte| value << 8 | byte as u64);
    match data.len() {
        4 => Some((number(data) as i64, 0)),
        8 => {
            let both = number(data);
            Some(((both & 0x3_ffff_ffff) as i64, (both >> 34) as u32))
        }
        12 => Some((number(&data[4..]) as i64, number(&data[..4]) as u32)),
        _ => None,
    }
}

fn reader(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Reader>> {
    native::get::<Mutex<Reader>>(object).map(|r| r.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The events of one value in `input`, or the first failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_parse(input: *mut ArrayHeader) -> *mut NativeObject {
    let input = bytes_of(input);
    let input = input.as_slice();
    let mut decoder = Decoder { input, at: 0, frames: Vec::new(), events: Vec::new() };
    let mut error = None;
    if input.is_empty() {
        error = Some(("the input is empty".to_string(), "$".to_string()));
    }
    while error.is_none() {
        if let Err(failure) = decoder.value() {
            error = Some(failure);
        }
        if decoder.frames.is_empty() {
            break;
        }
    }
    if error.is_none() && decoder.at < input.len() {
        error = Some((format!("unexpected data after the value (byte {})", decoder.at), "$".to_string()));
    }
    native::new(Mutex::new(Reader { events: decoder.events, read: 0, error }))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_next(object: *mut NativeObject) -> i64 {
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
pub extern "C" fn tondo_rt_msgpack_text(object: *mut NativeObject) -> *mut Str {
    str_new(current(object, |e| e.text.clone(), String::new()).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_int(object: *mut NativeObject) -> i64 {
    current(object, |e| e.int, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_float(object: *mut NativeObject) -> f64 {
    current(object, |e| e.float, 0.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_bytes(object: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(&current(object, |e| e.bytes.clone(), Vec::new()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_skip(object: *mut NativeObject, code: i64) -> i64 {
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
pub unsafe extern "C" fn tondo_rt_msgpack_fail(object: *mut NativeObject, message: *const Str) {
    let path = current(object, |e| e.path.clone(), "$".to_string());
    if let Some(mut reader) = reader(object) {
        reader.error = Some((text(message), path));
    }
}

/// The failure's message (0) or path (1).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_where(object: *mut NativeObject, part: i64) -> *mut Str {
    let failure = reader(object).and_then(|r| r.error.clone()).unwrap_or_default();
    str_new(if part == 0 { failure.0 } else { failure.1 }.as_bytes())
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A list or map being written: the count it declared, how many it has,
/// and for a map whether a key came before the next value.
struct Open {
    map: bool,
    declared: u64,
    written: u64,
    keyed: bool,
}

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    open: Vec<Open>,
    error: Option<String>,
}

fn writer(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Writer>> {
    native::get::<Mutex<Writer>>(object).map(|w| w.lock().unwrap_or_else(|e| e.into_inner()))
}

impl Writer {
    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    /// A value was written in the innermost list or map.
    fn wrote(&mut self) {
        let Some(open) = self.open.last_mut() else { return };
        if open.map && !open.keyed {
            return self.fail("a value of a map came without its key".into());
        }
        open.keyed = false;
        open.written += 1;
    }
}

fn write(object: *mut NativeObject, value: impl FnOnce(&mut Vec<u8>)) {
    if let Some(mut writer) = writer(object) {
        value(&mut writer.out);
        writer.wrote();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_writer() -> *mut NativeObject {
    native::new(Mutex::new(Writer::default()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_null(object: *mut NativeObject) {
    write(object, |out| out.push(0xc0));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_bool(object: *mut NativeObject, value: u8) {
    write(object, |out| out.push(if value != 0 { 0xc3 } else { 0xc2 }));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_int(object: *mut NativeObject, value: i64) {
    write(object, |out| {
        let _ = rmp::encode::write_sint(out, value);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_float(object: *mut NativeObject, value: f64) {
    write(object, |out| {
        let _ = rmp::encode::write_f64(out, value);
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_msgpack_write_text(object: *mut NativeObject, value: *const Str) {
    let value = text(value);
    write(object, |out| {
        let _ = rmp::encode::write_str(out, &value);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_bytes(object: *mut NativeObject, value: *mut ArrayHeader) {
    let value = bytes_of(value);
    write(object, |out| {
        let _ = rmp::encode::write_bin(out, &value);
    });
}

/// A date and time as a timestamp extension, in its shortest form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_msgpack_write_datetime(object: *mut NativeObject, value: *const Str) {
    let value = text(value);
    let Some((seconds, nanos)) = crate::civil::parse(&value) else {
        if let Some(mut writer) = writer(object) {
            writer.fail(format!("`{value}` is not a date and time"));
        }
        return;
    };
    write(object, |out| {
        if nanos == 0 && (0..=u32::MAX as i64).contains(&seconds) {
            out.extend([0xd6, TIMESTAMP as u8]);
            out.extend((seconds as u32).to_be_bytes());
        } else if (0..1 << 34).contains(&seconds) {
            out.extend([0xd7, TIMESTAMP as u8]);
            out.extend(((nanos as u64) << 34 | seconds as u64).to_be_bytes());
        } else {
            out.extend([0xc7, 12, TIMESTAMP as u8]);
            out.extend(nanos.to_be_bytes());
            out.extend(seconds.to_be_bytes());
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_msgpack_write_key(object: *mut NativeObject, key: *const Str) {
    let key = text(key);
    let Some(mut writer) = writer(object) else { return };
    match writer.open.last_mut() {
        Some(open) if open.map && !open.keyed => open.keyed = true,
        _ => return writer.fail(format!("the key `{key}` is outside a map, or follows another key")),
    }
    let _ = rmp::encode::write_str(&mut writer.out, &key);
}

/// Opens a map (`map`) or a list of `count` items.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_open(object: *mut NativeObject, map: u8, count: i64) {
    let Some(mut writer) = writer(object) else { return };
    let count = count.max(0) as u32;
    let _ = if map != 0 {
        rmp::encode::write_map_len(&mut writer.out, count)
    } else {
        rmp::encode::write_array_len(&mut writer.out, count)
    };
    writer.open.push(Open { map: map != 0, declared: count as u64, written: 0, keyed: false });
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_write_close(object: *mut NativeObject) {
    let Some(mut writer) = writer(object) else { return };
    let Some(open) = writer.open.pop() else { return };
    if open.written != open.declared {
        let what = if open.map { "a map" } else { "a list" };
        writer.fail(format!("{what} declared {} items and has {}", open.declared, open.written));
    }
    writer.wrote();
}

/// The bytes written, or none after a failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_finish(object: *mut NativeObject) -> *mut ArrayHeader {
    let out = writer(object).filter(|w| w.error.is_none()).map(|w| w.out.clone()).unwrap_or_default();
    byte_array(&out)
}

/// The first value that could not be written, or empty.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_msgpack_failure(object: *mut NativeObject) -> *mut Str {
    let message = writer(object).and_then(|w| w.error.clone()).unwrap_or_default();
    str_new(message.as_bytes())
}
