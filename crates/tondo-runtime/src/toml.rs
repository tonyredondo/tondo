//! TOML for `std.toml`: a document parsed with `toml_edit` and read as the
//! events of `std.serialization`, each with the path, line and column it
//! comes from; and a writer that builds a document from those events,
//! choosing tables, arrays of tables and inline tables as TOML needs.

use crate::native::{self, NativeObject};
use crate::{Str, str_bytes, str_new};
use std::sync::Mutex;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

/// The events, as `std.toml` reads them; `ERROR` when the input is not
/// TOML.
const END: i64 = 0;
const TRUE: i64 = 2;
const FALSE: i64 = 3;
const INT: i64 = 4;
const FLOAT: i64 = 5;
const TEXT: i64 = 6;
const DATETIME: i64 = 7;
const BEGIN_LIST: i64 = 8;
const END_LIST: i64 = 9;
const BEGIN_MAP: i64 = 10;
const END_MAP: i64 = 11;
const KEY: i64 = 12;
const ERROR: i64 = -1;

struct Event {
    code: i64,
    text: String,
    int: i64,
    float: f64,
    path: String,
    /// Its byte in the input.
    at: usize,
}

struct Reader {
    source: String,
    events: Vec<Event>,
    /// The event just read, counted from 1.
    read: usize,
    /// A failure and the byte where it happened, and its path.
    error: Option<(String, usize, String)>,
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

fn reader(object: *mut NativeObject) -> Option<&'static Mutex<Reader>> {
    native::get::<Mutex<Reader>>(object)
}

/// The events of a document of `text`, or the failure to parse it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_parse(source: *const Str) -> *mut NativeObject {
    let source = text(source);
    let mut events = Vec::new();
    let mut error = None;
    match toml_edit::Document::parse(source.as_str()) {
        Ok(document) => table(document.as_table(), "$", 0, &mut events),
        Err(failure) => {
            let at = failure.span().map_or(0, |span| span.start);
            error = Some((failure.message().trim_end().to_string(), at, "$".to_string()));
        }
    }
    native::new(Mutex::new(Reader { source, events, read: 0, error }))
}

fn push<'a>(events: &'a mut Vec<Event>, code: i64, path: &str, at: usize) -> &'a mut Event {
    events.push(Event { code, text: String::new(), int: 0, float: 0.0, path: path.to_string(), at });
    events.last_mut().unwrap()
}

/// A table's events: its start, each key and value, its end.
fn table(table: &Table, path: &str, at: usize, events: &mut Vec<Event>) {
    let at = table.span().map_or(at, |span| span.start);
    push(events, BEGIN_MAP, path, at);
    for (key, item) in table.iter() {
        let inner = format!("{path}.{key}");
        let start = item.span().map_or(at, |span| span.start);
        push(events, KEY, &inner, start).text = key.to_string();
        self::item(item, &inner, start, events);
    }
    push(events, END_MAP, path, at);
}

fn item(item: &Item, path: &str, at: usize, events: &mut Vec<Event>) {
    match item {
        Item::None => {}
        Item::Value(value) => self::value(value, path, at, events),
        Item::Table(inner) => table(inner, path, at, events),
        Item::ArrayOfTables(tables) => {
            push(events, BEGIN_LIST, path, at);
            for (index, inner) in tables.iter().enumerate() {
                table(inner, &format!("{path}[{index}]"), at, events);
            }
            push(events, END_LIST, path, at);
        }
    }
}

fn value(value: &Value, path: &str, at: usize, events: &mut Vec<Event>) {
    let at = value.span().map_or(at, |span| span.start);
    match value {
        Value::String(text) => push(events, TEXT, path, at).text = text.value().clone(),
        Value::Integer(int) => push(events, INT, path, at).int = *int.value(),
        Value::Float(float) => push(events, FLOAT, path, at).float = *float.value(),
        Value::Boolean(flag) => {
            push(events, if *flag.value() { TRUE } else { FALSE }, path, at);
        }
        Value::Datetime(datetime) => push(events, DATETIME, path, at).text = datetime.value().to_string(),
        Value::Array(items) => {
            push(events, BEGIN_LIST, path, at);
            for (index, item) in items.iter().enumerate() {
                self::value(item, &format!("{path}[{index}]"), at, events);
            }
            push(events, END_LIST, path, at);
        }
        Value::InlineTable(inline) => {
            push(events, BEGIN_MAP, path, at);
            for (key, item) in inline.iter() {
                let inner = format!("{path}.{key}");
                let start = item.span().map_or(at, |span| span.start);
                push(events, KEY, &inner, start).text = key.to_string();
                self::value(item, &inner, start, events);
            }
            push(events, END_MAP, path, at);
        }
    }
}

