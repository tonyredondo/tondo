//! The wire format of `std.protobuf` (standard library 14.12): a writer of
//! tagged fields whose nested messages get their length when they end, and
//! a reader of fields that checks wire types, varints, lengths, UTF-8 and
//! how deep messages nest. The code the compiler writes for each message
//! calls them field by field.

use crate::io::{array_bytes, byte_array};
use crate::native::{self, NativeObject};
use crate::{ArrayHeader, Str, str_bytes, str_new};
use std::sync::Mutex;

const VARINT: u8 = 0;
const FIXED64: u8 = 1;
const LENGTH: u8 = 2;
const START_GROUP: u8 = 3;
const END_GROUP: u8 = 4;
const FIXED32: u8 = 5;

/// How deep messages may nest, as other implementations bound it.
const DEPTH: usize = 100;

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The bytes written, and those of each nested message still open with
/// its field's number.
struct Writer {
    out: Vec<u8>,
    open: Vec<(i64, Vec<u8>)>,
    deterministic: bool,
}

impl Writer {
    fn buffer(&mut self) -> &mut Vec<u8> {
        match self.open.last_mut() {
            Some((_, buffer)) => buffer,
            None => &mut self.out,
        }
    }

    /// The tag of a field, or nothing for the number 0, which an element
    /// of a packed field has.
    fn tag(&mut self, number: i64, wire: u8) {
        if number > 0 {
            varint(self.buffer(), (number as u64) << 3 | wire as u64);
        }
    }

