//! The JSON tokenizer and emitter behind `std.json` (standard library
//! 14.10), which is written in Tondo on top of them. RFC 8259, strict: one
//! root value, no trailing data, no duplicate keys, bounded depth.

use super::io::{StreamObject, retain, run, tondo_rt_stream_release};
use super::{LIVE_OBJECTS, Str, str_bytes, str_new};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_DEPTH: usize = 128;

/// Events, as `next()` returns them.
const END: i64 = 0;
const NULL: i64 = 1;
const TRUE: i64 = 2;
const FALSE: i64 = 3;
const NUMBER: i64 = 4;
const STRING: i64 = 5;
const BEGIN_ARRAY: i64 = 6;
const END_ARRAY: i64 = 7;
const BEGIN_OBJECT: i64 = 8;
const END_OBJECT: i64 = 9;
const KEY: i64 = 10;
/// A number that is an integer fitting `Int`, as `next_event` tells it.
const INTEGER: i64 = 11;
const ERROR: i64 = -1;

/// Where the reader is inside a container.
enum Frame {
    Array {
        index: usize,
        started: bool,
    },
    /// Where its keys start among the reader's, the current one last;
    /// past a few, also in a set, to find a repeated one; and a bit for
    /// each of them, which a key whose bit is not set cannot repeat.
    Object {
        first: usize,
        set: Option<HashSet<String>>,
        bits: u64,
        expect_key: bool,
        started: bool,
    },
}

/// How many keys an object has before the reader looks them up in a set.
const FEW_KEYS: usize = 16;

/// A stream of `std.io` the reader pulls its input from.
struct Source(*mut StreamObject);

unsafe impl Send for Source {}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe { tondo_rt_stream_release(self.0) }
    }
}

/// Bytes read from a stream per refill, and how much consumed input a
/// stream reader keeps before it lets it go.
const CHUNK: usize = 64 * 1024;

struct Reader {
    /// The input not let go yet; `offset` is where it starts in the whole.
    input: Vec<u8>,
    offset: usize,
    position: usize,
    /// More input, read as parsing needs it, and whether it ended.
    source: Option<Source>,
    exhausted: bool,
    /// The lines that ended in input already let go, and the characters
    /// since the last of them, for line and column numbers.
    lines_before: usize,
    column_carry: usize,
    /// Where a value whose text is being kept started.
    pinned: Option<usize>,
    frames: Vec<Frame>,
    /// The text of the last string, key or number.
    text: String,
    /// The keys of the objects open, one after another, and where each
    /// ends: one buffer for them all rather than a string each.
    keys: String,
    key_ends: Vec<usize>,
    /// The last number, when it is an integer that fits `Int`.
    integer: Option<i64>,
    done: bool,
    root_seen: bool,
    error: Option<(String, usize)>,
    /// Whether a later key that repeats an earlier one is an error.
    duplicates: bool,
    /// Where the value of the last event started.
    start: usize,
}

#[repr(C)]
pub struct ReaderObject {
    rc: AtomicUsize,
    reader: Mutex<Reader>,
}

impl Reader {
    fn new(input: Vec<u8>, source: Option<Source>, duplicates: bool) -> Reader {
        Reader {
            input,
            offset: 0,
            position: 0,
            exhausted: source.is_none(),
            source,
            lines_before: 0,
            column_carry: 0,
            pinned: None,
            frames: Vec::with_capacity(8),
            // Room for a usual document's text and keys from the start,
            // rather than a few allocations to grow into it.
            text: String::with_capacity(64),
            keys: String::with_capacity(128),
            key_ends: Vec::with_capacity(16),
            integer: None,
            done: false,
            root_seen: false,
            error: None,
            duplicates,
            start: 0,
        }
    }

    fn fail(&mut self, message: impl Into<String>) -> i64 {
        if self.error.is_none() {
            self.error = Some((message.into(), self.offset + self.position));
        }
        ERROR
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.position += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        if self.position >= self.input.len() {
            self.ensure(1);
        }
        self.input.get(self.position).copied()
    }

