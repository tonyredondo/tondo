//! The sockets of `std.net`, polled by the workers themselves, as Go's
//! scheduler polls its network: a worker with nothing to run waits for
//! their events, and an event's task runs on that worker's thread, with no
//! other thread to wake on the way.

use std::future::Future;
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll as Polled, Waker};
use std::time::{Duration, Instant};

/// The token of the waker, which no socket has.
const WAKE: mio::Token = mio::Token(usize::MAX);

pub(crate) struct Poller {
    poll: Mutex<Polling>,
    registry: mio::Registry,
    waker: mio::Waker,
    sources: Mutex<Slots>,
}

/// The wait for events, which one worker at a time has.
pub(crate) struct Polling {
    poll: mio::Poll,
    events: mio::Events,
}

#[derive(Default)]
struct Slots {
    sources: Vec<Option<Arc<Source>>>,
    free: Vec<usize>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

static POLLER: OnceLock<Option<Poller>> = OnceLock::new();

/// The poller, once a socket needs it.
pub(crate) fn poller() -> Option<&'static Poller> {
    POLLER.get()?.as_ref()
}

fn start() -> Option<&'static Poller> {
    POLLER
        .get_or_init(|| {
            let poll = mio::Poll::new().ok()?;
            let registry = poll.registry().try_clone().ok()?;
            let waker = mio::Waker::new(poll.registry(), WAKE).ok()?;
            let sources = Mutex::new(Slots::default());
            let polling = Polling { poll, events: mio::Events::with_capacity(256) };
            Some(Poller { poll: Mutex::new(polling), registry, waker, sources })
        })
        .as_ref()
}

impl Poller {
    /// Ends the wait of the worker in `poll`, if any.
    pub(crate) fn wake(&self) {
        let _ = self.waker.wake();
    }

    /// Takes the poll, unless another worker has it.
    pub(crate) fn claim(&self) -> Option<MutexGuard<'_, Polling>> {
        self.poll.try_lock().ok()
    }

    /// Waits for events until `deadline`: what they make ready.
    pub(crate) fn wait(&self, mut polling: MutexGuard<'_, Polling>, deadline: Option<Instant>) -> Events {
        let timeout = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        let Polling { poll, events } = &mut *polling;
        if poll.poll(events, timeout).is_err() {
            // An interrupted wait: the worker looks again.
            drop(polling);
            std::thread::sleep(Duration::from_micros(50));
            return Events(Vec::new());
        }
        let slots = lock(&self.sources);
        let mut ready = Vec::new();
        for event in polling.events.iter() {
            if let Some(Some(source)) = slots.sources.get(event.token().0) {
                let read = event.is_readable() || event.is_read_closed() || event.is_error();
                let write = event.is_writable() || event.is_write_closed() || event.is_error();
                ready.push((source.clone(), read, write));
            }
        }
        Events(ready)
    }

    fn add(&self, source: Arc<Source>) -> usize {
        let mut slots = lock(&self.sources);
        match slots.free.pop() {
            Some(token) => {
                slots.sources[token] = Some(source);
                token
            }
            None => {
                slots.sources.push(Some(source));
                slots.sources.len() - 1
            }
        }
    }

    fn remove(&self, token: usize) {
        let mut slots = lock(&self.sources);
        slots.sources[token] = None;
        slots.free.push(token);
    }
}

/// The sockets a wait found ready, and for what.
pub(crate) struct Events(Vec<(Arc<Source>, bool, bool)>);

impl Events {
    /// Wakes what waits for them.
    pub(crate) fn wake(self) {
        for (source, read, write) in self.0 {
            source.ready(read, write);
        }
    }
}

/// What a socket's events wake: the tasks waiting to read or to write,
/// and what sends the output it holds when it can take more.
pub(crate) struct Source {
    state: Mutex<Readiness>,
}