    /// Writes the innermost open message after its tag and length.
    fn close(&mut self) {
        let Some((number, inner)) = self.open.pop() else { return };
        self.tag(number, LENGTH);
        let buffer = self.buffer();
        varint(buffer, inner.len() as u64);
        buffer.extend_from_slice(&inner);
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn writer<'a>(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'a, Writer>> {
    native::get::<Mutex<Writer>>(object).map(|w| w.lock().unwrap_or_else(|e| e.into_inner()))
}

fn write(object: *mut NativeObject, number: i64, wire: u8, bytes: &[u8]) {
    if let Some(mut w) = writer(object) {
        w.tag(number, wire);
        w.buffer().extend_from_slice(bytes);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_writer(deterministic: u8) -> *mut NativeObject {
    native::new(Mutex::new(Writer { out: Vec::new(), open: Vec::new(), deterministic: deterministic != 0 }))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_deterministic(object: *mut NativeObject) -> u8 {
    writer(object).is_some_and(|w| w.deterministic) as u8
}

/// A varint: int32, int64, uint32, uint64, bool and enums; a negative
/// int32 in ten bytes, as protobuf writes it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_write_varint(object: *mut NativeObject, number: i64, value: i64) {
    let mut bytes = Vec::with_capacity(10);
    varint(&mut bytes, value as u64);
    write(object, number, VARINT, &bytes);
}

/// sint32 and sint64.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_write_zigzag(object: *mut NativeObject, number: i64, value: i64) {
    tondo_rt_pb_write_varint(object, number, (value << 1) ^ (value >> 63));
}

/// fixed32, sfixed32 and float, from their 32 bits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_write_fixed32(object: *mut NativeObject, number: i64, bits: i64) {
    write(object, number, FIXED32, &(bits as u32).to_le_bytes());
}

/// fixed64, sfixed64 and double, from their 64 bits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_write_fixed64(object: *mut NativeObject, number: i64, bits: i64) {
    write(object, number, FIXED64, &bits.to_le_bytes());
}

fn write_length(object: *mut NativeObject, number: i64, bytes: &[u8]) {
    if let Some(mut w) = writer(object) {
        w.tag(number, LENGTH);
        let buffer = w.buffer();
        varint(buffer, bytes.len() as u64);
        buffer.extend_from_slice(bytes);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pb_write_bytes(object: *mut NativeObject, number: i64, bytes: *mut ArrayHeader) {
    write_length(object, number, &unsafe { array_bytes(bytes) });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pb_write_text(object: *mut NativeObject, number: i64, text: *const Str) {
    write_length(object, number, unsafe { str_bytes(text) });
}

/// Opens a nested message, a packed field or a map entry of field
/// `number`, whose length `end` writes before it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_begin(object: *mut NativeObject, number: i64) {
    if let Some(mut w) = writer(object) {
        w.open.push((number, Vec::new()));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_end(object: *mut NativeObject) {
    if let Some(mut w) = writer(object) {
        w.close();
    }
}

/// The start or the end of a group of proto2.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_write_group(object: *mut NativeObject, number: i64, start: u8) {
    if let Some(mut w) = writer(object) {
        w.tag(number, if start != 0 { START_GROUP } else { END_GROUP });
    }
}

/// Fields as they were read, which a message did not know.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pb_write_raw(object: *mut NativeObject, bytes: *mut ArrayHeader) {
    let bytes = unsafe { array_bytes(bytes) };
    if let Some(mut w) = writer(object) {
        w.buffer().extend_from_slice(&bytes);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_finish(object: *mut NativeObject) -> *mut ArrayHeader {
    let Some(mut w) = writer(object) else { return byte_array(&[]) };
    while !w.open.is_empty() {
        w.close();
    }
    byte_array(&std::mem::take(&mut w.out))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Where reading stops: at the end of a nested message or packed field,
/// or at the end of a group of a number.
#[derive(Clone, Copy)]
enum Bound {
    Length { end: usize, packed: bool },
    Group(u64),
}

struct Reader {
    input: Vec<u8>,
    position: usize,
    bounds: Vec<Bound>,
    /// The field just read: its number, wire type and where its tag starts.
    number: u64,
    wire: u8,
    start: usize,
    error: Option<String>,
}

fn reader<'a>(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'a, Reader>> {
    native::get::<Mutex<Reader>>(object).map(|r| r.lock().unwrap_or_else(|e| e.into_inner()))
}

impl Reader {
    fn fail(&mut self, message: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(message.into());
        }
    }

    /// Where the innermost length ends, or the input does.
    fn end(&self) -> usize {
        self.bounds
            .iter()
            .rev()
            .find_map(|bound| match bound {
                Bound::Length { end, .. } => Some(*end),
                Bound::Group(_) => None,
            })
            .unwrap_or(self.input.len())
    }

    fn varint(&mut self) -> Option<u64> {
        let end = self.end();
        let mut value = 0u64;
        for shift in 0..10 {
            let Some(&byte) = self.input.get(self.position).filter(|_| self.position < end) else {
                self.fail("a varint ends before its last byte");
                return None;
            };
            self.position += 1;
            value |= ((byte & 0x7f) as u64) << (shift * 7);
            if byte < 0x80 {
                return Some(value);
            }
        }
        self.fail("a varint is longer than ten bytes");
        None
    }

    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let start = self.position;
        if count > self.end().saturating_sub(start) {
            self.fail(format!("a field of {count} bytes goes past the end of its message"));
            return None;
        }
        self.position += count;
        Some(&self.input[start..start + count])
    }

    /// Whether the value read now is an element of a packed field, which
    /// has no tag of its own.
    fn packed(&self) -> bool {
        matches!(self.bounds.last(), Some(Bound::Length { packed: true, .. }))
    }

    /// Checks the wire type of the field read, but in a packed field.
    fn expect(&mut self, wire: u8, what: &str) -> bool {
        if self.packed() || self.wire == wire {
            return true;
        }
        let found = match self.wire {
            VARINT => "a varint",
            FIXED64 => "8 bytes",
            LENGTH => "a length-delimited value",
            START_GROUP => "a group",
            _ => "4 bytes",
        };
        self.fail(format!("expected {what}, found {found}"));
        false
    }

    fn next(&mut self) -> i64 {
        if self.error.is_some() {
            return -1;
        }
        if let Some(&Bound::Length { end, .. }) = self.bounds.last()
            && self.position >= end
        {
            self.bounds.pop();
            return 0;
        }
        if self.bounds.is_empty() && self.position >= self.input.len() {
            return 0;
        }
        self.start = self.position;
        let Some(tag) = self.varint() else { return -1 };
        let (number, wire) = (tag >> 3, (tag & 7) as u8);
        if number == 0 || number > 0x1fff_ffff {
            self.fail(format!("field number {number} is outside 1 to 536870911"));
            return -1;
        }
        if wire == END_GROUP {
            return match self.bounds.last() {
                Some(&Bound::Group(open)) if open == number => {
                    self.bounds.pop();
                    0
                }
                _ => {
                    self.fail(format!("the end of group {number} closes no group of that number"));
                    -1
                }
            };
        }
        if wire > FIXED32 {
            self.fail(format!("field {number} has the wire type {wire}, which does not exist"));
            return -1;
        }
        (self.number, self.wire) = (number, wire);
        number as i64
    }

    /// Skips the value of the field read.
    fn skip(&mut self) {
        match self.wire {
            VARINT => {
                self.varint();
            }
            FIXED64 => {
                self.take(8);
            }
            FIXED32 => {
                self.take(4);
            }
            LENGTH => {
                if let Some(length) = self.varint() {
                    self.take(length as usize);
                }
            }
            START_GROUP => {
                let number = self.number;
                if self.bounds.len() >= DEPTH {
                    self.fail(format!("messages nest deeper than {DEPTH}"));
                    return;
                }
                self.bounds.push(Bound::Group(number));
                loop {
                    match self.next() {
                        0 => break,
                        -1 => return,
                        _ => self.skip(),
                    }
                }
            }
            _ => {}
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pb_reader(bytes: *mut ArrayHeader) -> *mut NativeObject {
    let input = unsafe { array_bytes(bytes) };
    native::new(Mutex::new(Reader {
        input,
        position: 0,
        bounds: Vec::new(),
        number: 0,
        wire: 0,
        start: 0,
        error: None,
    }))
}

/// The number of the next field, 0 at the end of the message or group
/// being read, or -1 when the input is not protobuf.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_next(object: *mut NativeObject) -> i64 {
    reader(object).map_or(-1, |mut r| r.next())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_varint(object: *mut NativeObject) -> i64 {
    let Some(mut r) = reader(object) else { return 0 };
    if !r.expect(VARINT, "a varint") {
        return 0;
    }
    r.varint().unwrap_or(0) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_zigzag(object: *mut NativeObject) -> i64 {
    let value = tondo_rt_pb_read_varint(object) as u64;
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_fixed32(object: *mut NativeObject) -> i64 {
    let Some(mut r) = reader(object) else { return 0 };
    if !r.expect(FIXED32, "4 bytes") {
        return 0;
    }
    r.take(4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_fixed64(object: *mut NativeObject) -> i64 {
    let Some(mut r) = reader(object) else { return 0 };
    if !r.expect(FIXED64, "8 bytes") {
        return 0;
    }
    r.take(8).map_or(0, |b| i64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

fn read_length(object: *mut NativeObject) -> Vec<u8> {
    let Some(mut r) = reader(object) else { return Vec::new() };
    if !r.expect(LENGTH, "a length-delimited value") {
        return Vec::new();
    }
    let Some(length) = r.varint() else { return Vec::new() };
    r.take(length as usize).map(<[u8]>::to_vec).unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_bytes(object: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(&read_length(object))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_read_text(object: *mut NativeObject) -> *mut Str {
    let bytes = read_length(object);
    match std::str::from_utf8(&bytes) {
        Ok(text) => str_new(text.as_bytes()),
        Err(_) => {
            if let Some(mut r) = reader(object) {
                r.fail("a string is not UTF-8");
            }
            str_new(b"")
        }
    }
}

/// Enters the field read: a nested message or a map entry, whose fields
/// `next` reads until it gives 0; a group, until its end; or, with
/// `packed`, the elements of a packed field, while `more`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_enter(object: *mut NativeObject, packed: u8) {
    let Some(mut r) = reader(object) else { return };
    if r.bounds.len() >= DEPTH {
        r.fail(format!("messages nest deeper than {DEPTH}"));
        return;
    }
    if r.wire == START_GROUP && packed == 0 {
        let number = r.number;
        r.bounds.push(Bound::Group(number));
        return;
    }
    if !r.expect(LENGTH, "a length-delimited value") {
        return;
    }
    let Some(length) = r.varint() else { return };
    let end = r.position.saturating_add(length as usize);
    if end > r.end() {
        r.fail(format!("a field of {length} bytes goes past the end of its message"));
        return;
    }
    r.bounds.push(Bound::Length { end, packed: packed != 0 });
}

/// Whether the field read holds its values packed, length-delimited.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_is_packed(object: *mut NativeObject) -> u8 {
    reader(object).is_some_and(|r| r.wire == LENGTH) as u8
}

/// Whether a packed field has more elements; at its end, leaves it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_more(object: *mut NativeObject) -> u8 {
    let Some(mut r) = reader(object) else { return 0 };
    if r.error.is_some() {
        return 0;
    }
    match r.bounds.last() {
        Some(&Bound::Length { end, packed: true }) if r.position < end => 1,
        Some(Bound::Length { packed: true, .. }) => {
            r.bounds.pop();
            0
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_skip(object: *mut NativeObject) {
    if let Some(mut r) = reader(object) {
        r.skip();
    }
}

/// The field read as it is written, its tag with its value, for a message
/// that keeps the fields it does not know.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_raw(object: *mut NativeObject) -> *mut ArrayHeader {
    let Some(mut r) = reader(object) else { return byte_array(&[]) };
    let start = r.start;
    r.skip();
    let end = r.position;
    byte_array(&r.input[start..end])
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_failed(object: *mut NativeObject) -> u8 {
    reader(object).is_none_or(|r| r.error.is_some()) as u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pb_fail(object: *mut NativeObject, message: *const Str) {
    let message = String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned();
    if let Some(mut r) = reader(object) {
        r.fail(message);
    }
}

/// Why reading failed, or empty.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_error(object: *mut NativeObject) -> *mut Str {
    let message = reader(object).map_or("not a reader".to_string(), |r| r.error.clone().unwrap_or_default());
    str_new(message.as_bytes())
}

/// The byte where the field read starts.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pb_offset(object: *mut NativeObject) -> i64 {
    reader(object).map_or(0, |r| r.start as i64)
}