    /// Reads from the source until `count` bytes follow the position, or
    /// it ends; whether they do.
    fn ensure(&mut self, count: usize) -> bool {
        while self.input.len() < self.position + count && !self.exhausted {
            let Some(source) = &self.source else {
                self.exhausted = true;
                break;
            };
            match run(source.0, |s| s.read(CHUNK)) {
                Ok(Some(chunk)) => self.input.extend_from_slice(&chunk),
                Ok(None) => self.exhausted = true,
                Err(error) => {
                    self.exhausted = true;
                    self.fail(format!("cannot read the stream: {error}"));
                }
            }
        }
        self.input.len() >= self.position + count
    }

    /// Lets consumed input go, keeping what a kept value still needs.
    fn compact(&mut self) {
        if self.source.is_none() || self.position < CHUNK {
            return;
        }
        let keep = self.pinned.map_or(self.position, |p| (p - self.offset).min(self.position));
        let dropped = &self.input[..keep];
        match dropped.iter().rposition(|&b| b == b'\n') {
            Some(last) => {
                self.lines_before += dropped.iter().filter(|&&b| b == b'\n').count();
                self.column_carry = String::from_utf8_lossy(&dropped[last + 1..]).chars().count();
            }
            None => self.column_carry += String::from_utf8_lossy(dropped).chars().count(),
        }
        self.input.drain(..keep);
        self.offset += keep;
        self.position -= keep;
    }

    /// The key `at` of the objects open.
    fn key(&self, at: usize) -> &str {
        let start = if at == 0 { 0 } else { self.key_ends[at - 1] };
        &self.keys[start..self.key_ends[at]]
    }

