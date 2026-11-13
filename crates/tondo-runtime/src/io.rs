//! `std.io` and the console streams (standard library 10.3, 19.1). A stream
//! is one object behind `fs.File`, `io.Writer` and `io.Reader`; work that
//! may block runs on host threads while the task waits, so it never holds a
//! worker.

use super::tasks::{HOST_WORK, Wait, current, notify};
use super::{ArrayHeader, LIVE_OBJECTS, Str, byte_array, str_bytes, str_new};
use crate::array_items;
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
/// waits; directly outside a task, and in a recorded run, whose calls are
/// those of its one thread (toolchain 7.6), where a wait holds the other
/// tasks; and in an order of the tasks, which the time of a thread would
/// change, unless it is `long` work, which may wait indefinitely. The task
/// waits until the work has run, so the work may borrow, as a scoped
/// thread's does.
pub(crate) fn host<R: Send + 'static>(long: bool, job: impl FnOnce() -> R + Send) -> R {
    if current().is_null() || super::replay::on() || (!long && super::tasks::deterministic()) {
        return job();
    }
    let result = Arc::new(Mutex::new(None));
    let wait = Wait::new();
    let (slot, waiting) = (result.clone(), wait.clone());
    HOST_WORK.fetch_add(1, Ordering::SeqCst);
    // A virtual clock does not move while its tasks wait on the host.
    let virtual_clock = super::clock::in_domain();
    super::clock::external(true);
    let work: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        // Calling `job` drops what it borrows, before the task goes on.
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
    });
    // SAFETY: only the work claims `wait`, after `job` has run, and
    // `block_fully` returns only then, even for a cancelled task.
    submit(unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Job>(work) }, long);
    wait.block_fully();
    result.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the host operation finished")
}

