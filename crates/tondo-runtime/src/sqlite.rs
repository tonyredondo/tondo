//! The SQLite of `std.sql` (standard library 14.37): the C library, built
//! into the runtime, behind a connection whose statements run on the host's
//! threads, prepared once and kept. Parameters and rows cross as bytes, a
//! tag and a value each: 0 null, 1 an integer, 2 a float, 3 text, 4 bytes.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, element, str_bytes, str_new};
use crate::io::byte_array;
use rusqlite::types::ValueRef;
use std::sync::Mutex;
use std::time::Duration;

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

fn bytes(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(element(array, 1, 0), (*array).len).to_vec() }
}

/// A connection, or why there is none.
enum Database {
    Open(Mutex<rusqlite::Connection>),
    Failed(String),
}

/// `open`: the database in the file at `path`, made if it is not there, or
/// one in memory for `:memory:`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sqlite_open(path: *const Str) -> *mut NativeObject {
    let path = text(path);
    let opened = crate::io::host(true, move || {
        let connection = match path.as_str() {
            ":memory:" => rusqlite::Connection::open_in_memory(),
            _ => rusqlite::Connection::open(&path),
        }?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON")?;
        Ok::<_, rusqlite::Error>(connection)
    });
    native::new(match opened {
        Ok(connection) => Database::Open(Mutex::new(connection)),
        Err(error) => Database::Failed(error.to_string()),
    })
}

/// Why the database did not open, or nothing.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_sqlite_failure(object: *mut NativeObject) -> *mut Str {
    match native::get::<Database>(object) {
        Some(Database::Failed(why)) => str_new(why.as_bytes()),
        _ => str_new(b""),
    }
}

/// Runs a statement with its parameters, encoded, or, as a `batch`, the
/// statements of a text without parameters, and gives what it did: 0, then
/// the rows it changed, the last row's id, the names of the columns and the
/// rows; or 1, the kind of the error, its message, the database's code and
/// the constraint it names.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sqlite_run(
    object: *mut NativeObject,
    sql: *const Str,
    parameters: *mut ArrayHeader,
    batch: u8,
) -> *mut ArrayHeader {
    let Some(Database::Open(connection)) = native::get::<Database>(object) else {
        return byte_array(&failed(0, "the database is not open", "", ""));
    };
    let (sql, parameters) = (text(sql), bytes(parameters));
    // The connection outlives the work, which the program waits for.
    let connection: &'static Mutex<rusqlite::Connection> = unsafe { &*(connection as *const _) };
    let out = crate::io::host(true, move || {
        let connection = connection.lock().unwrap_or_else(|e| e.into_inner());
        let done = match batch != 0 {
            true => connection.execute_batch(&sql).map(|()| outcome(&connection, &[], 0, Vec::new())),
            false => run(&connection, &sql, &parameters),
        };
        done.unwrap_or_else(|error| refused(&error))
    });
    byte_array(&out)
}

fn run(connection: &rusqlite::Connection, sql: &str, parameters: &[u8]) -> rusqlite::Result<Vec<u8>> {
    let mut statement = connection.prepare_cached(sql)?;
    let mut at = 0;
    let mut index = 1;
    while at < parameters.len() {
        let tag = parameters[at];
        at += 1;
        match tag {
            1 => {
                statement.raw_bind_parameter(index, i64::from_le_bytes(word(parameters, at)))?;
                at += 8;
            }
            2 => {
                statement.raw_bind_parameter(index, f64::from_le_bytes(word(parameters, at)))?;
                at += 8;
            }
            3 | 4 => {
                let length = u32::from_le_bytes(parameters[at..at + 4].try_into().unwrap_or_default()) as usize;
                let value = &parameters[at + 4..at + 4 + length];
                if tag == 3 {
                    statement.raw_bind_parameter(index, std::str::from_utf8(value).unwrap_or_default())?;
                } else {
                    statement.raw_bind_parameter(index, value)?;
                }
                at += 4 + length;
            }
            _ => statement.raw_bind_parameter(index, rusqlite::types::Null)?,
        }
        index += 1;
    }
    let columns: Vec<String> = statement.column_names().map(|name| name.to_string()).collect();
    let mut rows = Vec::new();
    let mut count: u32 = 0;
    if columns.is_empty() {
        statement.raw_execute()?;
    } else {
        let mut cursor = statement.raw_query();
        while let Some(row) = cursor.next()? {
            count += 1;
            for column in 0..columns.len() {
                match row.get_ref(column)? {
                    ValueRef::Null => rows.push(0),
                    ValueRef::Integer(value) => {
                        rows.push(1);
                        rows.extend(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        rows.push(2);
                        rows.extend(value.to_le_bytes());
                    }
                    ValueRef::Text(value) => put(&mut rows, 3, value),
                    ValueRef::Blob(value) => put(&mut rows, 4, value),
                }
            }
        }
    }
    Ok(outcome(connection, &columns, count, rows))
}

fn outcome(connection: &rusqlite::Connection, columns: &[String], count: u32, rows: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0];
    out.extend((connection.changes() as i64).to_le_bytes());
    out.extend(connection.last_insert_rowid().to_le_bytes());
    out.extend((columns.len() as u32).to_le_bytes());
    for name in columns {
        put(&mut out, 3, name.as_bytes());
    }
    out.extend(count.to_le_bytes());
    out.extend(rows);
    out
}

fn word(bytes: &[u8], at: usize) -> [u8; 8] {
    bytes.get(at..at + 8).and_then(|w| w.try_into().ok()).unwrap_or_default()
}

fn put(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    out.extend((value.len() as u32).to_le_bytes());
    out.extend(value);
}

/// The kinds of `std.sql`'s `ErrorKind`, in order.
const CONNECTION: u8 = 0;
const UNIQUE: u8 = 2;
const FOREIGN_KEY: u8 = 3;
const NOT_NULL: u8 = 4;
const CHECK: u8 = 5;
const CONFLICT: u8 = 6;
const SYNTAX: u8 = 7;
const OTHER: u8 = 10;

fn failed(kind: u8, message: &str, code: &str, constraint: &str) -> Vec<u8> {
    let mut out = vec![1, kind];
    for part in [message, code, constraint] {
        put(&mut out, 3, part.as_bytes());
    }
    out
}

/// An error of SQLite as one of `std.sql`, with the constraint its message
/// names: `UNIQUE constraint failed: users.email`.
fn refused(error: &rusqlite::Error) -> Vec<u8> {
    use rusqlite::ffi;
    let message = error.to_string();
    let Some(code) = error.sqlite_error() else { return failed(OTHER, &message, "", "") };
    let kind = match code.extended_code {
        ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => UNIQUE,
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => FOREIGN_KEY,
        ffi::SQLITE_CONSTRAINT_NOTNULL => NOT_NULL,
        ffi::SQLITE_CONSTRAINT_CHECK => CHECK,
        _ if matches!(code.code, ffi::ErrorCode::DatabaseBusy | ffi::ErrorCode::DatabaseLocked) => CONFLICT,
        _ if matches!(code.code, ffi::ErrorCode::CannotOpen | ffi::ErrorCode::NotADatabase) => CONNECTION,
        _ if message.contains("syntax error") || message.contains("no such") => SYNTAX,
        _ => OTHER,
    };
    let constraint = match kind {
        UNIQUE | FOREIGN_KEY | NOT_NULL | CHECK => message.rsplit_once(": ").map_or("", |(_, name)| name),
        _ => "",
    };
    failed(kind, &message, &code.extended_code.to_string(), constraint)
}