    /// The JSON path of the value at the current position.
    fn path(&self) -> String {
        let mut path = String::from("$");
        // An object's keys end where those of the next object open start.
        let firsts: Vec<usize> = self
            .frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Object { first, .. } => Some(*first),
                _ => None,
            })
            .collect();
        let mut object = 0;
        for frame in &self.frames {
            match frame {
                Frame::Array { index, started: true } => write!(path, "[{}]", index.saturating_sub(1)).unwrap(),
                Frame::Object { first, .. } => {
                    let end = firsts.get(object + 1).copied().unwrap_or(self.key_ends.len());
                    object += 1;
                    if end == *first {
                        continue;
                    }
                    let key = self.key(end - 1);
                    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        write!(path, ".{key}").unwrap()
                    } else {
                        write!(path, "[{key:?}]").unwrap()
                    }
                }
                _ => {}
            }
        }
        path
    }

    /// The line and column of an offset in the whole input.
    fn line_column(&self, offset: usize) -> (usize, usize) {
        let before = &self.input[..offset.saturating_sub(self.offset).min(self.input.len())];
        let line = self.lines_before + before.iter().filter(|&&b| b == b'\n').count() + 1;
        let column = match before.iter().rposition(|&b| b == b'\n') {
            Some(last) => String::from_utf8_lossy(&before[last + 1..]).chars().count() + 1,
            None => self.column_carry + String::from_utf8_lossy(before).chars().count() + 1,
        };
        (line, column)
    }

    fn next(&mut self) -> i64 {
        if self.error.is_some() {
            return ERROR;
        }
        if self.done {
            return END;
        }
        self.compact();
        self.skip_space();
        // Close containers, and separate and key their members.
        let (array, expect_key, started) = match self.frames.last() {
            Some(Frame::Array { started, .. }) => (true, false, *started),
            Some(Frame::Object { expect_key, started, .. }) => (false, *expect_key, *started),
            None if self.root_seen => {
                if self.peek().is_some() {
                    return self.fail("unexpected data after the value");
                }
                self.done = true;
                return END;
            }
            None => {
                self.root_seen = true;
                return self.value();
            }
        };
        if array || expect_key {
            let close = if array { b']' } else { b'}' };
            if self.peek() == Some(close) {
                self.position += 1;
                if let Some(Frame::Object { first, .. }) = self.frames.pop() {
                    self.keys.truncate(if first == 0 { 0 } else { self.key_ends[first - 1] });
                    self.key_ends.truncate(first);
                }
                return self.after_value(if array { END_ARRAY } else { END_OBJECT });
            }
            if started {
                if self.peek() != Some(b',') {
                    return self.fail(if array { "expected `,` or `]`" } else { "expected `,` or `}`" });
                }
                self.position += 1;
                self.skip_space();
            }
        }
        if array {
            if let Some(Frame::Array { index, started }) = self.frames.last_mut() {
                *started = true;
                *index += 1;
            }
        } else if expect_key {
            if self.peek() != Some(b'"') {
                return self.fail("expected a key in quotes");
            }
            if let Err(message) = self.string() {
                return self.fail(message);
            }
            self.skip_space();
            if self.peek() != Some(b':') {
                return self.fail("expected `:` after a key");
            }
            self.position += 1;
            let Some(&Frame::Object { first, bits, .. }) = self.frames.last() else {
                unreachable!("an object is open")
            };
            let bit =
                1u64 << (self.text.bytes().fold(self.text.len() as u32, |h, b| h.wrapping_mul(31) ^ b as u32) % 64);
            if let Some(Frame::Object { bits, .. }) = self.frames.last_mut() {
                *bits |= bit;
            }
            let repeated = if self.duplicates || bits & bit == 0 && self.key_ends.len() - first < FEW_KEYS {
                false
            } else if self.key_ends.len() - first < FEW_KEYS {
                (first..self.key_ends.len()).any(|at| self.key(at) == self.text)
            } else {
                let mut set = match self.frames.last_mut() {
                    Some(Frame::Object { set, .. }) => set.take(),
                    _ => None,
                }
                .unwrap_or_else(|| (first..self.key_ends.len()).map(|at| self.key(at).to_string()).collect());
                let repeated = !set.insert(self.text.clone());
                if let Some(Frame::Object { set: kept, .. }) = self.frames.last_mut() {
                    *kept = Some(set);
                }
                repeated
            };
            self.keys.push_str(&self.text);
            self.key_ends.push(self.keys.len());
            if let Some(Frame::Object { expect_key, started, .. }) = self.frames.last_mut() {
                *started = true;
                *expect_key = false;
            }
            if repeated {
                let message = format!("duplicate key {:?}", self.text);
                return self.fail(message);
            }
            return KEY;
        }
        self.value()
    }

    /// Records that a member value ended.
    fn after_value(&mut self, event: i64) -> i64 {
        if let Some(Frame::Object { expect_key, .. }) = self.frames.last_mut() {
            *expect_key = true;
        }
        event
    }

    fn value(&mut self) -> i64 {
        self.skip_space();
        self.start = self.offset + self.position;
        let Some(byte) = self.peek() else {
            return self.fail("unexpected end of input");
        };
        match byte {
            b'{' | b'[' => {
                if self.frames.len() >= MAX_DEPTH {
                    return self.fail(format!("nested deeper than {MAX_DEPTH} levels"));
                }
                self.position += 1;
                if byte == b'{' {
                    let frame = Frame::Object {
                        first: self.key_ends.len(),
                        set: None,
                        bits: 0,
                        expect_key: true,
                        started: false,
                    };
                    self.frames.push(frame);
                    BEGIN_OBJECT
                } else {
                    self.frames.push(Frame::Array { index: 0, started: false });
                    BEGIN_ARRAY
                }
            }
            b'"' => match self.string() {
                Ok(()) => self.after_value(STRING),
                Err(message) => self.fail(message),
            },
            b't' | b'f' | b'n' => {
                let (word, event) = match byte {
                    b't' => ("true", TRUE),
                    b'f' => ("false", FALSE),
                    _ => ("null", NULL),
                };
                if self.ensure(word.len()) && self.input[self.position..].starts_with(word.as_bytes()) {
                    self.position += word.len();
                    self.after_value(event)
                } else {
                    self.fail("unexpected character")
                }
            }
            b'-' | b'0'..=b'9' => match self.number() {
                Ok(()) => {
                    let integral = !self.text.contains(['.', 'e', 'E']);
                    self.integer = if integral { self.text.parse().ok() } else { None };
                    self.after_value(NUMBER)
                }
                Err(message) => self.fail(message),
            },
            _ => self.fail("unexpected character"),
        }
    }

    /// A number, into `text`.
    fn number(&mut self) -> Result<(), &'static str> {
        let start = self.position;
        let digits = |r: &mut Reader| {
            let begin = r.position;
            while let Some(b'0'..=b'9') = r.peek() {
                r.position += 1;
            }
            r.position - begin
        };
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err("invalid number"),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if digits(self) == 0 {
                return Err("invalid number: digits must follow `.`");
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.position += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.position += 1;
            }
            if digits(self) == 0 {
                return Err("invalid number: digits must follow the exponent");
            }
        }
        self.text.clear();
        self.text.push_str(std::str::from_utf8(&self.input[start..self.position]).unwrap_or_default());
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        self.ensure(4);
        let digits = self.input.get(self.position..self.position + 4).ok_or("invalid escape")?;
        let text = std::str::from_utf8(digits).map_err(|_| "invalid escape")?;
        let value = u32::from_str_radix(text, 16).map_err(|_| "invalid escape")?;
        self.position += 4;
        Ok(value)
    }

    /// A string, into `text`, reusing its buffer.
    fn string(&mut self) -> Result<(), &'static str> {
        let mut out = std::mem::take(&mut self.text).into_bytes();
        out.clear();
        let read = self.string_into(&mut out);
        // Text read from a string is UTF-8 already: a run of it ends at an
        // ASCII byte, and an escape adds a whole character.
        let valid = read.and_then(|()| match self.source {
            None => Ok(()),
            Some(_) => std::str::from_utf8(&out).map(|_| ()).map_err(|_| "invalid UTF-8 in string"),
        });
        if valid.is_err() {
            out.clear();
        }
        // Valid UTF-8, or empty.
        self.text = unsafe { String::from_utf8_unchecked(out) };
        valid
    }

    fn string_into(&mut self, out: &mut Vec<u8>) -> Result<(), &'static str> {
        self.position += 1;
        loop {
            // The bytes that need nothing done, at once.
            let rest = &self.input[self.position.min(self.input.len())..];
            let run = rest.iter().position(|&b| b == b'"' || b == b'\\' || b < 0x20).unwrap_or(rest.len());
            out.extend_from_slice(&rest[..run]);
            self.position += run;
            let Some(byte) = self.peek() else { return Err("unterminated string") };
            self.position += 1;
            match byte {
                b'"' => return Ok(()),
                b'\\' => {
                    let Some(escape) = self.peek() else { return Err("unterminated string") };
                    self.position += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if !(self.ensure(2) && self.input[self.position..].starts_with(b"\\u")) {
                                    return Err("unpaired surrogate in escape");
                                }
                                self.position += 2;
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err("unpaired surrogate in escape");
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err("unpaired surrogate in escape");
                            } else {
                                first
                            };
                            char::from_u32(code).ok_or("invalid escape")?
                        }
                        _ => return Err("invalid escape"),
                    };
                    let mut buffer = [0; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                0..0x20 => return Err("control character in string"),
                _ => out.push(byte),
            }
        }
    }

    /// Skips the rest of the value whose first event was `event`.
    fn skip(&mut self, event: i64) -> i64 {
        if event != BEGIN_ARRAY && event != BEGIN_OBJECT {
            return 0;
        }
        let depth = self.frames.len();
        loop {
            match self.next() {
                ERROR => return ERROR,
                END => return self.fail("unexpected end of input"),
                END_ARRAY | END_OBJECT if self.frames.len() < depth => return 0,
                _ => {}
            }
        }
    }
}

