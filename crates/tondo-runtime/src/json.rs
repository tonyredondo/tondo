//! The JSON tokenizer and emitter behind `std.json` (standard library
//! 14.10), which is written in Tondo on top of them. RFC 8259, strict: one
//! root value, no trailing data, no duplicate keys, bounded depth.

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
const ERROR: i64 = -1;

/// Where the reader is inside a container.
enum Frame {
    Array { index: usize, started: bool },
    Object { keys: HashSet<String>, key: Option<String>, expect_key: bool, started: bool },
}

struct Reader {
    input: Vec<u8>,
    position: usize,
    frames: Vec<Frame>,
    /// The text of the last string, key or number.
    text: String,
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
    fn fail(&mut self, message: impl Into<String>) -> i64 {
        if self.error.is_none() {
            self.error = Some((message.into(), self.position));
        }
        ERROR
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.input.get(self.position) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    /// The JSON path of the value at the current position.
    fn path(&self) -> String {
        let mut path = String::from("$");
        for frame in &self.frames {
            match frame {
                Frame::Array { index, started: true } => write!(path, "[{}]", index.saturating_sub(1)).unwrap(),
                Frame::Object { key: Some(key), .. } => {
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

    fn line_column(&self, offset: usize) -> (usize, usize) {
        let before = &self.input[..offset.min(self.input.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let start = before.iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
        let column = String::from_utf8_lossy(&before[start..]).chars().count() + 1;
        (line, column)
    }

    fn next(&mut self) -> i64 {
        if self.error.is_some() {
            return ERROR;
        }
        if self.done {
            return END;
        }
        self.skip_space();
        // Close containers, and separate and key their members.
        let (array, expect_key, started) = match self.frames.last() {
            Some(Frame::Array { started, .. }) => (true, false, *started),
            Some(Frame::Object { expect_key, started, .. }) => (false, *expect_key, *started),
            None if self.root_seen => {
                if self.position < self.input.len() {
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
                self.frames.pop();
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
            let name = match self.string() {
                Ok(name) => name,
                Err(message) => return self.fail(message),
            };
            self.skip_space();
            if self.peek() != Some(b':') {
                return self.fail("expected `:` after a key");
            }
            self.position += 1;
            let duplicates = self.duplicates;
            let Some(Frame::Object { keys, key, expect_key, started }) = self.frames.last_mut() else {
                unreachable!("an object is open")
            };
            let repeated = !keys.insert(name.clone()) && !duplicates;
            *key = Some(name.clone());
            *started = true;
            *expect_key = false;
            if repeated {
                return self.fail(format!("duplicate key {name:?}"));
            }
            self.text = name;
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
        self.start = self.position;
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
                    let frame = Frame::Object { keys: HashSet::new(), key: None, expect_key: true, started: false };
                    self.frames.push(frame);
                    BEGIN_OBJECT
                } else {
                    self.frames.push(Frame::Array { index: 0, started: false });
                    BEGIN_ARRAY
                }
            }
            b'"' => match self.string() {
                Ok(text) => {
                    self.text = text;
                    self.after_value(STRING)
                }
                Err(message) => self.fail(message),
            },
            b't' | b'f' | b'n' => {
                let (word, event) = match byte {
                    b't' => ("true", TRUE),
                    b'f' => ("false", FALSE),
                    _ => ("null", NULL),
                };
                if self.input[self.position..].starts_with(word.as_bytes()) {
                    self.position += word.len();
                    self.after_value(event)
                } else {
                    self.fail("unexpected character")
                }
            }
            b'-' | b'0'..=b'9' => match self.number() {
                Ok(text) => {
                    self.text = text;
                    self.after_value(NUMBER)
                }
                Err(message) => self.fail(message),
            },
            _ => self.fail("unexpected character"),
        }
    }

    fn number(&mut self) -> Result<String, &'static str> {
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
        Ok(String::from_utf8_lossy(&self.input[start..self.position]).into_owned())
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        let digits = self.input.get(self.position..self.position + 4).ok_or("invalid escape")?;
        let text = std::str::from_utf8(digits).map_err(|_| "invalid escape")?;
        let value = u32::from_str_radix(text, 16).map_err(|_| "invalid escape")?;
        self.position += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, &'static str> {
        self.position += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(byte) = self.peek() else { return Err("unterminated string") };
            self.position += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|_| "invalid UTF-8 in string"),
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
                                if !self.input[self.position..].starts_with(b"\\u") {
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
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let reader = Reader {
        input: unsafe { str_bytes(text) }.to_vec(),
        position: 0,
        frames: Vec::new(),
        text: String::new(),
        done: false,
        root_seen: false,
        error: None,
        duplicates: duplicates != 0,
        start: 0,
    };
    Box::into_raw(Box::new(ReaderObject { rc: AtomicUsize::new(1), reader: Mutex::new(reader) }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_next(object: *mut ReaderObject) -> i64 {
    reader(object).next()
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

/// The text of the value whose first event was `event`, skipping the rest
/// of it; empty after a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_raw(object: *mut ReaderObject, event: i64) -> *mut Str {
    let mut r = reader(object);
    let begin = r.start;
    if r.skip(event) < 0 {
        return str_new(b"");
    }
    let text = r.input[begin..r.position].to_vec();
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
    let offset = r.error.as_ref().map_or(r.position, |(_, at)| *at);
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
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
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
    /// Per open container: whether it has members yet, and whether a key was
    /// just written.
    open: Vec<(bool, bool)>,
    /// Canonical output: the containers being built, and finished roots.
    stack: Vec<(Tree, Option<String>)>,
    root: Option<Tree>,
    pending_key: Option<String>,
}

#[repr(C)]
pub struct WriterObject {
    rc: AtomicUsize,
    writer: Mutex<Writer>,
}

fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
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
        self.separator();
        self.out.push_str(&text);
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
        quote(&mut self.out, name);
        self.out.push_str(if self.pretty { ": " } else { ":" });
        if let Some((_, keyed)) = self.open.last_mut() {
            *keyed = true;
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
                            quote(out, key);
                            out.push(':');
                            emit(value, out);
                        }
                        out.push('}');
                    }
                }
            }
            emit(&root, &mut self.out);
        }
        std::mem::take(&mut self.out)
    }
}

fn writer<'a>(object: *mut WriterObject) -> std::sync::MutexGuard<'a, Writer> {
    unsafe { (*object).writer.lock().unwrap_or_else(|e| e.into_inner()) }
}

/// `jsonnative.writer(pretty, canonical)`
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_json_writer(pretty: u8, canonical: u8) -> *mut WriterObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let writer = Writer {
        out: String::new(),
        pretty: pretty != 0 && canonical == 0,
        canonical: canonical != 0,
        open: Vec::new(),
        stack: Vec::new(),
        root: None,
        pending_key: None,
    };
    Box::into_raw(Box::new(WriterObject { rc: AtomicUsize::new(1), writer: Mutex::new(writer) }))
}

/// Writes a scalar: 0 null, 1 true, 2 false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_word(object: *mut WriterObject, word: i64) {
    let text = ["null", "true", "false"][word as usize];
    writer(object).leaf(text.into());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_int(object: *mut WriterObject, value: i64) {
    writer(object).leaf(value.to_string());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_json_write_float(object: *mut WriterObject, value: f64) {
    writer(object).leaf(float_text(value));
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
    let mut quoted = String::new();
    quote(&mut quoted, &text);
    writer(object).leaf(quoted);
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
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
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
    let mut r = Reader {
        input: bytes,
        position: 0,
        frames: Vec::new(),
        text: String::new(),
        done: false,
        root_seen: false,
        error: None,
        duplicates: true,
        start: 0,
    };
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
                quote(&mut quoted, &r.text);
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
