//! `std.fs` and `std.path` (standard library 10.4, 10.5). Filesystem calls
//! run on threads of their own while the task waits, so they never block a
//! worker. Each operation returns 0, or 1 plus the index of the `FsError`
//! variant, and writes its value to `out`.

use super::tasks::{HOST_WORK, Wait, current, notify};
use super::{ArrayHeader, LIVE_OBJECTS, Str, element, str_bytes, str_new, tondo_rt_array_new};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

// ---------------------------------------------------------------------------
// Host threads
// ---------------------------------------------------------------------------

type Job = Box<dyn FnOnce() + Send>;

struct Pool {
    jobs: VecDeque<Job>,
    idle: usize,
    threads: usize,
}

static POOL: Mutex<Pool> = Mutex::new(Pool { jobs: VecDeque::new(), idle: 0, threads: 0 });
static WORK: Condvar = Condvar::new();
const MOST_THREADS: usize = 64;

/// Runs a host operation on a thread of the pool while the current task
/// waits; directly outside a task.
fn host<R: Send + 'static>(job: impl FnOnce() -> R + Send + 'static) -> R {
    if current().is_null() {
        return job();
    }
    let result = Arc::new(Mutex::new(None));
    let wait = Wait::new();
    let (slot, waiting) = (result.clone(), wait.clone());
    HOST_WORK.fetch_add(1, Ordering::SeqCst);
    submit(Box::new(move || {
        let value = job();
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
        HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        if waiting.claim(0) {
            notify(waiting.task);
        }
    }));
    wait.block_fully();
    result.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the host operation finished")
}

fn submit(job: Job) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.jobs.push_back(job);
    if pool.idle > 0 {
        drop(pool);
        WORK.notify_one();
        return;
    }
    if pool.threads < MOST_THREADS {
        pool.threads += 1;
        drop(pool);
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(serve)
            .unwrap_or_else(|_| super::runtime_fault("cannot start a host thread"));
    }
}

fn serve() {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(job) = pool.jobs.pop_front() {
            drop(pool);
            job();
            pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
            continue;
        }
        pool.idle += 1;
        pool = WORK.wait(pool).unwrap_or_else(|e| e.into_inner());
        pool.idle -= 1;
    }
}

// ---------------------------------------------------------------------------
// Values and errors
// ---------------------------------------------------------------------------

/// The `FsError` variants, in declaration order.
#[derive(Copy, Clone)]
enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    InvalidData,
    InvalidName,
    Host,
}

fn code(error: FsError) -> i64 {
    error as i64 + 1
}

fn classify(error: &std::io::Error) -> FsError {
    match error.kind() {
        ErrorKind::NotFound => FsError::NotFound,
        ErrorKind::PermissionDenied => FsError::PermissionDenied,
        ErrorKind::AlreadyExists => FsError::AlreadyExists,
        ErrorKind::NotADirectory => FsError::NotADirectory,
        ErrorKind::IsADirectory | ErrorKind::DirectoryNotEmpty => FsError::IsADirectory,
        ErrorKind::InvalidData => FsError::InvalidData,
        ErrorKind::InvalidInput | ErrorKind::InvalidFilename => FsError::InvalidName,
        _ => FsError::Host,
    }
}

type Outcome<T> = Result<T, FsError>;

fn io<T>(result: std::io::Result<T>) -> Outcome<T> {
    result.map_err(|e| classify(&e))
}

/// The path a string names; NUL cannot appear in a host path.
unsafe fn path(text: *const Str) -> Outcome<String> {
    let bytes = unsafe { str_bytes(text) };
    if bytes.contains(&0) {
        return Err(FsError::InvalidName);
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn text(bytes: Vec<u8>) -> Outcome<String> {
    String::from_utf8(bytes).map_err(|_| FsError::InvalidData)
}

/// Stores a result: the value through `store`, or the error's code.
fn finish<T>(outcome: Outcome<T>, store: impl FnOnce(T)) -> i64 {
    match outcome {
        Ok(value) => {
            store(value);
            0
        }
        Err(error) => code(error),
    }
}

fn byte_array(bytes: &[u8]) -> *mut ArrayHeader {
    let array = tondo_rt_array_new(1, bytes.len());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), element(array, 1, 0), bytes.len()) };
    array
}

unsafe fn array_bytes(array: *mut ArrayHeader) -> Vec<u8> {
    unsafe { (0..(*array).len).map(|i| *element(array, 1, i)).collect() }
}

// ---------------------------------------------------------------------------
// Whole files and directories
// ---------------------------------------------------------------------------

