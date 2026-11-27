//! `std.fs` and `std.path` (standard library 10.4, 10.5). Filesystem calls
//! run on threads of their own while the task waits, so they never block a
//! worker. Each operation returns 0, or 1 plus the index of the `FsError`
//! variant, and writes its value to `out`.

use super::io::{Mode, StreamObject, StreamState, run, run_ready};
use super::{ArrayHeader, Str, str_bytes, str_new};
use crate::array_items;
use crate::byte_array;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

fn host<R: Send + 'static>(job: impl FnOnce() -> R + Send) -> R {
    super::io::host(false, job)
}

// ---------------------------------------------------------------------------
// Values and errors
// ---------------------------------------------------------------------------

/// The `FsError` variants, in declaration order; `Host` with the
/// system's number of the error, and `TooLarge` with the limit passed.
#[derive(Copy, Clone)]
enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    InvalidData,
    InvalidName,
    Host(i32),
    TooLarge(usize),
}

fn code(error: FsError) -> i64 {
    match error {
        FsError::NotFound => 1,
        FsError::PermissionDenied => 2,
        FsError::AlreadyExists => 3,
        FsError::NotADirectory => 4,
        FsError::IsADirectory => 5,
        FsError::InvalidData => 6,
        FsError::InvalidName => 7,
        FsError::Host(number) => {
            crate::note_error_number(number as i64);
            8
        }
        FsError::TooLarge(limit) => {
            crate::note_error_number(limit as i64);
            9
        }
    }
}

/// `FsError.Host(number)` as it shows: its number and the system's text of
/// it, as `os error 28: No space left on device`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_os_error(b: *mut super::Builder, number: i64) {
    let text = match number {
        0 => "host".to_string(),
        number => os_error(number),
    };
    unsafe { (*b).extend(text.as_bytes()) }
}

/// A number of the system with its text: `os error 2: No such file or
/// directory`.
fn os_error(number: i64) -> String {
    let shown = std::io::Error::from_raw_os_error(number as i32).to_string();
    let suffix = format!(" (os error {number})");
    format!("os error {number}: {}", shown.strip_suffix(&suffix).unwrap_or(&shown))
}

/// `os.Errno` as it shows (standard library 14.45).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_error_text(number: i64, region: *mut crate::regions::Region) -> *mut super::Str {
    crate::strings::str_in(region, os_error(number).as_bytes())
}

fn classify(error: &std::io::Error) -> FsError {
    if let Some(limit) = crate::limit_passed(error) {
        return FsError::TooLarge(limit);
    }
    match error.kind() {
        ErrorKind::NotFound => FsError::NotFound,
        ErrorKind::PermissionDenied => FsError::PermissionDenied,
        ErrorKind::AlreadyExists => FsError::AlreadyExists,
        ErrorKind::NotADirectory => FsError::NotADirectory,
        ErrorKind::IsADirectory | ErrorKind::DirectoryNotEmpty => FsError::IsADirectory,
        ErrorKind::InvalidData => FsError::InvalidData,
        ErrorKind::InvalidInput | ErrorKind::InvalidFilename => FsError::InvalidName,
        _ => FsError::Host(error.raw_os_error().unwrap_or(0)),
    }
}

type Outcome<T> = Result<T, FsError>;

fn io<T>(result: std::io::Result<T>) -> Outcome<T> {
    result.map_err(|e| classify(&e))
}