fn reader<'a>(object: *mut ReaderObject) -> std::sync::MutexGuard<'a, Reader> {
    unsafe { (*object).reader.lock().unwrap_or_else(|e| e.into_inner()) }
}

/// `jsonnative.reader(text, duplicates)`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_reader(text: *const Str, duplicates: u8) -> *mut ReaderObject {
    let reader = Reader::new(unsafe { str_bytes(text) }.to_vec(), None, duplicates != 0);
    reader_object(reader)
}

/// `jsonnative.streamReader(stream, duplicates)`: reads the stream as the
/// document needs it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_stream_reader(stream: *mut StreamObject, duplicates: u8) -> *mut ReaderObject {
    reader_object(Reader::new(Vec::new(), Some(Source(retain(stream))), duplicates != 0))
}

fn reader_object(reader: Reader) -> *mut ReaderObject {
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(ReaderObject { rc: AtomicUsize::new(1), reader: Mutex::new(reader) }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_next(object: *mut ReaderObject) -> i64 {
    reader(object).next()
}

/// The next event as `next`, but `INTEGER` for a number that is an
/// integer fitting `Int`, whose value `integer` gives without its text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_next_event(object: *mut ReaderObject) -> i64 {
    let mut reader = reader(object);
    match reader.next() {
        NUMBER if reader.integer.is_some() => INTEGER,
        event => event,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_integer(object: *mut ReaderObject) -> i64 {
    reader(object).integer.unwrap_or(0)
}

/// The text of the last string, key or number.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_text(object: *mut ReaderObject) -> *mut Str {
    str_new(reader(object).text.as_bytes())
}

/// Skips the rest of a value whose first event was `event`; 0 or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_skip(object: *mut ReaderObject, event: i64) -> i64 {
    reader(object).skip(event)
}

/// What `field` gives at the end of an object, and for a member none of
/// the names is when unknown fields are rejected.
const FIELDS_END: i64 = -2;
const UNKNOWN: i64 = -3;

/// `reader.field(names, reject)`: the next member of an object, read up to
/// its value, as a derived decoder numbers its fields: the index of its key
/// among `names`, compared in place, with no string made for it. A member
/// none of them is skipped whole and gives the number of names, or, when
/// `reject`, fails after its value's first token, as `unknownField` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_field(
    object: *mut ReaderObject,
    names: *mut crate::ArrayHeader,
    reject: u8,
) -> i64 {
    let mut r = reader(object);
    match r.next() {
        KEY => {}
        END_OBJECT => return FIELDS_END,
        ERROR => return ERROR,
        _ => return r.fail("expected a key"),
    }
    let count = unsafe { (*names).len };
    for index in 0..count {
        let name = unsafe { str_bytes(*(crate::element(names, 8, index) as *const *const Str)) };
        if name == r.text.as_bytes() {
            return index as i64;
        }
    }
    let key = (reject != 0).then(|| r.text.clone());
    let first = r.next();
    if first == ERROR {
        return ERROR;
    }
    if let Some(key) = key {
        r.fail(format!("unknown field `{key}`"));
        return UNKNOWN;
    }
    match r.skip(first) {
        ERROR => ERROR,
        _ => count as i64,
    }
}

