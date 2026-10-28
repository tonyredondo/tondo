//! YAML for `std.yaml`: a safe subset of YAML 1.2 read with `saphyr` into
//! the events of `std.serialization`, each with the path, line and column
//! it comes from; and a writer that builds documents from those events.
//!
//! Plain scalars resolve with the core schema: `null`, `~` and nothing are
//! null, `true` and `false` booleans, and numbers integers or floats; any
//! other scalar, and every quoted one, is text, dates included. Only the
//! core tags are accepted, keys are text and unique, and an alias repeats
//! its anchor's value, up to a limit that stops documents that expand
//! without end.

use crate::native::{self, NativeObject};
use crate::{Str, str_bytes, str_new};
use saphyr::{Mapping, Scalar, Yaml, YamlEmitter};
use saphyr_parser::{Event as Parsed, Parser, ScalarStyle};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;

/// The events, as `std.yaml` reads them; `END` after a document.
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
const ERROR: i64 = -1;

/// The most values a document may hold once its aliases are expanded.
const LIMIT: usize = 1_000_000;

#[derive(Clone)]
struct Event {
    code: i64,
    text: String,
    int: i64,
    float: f64,
    /// Where in the document, after `$`, as `.users[2]`.
    path: String,
    line: usize,
    column: usize,
}