/// Like `host` for long work, but a cancelled task stops waiting:
/// `on_cancel` makes the work end soon, and the task waits for that.
pub(crate) fn host_cancellable<R: Send + 'static>(
    job: impl FnOnce() -> R + Send + 'static,
    on_cancel: impl FnOnce(),
) -> R {
    if current().is_null() || super::replay::on() {
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
    wait_in(true, future)
}

/// `wait_for`, polling in the runtime's context or, where the future needs
/// none, as our sockets' readiness, without entering it each time.
fn wait_in<T>(context: bool, future: impl std::future::Future<Output = T>) -> Option<T> {
    use std::task::{Poll, Waker};
    if current().is_null() {
        return Some(super::http::runtime().block_on(future));
    }
    let mut future = std::pin::pin!(future);
    loop {
        let wait = Wait::new();
        let waker = Waker::from(wait.clone());
        let (polled, let_go) = poll_once(future.as_mut(), &waker, context);
        // Not here, between the poll that gave the waker away and the wait
        // it wakes: a wait of closing would take the wake meant for that one.
        close_later(let_go);
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

pub(crate) fn submit(job: Job, long: bool) {
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

/// Ends the pool's threads once their jobs are done, for a library's end.
pub(crate) fn stop() {
    let _pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    WORK.notify_all();
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
        if super::tasks::stopping() {
            pool.threads -= 1;
            return;
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
    /// Takes the file's lock, shared or not, waiting for it or not: whether
    /// it took it.
    fn lock(&mut self, _shared: bool, _wait: bool) -> std::io::Result<bool> {
        Err(ErrorKind::Unsupported.into())
    }
    /// For an end of a pipe: waits until either end is gone, and gives
    /// whether the other went first.
    fn other_end_gone(&mut self) -> Option<Gone> {
        None
    }
}

/// What `Host::other_end_gone` waits for.
pub(crate) type Gone = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// What tells, for a stream that is an end of a pipe, whether its other
/// end goes first.
pub(crate) fn other_end_gone(stream: *mut StreamObject) -> Option<Gone> {
    let mut state = unsafe { (*stream).state.lock().unwrap_or_else(|e| e.into_inner()) };
    state.host.as_mut()?.other_end_gone()
}

/// What a `Mode::Ready` host waits for: a socket of the workers' poller,
/// which takes no allocation, or another future.
pub(crate) enum Ready {
    Socket(super::poll::Ready),
    Other(std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>),
}

impl std::future::Future for Ready {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        match self.get_mut() {
            Ready::Socket(socket) => std::pin::Pin::new(socket).poll(context),
            Ready::Other(future) => future.as_mut().poll(context),
        }
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
    fn lock(&mut self, shared: bool, wait: bool) -> std::io::Result<bool> {
        use std::fs::TryLockError;
        let tried = match (shared, wait) {
            (false, true) => return std::fs::File::lock(self).map(|_| true),
            (true, true) => return self.lock_shared().map(|_| true),
            (false, false) => self.try_lock(),
            (true, false) => self.try_lock_shared(),
        };
        match tried {
            Ok(()) => Ok(true),
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(error)) => Err(error),
        }
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
        // A recorded run reads on its one thread, whose calls it records,
        // and the other tasks wait meanwhile.
        if super::replay::on() {
            drop(input);
            super::flush_stdout();
            let mut chunk = vec![0; READ_CHUNK];
            let read = std::io::stdin().read(&mut chunk);
            let mut input = console_input();
            match read {
                Ok(0) => input.end = true,
                Ok(count) => input.bytes.extend_from_slice(&chunk[..count]),
                Err(failure) => input.failure = Some(failure),
            }
            drop(input);
            return self.read(buffer);
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
            Ready::Other(Box::pin(async {
                loop {
                    let notified = INPUT_READ.notified();
                    let mut notified = std::pin::pin!(notified);
                    notified.as_mut().enable();
                    if !console_input().reading {
                        return;
                    }
                    notified.await;
                }
            }))
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

    pub(crate) fn lock(&mut self, shared: bool, wait: bool) -> std::io::Result<bool> {
        self.host()?.lock(shared, wait)
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

    /// What `make` makes of the next line without its `\n` or `\r\n`, or
    /// `None` at the end.
    pub(crate) fn read_line<T>(&mut self, make: impl FnOnce(&[u8]) -> T) -> std::io::Result<Option<T>> {
        loop {
            if let Some(end) = self.ahead[self.position..self.filled].iter().position(|&b| b == b'\n') {
                let line = &self.ahead[self.position..self.position + end];
                let line = make(line.strip_suffix(b"\r").unwrap_or(line));
                self.position += end + 1;
                return Ok(Some(line));
            }
            if !self.fill()? {
                if self.position == self.filled {
                    return Ok(None);
                }
                let line = make(&self.ahead[self.position..self.filled]);
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
pub(crate) fn run<R: Send + 'static>(stream: *mut StreamObject, op: impl FnMut(&mut StreamState) -> R + Send) -> R {
    run_until(stream, None, op)
}

/// Like `run`; on a `Mode::Ready` stream the operation fails with
/// `TimedOut` if it would wait past `deadline`.
pub(crate) fn run_until<R: Send + 'static>(
    stream: *mut StreamObject,
    deadline: Option<std::time::Instant>,
    mut op: impl FnMut(&mut StreamState) -> R + Send,
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
            // A socket's readiness, with no deadline, needs no runtime's
            // context to wait for.
            let context = deadline.is_some() || !matches!(blocked, Ready::Socket(_));
            let waited = wait_in(context, async move {
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

/// Whether a stream fails where it would wait, as a pipe's ends do, for
/// `attempt`.
pub(crate) fn waits_without_a_thread(stream: *mut StreamObject) -> bool {
    matches!(unsafe { (*stream).mode }, Mode::Ready)
}

/// One try of an operation on a `Mode::Ready` stream, from a future: its
/// result, and what to wait for before another when it would have waited.
/// Dropping the future that waits gives the operation up.
pub(crate) fn attempt<R>(stream: *mut StreamObject, op: impl FnOnce(&mut StreamState) -> R) -> (R, Option<Ready>) {
    let mut state = unsafe { (*stream).state.lock().unwrap_or_else(|e| e.into_inner()) };
    let result = op(&mut state);
    let blocked = state.host.as_mut().and_then(|host| host.blocked());
    (result, blocked)
}

/// Runs an operation in place when `ready` says the bytes already read
/// ahead, or the room left to gather writes, are enough; otherwise where
/// the stream's mode says.
pub(crate) fn run_ready<R: Send + 'static>(
    stream: *mut StreamObject,
    ready: impl FnOnce(&StreamState) -> bool,
    mut op: impl FnMut(&mut StreamState) -> R + Send,
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
        // Inside a poll `wait_for` makes, closing may wait, which would move
        // the task to another thread in the middle of the poll: it waits
        // until the poll returns.
        if HELD.with(|held| held.borrow_mut().as_mut().map(|held| held.push(stream as usize)).is_some()) {
            return;
        }
        free(stream);
    }
}

/// Closes a stream no one holds any more, and frees it.
unsafe fn free(stream: *mut StreamObject) {
    unsafe {
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

thread_local! {
    /// While `wait_for` polls a future on this thread, the streams it let go.
    static HELD: std::cell::RefCell<Option<Vec<usize>>> = const { std::cell::RefCell::new(None) };
}

/// One poll of a future in the runtime's context, which it has only while
/// polling: the task may resume on another thread. Not inlined, so that
/// the addresses of the thread's own variables are this thread's, not
/// those of the thread it ran on before.
#[inline(never)]
fn poll_once<F: std::future::Future>(
    future: std::pin::Pin<&mut F>,
    waker: &std::task::Waker,
    context: bool,
) -> (std::task::Poll<F::Output>, Vec<usize>) {
    if context {
        super::tasks::leave_host_thread();
    }
    let _context = context.then(|| super::http::runtime().enter());
    holding(|| future.poll(&mut std::task::Context::from_waker(waker)))
}

/// Runs the poll of a future of the host at a worker's turn, in the
/// runtime's context; the streams it let go are closed on a thread of the
/// pool, as closing may wait.
pub(crate) fn polled_here(poll: impl FnOnce()) {
    let ((), let_go) = {
        let _context = super::http::runtime().enter();
        holding(poll)
    };
    close_later(let_go);
}

/// Closes the streams a poll let go on a thread of the pool, as closing
/// may wait; a recorded run, which calls the system from its worker alone,
/// closes them there, where closing does its work in place and never waits.
fn close_later(let_go: Vec<usize>) {
    if let_go.is_empty() {
        return;
    }
    let close = move || {
        for stream in let_go {
            unsafe { free(stream as *mut StreamObject) };
        }
    };
    match super::replay::on() {
        true => close(),
        false => submit(Box::new(close), false),
    }
}

/// Runs a poll, during which the task must not wait: the streams let go
/// meanwhile are given back, to close once it returned.
fn holding<T>(poll: impl FnOnce() -> T) -> (T, Vec<usize>) {
    /// What a poll this one runs inside held, back once it ends.
    struct Outer(Option<Option<Vec<usize>>>);
    impl Drop for Outer {
        fn drop(&mut self) {
            if let Some(outer) = self.0.take() {
                HELD.with(|held| *held.borrow_mut() = outer);
            }
        }
    }
    let mut outer = Outer(Some(HELD.with(|held| held.borrow_mut().replace(Vec::new()))));
    let value = poll();
    let let_go = HELD.with(|held| std::mem::replace(&mut *held.borrow_mut(), outer.0.take().flatten()));
    (value, let_go.unwrap_or_default())
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
    crate::status(result, |error| io_status(&error), store)
}

pub(crate) fn utf8(bytes: Vec<u8>) -> std::io::Result<String> {
    String::from_utf8(bytes).map_err(|_| ErrorKind::InvalidData.into())
}

/// A string of the program's, which an operation may make on a thread of
/// the pool for the task that waits on it.
pub(crate) struct Made(pub(crate) *mut Str);

unsafe impl Send for Made {}

/// `bytes` as a string of the program's, if they are UTF-8.
pub(crate) fn made_text(bytes: &[u8]) -> Option<Made> {
    std::str::from_utf8(bytes).is_ok().then(|| Made(str_new(bytes)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_io_write(stream: *mut StreamObject, text: *const Str) -> i64 {
    let data = unsafe { str_bytes(text) };
    status(run_ready(stream, |s| s.has_room(data.len()), |s| s.write(data)), |_| ())
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
    let data = unsafe { array_items::<u8>(data) };
    status(run_ready(stream, |s| s.has_room(data.len()), |s| s.write(data)), |_| ())
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
    let line = run_ready(stream, StreamState::has_line, |s| {
        s.read_line(made_text)?.map(|line| line.ok_or_else(|| ErrorKind::InvalidData.into())).transpose()
    });
    status(line, |line| unsafe { *out = line.map_or(std::ptr::null_mut(), |l| l.0) })
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
    let line = run_ready(stream, StreamState::has_line, |s| {
        s.read_line(|line| Made(str_new(String::from_utf8_lossy(line).as_bytes()))).ok().flatten()
    });
    unsafe { tondo_rt_stream_release(stream) };
    line.map_or(std::ptr::null_mut(), |l| l.0)
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

/// Whether the console stream at `index`, 0 the output and 2 the input, is
/// still the process's own and a terminal.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_terminal(index: i64) -> u8 {
    let replaced = console().as_ref().unwrap().1[index as usize];
    (!replaced && unsafe { libc::isatty(index.clamp(0, 2) as i32) } == 1) as u8
}

/// The columns of the terminal of standard output, or 0 without one.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_width() -> i64 {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    match unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } {
        0 => size.ws_col as i64,
        _ => 0,
    }
}

/// Turns the echo of the terminal of standard input on or off, and says
/// whether it could.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_console_echo(on: u8) -> u8 {
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut settings) } != 0 {
        return 0;
    }
    if on != 0 {
        settings.c_lflag |= libc::ECHO;
    } else {
        settings.c_lflag &= !libc::ECHO;
    }
    (unsafe { libc::tcsetattr(0, libc::TCSANOW, &settings) } == 0) as u8
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