/// The text of the value whose first event was `event`, skipping the rest
/// of it; empty after a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_raw(object: *mut ReaderObject, event: i64) -> *mut Str {
    let mut r = reader(object);
    let begin = r.start;
    r.pinned = Some(begin);
    let skipped = r.skip(event);
    r.pinned = None;
    if skipped < 0 {
        return str_new(b"");
    }
    let text = r.input[begin - r.offset..r.position].to_vec();
    str_new(&text)
}

/// Records a failure the caller found at the current position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_fail(object: *mut ReaderObject, message: *const Str) {
    let message = String::from_utf8_lossy(unsafe { str_bytes(message) }).into_owned();
    reader(object).fail(message);
}

/// The failure, if any: its message; or the path, line and column of the
/// current position (`which` 0, 1, 2, 3).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_where(object: *mut ReaderObject, which: i64) -> *mut Str {
    let r = reader(object);
    let offset = r.error.as_ref().map_or(r.offset + r.position, |(_, at)| *at);
    let (line, column) = r.line_column(offset);
    let text = match which {
        0 => r.error.as_ref().map_or(String::new(), |(message, _)| message.clone()),
        1 => r.path(),
        2 => line.to_string(),
        _ => column.to_string(),
    };
    str_new(text.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_reader_release(object: *mut ReaderObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        if (*object).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(object));
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// A value kept for canonical output, which sorts keys at the end.
enum Tree {
    Leaf(String),
    Array(Vec<Tree>),
    Object(Vec<(String, Tree)>),
}

struct Writer {
    out: String,
    pretty: bool,
    canonical: bool,
    /// `htmlSafe`: `<`, `>`, `&`, U+2028 and U+2029 escaped, so the output
    /// can go inside an HTML page's script.
    html: bool,
    /// Per open container: whether it has members yet, and whether a key was
    /// just written.
    open: Vec<(bool, bool)>,
    /// Canonical output: the containers being built, and finished roots.
    stack: Vec<(Tree, Option<String>)>,
    root: Option<Tree>,
    pending_key: Option<String>,
    /// The stream output goes to as it grows, and its first failure.
    sink: Option<Source>,
    failed: Option<String>,
}

#[repr(C)]
pub struct WriterObject {
    rc: AtomicUsize,
    writer: Mutex<Writer>,
}

/// A text as a JSON string; with `html`, also safe inside an HTML page.
fn quote(out: &mut String, text: &str, html: bool) {
    out.push('"');
    // The runs that need no escape go in whole.
    let mut run = 0;
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let (escape, length) = match byte {
            b'"' => ("\\\"", 1),
            b'\\' => ("\\\\", 1),
            b'\n' => ("\\n", 1),
            b'\r' => ("\\r", 1),
            b'\t' => ("\\t", 1),
            0x08 => ("\\b", 1),
            0x0c => ("\\f", 1),
            0x00..=0x1f => ("", 1),
            b'<' if html => ("\\u003c", 1),
            b'>' if html => ("\\u003e", 1),
            b'&' if html => ("\\u0026", 1),
            // U+2028 and U+2029, which end a line of JavaScript.
            0xe2 if html && bytes.get(at + 1) == Some(&0x80) && matches!(bytes.get(at + 2), Some(0xa8 | 0xa9)) => {
                (if bytes[at + 2] == 0xa8 { "\\u2028" } else { "\\u2029" }, 3)
            }
            _ => {
                at += 1;
                continue;
            }
        };
        out.push_str(&text[run..at]);
        if escape.is_empty() {
            write!(out, "\\u{byte:04x}").unwrap();
        } else {
            out.push_str(escape);
        }
        at += length;
        run = at;
    }
    out.push_str(&text[run..]);
    out.push('"');
}