struct Reader {
    documents: Vec<Vec<Event>>,
    document: usize,
    /// The event just read in the document, counted from 1.
    read: usize,
    /// A failure: its message, path, line and column.
    error: Option<(String, String, usize, usize)>,
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

fn reader(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Reader>> {
    native::get::<Mutex<Reader>>(object).map(|r| r.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A collection being read, for the paths of its values.
enum Frame {
    List(usize),
    /// A map, whether its next node is a key, its keys so far and the key
    /// of the value being read.
    Map {
        key: bool,
        keys: Vec<String>,
        current: String,
    },
}

/// Turns the parser's events into those of `std.serialization`.
struct Builder {
    documents: Vec<Vec<Event>>,
    events: Vec<Event>,
    frames: Vec<Frame>,
    /// The events of each anchored node, with paths from the node, and the
    /// anchored collections still open: their anchor, first event and path.
    anchors: HashMap<usize, Vec<Event>>,
    open: Vec<(usize, usize, String, usize)>,
}

type Failure = (String, String, usize, usize);

impl Builder {
    fn path(&self) -> String {
        let mut path = String::new();
        for frame in &self.frames {
            match frame {
                Frame::List(index) => path.push_str(&format!("[{index}]")),
                Frame::Map { current, .. } => path.push_str(&format!(".{current}")),
            }
        }
        path
    }

    fn failure(&self, message: impl Into<String>, line: usize, column: usize) -> Failure {
        (message.into(), format!("${}", self.path()), line, column)
    }

    /// Whether the next node of the innermost map is a key.
    fn at_key(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Map { key: true, .. }))
    }

    /// After a value: the map waits for a key, the list counts it.
    fn done(&mut self) {
        match self.frames.last_mut() {
            Some(Frame::List(index)) => *index += 1,
            Some(Frame::Map { key, .. }) => *key = true,
            None => {}
        }
    }

    fn push(&mut self, code: i64, line: usize, column: usize) -> Result<&mut Event, Failure> {
        if self.events.len() >= LIMIT {
            return Err(self.failure(format!("the document holds more than {LIMIT} values"), line, column));
        }
        let path = self.path();
        self.events.push(Event { code, text: String::new(), int: 0, float: 0.0, path, line, column });
        Ok(self.events.last_mut().unwrap())
    }

    /// A key of the innermost map.
    fn key(&mut self, key: String, line: usize, column: usize) -> Result<(), Failure> {
        let Some(Frame::Map { key: expecting, keys, current }) = self.frames.last_mut() else { return Ok(()) };
        if keys.contains(&key) {
            return Err(self.failure(format!("the key `{key}` appears twice in this map"), line, column));
        }
        keys.push(key.clone());
        *current = key.clone();
        *expecting = false;
        self.push(KEY, line, column)?.text = key;
        Ok(())
    }

    fn event(&mut self, parsed: Parsed, line: usize, column: usize) -> Result<(), Failure> {
        match parsed {
            Parsed::DocumentStart(_) => {
                self.events.clear();
                self.anchors.clear();
            }
            Parsed::DocumentEnd => {
                let events = std::mem::take(&mut self.events);
                self.documents.push(events);
            }
            Parsed::Scalar(value, style, anchor, tag) => {
                if self.at_key() {
                    if tag.as_ref().is_some_and(|t| !t.is_yaml_core_schema() || t.suffix != "str") {
                        return Err(self.failure("a key is text", line, column));
                    }
                    return self.key(value.into_owned(), line, column);
                }
                let first = self.events.len();
                self.scalar(&value, style, tag.as_deref(), line, column)?;
                if anchor > 0 {
                    let base = self.path().len();
                    let recorded = self.events[first..].iter().map(|e| relative(e, base)).collect();
                    self.anchors.insert(anchor, recorded);
                }
                self.done();
            }
            Parsed::Alias(anchor) => {
                if self.at_key() {
                    return Err(self.failure("a key is text, not an alias", line, column));
                }
                let Some(recorded) = self.anchors.get(&anchor).cloned() else {
                    return Err(self.failure(
                        "an alias refers to an anchor that is not complete before it",
                        line,
                        column,
                    ));
                };
                if self.events.len() + recorded.len() > LIMIT {
                    return Err(self.failure(
                        format!("its aliases expand the document beyond {LIMIT} values"),
                        line,
                        column,
                    ));
                }
                let base = self.path();
                self.events.extend(recorded.into_iter().map(|mut e| {
                    e.path = format!("{base}{}", e.path);
                    e
                }));
                self.done();
            }
            Parsed::SequenceStart(anchor, tag) => self.open(false, anchor, tag, line, column)?,
            Parsed::MappingStart(anchor, tag) => self.open(true, anchor, tag, line, column)?,
            Parsed::SequenceEnd | Parsed::MappingEnd => {
                let map = matches!(self.frames.pop(), Some(Frame::Map { .. }));
                self.push(if map { END_MAP } else { END_LIST }, line, column)?;
                if let Some(&(anchor, first, ref path, depth)) = self.open.last()
                    && depth == self.frames.len()
                {
                    let base = path.len();
                    let recorded = self.events[first..].iter().map(|e| relative(e, base)).collect();
                    self.anchors.insert(anchor, recorded);
                    self.open.pop();
                }
                self.done();
            }
            _ => {}
        }
        Ok(())
    }

    /// A list or a map starts.
    fn open(
        &mut self,
        map: bool,
        anchor: usize,
        tag: Option<Cow<'_, saphyr_parser::Tag>>,
        line: usize,
        column: usize,
    ) -> Result<(), Failure> {
        if self.at_key() {
            return Err(self.failure("a key is text, not a list or a map", line, column));
        }
        if let Some(tag) = &tag
            && (!tag.is_yaml_core_schema() || !matches!(tag.suffix.as_str(), "seq" | "map"))
        {
            return Err(self.failure(
                format!("the tag `{}{}` is not a core tag", tag.handle, tag.suffix),
                line,
                column,
            ));
        }
        let first = self.events.len();
        self.push(if map { BEGIN_MAP } else { BEGIN_LIST }, line, column)?;
        if anchor > 0 {
            self.open.push((anchor, first, self.path(), self.frames.len()));
        }
        self.frames.push(if map {
            Frame::Map { key: true, keys: Vec::new(), current: String::new() }
        } else {
            Frame::List(0)
        });
        Ok(())
    }

    /// A scalar value, resolved by its tag or, plain, by the core schema.
    fn scalar(
        &mut self,
        value: &str,
        style: ScalarStyle,
        tag: Option<&saphyr_parser::Tag>,
        line: usize,
        column: usize,
    ) -> Result<(), Failure> {
        let kind = match tag {
            Some(tag) if !tag.is_yaml_core_schema() => {
                return Err(self.failure(
                    format!("the tag `{}{}` is not a core tag", tag.handle, tag.suffix),
                    line,
                    column,
                ));
            }
            Some(tag) => tag.suffix.as_str(),
            None if style != ScalarStyle::Plain => "str",
            None => "",
        };
        let null = matches!(value, "null" | "Null" | "NULL" | "~" | "");
        let truth = match value {
            "true" | "True" | "TRUE" => Some(true),
            "false" | "False" | "FALSE" => Some(false),
            _ => None,
        };
        match kind {
            "str" => self.push(TEXT, line, column)?.text = value.to_string(),
            "null" | "" if null => {
                self.push(NULL, line, column)?;
            }
            "bool" | "" if truth.is_some() => {
                self.push(if truth == Some(true) { TRUE } else { FALSE }, line, column)?;
            }
            "int" | "float" | "" if is_number(value) => {
                if let Some(int) = integer(value) {
                    self.push(INT, line, column)?.int = int;
                } else if is_integer(value) {
                    // Too large for an `Int`: kept as written.
                    self.push(NUMBER, line, column)?.text = value.to_string();
                } else {
                    self.push(FLOAT, line, column)?.float = value.parse().unwrap_or(0.0);
                }
            }
            "float" | ""
                if matches!(
                    value.trim_start_matches(['+', '-']),
                    ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN"
                ) =>
            {
                return Err(self.failure(format!("`{value}` is not a finite number"), line, column));
            }
            "" => self.push(TEXT, line, column)?.text = value.to_string(),
            _ => return Err(self.failure(format!("`{value}` is not a `!!{kind}`"), line, column)),
        }
        Ok(())
    }
}

/// An event with its path from the node at `base` bytes of path on.
fn relative(event: &Event, base: usize) -> Event {
    let mut event = event.clone();
    event.path = event.path.get(base..).unwrap_or_default().to_string();
    event
}

fn is_integer(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// An integer of the core schema: decimal, `0o` octal or `0x` hexadecimal.
fn integer(text: &str) -> Option<i64> {
    if let Some(octal) = text.strip_prefix("0o") {
        return i64::from_str_radix(octal, 8).ok();
    }
    if let Some(hex) = text.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16).ok();
    }
    if is_integer(text) { text.parse().ok() } else { None }
}

/// A number of the core schema.
fn is_number(text: &str) -> bool {
    if integer(text).is_some() || is_integer(text) {
        return true;
    }
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let exponent_ok = exponent.is_none_or(is_integer);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    exponent_ok && digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty())
}