/// The next event's code, `END` after the document, or `ERROR`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_next(object: *mut NativeObject) -> i64 {
    let Some(reader) = reader(object) else { return ERROR };
    let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
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
    let reader = reader.lock().unwrap_or_else(|e| e.into_inner());
    match reader.read.checked_sub(1).and_then(|index| reader.events.get(index)) {
        Some(event) => part(event),
        None => empty,
    }
}

/// The text of the event just read: a string, a key or a date-time.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_text(object: *mut NativeObject) -> *mut Str {
    str_new(current(object, |event| event.text.clone(), String::new()).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_int(object: *mut NativeObject) -> i64 {
    current(object, |event| event.int, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_float(object: *mut NativeObject) -> f64 {
    current(object, |event| event.float, 0.0)
}

/// Skips the rest of a value that started with `code`; 0, or `ERROR`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_skip(object: *mut NativeObject, code: i64) -> i64 {
    if code != BEGIN_LIST && code != BEGIN_MAP {
        return 0;
    }
    let Some(reader) = reader(object) else { return ERROR };
    let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
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

/// A failure at the event just read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_fail(object: *mut NativeObject, message: *const Str) {
    let Some(reader) = reader(object) else { return };
    let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
    let (at, path) = match reader.read.checked_sub(1).and_then(|index| reader.events.get(index)) {
        Some(event) => (event.at, event.path.clone()),
        None => (0, "$".to_string()),
    };
    reader.error = Some((text(message), at, path));
}

/// Where the failure is: its message (0), path (1), line (2) or column (3).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_where(object: *mut NativeObject, part: i64) -> *mut Str {
    let Some(reader) = reader(object) else { return str_new(b"") };
    let reader = reader.lock().unwrap_or_else(|e| e.into_inner());
    let Some((message, at, path)) = &reader.error else { return str_new(b"") };
    let before = &reader.source[..(*at).min(reader.source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
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

/// A value being built: a table and the key of its next member, or the
/// items of an array.
enum Frame {
    Table(Table, Option<String>),
    Array(Vec<Built>),
}

/// A finished value, before its place decides how TOML writes it.
enum Built {
    Value(Value),
    Table(Table),
    Tables(ArrayOfTables),
}

#[derive(Default)]
struct Writer {
    stack: Vec<Frame>,
    root: Option<Table>,
    /// The first value that could not be written, and where.
    error: Option<(String, String)>,
}

fn writer(object: *mut NativeObject) -> Option<std::sync::MutexGuard<'static, Writer>> {
    native::get::<Mutex<Writer>>(object).map(|w| w.lock().unwrap_or_else(|e| e.into_inner()))
}

impl Writer {
    fn fail(&mut self, message: String) {
        let place = self.place();
        self.error.get_or_insert((message, place));
    }

    /// Where the value being written goes, as `server.ports[2]`.
    fn place(&self) -> String {
        let mut path = String::from("$");
        for frame in &self.stack {
            match frame {
                Frame::Table(_, Some(key)) => path.push_str(&format!(".{key}")),
                Frame::Array(items) => path.push_str(&format!("[{}]", items.len())),
                Frame::Table(_, None) => {}
            }
        }
        path
    }

    /// Puts a finished value in the table or array being built.
    fn add(&mut self, built: Built) {
        match self.stack.last_mut() {
            None => match built {
                Built::Table(table) if self.root.is_none() => self.root = Some(table),
                Built::Table(_) => self.fail("a TOML document holds one table".into()),
                _ => self.fail("a TOML document is a table, so it cannot be another value".into()),
            },
            Some(Frame::Table(table, key)) => {
                let Some(key) = key.take() else {
                    return self.fail("a member of a table has no key".into());
                };
                let item = match built {
                    Built::Value(value) => Item::Value(value),
                    Built::Table(inner) => Item::Table(inner),
                    Built::Tables(tables) => Item::ArrayOfTables(tables),
                };
                table.insert(&key, item);
            }
            Some(Frame::Array(items)) => items.push(built),
        }
    }
}

/// A value inside an array: tables inline.
fn inline(built: Built) -> Value {
    match built {
        Built::Value(value) => value,
        Built::Table(table) => Value::InlineTable(table.into_inline_table()),
        Built::Tables(tables) => {
            Value::Array(tables.into_iter().map(|t| Value::InlineTable(t.into_inline_table())).collect())
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_writer() -> *mut NativeObject {
    native::new(Mutex::new(Writer::default()))
}

fn scalar(object: *mut NativeObject, value: Value) {
    if let Some(mut writer) = writer(object) {
        writer.add(Built::Value(value));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_null(object: *mut NativeObject) {
    if let Some(mut writer) = writer(object) {
        writer.fail("TOML has no null to write this `none`".into());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_bool(object: *mut NativeObject, value: u8) {
    scalar(object, Value::from(value != 0));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_int(object: *mut NativeObject, value: i64) {
    scalar(object, Value::from(value));
}

/// A number its digits write: TOML's integers have 64 bits with a sign,
/// and one past them cannot be written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_write_number(object: *mut NativeObject, digits: *const Str) {
    let text = text(digits);
    match (text.parse::<i64>(), crate::numbers::integer_digits(&text)) {
        (Ok(value), _) => scalar(object, Value::from(value)),
        (Err(_), Some(_)) => {
            if let Some(mut writer) = writer(object) {
                writer.fail(format!("{text} does not fit in TOML, whose integers have 64 bits with a sign"));
            }
        }
        (Err(_), None) => scalar(object, Value::from(crate::text::parse_float(&text).unwrap_or(0.0))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_float(object: *mut NativeObject, value: f64) {
    scalar(object, Value::from(value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_write_text(object: *mut NativeObject, value: *const Str) {
    scalar(object, Value::from(text(value)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_write_datetime(object: *mut NativeObject, value: *const Str) {
    let written = text(value);
    match written.parse::<toml_edit::Datetime>() {
        Ok(datetime) => scalar(object, Value::from(datetime)),
        Err(_) => {
            if let Some(mut writer) = writer(object) {
                writer.fail(format!("`{written}` is not a TOML date or time"));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_toml_write_key(object: *mut NativeObject, key: *const Str) {
    if let Some(mut writer) = writer(object)
        && let Some(Frame::Table(_, pending)) = writer.stack.last_mut()
    {
        *pending = Some(text(key));
    }
}

/// Opens a table (`map`) or an array.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_open(object: *mut NativeObject, map: u8) {
    let Some(mut writer) = writer(object) else { return };
    if writer.stack.is_empty() && map == 0 {
        writer.fail("a TOML document is a table, so it cannot be an array".into());
    }
    let frame = if map != 0 { Frame::Table(Table::new(), None) } else { Frame::Array(Vec::new()) };
    writer.stack.push(frame);
}

/// Closes the table or array opened last: an array of tables only becomes
/// `[[name]]` sections when all its items are tables.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_write_close(object: *mut NativeObject) {
    let Some(mut writer) = writer(object) else { return };
    let built = match writer.stack.pop() {
        Some(Frame::Table(table, _)) => Built::Table(table),
        Some(Frame::Array(items)) => {
            let tables = !items.is_empty() && items.iter().all(|item| matches!(item, Built::Table(_)));
            if tables {
                let mut array = ArrayOfTables::new();
                for item in items {
                    if let Built::Table(table) = item {
                        array.push(table);
                    }
                }
                Built::Tables(array)
            } else {
                Built::Value(Value::Array(items.into_iter().map(inline).collect()))
            }
        }
        None => return,
    };
    writer.add(built);
}

/// The document written, or empty after a failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_finish(object: *mut NativeObject) -> *mut Str {
    let Some(mut writer) = writer(object) else { return str_new(b"") };
    if writer.error.is_some() {
        return str_new(b"");
    }
    let mut document = DocumentMut::new();
    *document.as_table_mut() = writer.root.take().unwrap_or_default();
    str_new(document.to_string().as_bytes())
}

/// Why the first value could not be written (0) and where (1), or empty.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_toml_failure(object: *mut NativeObject, part: i64) -> *mut Str {
    let failure = writer(object).and_then(|w| w.error.clone()).unwrap_or_default();
    str_new(if part == 0 { failure.0 } else { failure.1 }.as_bytes())
}