/// A float as JSON: shortest round trip, `null` when it is not finite.
fn float_text(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    // ECMAScript's number to string, which RFC 8785 uses.
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if (1e-7..1e21).contains(&magnitude) {
        let text = format!("{value}");
        text.strip_suffix(".0").map(String::from).unwrap_or(text)
    } else {
        let text = format!("{value:e}");
        let (mantissa, exponent) = text.split_once('e').unwrap();
        let sign = if exponent.starts_with('-') { "" } else { "+" };
        format!("{mantissa}e{sign}{exponent}")
    }
}

impl Writer {
    fn separator(&mut self) {
        let indent = self.open.len();
        if let Some((members, keyed)) = self.open.last_mut() {
            if *keyed {
                *keyed = false;
                return;
            }
            if *members {
                self.out.push(',');
            }
            *members = true;
            if self.pretty {
                self.out.push('\n');
                self.out.push_str(&"  ".repeat(indent));
            }
        }
    }

    /// A scalar, already as JSON text.
    fn leaf(&mut self, text: String) {
        if self.canonical {
            self.add(Tree::Leaf(text));
            return;
        }
        self.leaf_text(&text);
    }

    /// A leaf written as it is, without a copy of its text.
    fn leaf_text(&mut self, text: &str) {
        if self.canonical {
            return self.add(Tree::Leaf(text.to_string()));
        }
        self.separator();
        self.out.push_str(text);
    }

    fn add(&mut self, tree: Tree) {
        let key = self.pending_key.take();
        match self.stack.last_mut() {
            Some((Tree::Array(items), _)) => items.push(tree),
            Some((Tree::Object(members), _)) => members.push((key.unwrap_or_default(), tree)),
            _ => self.root = Some(tree),
        }
    }

    fn begin(&mut self, object: bool) {
        if self.canonical {
            let key = self.pending_key.take();
            let tree = if object { Tree::Object(Vec::new()) } else { Tree::Array(Vec::new()) };
            self.stack.push((tree, key));
            return;
        }
        self.separator();
        self.out.push(if object { '{' } else { '[' });
        self.open.push((false, false));
    }

    fn end(&mut self, object: bool) {
        if self.canonical {
            if let Some((tree, key)) = self.stack.pop() {
                self.pending_key = key;
                self.add(tree);
            }
            return;
        }
        if let Some((members, _)) = self.open.pop()
            && members
            && self.pretty
        {
            self.out.push('\n');
            self.out.push_str(&"  ".repeat(self.open.len()));
        }
        self.out.push(if object { '}' } else { ']' });
    }

    fn key(&mut self, name: &str) {
        if self.canonical {
            self.pending_key = Some(name.into());
            return;
        }
        self.separator();
        quote(&mut self.out, name, self.html);
        self.out.push_str(if self.pretty { ": " } else { ":" });
        if let Some((_, keyed)) = self.open.last_mut() {
            *keyed = true;
        }
    }

