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
    // A virtual clock does not move while its tasks wait on the host.
    let virtual_clock = super::clock::in_domain();
    super::clock::external(true);
    submit(
        Box::new(move || {
            let value = job();
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
            // The task is runnable before the work ends, so no worker can
            // see a moment with neither and report a deadlock.
            if waiting.claim(0) {
                notify(waiting.task);
            }
            if virtual_clock {
                super::clock::external(false);
            }
            HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        }),
        long,
    );
    wait.block_fully();
    result.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the host operation finished")
}

/// Like `host` for long work, but a cancelled task stops waiting:
/// `on_cancel` makes the work end soon, and the task waits for that.
pub(crate) fn host_cancellable<R: Send + 'static>(
    job: impl FnOnce() -> R + Send + 'static,
    on_cancel: impl FnOnce(),
) -> R {
    if current().is_null() {
        return job();
    }
    let result = Arc::new((Mutex::new(None), Condvar::new()));
    let wait = Wait::new();
    let (slot, waiting) = (result.clone(), wait.clone());
    HOST_WORK.fetch_add(1, Ordering::SeqCst);
    let virtual_clock = super::clock::in_domain();
    super::clock::external(true);
    submit(
        Box::new(move || {
            let value = job();
            *slot.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
            slot.1.notify_all();
            if waiting.claim(0) {
                notify(waiting.task);
            }
            if virtual_clock {
                super::clock::external(false);
            }
            HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        }),
        true,
    );
    if wait.block() == super::tasks::CANCELLED {
        on_cancel();
    }
    let mut value = result.0.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(value) = value.take() {
            return value;
        }
        value = result.1.wait(value).unwrap_or_else(|e| e.into_inner());
    }
}

/// What `future` gives, polled on the current task, which parks between
/// polls without holding its worker until the future's waker notifies it;
/// `None` when the task is cancelled first, which drops the future.
pub(crate) fn wait_for<T>(future: impl std::future::Future<Output = T>) -> Option<T> {
    use std::task::{Context, Poll, Wake, Waker};
    struct Notify(Arc<Wait>);
    impl Wake for Notify {
        fn wake(self: Arc<Self>) {
            if self.0.claim(0) {
                notify(self.0.task);
            }
        }
    }
    let runtime = super::http::runtime();
    if current().is_null() {
        return Some(runtime.block_on(future));
    }
    let mut future = std::pin::pin!(future);
    loop {
        let wait = Wait::new();
        let waker = Waker::from(Arc::new(Notify(wait.clone())));
        // The runtime's context only while polling: the task may resume on
        // another thread.
        let polled = {
            let _context = runtime.enter();
            future.as_mut().poll(&mut Context::from_waker(&waker))
        };
        if let Poll::Ready(value) = polled {
            return Some(value);
        }
        HOST_WORK.fetch_add(1, Ordering::SeqCst);
        // A virtual clock does not move while its tasks wait on the host.
        let virtual_clock = super::clock::in_domain();
        super::clock::external(true);
        let woken = wait.block();
        if virtual_clock {
            super::clock::external(false);
        }
        HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        if woken == super::tasks::CANCELLED {
            return None;
        }
    }
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
        super::tasks::spawn_thread(Some(256 * 1024), serve);
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
    /// Writes what ends its data, when the stream closes, after a flush.
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    /// Moves the host position back over bytes read ahead, before a write.
    fn back(&mut self, _count: usize) -> std::io::Result<()> {
        Ok(())
    }
    /// For a `Mode::Ready` host, which fails with `WouldBlock` instead of
    /// waiting: after it did, what to wait for before trying again.
    fn blocked(&mut self) -> Option<Ready> {
        None
    }
    /// From now on a `Mode::Ready` host fails with `kind` where it would
    /// block; `None` lets it block again.
    fn give_up(&mut self, _kind: Option<ErrorKind>) {}
}

/// What a `Mode::Ready` host waits for.
pub(crate) type Ready = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

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

/// The console input, which a host thread reads when a task asks for more:
/// the task waiting for it holds no thread, and its wait can end (11.15).
#[derive(Default)]
struct Stdin {
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// What the host thread read from the console input and no task took yet.
struct Input {
    bytes: Vec<u8>,
    end: bool,
    failure: Option<std::io::Error>,
    reading: bool,
}

static INPUT: Mutex<Input> = Mutex::new(Input { bytes: Vec::new(), end: false, failure: None, reading: false });
static INPUT_READ: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn console_input() -> MutexGuard<'static, Input> {
    INPUT.lock().unwrap_or_else(|e| e.into_inner())
}

