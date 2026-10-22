//! `std.io` and the console streams (standard library 10.3, 19.1). A stream
//! is one object behind `fs.File`, `io.Writer` and `io.Reader`; work that
//! may block runs on host threads while the task waits, so it never holds a
//! worker.

use super::tasks::{HOST_WORK, Wait, current, notify};
use super::{ArrayHeader, LIVE_OBJECTS, Str, element, str_bytes, str_new, tondo_rt_array_new};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

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
/// Threads for short host work; work that may wait indefinitely, such as
/// reading a terminal or a pipe, may always start another.
const MOST_THREADS: usize = 64;

/// Runs a host operation on a thread of the pool while the current task
/// waits; directly outside a task. `long` work may wait indefinitely.
pub(crate) fn host<R: Send + 'static>(long: bool, job: impl FnOnce() -> R + Send + 'static) -> R {
    if current().is_null() {
        return job();
    }
    let result = Arc::new(Mutex::new(None));
    let wait = Wait::new();
    let (slot, waiting) = (result.clone(), wait.clone());
    HOST_WORK.fetch_add(1, Ordering::SeqCst);
    submit(
        Box::new(move || {
            let value = job();
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
            // The task is runnable before the work ends, so no worker can
            // see a moment with neither and report a deadlock.
            if waiting.claim(0) {
                notify(waiting.task);
            }
            HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        }),
        long,
    );
    wait.block_fully();
    result.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the host operation finished")
}

fn submit(job: Job, long: bool) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.jobs.push_back(job);
    if pool.idle >= pool.jobs.len() {
        drop(pool);
        WORK.notify_one();
        return;
    }
    if long || pool.threads < MOST_THREADS {
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
// Streams
// ---------------------------------------------------------------------------

/// What a stream reads from or writes to.
pub(crate) trait Host: Send {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Err(ErrorKind::Unsupported.into())
    }
    fn write(&mut self, _data: &[u8]) -> std::io::Result<()> {
        Err(ErrorKind::Unsupported.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    /// Moves the host position back over bytes read ahead, before a write.
    fn back(&mut self, _count: usize) -> std::io::Result<()> {
        Ok(())
    }
}

impl Host for std::fs::File {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        Read::read(self, buffer)
    }
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_all(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Write::flush(self)
    }
    fn back(&mut self, count: usize) -> std::io::Result<()> {
        self.seek(SeekFrom::Current(-(count as i64))).map(|_| ())
    }
}

struct Stdin;

impl Host for Stdin {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        // Output written so far is delivered before waiting for input.
        super::flush_stdout();
        std::io::stdin().read(buffer)
    }
}

struct Stdout;

impl Host for Stdout {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        super::console_append(data, false);
        Ok(())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        super::flush_stdout();
        Ok(())
    }
}

struct Stderr;

impl Host for Stderr {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        // Keeps the order of output and errors on a terminal.
        super::flush_stdout();
        std::io::stderr().write_all(data)
    }
}

struct Discard;

impl Host for Discard {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
    fn write(&mut self, _data: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
}

impl Host for std::io::Cursor<Vec<u8>> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        Read::read(self, buffer)
    }
}

/// Appends to a `Buffer`, which it keeps alive.
struct BufferSink(*mut BufferObject);

unsafe impl Send for BufferSink {}

impl Host for BufferSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        unsafe { (*self.0).data() }.extend_from_slice(data);
        Ok(())
    }
}

impl Drop for BufferSink {
    fn drop(&mut self) {
        unsafe { tondo_rt_io_buffer_release(self.0) }
    }
}

/// Where a stream's operations run.
#[derive(Copy, Clone, PartialEq)]
pub(crate) enum Mode {
    /// On the calling thread: they never block for long.
    Inline,
    /// On the host pool.
    Host,
    /// On the host pool, possibly waiting indefinitely.
    Long,
}

/// A `File`, `Writer` or `Reader`: the host, the bytes read ahead of the
/// program and the bytes written but not yet passed to the host. It closes
/// when the last reference goes.
#[repr(C)]
pub struct StreamObject {
    rc: AtomicUsize,
    mode: Mode,
    state: Mutex<StreamState>,
}

pub(crate) struct StreamState {
    host: Option<Box<dyn Host>>,
    ahead: Vec<u8>,
    position: usize,
    pending: Vec<u8>,
    /// Whether writes gather in `pending` before reaching the host.
    buffered: bool,
    /// The first failure of a console write nobody observed yet.
    failed: Option<std::io::ErrorKind>,
}

const WRITE_BUFFER: usize = 64 * 1024;
const READ_CHUNK: usize = 64 * 1024;

fn closed() -> std::io::Error {
    ErrorKind::NotConnected.into()
}