/// The documents of `text` as events, or the first failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_yaml_parse(source: *const Str) -> *mut NativeObject {
    let source = text(source);
    let mut builder = Builder {
        documents: Vec::new(),
        events: Vec::new(),
        frames: Vec::new(),
        anchors: HashMap::new(),
        open: Vec::new(),
    };
    let mut error = None;
    for result in Parser::new_from_str(&source) {
        let outcome = match result {
            Ok((parsed, span)) => builder.event(parsed, span.start.line(), span.start.col() + 1),
            Err(failure) => {
                let at = failure.marker();
                Err((failure.info().to_string(), format!("${}", builder.path()), at.line(), at.col() + 1))
            }
        };
        if let Err(failure) = outcome {
            error = Some(failure);
            break;
        }
    }
    let reader = Reader { documents: builder.documents, document: 0, read: 0, error };
    native::new(Mutex::new(reader))
}

/// How many documents the text holds.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_documents(object: *mut NativeObject) -> i64 {
    reader(object).map_or(0, |r| r.documents.len() as i64)
}

/// Moves to the next document; false after the last.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_next_document(object: *mut NativeObject) -> u8 {
    let Some(mut reader) = reader(object) else { return 0 };
    if reader.document + 1 >= reader.documents.len() {
        return 0;
    }
    reader.document += 1;
    reader.read = 0;
    1
}

/// The next event's code, `END` after the document, or `ERROR`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_next(object: *mut NativeObject) -> i64 {
    let Some(mut reader) = reader(object) else { return ERROR };
    if reader.error.is_some() {
        return ERROR;
    }
    let document = reader.document;
    let Some(count) = reader.documents.get(document).map(Vec::len) else { return END };
    if reader.read == count {
        return END;
    }
    reader.read += 1;
    reader.documents[document][reader.read - 1].code
}

fn current<T>(object: *mut NativeObject, part: impl Fn(&Event) -> T, empty: T) -> T {
    let Some(reader) = reader(object) else { return empty };
    let event = reader.documents.get(reader.document).and_then(|d| reader.read.checked_sub(1).and_then(|i| d.get(i)));
    event.map_or(empty, part)
}