    /// Passes the output to the sink once enough gathers, or `all` of it.
    fn deliver(&mut self, all: bool) {
        let Some(sink) = &self.sink else { return };
        if self.out.is_empty() || (!all && self.out.len() < CHUNK) {
            return;
        }
        let data = std::mem::take(&mut self.out).into_bytes();
        if self.failed.is_none()
            && let Err(error) = run(sink.0, move |s| s.write(&data))
        {
            self.failed = Some(format!("cannot write the stream: {error}"));
        }
    }

    fn finish(&mut self) -> String {
        if let Some(root) = self.root.take() {
            fn emit(tree: &Tree, out: &mut String) {
                match tree {
                    Tree::Leaf(text) => out.push_str(text),
                    Tree::Array(items) => {
                        out.push('[');
                        for (index, item) in items.iter().enumerate() {
                            if index > 0 {
                                out.push(',');
                            }
                            emit(item, out);
                        }
                        out.push(']');
                    }
                    Tree::Object(members) => {
                        // RFC 8785 orders keys by their UTF-16 code units.
                        let mut members: Vec<&(String, Tree)> = members.iter().collect();
                        members.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
                        out.push('{');
                        for (index, (key, value)) in members.iter().enumerate() {
                            if index > 0 {
                                out.push(',');
                            }
                            quote(out, key, false);
                            out.push(':');
                            emit(value, out);
                        }
                        out.push('}');
                    }
                }
            }
            emit(&root, &mut self.out);
        }
        if self.sink.is_some() {
            self.deliver(true);
            if let Some(sink) = &self.sink
                && self.failed.is_none()
                && let Err(error) = run(sink.0, |s| s.flush())
            {
                self.failed = Some(format!("cannot write the stream: {error}"));
            }
        }
        std::mem::take(&mut self.out)
    }
}

/// The writer, after the previous operation, which may have grown the
/// output enough to pass it to a sink.
struct Guard<'a>(std::sync::MutexGuard<'a, Writer>);

impl std::ops::Deref for Guard<'_> {
    type Target = Writer;
    fn deref(&self) -> &Writer {
        &self.0
    }
}

impl std::ops::DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut Writer {
        &mut self.0
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.deliver(false);
    }
}

fn writer<'a>(object: *mut WriterObject) -> Guard<'a> {
    Guard(unsafe { (*object).writer.lock().unwrap_or_else(|e| e.into_inner()) })
}

fn writer_object(pretty: u8, canonical: u8, html: u8, sink: Option<Source>) -> *mut WriterObject {
    LIVE_OBJECTS.made();
    let writer = Writer {
        out: String::new(),
        pretty: pretty != 0 && canonical == 0,
        canonical: canonical != 0,
        // Canonical output has one way to write each text.
        html: html != 0 && canonical == 0,
        open: Vec::new(),
        stack: Vec::new(),
        root: None,
        pending_key: None,
        sink,
        failed: None,
    };
    Box::into_raw(Box::new(WriterObject { rc: AtomicUsize::new(1), writer: Mutex::new(writer) }))
}

/// `jsonnative.writer(pretty, canonical, htmlSafe)`
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_json_writer(pretty: u8, canonical: u8, html: u8) -> *mut WriterObject {
    writer_object(pretty, canonical, html, None)
}

/// `jsonnative.streamWriter(stream, pretty, canonical, htmlSafe)`: passes
/// the output to the stream as it grows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_stream_writer(
    stream: *mut StreamObject,
    pretty: u8,
    canonical: u8,
    html: u8,
) -> *mut WriterObject {
    writer_object(pretty, canonical, html, Some(Source(retain(stream))))
}

/// The first failure to write a stream writer's stream, or empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_writer_error(object: *mut WriterObject) -> *mut Str {
    str_new(writer(object).failed.clone().unwrap_or_default().as_bytes())
}

/// Writes a scalar: 0 null, 1 true, 2 false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_word(object: *mut WriterObject, word: i64) {
    let text = ["null", "true", "false"][word as usize];
    writer(object).leaf_text(text);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_int(object: *mut WriterObject, value: i64) {
    let mut digits = [0u8; 20];
    writer(object).leaf_text(decimal(value, &mut digits));
}