impl StreamState {
    fn host(&mut self) -> std::io::Result<&mut Box<dyn Host>> {
        self.host.as_mut().ok_or_else(closed)
    }

    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.host()?.write(&pending)?;
        }
        self.host()?.flush()
    }

    /// Before writing: the host position goes back over unread bytes.
    fn unread(&mut self) -> std::io::Result<()> {
        let rest = self.ahead.len() - self.position;
        if rest > 0 {
            self.host()?.back(rest)?;
        }
        self.ahead.clear();
        self.position = 0;
        Ok(())
    }

    /// Reads more bytes ahead; false at the end of the data.
    fn fill(&mut self) -> std::io::Result<bool> {
        if !self.pending.is_empty() {
            self.flush()?;
        }
        if self.position == self.ahead.len() {
            self.ahead.clear();
            self.position = 0;
        }
        let start = self.ahead.len();
        self.ahead.resize(start + READ_CHUNK, 0);
        let read = self.host.as_mut().ok_or_else(closed).and_then(|h| h.read(&mut self.ahead[start..]));
        let count = *read.as_ref().unwrap_or(&0);
        self.ahead.truncate(start + count);
        read.map(|n| n > 0)
    }

    /// The next line without its `\n` or `\r\n`, or `None` at the end.
    pub(crate) fn read_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.ahead[self.position..].iter().position(|&b| b == b'\n') {
                let mut line = self.ahead[self.position..self.position + end].to_vec();
                self.position += end + 1;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if !self.fill()? {
                if self.position == self.ahead.len() {
                    return Ok(None);
                }
                let line = self.ahead[self.position..].to_vec();
                self.position = self.ahead.len();
                return Ok(Some(line));
            }
        }
    }

    pub(crate) fn read(&mut self, max: usize) -> std::io::Result<Option<Vec<u8>>> {
        if self.position == self.ahead.len() && !self.fill()? {
            return Ok(None);
        }
        let end = (self.position + max).min(self.ahead.len());
        let bytes = self.ahead[self.position..end].to_vec();
        self.position = end;
        Ok(Some(bytes))
    }

    pub(crate) fn read_rest(&mut self) -> std::io::Result<Vec<u8>> {
        let mut bytes = self.ahead.split_off(self.position);
        self.ahead.clear();
        self.position = 0;
        while self.fill()? {
            bytes.append(&mut self.ahead);
            self.position = 0;
        }
        Ok(bytes)
    }

    pub(crate) fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.unread()?;
        if !self.buffered {
            return self.host()?.write(data);
        }
        self.host()?;
        self.pending.extend_from_slice(data);
        if self.pending.len() >= WRITE_BUFFER {
            self.flush()?;
        }
        Ok(())
    }

    pub(crate) fn close(&mut self) -> std::io::Result<()> {
        if self.host.is_none() {
            return Ok(());
        }
        let flushed = self.flush();
        self.host = None;
        flushed
    }
}

/// A new stream over `host`, with one reference.
pub(crate) fn stream(host: Box<dyn Host>, mode: Mode, buffered: bool) -> *mut StreamObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let state =
        StreamState { host: Some(host), ahead: Vec::new(), position: 0, pending: Vec::new(), buffered, failed: None };
    Box::into_raw(Box::new(StreamObject { rc: AtomicUsize::new(1), mode, state: Mutex::new(state) }))
}

/// A stream shared with a host thread for one operation.
struct Shared(*mut StreamObject);

unsafe impl Send for Shared {}

impl Shared {
    fn state(&self) -> MutexGuard<'_, StreamState> {
        unsafe { (*self.0).state.lock().unwrap_or_else(|e| e.into_inner()) }
    }
}

/// Runs an operation on a stream where its mode says.
pub(crate) fn run<R: Send + 'static>(
    stream: *mut StreamObject,
    op: impl FnOnce(&mut StreamState) -> R + Send + 'static,
) -> R {
    let shared = Shared(stream);
    match unsafe { (*stream).mode } {
        Mode::Inline => op(&mut shared.state()),
        Mode::Host => host(false, move || op(&mut shared.state())),
        Mode::Long => host(true, move || op(&mut shared.state())),
    }
}

/// Runs an operation in place when `ready` says the bytes already read
/// ahead, or the room left to gather writes, are enough; otherwise where
/// the stream's mode says.
pub(crate) fn run_ready<R: Send + 'static>(
    stream: *mut StreamObject,
    ready: impl FnOnce(&StreamState) -> bool,
    op: impl FnOnce(&mut StreamState) -> R + Send + 'static,
) -> R {
    if let Ok(mut state) = unsafe { (*stream).state.try_lock() }
        && ready(&state)
    {
        return op(&mut state);
    }
    run(stream, op)
}

impl StreamState {
    /// Whether a whole line was read ahead.
    pub(crate) fn has_line(&self) -> bool {
        self.ahead[self.position..].contains(&b'\n')
    }