impl Host for Stdin {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut input = console_input();
        if !input.bytes.is_empty() {
            let count = buffer.len().min(input.bytes.len());
            buffer[..count].copy_from_slice(&input.bytes[..count]);
            input.bytes.drain(..count);
            return Ok(count);
        }
        if let Some(failure) = input.failure.take() {
            return Err(failure);
        }
        if std::mem::take(&mut input.end) {
            return Ok(0);
        }
        if let Some(kind) = self.gave_up {
            return Err(kind.into());
        }
        if !input.reading {
            input.reading = true;
            // Output written so far is delivered before waiting for input.
            super::flush_stdout();
            submit(
                Box::new(|| {
                    let mut chunk = vec![0; READ_CHUNK];
                    let read = std::io::stdin().read(&mut chunk);
                    let mut input = console_input();
                    match read {
                        Ok(0) => input.end = true,
                        Ok(count) => input.bytes.extend_from_slice(&chunk[..count]),
                        Err(failure) => input.failure = Some(failure),
                    }
                    input.reading = false;
                    drop(input);
                    INPUT_READ.notify_waiters();
                }),
                true,
            );
        }
        self.waiting = true;
        Err(ErrorKind::WouldBlock.into())
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.waiting).then(|| {
            Box::pin(async {
                loop {
                    let notified = INPUT_READ.notified();
                    let mut notified = std::pin::pin!(notified);
                    notified.as_mut().enable();
                    if !console_input().reading {
                        return;
                    }
                    notified.await;
                }
            }) as Ready
        })
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
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

impl Host for std::io::PipeReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        Read::read(self, buffer)
    }
}

impl Host for std::io::PipeWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_all(data)
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
    /// On the calling task, with a host that never blocks: when it would,
    /// the task waits for it without holding the stream or a thread, and
    /// the operation runs again, so it must change nothing before then.
    Ready,
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
    /// Bytes read ahead, `ahead[position..filled]`; the rest of `ahead` is
    /// room for the next read, zeroed once.
    ahead: Vec<u8>,
    position: usize,
    filled: usize,
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
        let rest = self.filled - self.position;
        if rest > 0 {
            self.host()?.back(rest)?;
        }
        self.position = 0;
        self.filled = 0;
        Ok(())
    }

    /// Reads more bytes ahead; false at the end of the data.
    fn fill(&mut self) -> std::io::Result<bool> {
        if !self.pending.is_empty() {
            self.flush()?;
        }
        if self.position == self.filled {
            self.position = 0;
            self.filled = 0;
        }
        let start = self.filled;
        if self.ahead.len() < start + READ_CHUNK {
            self.ahead.resize(start + READ_CHUNK, 0);
        }
        let read =
            self.host.as_mut().ok_or_else(closed).and_then(|h| h.read(&mut self.ahead[start..start + READ_CHUNK]));
        self.filled += *read.as_ref().unwrap_or(&0);
        read.map(|n| n > 0)
    }

    /// The next line without its `\n` or `\r\n`, or `None` at the end.
    pub(crate) fn read_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.ahead[self.position..self.filled].iter().position(|&b| b == b'\n') {
                let mut line = self.ahead[self.position..self.position + end].to_vec();
                self.position += end + 1;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if !self.fill()? {
                if self.position == self.filled {
                    return Ok(None);
                }
                let line = self.ahead[self.position..self.filled].to_vec();
                self.position = self.filled;
                return Ok(Some(line));
            }
        }
    }

    pub(crate) fn read(&mut self, max: usize) -> std::io::Result<Option<Vec<u8>>> {
        if self.position == self.filled && !self.fill()? {
            return Ok(None);
        }
        let end = (self.position + max).min(self.filled);
        let bytes = self.ahead[self.position..end].to_vec();
        self.position = end;
        Ok(Some(bytes))
    }

    pub(crate) fn read_rest(&mut self) -> std::io::Result<Vec<u8>> {
        // What is read stays ahead until the end, so a host that would
        // block loses nothing.
        while self.fill()? {}
        let bytes = self.ahead[self.position..self.filled].to_vec();
        self.position = 0;
        self.filled = 0;
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
        if flushed.as_ref().is_err_and(|error| error.kind() == ErrorKind::WouldBlock) {
            return flushed;
        }
        let finished = self.host.as_mut().map_or(Ok(()), |host| host.finish());
        self.host = None;
        flushed.and(finished)
    }
}

/// A new stream over `host`, with one reference.
pub(crate) fn stream(host: Box<dyn Host>, mode: Mode, buffered: bool) -> *mut StreamObject {
    LIVE_OBJECTS.made();
    let state = StreamState {
        host: Some(host),
        ahead: Vec::new(),
        position: 0,
        filled: 0,
        pending: Vec::new(),
        buffered,
        failed: None,
    };
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
    op: impl FnMut(&mut StreamState) -> R + Send + 'static,
) -> R {
    run_until(stream, None, op)
}