#[derive(Default)]
struct Readiness {
    readable: bool,
    writable: bool,
    /// Counts the events, so that an operation that would block forgets a
    /// readiness only if no event came since it started.
    ticks: u64,
    readers: Vec<Waker>,
    writers: Vec<Waker>,
    on_writable: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Copy, Clone, PartialEq)]
pub(crate) enum Direction {
    Read,
    Write,
}

impl Source {
    fn ready(&self, read: bool, write: bool) {
        let (wakers, flush) = {
            let mut state = lock(&self.state);
            state.ticks += 1;
            let mut wakers = Vec::new();
            if read {
                state.readable = true;
                wakers.append(&mut state.readers);
            }
            if write {
                state.writable = true;
                wakers.append(&mut state.writers);
            }
            (wakers, if write { state.on_writable.clone() } else { None })
        };
        if let Some(flush) = flush {
            flush();
        }
        wakers.into_iter().for_each(Waker::wake);
    }
}

/// A socket of mio registered with the workers' poller, until it goes.
pub(crate) struct Registered<S: mio::event::Source> {
    io: S,
    source: Arc<Source>,
    token: usize,
}

impl<S: mio::event::Source> Registered<S> {
    pub(crate) fn new(mut io: S) -> std::io::Result<Registered<S>> {
        let poller = start().ok_or_else(|| std::io::Error::other("the network cannot be polled"))?;
        let source = Arc::new(Source { state: Mutex::new(Readiness::default()) });
        let token = poller.add(source.clone());
        let interest = mio::Interest::READABLE | mio::Interest::WRITABLE;
        if let Err(error) = poller.registry.register(&mut io, mio::Token(token), interest) {
            poller.remove(token);
            return Err(error);
        }
        Ok(Registered { io, source, token })
    }

    pub(crate) fn io(&self) -> &S {
        &self.io
    }

    /// Runs a non-blocking operation; when it would block, the readiness it
    /// waited on is forgotten unless an event came meanwhile.
    pub(crate) fn attempt<R>(
        &self,
        direction: Direction,
        mut operation: impl FnMut(&S) -> std::io::Result<R>,
    ) -> std::io::Result<R> {
        loop {
            let tick = lock(&self.source.state).ticks;
            let result = operation(&self.io);
            match &result {
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    let mut state = lock(&self.source.state);
                    if state.ticks == tick {
                        match direction {
                            Direction::Read => state.readable = false,
                            Direction::Write => state.writable = false,
                        }
                    }
                }
                _ => {}
            }
            return result;
        }
    }

    /// Becomes ready when the socket may be read or written again.
    pub(crate) fn ready(&self, direction: Direction) -> Ready {
        Ready { source: self.source.clone(), direction }
    }

    /// What sends held output when the socket can take more.
    pub(crate) fn on_writable(&self, flush: Arc<dyn Fn() + Send + Sync>) {
        lock(&self.source.state).on_writable = Some(flush);
    }
}

impl<S: mio::event::Source> Drop for Registered<S> {
    fn drop(&mut self) {
        if let Some(poller) = poller() {
            let _ = poller.registry.deregister(&mut self.io);
            poller.remove(self.token);
        }
        let mut state = lock(&self.source.state);
        state.on_writable = None;
        // Whoever waits finds the socket gone when it tries again.
        let mut wakers = std::mem::take(&mut state.readers);
        wakers.append(&mut state.writers);
        drop(state);
        wakers.into_iter().for_each(Waker::wake);
    }
}

/// The readiness of a socket for a direction, as a future.
pub(crate) struct Ready {
    source: Arc<Source>,
    direction: Direction,
}

impl Future for Ready {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Polled<()> {
        let mut state = lock(&self.source.state);
        let (ready, waiting) = match self.direction {
            Direction::Read => (state.readable, &mut state.readers),
            Direction::Write => (state.writable, &mut state.writers),
        };
        if ready {
            return Polled::Ready(());
        }
        if !waiting.iter().any(|w| w.will_wake(context.waker())) {
            waiting.push(context.waker().clone());
        }
        Polled::Pending
    }
}