    /// Whether bytes were read ahead.
    pub(crate) fn has_bytes(&self) -> bool {
        self.position < self.ahead.len()
    }

    /// Whether `count` bytes fit among the gathered writes.
    pub(crate) fn has_room(&self, count: usize) -> bool {
        self.buffered
            && self.host.is_some()
            && self.position == self.ahead.len()
            && self.pending.len() + count < WRITE_BUFFER
    }
}

/// Releases a stream; the last reference writes what is pending and
/// closes it, ignoring errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_stream_release(stream: *mut StreamObject) {
    if stream.is_null() {
        return;
    }
    unsafe {
        if (*stream).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        run(stream, |state| {
            let _ = state.close();
        });
        drop(Box::from_raw(stream));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn retain(stream: *mut StreamObject) -> *mut StreamObject {
    unsafe { (*stream).rc.fetch_add(1, Ordering::Relaxed) };
    stream
}

// ---------------------------------------------------------------------------
// std.io
// ---------------------------------------------------------------------------

/// The status of an `IoError`: 1 plus its variant, `Closed`, `InvalidData`
/// or `Host`.
fn io_status(error: &std::io::Error) -> i64 {
    match error.kind() {
        ErrorKind::NotConnected | ErrorKind::BrokenPipe | ErrorKind::WriteZero | ErrorKind::Unsupported => 1,
        ErrorKind::InvalidData => 2,
        _ => 3,
    }
}

fn status<T>(result: std::io::Result<T>, store: impl FnOnce(T)) -> i64 {
    match result {
        Ok(value) => {
            store(value);
            0
        }
        Err(error) => io_status(&error),
    }
}

pub(crate) fn utf8(bytes: Vec<u8>) -> std::io::Result<String> {
    String::from_utf8(bytes).map_err(|_| ErrorKind::InvalidData.into())
}

pub(crate) fn byte_array(bytes: &[u8]) -> *mut ArrayHeader {
    let array = tondo_rt_array_new(1, bytes.len());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), element(array, 1, 0), bytes.len()) };
    array
}

