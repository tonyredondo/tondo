//! `io.pipe` and the writer of `io.multiWriter` (standard library 10.3),
//! which `std/streams.to` is written on. The ends of a pipe never block a
//! thread: a task that would wait gives the stream back and waits for the
//! other end to change, as the network's streams do.

use super::io::{Host, Mode, Ready, StreamObject, retain, run, stream, tondo_rt_stream_release};
use super::native::{self, NativeObject};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;

/// What both ends of a pipe share: the bytes written and not read yet, and
/// whether each end is gone.
struct Pipe {
    flow: Mutex<Flow>,
    changed: Notify,
    capacity: usize,
}

#[derive(Default)]
struct Flow {
    bytes: VecDeque<u8>,
    writer_gone: bool,
    reader_gone: bool,
}

impl Pipe {
    fn flow(&self) -> MutexGuard<'_, Flow> {
        self.flow.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn change(&self, change: impl FnOnce(&mut Flow)) {
        change(&mut self.flow());
        self.changed.notify_waiters();
    }

    /// What a task that would block waits for: `ready` to hold.
    fn until(self: &Arc<Self>, ready: fn(&Flow, usize) -> bool) -> Ready {
        let pipe = self.clone();
        Box::pin(async move {
            loop {
                let notified = pipe.changed.notified();
                let mut notified = std::pin::pin!(notified);
                notified.as_mut().enable();
                if ready(&pipe.flow(), pipe.capacity) {
                    return;
                }
                notified.await;
            }
        })
    }
}

/// An end of a pipe, and whether its last operation would have waited, or
/// fails instead of waiting.
struct End {
    pipe: Arc<Pipe>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

impl End {
    fn would_block(&mut self) -> std::io::Error {
        self.gave_up
            .unwrap_or_else(|| {
                self.waiting = true;
                ErrorKind::WouldBlock
            })
            .into()
    }
}

/// The writing end: a write waits while `capacity` bytes are unread, and
/// then takes all its bytes at once.
struct Sink(End);

impl Host for Sink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut flow = self.0.pipe.flow();
        if flow.reader_gone {
            return Err(ErrorKind::BrokenPipe.into());
        }
        if !flow.bytes.is_empty() && flow.bytes.len() >= self.0.pipe.capacity {
            drop(flow);
            return Err(self.0.would_block());
        }
        flow.bytes.extend(data);
        drop(flow);
        self.0.pipe.changed.notify_waiters();
        Ok(())
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.0.waiting)
            .then(|| self.0.pipe.until(|flow, capacity| flow.bytes.len() < capacity || flow.reader_gone))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.0.gave_up = kind;
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.0.pipe.change(|flow| flow.writer_gone = true);
    }
}

/// The reading end: a read waits for bytes, and ends once the writer is
/// gone and they are read.
struct Source(End);

impl Host for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut flow = self.0.pipe.flow();
        if flow.bytes.is_empty() {
            if flow.writer_gone {
                return Ok(0);
            }
            drop(flow);
            return Err(self.0.would_block());
        }
        let count = buffer.len().min(flow.bytes.len());
        for (slot, byte) in buffer.iter_mut().zip(flow.bytes.drain(..count)) {
            *slot = byte;
        }
        drop(flow);
        self.0.pipe.changed.notify_waiters();
        Ok(count)
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.0.waiting)
            .then(|| self.0.pipe.until(|flow, _| !flow.bytes.is_empty() || flow.writer_gone))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.0.gave_up = kind;
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.0.pipe.change(|flow| flow.reader_gone = true);
    }
}

/// The two ends of a new pipe, until `std/streams.to` takes them.
struct Ends(Mutex<(usize, usize)>);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pipe(capacity: i64) -> *mut NativeObject {
    let pipe = Arc::new(Pipe { flow: Mutex::default(), changed: Notify::new(), capacity: capacity.max(1) as usize });
    let end = || End { pipe: pipe.clone(), waiting: false, gave_up: None };
    let writer = stream(Box::new(Sink(end())), Mode::Ready, false);
    let reader = stream(Box::new(Source(end())), Mode::Ready, false);
    native::new(Ends(Mutex::new((writer as usize, reader as usize))))
}

/// The writing end of a pipe, taken once.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pipe_writer(ends: *mut NativeObject) -> *mut StreamObject {
    native::get::<Ends>(ends).map_or(std::ptr::null_mut(), |e| {
        std::mem::take(&mut e.0.lock().unwrap_or_else(|e| e.into_inner()).0) as *mut StreamObject
    })
}

/// The reading end of a pipe, taken once.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_pipe_reader(ends: *mut NativeObject) -> *mut StreamObject {
    native::get::<Ends>(ends).map_or(std::ptr::null_mut(), |e| {
        std::mem::take(&mut e.0.lock().unwrap_or_else(|e| e.into_inner()).1) as *mut StreamObject
    })
}

/// A stream another one writes to, which it lets go when it closes.
struct Inner(*mut StreamObject);

unsafe impl Send for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe { tondo_rt_stream_release(self.0) }
    }
}

/// A writer that writes to two others in their order: what
/// `io.multiWriter` folds its writers into.
struct Tee(Inner, Inner);

impl Host for Tee {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        for inner in [&self.0, &self.1] {
            let data = data.to_vec();
            run(inner.0, move |s| s.write(&data))?;
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        run(self.0.0, |s| s.flush())?;
        run(self.1.0, |s| s.flush())
    }
}

/// A writer to both streams, which may wait on either for as long as they
/// take, as a pipe's reader does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_tee(first: *mut StreamObject, second: *mut StreamObject) -> *mut StreamObject {
    stream(Box::new(Tee(Inner(retain(first)), Inner(retain(second)))), Mode::Long, false)
}