/// The text of the event just read: a string, a key or a number as written.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_text(object: *mut NativeObject) -> *mut Str {
    str_new(current(object, |event| event.text.clone(), String::new()).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_int(object: *mut NativeObject) -> i64 {
    current(object, |event| event.int, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_float(object: *mut NativeObject) -> f64 {
    current(object, |event| event.float, 0.0)
}

/// Skips the rest of a value that started with `code`; 0, or `ERROR`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_skip(object: *mut NativeObject, code: i64) -> i64 {
    if code != BEGIN_LIST && code != BEGIN_MAP {
        return 0;
    }
    let Some(mut reader) = reader(object) else { return ERROR };
    let document = reader.document;
    let count = reader.documents.get(document).map_or(0, Vec::len);
    let mut depth = 1;
    while depth > 0 && reader.read < count {
        match reader.documents[document][reader.read].code {
            BEGIN_LIST | BEGIN_MAP => depth += 1,
            END_LIST | END_MAP => depth -= 1,
            _ => {}
        }
        reader.read += 1;
    }
    if depth == 0 { 0 } else { ERROR }
}

/// A failure at the event just read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_yaml_fail(object: *mut NativeObject, message: *const Str) {
    let message = text(message);
    let at = current(object, |e| Some((format!("${}", e.path), e.line, e.column)), None);
    let Some(mut reader) = reader(object) else { return };
    let (path, line, column) = at.unwrap_or(("$".into(), 0, 0));
    reader.error = Some((message, path, line, column));
}

/// Where the failure is: its message (0), path (1), line (2) or column (3).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_where(object: *mut NativeObject, part: i64) -> *mut Str {
    let Some(reader) = reader(object) else { return str_new(b"") };
    let Some((message, path, line, column)) = &reader.error else { return str_new(b"") };
    let shown = match part {
        0 => message.clone(),
        1 => path.clone(),
        2 => line.to_string(),
        _ => column.to_string(),
    };
    str_new(shown.as_bytes())
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A collection being built: a map and the key of its next member, or the
/// items of a list.
enum Building {
    Map(Mapping<'static>, Option<String>),
    List(Vec<Yaml<'static>>),
}

#[derive(Default)]
struct Writer {
    stack: Vec<Building>,
    documents: Vec<Yaml<'static>>,
}

fn writer(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Writer>> {
    native::get::<Mutex<Writer>>(object).map(|w| w.lock().unwrap_or_else(|e| e.into_inner()))
}

impl Writer {
    fn add(&mut self, value: Yaml<'static>) {
        match self.stack.last_mut() {
            None => self.documents.push(value),
            Some(Building::Map(map, key)) => {
                let key = key.take().unwrap_or_default();
                map.insert(Yaml::Value(Scalar::String(Cow::Owned(key))), value);
            }
            Some(Building::List(items)) => items.push(value),
        }
    }
}

fn scalar(object: *mut NativeObject, value: Scalar<'static>) {
    if let Some(mut writer) = writer(object) {
        writer.add(Yaml::Value(value));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_writer() -> *mut NativeObject {
    native::new(Mutex::new(Writer::default()))
}

/// `null`, which reads better than the emitter's `~`.
fn null() -> Yaml<'static> {
    Yaml::Representation(Cow::Borrowed("null"), ScalarStyle::Plain, None)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_null(object: *mut NativeObject) {
    if let Some(mut writer) = writer(object) {
        writer.add(null());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_bool(object: *mut NativeObject, value: u8) {
    scalar(object, Scalar::Boolean(value != 0));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_int(object: *mut NativeObject, value: i64) {
    scalar(object, Scalar::Integer(value));
}

/// A number its digits write: an integer past `Int` as its digits, plain,
/// which YAML reads as the integer they are.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_yaml_write_number(object: *mut NativeObject, digits: *const Str) {
    let text = String::from_utf8_lossy(unsafe { crate::str_bytes(digits) }).into_owned();
    match (text.parse::<i64>(), crate::numbers::integer_digits(&text)) {
        (Ok(value), _) => scalar(object, Scalar::Integer(value)),
        (Err(_), Some(_)) => {
            if let Some(mut writer) = writer(object) {
                writer.add(Yaml::Representation(text.into(), ScalarStyle::Plain, None));
            }
        }
        (Err(_), None) => tondo_rt_yaml_write_float(object, crate::text::parse_float(&text).unwrap_or(0.0)),
    }
}

/// A float; `null` when it is not finite, which the safe subset does not
/// read.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_float(object: *mut NativeObject, value: f64) {
    if value.is_finite() {
        scalar(object, Scalar::FloatingPoint(value.into()));
    } else if let Some(mut writer) = writer(object) {
        writer.add(null());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_yaml_write_text(object: *mut NativeObject, value: *const Str) {
    scalar(object, Scalar::String(Cow::Owned(text(value))));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_yaml_write_key(object: *mut NativeObject, key: *const Str) {
    if let Some(mut writer) = writer(object)
        && let Some(Building::Map(_, pending)) = writer.stack.last_mut()
    {
        *pending = Some(text(key));
    }
}

/// Opens a map (`map`) or a list.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_open(object: *mut NativeObject, map: u8) {
    if let Some(mut writer) = writer(object) {
        let building = if map != 0 { Building::Map(Mapping::new(), None) } else { Building::List(Vec::new()) };
        writer.stack.push(building);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_write_close(object: *mut NativeObject) {
    let Some(mut writer) = writer(object) else { return };
    let value = match writer.stack.pop() {
        Some(Building::Map(map, _)) => Yaml::Mapping(map),
        Some(Building::List(items)) => Yaml::Sequence(items),
        None => return,
    };
    writer.add(value);
}

/// The documents written, each after `---` when there are several.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_yaml_finish(object: *mut NativeObject) -> *mut Str {
    let Some(writer) = writer(object) else { return str_new(b"") };
    let several = writer.documents.len() > 1;
    let mut out = String::new();
    for document in &writer.documents {
        let mut text = String::new();
        let mut emitter = YamlEmitter::new(&mut text);
        emitter.multiline_strings(true);
        if emitter.dump(document).is_err() {
            continue;
        }
        // The emitter starts every document with `---`, which one alone
        // does not need.
        match text.strip_prefix("---") {
            Some(rest) if !several => out.push_str(rest.trim_start_matches([' ', '\n'])),
            _ => out.push_str(&text),
        }
        out.push('\n');
    }
    str_new(out.as_bytes())
}