pub(crate) unsafe fn array_bytes(array: *mut ArrayHeader) -> Vec<u8> {
    unsafe { (0..(*array).len).map(|i| *element(array, 1, i)).collect() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_write(stream: *mut StreamObject, text: *const Str) -> i64 {
    let data = unsafe { str_bytes(text).to_vec() };
    let count = data.len();
    status(run_ready(stream, |s| s.has_room(count), move |s| s.write(&data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_write_line(stream: *mut StreamObject, text: *const Str) -> i64 {
    let mut data = unsafe { str_bytes(text).to_vec() };
    data.push(b'\n');
    let count = data.len();
    status(run_ready(stream, |s| s.has_room(count), move |s| s.write(&data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_write_bytes(stream: *mut StreamObject, data: *mut ArrayHeader) -> i64 {
    let data = unsafe { array_bytes(data) };
    let count = data.len();
    status(run_ready(stream, |s| s.has_room(count), move |s| s.write(&data)), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_flush(stream: *mut StreamObject) -> i64 {
    status(run(stream, |s| s.flush()), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_close(stream: *mut StreamObject) -> i64 {
    status(run(stream, |s| s.close()), |_| ())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_read_line(stream: *mut StreamObject, out: *mut *mut Str) -> i64 {
    let line = run_ready(stream, StreamState::has_line, |s| s.read_line()?.map(utf8).transpose());
    status(line, |line| unsafe { *out = line.map_or(std::ptr::null_mut(), |l| str_new(l.as_bytes())) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_read(stream: *mut StreamObject, max: i64, out: *mut *mut ArrayHeader) -> i64 {
    let max = max.max(0) as usize;
    let read = run_ready(stream, StreamState::has_bytes, move |s| s.read(max));
    status(read, |bytes| unsafe { *out = bytes.map_or(std::ptr::null_mut(), |b| byte_array(&b)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_read_text(stream: *mut StreamObject, out: *mut *mut Str) -> i64 {
    let read = run(stream, |s| utf8(s.read_rest()?));
    status(read, |text| unsafe { *out = str_new(text.as_bytes()) })
}

/// `io.reader(text)`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_reader(text: *const Str) -> *mut StreamObject {
    let bytes = unsafe { str_bytes(text).to_vec() };
    stream(Box::new(std::io::Cursor::new(bytes)), Mode::Inline, false)
}

/// `io.discard()`
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_io_discard() -> *mut StreamObject {
    stream(Box::new(Discard), Mode::Inline, false)
}

/// `Buffer`: bytes in memory that writers append to.
#[repr(C)]
pub struct BufferObject {
    rc: AtomicUsize,
    data: Mutex<Vec<u8>>,
}

impl BufferObject {
    fn data(&self) -> MutexGuard<'_, Vec<u8>> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `io.Buffer()`
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_io_buffer_new() -> *mut BufferObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(BufferObject { rc: AtomicUsize::new(1), data: Mutex::new(Vec::new()) }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_writer(buffer: *mut BufferObject) -> *mut StreamObject {
    unsafe { (*buffer).rc.fetch_add(1, Ordering::Relaxed) };
    stream(Box::new(BufferSink(buffer)), Mode::Inline, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_reader(buffer: *mut BufferObject) -> *mut StreamObject {
    let bytes = unsafe { (*buffer).data().clone() };
    stream(Box::new(std::io::Cursor::new(bytes)), Mode::Inline, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_text(buffer: *mut BufferObject, out: *mut *mut Str) -> i64 {
    let bytes = unsafe { (*buffer).data().clone() };
    status(utf8(bytes), |text| unsafe { *out = str_new(text.as_bytes()) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_bytes(buffer: *mut BufferObject) -> *mut ArrayHeader {
    byte_array(&unsafe { (*buffer).data() })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_length(buffer: *mut BufferObject) -> i64 {
    unsafe { (*buffer).data().len() as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_clear(buffer: *mut BufferObject) {
    unsafe { (*buffer).data().clear() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_buffer_release(buffer: *mut BufferObject) {
    if buffer.is_null() {
        return;
    }
    unsafe {
        if (*buffer).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(buffer));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Console
// ---------------------------------------------------------------------------

/// The streams the console uses now: output, error and input.
struct Console([*mut StreamObject; 3]);

unsafe impl Send for Console {}

static CONSOLE: Mutex<Option<Console>> = Mutex::new(None);

fn console() -> MutexGuard<'static, Option<Console>> {
    let mut console = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
    console.get_or_insert_with(|| {
        Console([
            stream(Box::new(Stdout), Mode::Inline, false),
            stream(Box::new(Stderr), Mode::Inline, false),
            stream(Box::new(Stdin), Mode::Long, false),
        ])
    });
    console
}

/// One more reference to the console stream at `index`.
fn console_stream(index: usize) -> *mut StreamObject {
    retain(console().as_ref().unwrap().0[index])
}

/// Writes to the console output or error: failures wait for `flush`.
fn console_write(index: usize, text: *const Str, newline: bool) {
    let stream = console_stream(index);
    let mut data = unsafe { str_bytes(text).to_vec() };
    if newline {
        data.push(b'\n');
    }
    run(stream, move |s| {
        if let Err(error) = s.write(&data) {
            s.failed.get_or_insert(error.kind());
        }
    });
    unsafe { tondo_rt_stream_release(stream) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_console_print(text: *const Str) {
    console_write(0, text, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_console_println(text: *const Str) {
    console_write(0, text, true)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_console_eprint(text: *const Str) {
    console_write(1, text, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_console_eprintln(text: *const Str) {
    console_write(1, text, true)
}

/// `console.flush()`: delivers the output and returns the first failure
/// since the last `flush` as an `IoError` status.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_flush() -> i64 {
    let stream = console_stream(0);
    let result = run(stream, |s| {
        let flushed = s.flush();
        match s.failed.take() {
            Some(kind) => Err(kind.into()),
            None => flushed.and(super::stdout_failure()),
        }
    });
    unsafe { tondo_rt_stream_release(stream) };
    status(result, |_| ())
}

/// `console.readLine()`: invalid UTF-8 becomes U+FFFD and a failure ends
/// the input.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_read_line() -> *mut Str {
    let stream = console_stream(2);
    let line = run_ready(stream, StreamState::has_line, |s| s.read_line().ok().flatten());
    unsafe { tondo_rt_stream_release(stream) };
    line.map_or(std::ptr::null_mut(), |l| str_new(String::from_utf8_lossy(&l).as_bytes()))
}

/// `console.stdout()`, `stderr()` and `stdin()` (0, 1 and 2).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_stream(index: i64) -> *mut StreamObject {
    console_stream(index as usize)
}

/// `setOutput`, `setError` and `setInput` (0, 1 and 2): keeps a reference
/// to the stream; what the previous one holds reaches it first.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_set(index: i64, stream: *mut StreamObject) {
    let previous = std::mem::replace(&mut console().as_mut().unwrap().0[index as usize], retain(stream));
    run(previous, |s| {
        let _ = s.flush();
    });
    unsafe { tondo_rt_stream_release(previous) };
}

/// When the program ends: delivers what the console streams hold and lets
/// them go. A failure of the output nobody observed is the exit status.
pub(crate) fn finish() -> Option<std::io::ErrorKind> {
    let streams = CONSOLE.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    let failed = run(streams.0[0], |s| {
        let flushed = s.flush();
        s.failed.take().or(flushed.err().map(|e| e.kind()))
    });
    for stream in streams.0 {
        unsafe { tondo_rt_stream_release(stream) };
    }
    failed
}