/// The path a string names; NUL cannot appear in a host path.
unsafe fn path(text: *const Str) -> Outcome<String> {
    let bytes = unsafe { str_bytes(&text) };
    if bytes.contains(&0) {
        return Err(FsError::InvalidName);
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn text(bytes: Vec<u8>) -> Outcome<String> {
    if simdutf8::basic::from_utf8(&bytes).is_ok() {
        Ok(unsafe { String::from_utf8_unchecked(bytes) })
    } else {
        Err(FsError::InvalidData)
    }
}

/// Stores a result: the value through `store`, or the error's code.
fn finish<T>(outcome: Outcome<T>, store: impl FnOnce(T)) -> i64 {
    crate::status(outcome, code, store)
}

// ---------------------------------------------------------------------------
// Whole files and directories
// ---------------------------------------------------------------------------

/// Whether a file is one `write_atomically` writes through,
/// `.{name}.tondo-{process}-{count}`, which watches do not report.
pub(crate) fn temporary(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return false };
    let Some((_, rest)) = name.strip_prefix('.').and_then(|name| name.rsplit_once(".tondo-")) else { return false };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    rest.split_once('-').is_some_and(|(process, count)| digits(process) && digits(count))
}

/// Writes through a temporary file in the same directory and a rename: the
/// file a link names, keeping the permissions of the one it replaces.
fn write_atomically(path: &str, data: &[u8]) -> Outcome<()> {
    let real = std::fs::canonicalize(path).ok();
    let target = real.as_deref().unwrap_or(std::path::Path::new(path));
    let permissions = std::fs::metadata(target).ok().map(|m| m.permissions());
    let name = target.file_name().ok_or(FsError::InvalidName)?.to_string_lossy();
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = format!(".{name}.tondo-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let temporary = target.with_file_name(unique);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(data)?;
        file.flush()?;
        std::fs::rename(&temporary, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    io(written)
}

/// From this size a file is read straight into its text, where the caller
/// wants it, which saves a copy and its bytes for a second call of the
/// host; a smaller one is read and copied in one.
const READ_INTO_TEXT_FROM: usize = 64 * 1024;

/// A file opened to be read as text: whole, when it is small, or open with
/// the size it said.
enum Opened {
    Whole(String),
    Large(std::fs::File, usize),
}

fn opened(path: String) -> Outcome<Opened> {
    let mut file = io(std::fs::File::open(path))?;
    let size = file.metadata().map_or(0, |m| m.len() as usize);
    if size >= READ_INTO_TEXT_FROM {
        return Ok(Opened::Large(file, size));
    }
    let mut bytes = Vec::with_capacity(size + 1);
    io(file.read_to_end(&mut bytes))?;
    Ok(Opened::Whole(text(bytes)?))
}

/// A large file read straight into a text built in `region`, or on the
/// heap without one, and checked as UTF-8 where it lies. A file that grew
/// past the size it said has the rest added after it, and one that shrank
/// to a few bytes is a short text as any other.
fn read_large(file: std::fs::File, size: usize, region: *mut crate::regions::Region) -> Outcome<*mut Str> {
    let mut started = std::mem::MaybeUninit::<crate::Builder>::uninit();
    unsafe {
        match region.is_null() {
            true => {
                started.write(crate::Builder::new(size + 1));
            }
            false => super::builder::tondo_rt_sb_start_region(started.as_mut_ptr(), size + 1, region),
        }
    }
    let mut builder = unsafe { started.assume_init() };
    let (at, _, room) = builder.parts();
    let into = at as usize;
    // The task waits for the host's work to end, so its room stays. The
    // descriptor reads into it, which nothing has written yet, by its
    // address: no reference is made to bytes before they are read.
    let read = host(move || {
        let (mut file, mut filled) = (file, 0);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        while filled < room {
            let n = unsafe { libc::read(fd, (into + filled) as *mut libc::c_void, room - filled) };
            match n {
                0 => return Ok((filled, Vec::new())),
                n if n > 0 => filled += n as usize,
                _ => match std::io::Error::last_os_error() {
                    e if e.kind() == ErrorKind::Interrupted => {}
                    e => return io(Err(e)),
                },
            }
        }
        let mut rest = Vec::new();
        io(file.read_to_end(&mut rest))?;
        Ok((filled, rest))
    });
    // What goes unused is the heap's to free; a region frees its own.
    let discard = |builder: crate::Builder| {
        let text = builder.finish();
        if region.is_null() {
            unsafe { crate::strings::tondo_rt_str_release(text) };
        }
    };
    let (filled, rest) = match read {
        Ok(read) => read,
        Err(error) => {
            discard(builder);
            return Err(error);
        }
    };
    builder.set_len(filled);
    builder.extend(&rest);
    let (at, len, _) = builder.parts();
    let bytes = unsafe { std::slice::from_raw_parts(at, len) };
    if simdutf8::basic::from_utf8(bytes).is_err() {
        discard(builder);
        return Err(FsError::InvalidData);
    }
    if len <= crate::strings::SHORT_MAX {
        let short = crate::strings::str_in(region, bytes);
        discard(builder);
        return Ok(short);
    }
    Ok(builder.finish())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_text(
    path_text: *const Str,
    out: *mut *mut Str,
    region: *mut crate::regions::Region,
) -> i64 {
    let path = unsafe { path(path_text) };
    let read = match host(move || opened(path?)) {
        Ok(Opened::Whole(text)) => Ok(crate::strings::str_in(region, text.as_bytes())),
        Ok(Opened::Large(file, size)) => read_large(file, size, region),
        Err(error) => Err(error),
    };
    finish(read, |text| unsafe { *out = text })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_bytes(
    path_text: *const Str,
    out: *mut *mut ArrayHeader,
    region: *mut crate::regions::Region,
) -> i64 {
    let path = unsafe { path(path_text) };
    let read = host(move || io(std::fs::read(path?)));
    finish(read, |bytes| unsafe { *out = crate::arrays::byte_array_in(region, &bytes) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_text(path_text: *const Str, contents: *const Str) -> i64 {
    let (path, data) = unsafe { (path(path_text), str_bytes(&contents)) };
    finish(host(move || write_atomically(&path?, data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_bytes(path_text: *const Str, contents: *mut ArrayHeader) -> i64 {
    let (path, data) = unsafe { (path(path_text), array_items::<u8>(contents)) };
    finish(host(move || write_atomically(&path?, data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_append_text(path_text: *const Str, contents: *const Str) -> i64 {
    let (path, data) = unsafe { (path(path_text), str_bytes(&contents)) };
    let appended = host(move || {
        let mut file = io(std::fs::OpenOptions::new().append(true).create(true).open(path?))?;
        io(file.write_all(data))
    });
    finish(appended, |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_exists(path_text: *const Str) -> u8 {
    let path = unsafe { path(path_text) };
    host(move || path.is_ok_and(|p| std::path::Path::new(&p).exists())) as u8
}

/// `list`: the names of the entries, sorted by bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_list(
    path_text: *const Str,
    out: *mut *mut ArrayHeader,
    region: *mut crate::regions::Region,
) -> i64 {
    let path = unsafe { path(path_text) };
    let listed = host(move || {
        let mut names = Vec::new();
        for entry in io(std::fs::read_dir(path?))? {
            let name = io(entry)?.file_name();
            names.push(name.into_string().map_err(|_| FsError::InvalidName)?);
        }
        names.sort();
        Ok(names)
    });
    finish(listed, |names| unsafe { *out = crate::arrays::string_array_in(region, &names) })
}

/// An operation on one or two paths that gives nothing back.
unsafe fn change(
    first: *const Str,
    second: *const Str,
    run: impl FnOnce(String, String) -> std::io::Result<()> + Send + 'static,
) -> i64 {
    let first = unsafe { path(first) };
    let second = if second.is_null() { Ok(String::new()) } else { unsafe { path(second) } };
    finish(host(move || io(run(first?, second?))), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_create_directory(path: *const Str) -> i64 {
    unsafe { change(path, std::ptr::null(), |p, _| std::fs::create_dir_all(p)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_rename(from: *const Str, to: *const Str) -> i64 {
    unsafe { change(from, to, std::fs::rename) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_copy(from: *const Str, to: *const Str) -> i64 {
    unsafe { change(from, to, |from, to| std::fs::copy(from, to).map(|_| ())) }
}

/// `remove`: a file, a link or an empty directory; with `recursive`, a
/// directory and its contents, without following links.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_remove(path: *const Str, recursive: i64) -> i64 {
    unsafe {
        change(path, std::ptr::null(), move |p, _| match std::fs::symlink_metadata(&p)?.is_dir() {
            true if recursive != 0 => std::fs::remove_dir_all(p),
            true => std::fs::remove_dir(p),
            false => std::fs::remove_file(p),
        })
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// `open(path, mode)`, `mode` the index of the `OpenMode` variant. A `File`
/// is a stream of `std.io` (10.3).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_open(
    path_text: *const Str,
    mode: i64,
    out: *mut *mut StreamObject,
    region: *mut crate::regions::Region,
) -> i64 {
    let path = unsafe { path(path_text) };
    let opened = host(move || {
        let mut options = std::fs::OpenOptions::new();
        match mode {
            0 => options.read(true),
            1 => options.write(true).create(true).truncate(true),
            2 => options.append(true).create(true),
            3 => options.write(true).create_new(true),
            _ => options.read(true).write(true),
        };
        io(options.open(path?))
    });
    finish(opened, |file| unsafe { *out = crate::io::stream_in(region, Box::new(file), Mode::Host, true) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_line(
    file: *mut StreamObject,
    most: i64,
    out: *mut *mut Str,
    region: *mut crate::regions::Region,
) -> i64 {
    let (region, most) = (crate::io::WaitingRegion(region), most.max(0) as usize);
    let line = run_ready(file, StreamState::has_line, move |s| {
        io(s.read_line(most, move |bytes| region.made_text(bytes)))?
            .map(|line| line.ok_or(FsError::InvalidData))
            .transpose()
    });
    finish(line, |line| unsafe { *out = line.map_or(std::ptr::null_mut(), |l| l.0) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read(
    file: *mut StreamObject,
    max: i64,
    out: *mut *mut ArrayHeader,
    region: *mut crate::regions::Region,
) -> i64 {
    let max = max.max(0) as usize;
    let read = run_ready(file, StreamState::has_bytes, move |s| io(s.read(max)));
    finish(read, |bytes| unsafe {
        *out = bytes.map_or(std::ptr::null_mut(), |b| crate::arrays::byte_array_in(region, &b))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_rest(
    file: *mut StreamObject,
    out: *mut *mut Str,
    region: *mut crate::regions::Region,
) -> i64 {
    let read = run(file, |s| text(io(s.read_rest())?));
    finish(read, |text| unsafe { *out = crate::strings::str_in(region, text.as_bytes()) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write(file: *mut StreamObject, contents: *const Str) -> i64 {
    let data = unsafe { str_bytes(&contents) };
    finish(run_ready(file, |s| s.has_room(data.len()), |s| io(s.write(data))), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_bytes_to(file: *mut StreamObject, contents: *mut ArrayHeader) -> i64 {
    let data = unsafe { array_items::<u8>(contents) };
    finish(run_ready(file, |s| s.has_room(data.len()), |s| io(s.write(data))), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_flush(file: *mut StreamObject) -> i64 {
    finish(run(file, |s| io(s.flush())), |_| ())
}

/// `file.lock(shared)`: waits for the file's lock, which its closing
/// releases.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_lock(file: *mut StreamObject, shared: i64) -> i64 {
    finish(run(file, move |s| io(s.lock(shared != 0, true))), |_| ())
}

/// `file.tryLock(shared)`: whether it took the lock, without waiting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_try_lock(file: *mut StreamObject, shared: i64, out: *mut i64) -> i64 {
    let out = out as usize;
    finish(run(file, move |s| io(s.lock(shared != 0, false))), |taken| unsafe { *(out as *mut i64) = taken as i64 })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_close(file: *mut StreamObject) -> i64 {
    finish(run(file, |s| io(s.close())), |_| ())
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

unsafe fn lexical(text: *const Str) -> String {
    unsafe { super::str_text(&text) }.to_owned()
}

fn string_or_null(region: *mut crate::regions::Region, text: Option<&str>) -> *mut Str {
    text.map_or(std::ptr::null_mut(), |t| crate::strings::str_in(region, t.as_bytes()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_join(
    base: *const Str,
    part: *const Str,
    region: *mut super::regions::Region,
) -> *mut Str {
    let joined = unsafe { std::path::Path::new(&lexical(base)).join(lexical(part)) };
    super::strings::str_in(region, joined.to_string_lossy().as_bytes())
}

/// `parent`, `fileName` or `extension` (`which` 0, 1 or 2): null for none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_part(
    path: *const Str,
    which: u8,
    region: *mut crate::regions::Region,
) -> *mut Str {
    let text = unsafe { lexical(path) };
    let path = std::path::Path::new(&text);
    let part = match which {
        // A bare name has no parent, rather than an empty one.
        0 => path.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_string_lossy().into_owned()),
        1 => path.file_name().map(|p| p.to_string_lossy().into_owned()),
        _ => path.extension().map(|p| p.to_string_lossy().into_owned()),
    };
    string_or_null(region, part.as_deref())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_is_absolute(path: *const Str) -> u8 {
    std::path::Path::new(&unsafe { lexical(path) }).is_absolute() as u8
}

// ---------------------------------------------------------------------------
// Text and bytes
// ---------------------------------------------------------------------------

/// `text.bytes()`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_to_bytes(text: *const Str) -> *mut ArrayHeader {
    byte_array(unsafe { str_bytes(&text) })
}

/// `String.fromBytes(bytes)`: null when the bytes are not UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_from_bytes(bytes: *mut ArrayHeader) -> *mut Str {
    let bytes = unsafe { array_items::<u8>(bytes) };
    if simdutf8::basic::from_utf8(bytes).is_ok() { str_new(bytes) } else { std::ptr::null_mut() }
}