/// Writes through a temporary file in the same directory and a rename.
fn write_atomically(path: &str, data: &[u8]) -> Outcome<()> {
    let target = std::path::Path::new(path);
    let name = target.file_name().ok_or(FsError::InvalidName)?.to_string_lossy();
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = format!(".{name}.tondo-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let temporary = target.with_file_name(unique);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.flush()?;
        std::fs::rename(&temporary, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    io(written)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_text(path_text: *const Str, out: *mut *mut Str) -> i64 {
    let path = unsafe { path(path_text) };
    let read = host(move || text(io(std::fs::read(path?))?));
    finish(read, |text| unsafe { *out = str_new(text.as_bytes()) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_bytes(path_text: *const Str, out: *mut *mut ArrayHeader) -> i64 {
    let path = unsafe { path(path_text) };
    let read = host(move || io(std::fs::read(path?)));
    finish(read, |bytes| unsafe { *out = byte_array(&bytes) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_text(path_text: *const Str, contents: *const Str) -> i64 {
    let (path, data) = unsafe { (path(path_text), str_bytes(contents).to_vec()) };
    finish(host(move || write_atomically(&path?, &data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_bytes(path_text: *const Str, contents: *mut ArrayHeader) -> i64 {
    let (path, data) = unsafe { (path(path_text), array_bytes(contents)) };
    finish(host(move || write_atomically(&path?, &data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_append_text(path_text: *const Str, contents: *const Str) -> i64 {
    let (path, data) = unsafe { (path(path_text), str_bytes(contents).to_vec()) };
    let appended = host(move || {
        let mut file = io(std::fs::OpenOptions::new().append(true).create(true).open(path?))?;
        io(file.write_all(&data))
    });
    finish(appended, |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_exists(path_text: *const Str) -> u8 {
    let path = unsafe { path(path_text) };
    host(move || path.is_ok_and(|p| std::path::Path::new(&p).exists())) as u8
}

/// `metadata`: the kind (the `FileKind` index), size and read-only flag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_metadata(path_text: *const Str, out: *mut [i64; 3]) -> i64 {
    let path = unsafe { path(path_text) };
    let found = host(move || {
        let metadata = io(std::fs::symlink_metadata(path?))?;
        let kind = metadata.file_type();
        let kind = if kind.is_file() {
            0
        } else if kind.is_dir() {
            1
        } else if kind.is_symlink() {
            2
        } else {
            3
        };
        let size = i64::try_from(metadata.len()).map_err(|_| FsError::Host)?;
        Ok([kind, size, metadata.permissions().readonly() as i64])
    });
    finish(found, |fields| unsafe { *out = fields })
}

/// `list`: the names of the entries, sorted by bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_list(path_text: *const Str, out: *mut *mut ArrayHeader) -> i64 {
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
    finish(listed, |names| unsafe {
        let array = tondo_rt_array_new(8, names.len());
        for (index, name) in names.iter().enumerate() {
            *(element(array, 8, index) as *mut *mut Str) = str_new(name.as_bytes());
        }
        *out = array;
    })
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

/// An open `File`: the host file, the bytes read ahead of the program, and
/// the bytes written but not yet passed to the host. It closes when the
/// last reference goes.
#[repr(C)]
pub struct FileObject {
    rc: AtomicUsize,
    state: Mutex<FileState>,
}

struct FileState {
    file: Option<std::fs::File>,
    ahead: Vec<u8>,
    position: usize,
    pending: Vec<u8>,
}

const WRITE_BUFFER: usize = 64 * 1024;
const READ_CHUNK: usize = 64 * 1024;

impl FileState {
    fn file(&mut self) -> Outcome<&mut std::fs::File> {
        self.file.as_mut().ok_or(FsError::Host)
    }

    fn flush(&mut self) -> Outcome<()> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            io(self.file()?.write_all(&pending))?;
        }
        Ok(())
    }

    /// Before writing: the host position goes back over unread bytes.
    fn unread(&mut self) -> Outcome<()> {
        let rest = self.ahead.len() - self.position;
        if rest > 0 {
            io(self.file()?.seek(SeekFrom::Current(-(rest as i64))))?;
        }
        self.ahead.clear();
        self.position = 0;
        Ok(())
    }

    /// Reads more bytes ahead; false at the end of the file.
    fn fill(&mut self) -> Outcome<bool> {
        self.flush()?;
        if self.position == self.ahead.len() {
            self.ahead.clear();
            self.position = 0;
        }
        let start = self.ahead.len();
        self.ahead.resize(start + READ_CHUNK, 0);
        let file = self.file.as_mut().ok_or(FsError::Host)?;
        let read = io(file.read(&mut self.ahead[start..]));
        let count = *read.as_ref().unwrap_or(&0);
        self.ahead.truncate(start + count);
        read.map(|n| n > 0)
    }

    fn read_line(&mut self) -> Outcome<Option<String>> {
        loop {
            if let Some(end) = self.ahead[self.position..].iter().position(|&b| b == b'\n') {
                let mut line = self.ahead[self.position..self.position + end].to_vec();
                self.position += end + 1;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return text(line).map(Some);
            }
            if !self.fill()? {
                if self.position == self.ahead.len() {
                    return Ok(None);
                }
                let line = self.ahead[self.position..].to_vec();
                self.position = self.ahead.len();
                return text(line).map(Some);
            }
        }
    }

    fn read(&mut self, max: usize) -> Outcome<Option<Vec<u8>>> {
        if self.position == self.ahead.len() && !self.fill()? {
            return Ok(None);
        }
        let end = (self.position + max).min(self.ahead.len());
        let bytes = self.ahead[self.position..end].to_vec();
        self.position = end;
        Ok(Some(bytes))
    }

    fn read_rest(&mut self) -> Outcome<Vec<u8>> {
        self.flush()?;
        let mut bytes = self.ahead.split_off(self.position);
        self.ahead.clear();
        self.position = 0;
        io(self.file()?.read_to_end(&mut bytes))?;
        Ok(bytes)
    }

    fn write(&mut self, data: &[u8]) -> Outcome<()> {
        self.unread()?;
        self.pending.extend_from_slice(data);
        if self.pending.len() >= WRITE_BUFFER {
            self.flush()?;
        }
        Ok(())
    }

    fn close(&mut self) -> Outcome<()> {
        let flushed = self.flush();
        self.file = None;
        flushed
    }
}

/// A file object shared with a host thread for one operation.
struct Shared(*mut FileObject);

unsafe impl Send for Shared {}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, FileState> {
        unsafe { (*self.0).state.lock().unwrap_or_else(|e| e.into_inner()) }
    }
}

/// `open(path, mode)`, `mode` the index of the `OpenMode` variant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_open(path_text: *const Str, mode: i64, out: *mut *mut FileObject) -> i64 {
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
    finish(opened, |file| unsafe {
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        *out = Box::into_raw(Box::new(FileObject {
            rc: AtomicUsize::new(1),
            state: Mutex::new(FileState { file: Some(file), ahead: Vec::new(), position: 0, pending: Vec::new() }),
        }));
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_line(file: *mut FileObject, out: *mut *mut Str) -> i64 {
    let file = Shared(file);
    let line = host(move || file.state().read_line());
    finish(line, |line| unsafe { *out = line.map_or(std::ptr::null_mut(), |l| str_new(l.as_bytes())) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read(file: *mut FileObject, max: i64, out: *mut *mut ArrayHeader) -> i64 {
    let file = Shared(file);
    let max = max.max(0) as usize;
    let read = host(move || file.state().read(max));
    finish(read, |bytes| unsafe { *out = bytes.map_or(std::ptr::null_mut(), |b| byte_array(&b)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_read_rest(file: *mut FileObject, out: *mut *mut Str) -> i64 {
    let file = Shared(file);
    let read = host(move || text(file.state().read_rest()?));
    finish(read, |text| unsafe { *out = str_new(text.as_bytes()) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write(file: *mut FileObject, contents: *const Str) -> i64 {
    let (file, data) = (Shared(file), unsafe { str_bytes(contents).to_vec() });
    finish(host(move || file.state().write(&data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_write_bytes_to(file: *mut FileObject, contents: *mut ArrayHeader) -> i64 {
    let (file, data) = (Shared(file), unsafe { array_bytes(contents) });
    finish(host(move || file.state().write(&data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_flush(file: *mut FileObject) -> i64 {
    let file = Shared(file);
    finish(host(move || file.state().flush()), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_close(file: *mut FileObject) -> i64 {
    let file = Shared(file);
    finish(host(move || file.state().close()), |_| ())
}

/// Releases a `File`; the last reference writes what is pending and
/// closes it, ignoring errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fs_file_release(file: *mut FileObject) {
    if file.is_null() {
        return;
    }
    unsafe {
        if (*file).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let shared = Shared(file);
        host(move || {
            let _ = shared.state().close();
        });
        drop(Box::from_raw(file));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

unsafe fn lexical(text: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(text) }).into_owned()
}

fn string_or_null(text: Option<&str>) -> *mut Str {
    text.map_or(std::ptr::null_mut(), |t| str_new(t.as_bytes()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_join(base: *const Str, part: *const Str) -> *mut Str {
    let joined = unsafe { std::path::Path::new(&lexical(base)).join(lexical(part)) };
    str_new(joined.to_string_lossy().as_bytes())
}

/// `parent`, `fileName` or `extension` (`which` 0, 1 or 2): null for none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_part(path: *const Str, which: u8) -> *mut Str {
    let text = unsafe { lexical(path) };
    let path = std::path::Path::new(&text);
    let part = match which {
        // A bare name has no parent, rather than an empty one.
        0 => path.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_string_lossy().into_owned()),
        1 => path.file_name().map(|p| p.to_string_lossy().into_owned()),
        _ => path.extension().map(|p| p.to_string_lossy().into_owned()),
    };
    string_or_null(part.as_deref())
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
    byte_array(unsafe { str_bytes(text) })
}

/// `String.fromBytes(bytes)`: null when the bytes are not UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_str_from_bytes(bytes: *mut ArrayHeader) -> *mut Str {
    let bytes = unsafe { array_bytes(bytes) };
    if std::str::from_utf8(&bytes).is_ok() { str_new(&bytes) } else { std::ptr::null_mut() }
}