/// The decimal digits of an integer, written from the end of `buffer`.
fn decimal(value: i64, buffer: &mut [u8; 20]) -> &str {
    let mut magnitude = value.unsigned_abs();
    let mut at = buffer.len();
    loop {
        at -= 1;
        buffer[at] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        buffer[at] = b'-';
    }
    std::str::from_utf8(&buffer[at..]).unwrap_or("0")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_float(object: *mut WriterObject, value: f64) {
    let mut w = writer(object);
    // A float of the usual size is written from the stack, without a
    // string of its own for each.
    if !w.canonical && (1e-7..1e21).contains(&value.abs()) {
        let mut text = Short { bytes: [0; 32], length: 0 };
        if write!(text, "{value}").is_ok() {
            return w.leaf_text(text.as_str());
        }
    }
    w.leaf(float_text(value));
}

/// Up to 32 bytes of text on the stack, which `write!` fills.
struct Short {
    bytes: [u8; 32],
    length: usize,
}

impl Short {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.length]).unwrap_or("0")
    }
}

impl std::fmt::Write for Short {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let end = self.length + text.len();
        self.bytes.get_mut(self.length..end).ok_or(std::fmt::Error)?.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

/// A number already validated as JSON text; canonical output rewrites it as
/// a double, as RFC 8785 requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_number(object: *mut WriterObject, text: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned();
    let mut w = writer(object);
    let text = if w.canonical { float_text(text.parse().unwrap_or(f64::NAN)) } else { text };
    w.leaf(text);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_string(object: *mut WriterObject, text: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(text) });
    let mut w = writer(object);
    if w.canonical {
        let mut quoted = String::new();
        quote(&mut quoted, &text, false);
        return w.leaf(quoted);
    }
    // Quoted straight into the output.
    w.separator();
    let html = w.html;
    quote(&mut w.out, &text, html);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_key(object: *mut WriterObject, text: *const Str) {
    let text = String::from_utf8_lossy(unsafe { str_bytes(text) });
    writer(object).key(&text);
}

/// Opens (`begin` 1) or closes an object (`object` 1) or an array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_bracket(object: *mut WriterObject, begin: u8, is_object: u8) {
    let mut w = writer(object);
    if begin != 0 { w.begin(is_object != 0) } else { w.end(is_object != 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_finish(object: *mut WriterObject) -> *mut Str {
    str_new(writer(object).finish().as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_writer_release(object: *mut WriterObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        if (*object).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(object));
        LIVE_OBJECTS.gone();
    }
}

/// A JSON text validated before: as it is, or as its canonical form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_raw(object: *mut WriterObject, text: *const Str) {
    let bytes = unsafe { str_bytes(text) }.to_vec();
    let mut w = writer(object);
    if !w.canonical {
        w.leaf(String::from_utf8_lossy(&bytes).into_owned());
        return;
    }
    // Replayed event by event, so that keys sort and numbers normalize.
    let mut r = Reader::new(bytes, None, true);
    loop {
        match r.next() {
            NULL => w.leaf("null".into()),
            TRUE => w.leaf("true".into()),
            FALSE => w.leaf("false".into()),
            NUMBER => {
                let text = float_text(r.text.parse().unwrap_or(f64::NAN));
                w.leaf(text)
            }
            STRING => {
                let mut quoted = String::new();
                quote(&mut quoted, &r.text, w.html);
                w.leaf(quoted)
            }
            BEGIN_ARRAY => w.begin(false),
            END_ARRAY => w.end(false),
            BEGIN_OBJECT => w.begin(true),
            END_OBJECT => w.end(true),
            KEY => {
                let key = r.text.clone();
                w.key(&key)
            }
            _ => break,
        }
    }
}

/// `jsonnative.isInt(text)`: whether a JSON number is an `Int`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_is_int(text: *const Str) -> u8 {
    String::from_utf8_lossy(unsafe { str_bytes(text) }).parse::<i64>().is_ok() as u8
}

/// `jsonnative.parseInt(text)`, of a number `isInt` accepts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_parse_int(text: *const Str) -> i64 {
    String::from_utf8_lossy(unsafe { str_bytes(text) }).parse().unwrap_or(0)
}

/// `jsonnative.parseFloat(text)`: the nearest `Float`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_parse_float(text: *const Str) -> f64 {
    String::from_utf8_lossy(unsafe { str_bytes(text) }).parse().unwrap_or(f64::NAN)
}