/// Like `run`; on a `Mode::Ready` stream the operation fails with
/// `TimedOut` if it would wait past `deadline`.
pub(crate) fn run_until<R: Send + 'static>(
    stream: *mut StreamObject,
    deadline: Option<std::time::Instant>,
    mut op: impl FnMut(&mut StreamState) -> R + Send + 'static,
) -> R {
    let shared = Shared(stream);
    match unsafe { (*stream).mode } {
        Mode::Inline => op(&mut shared.state()),
        Mode::Host => host(false, move || op(&mut shared.state())),
        Mode::Long => host(true, move || op(&mut shared.state())),
        Mode::Ready => loop {
            let (result, blocked) = {
                let mut state = shared.state();
                let result = op(&mut state);
                (result, state.host.as_mut().and_then(|host| host.blocked()))
            };
            let Some(blocked) = blocked else { return result };
            let waited = wait_for(async move {
                match deadline {
                    Some(at) => tokio::time::timeout_at(at.into(), blocked).await.is_ok(),
                    None => {
                        blocked.await;
                        true
                    }
                }
            });
            if waited == Some(true) {
                continue;
            }
            // Cancelled or out of time: the operation fails where it would wait.
            let kind = if waited.is_none() { ErrorKind::Interrupted } else { ErrorKind::TimedOut };
            let mut state = shared.state();
            state.give_up(Some(kind));
            let result = op(&mut state);
            state.give_up(None);
            return result;
        },
    }
}

/// Runs an operation in place when `ready` says the bytes already read
/// ahead, or the room left to gather writes, are enough; otherwise where
/// the stream's mode says.
pub(crate) fn run_ready<R: Send + 'static>(
    stream: *mut StreamObject,
    ready: impl FnOnce(&StreamState) -> bool,
    mut op: impl FnMut(&mut StreamState) -> R + Send + 'static,
) -> R {
    if let Ok(mut state) = unsafe { (*stream).state.try_lock() }
        && ready(&state)
    {
        return op(&mut state);
    }
    run(stream, op)
}

impl StreamState {
    fn give_up(&mut self, kind: Option<ErrorKind>) {
        if let Some(host) = self.host.as_mut() {
            host.give_up(kind);
        }
    }

    /// Whether a whole line was read ahead.
    pub(crate) fn has_line(&self) -> bool {
        self.ahead[self.position..self.filled].contains(&b'\n')
    }

    /// Whether bytes were read ahead.
    pub(crate) fn has_bytes(&self) -> bool {
        self.position < self.filled
    }

    /// Whether `count` bytes fit among the gathered writes.
    pub(crate) fn has_room(&self, count: usize) -> bool {
        self.buffered
            && self.host.is_some()
            && self.position == self.filled
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
        // A `Ready` host delivers what it holds on its own, so letting it
        // go never waits on the other side of a connection.
        if (*stream).mode != Mode::Ready {
            run(stream, |state| {
                let _ = state.close();
            });
        }
        drop(Box::from_raw(stream));
        LIVE_OBJECTS.gone();
    }
}

pub(crate) fn retain(stream: *mut StreamObject) -> *mut StreamObject {
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

/// `Reader.readBytes()`: the rest, as bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_read_bytes(stream: *mut StreamObject, out: *mut *mut ArrayHeader) -> i64 {
    let read = run(stream, |s| s.read_rest());
    status(read, |bytes| unsafe { *out = byte_array(&bytes) })
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
    LIVE_OBJECTS.made();
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
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Console
// ---------------------------------------------------------------------------

/// The streams the console uses now: output, error and input, and which
/// the program replaced.
struct Console([*mut StreamObject; 3], [bool; 3]);

unsafe impl Send for Console {}

static CONSOLE: Mutex<Option<Console>> = Mutex::new(None);

fn console() -> MutexGuard<'static, Option<Console>> {
    let mut console = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
    console.get_or_insert_with(|| {
        Console(
            [
                stream(Box::new(Stdout), Mode::Inline, false),
                stream(Box::new(Stderr), Mode::Inline, false),
                stream(Box::<Stdin>::default(), Mode::Ready, false),
            ],
            [false; 3],
        )
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
    let previous = {
        let mut console = console();
        let console = console.as_mut().unwrap();
        console.1[index as usize] = true;
        std::mem::replace(&mut console.0[index as usize], retain(stream))
    };
    run(previous, |s| {
        let _ = s.flush();
    });
    unsafe { tondo_rt_stream_release(previous) };
}

/// The console stream at `index` when the program replaced it, with one
/// more reference: a child process that inherits it writes there.
pub(crate) fn replaced_console(index: usize) -> Option<*mut StreamObject> {
    let console = console();
    let console = console.as_ref().unwrap();
    console.1[index].then(|| retain(console.0[index]))
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
