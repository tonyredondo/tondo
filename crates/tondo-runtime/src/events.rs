//! The last things a program did, which a dump shows (toolchain 7.5): a
//! short list of what is rare enough to read, kept only by a program that
//! dumps, so that a crash says what came before it. An event is a load
//! anywhere else.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

static LAST: Mutex<VecDeque<(Instant, u64, What)>> = Mutex::new(VecDeque::new());
/// How many it keeps.
const KEPT: usize = 256;

/// The same events as lines of JSON, in memory that a core keeps as it is:
/// the list above has no form that what reads a core could know. Written
/// with `LAST` held; what follows the mark is how much it ever wrote.
#[repr(C)]
struct Text {
    mark: [u8; 16],
    written: u64,
    bytes: [u8; TEXT],
}

const TEXT: usize = 1 << 16;
struct Shared(std::cell::UnsafeCell<Text>);
unsafe impl Sync for Shared {}
static RECORD: Shared =
    Shared(std::cell::UnsafeCell::new(Text { mark: *b"TONDOEVENTS1\n\0\0\0", written: 0, bytes: [0; TEXT] }));

/// Adds an event to what a core keeps, with `LAST` held.
fn record(task: u64, what: &What) {
    use super::diagnostics::quoted;
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let detail = match what {
        What::Spawned(other, _) => format!("\"what\":\"spawned\",\"other\":{other}"),
        What::Ended => "\"what\":\"ended\"".to_string(),
        What::Panicked(text) => format!("\"what\":\"panicked\",\"text\":{}", quoted(text)),
        What::Request(text) => format!("\"what\":\"request\",\"text\":{}", quoted(text)),
        What::Answered(text, ms) => format!("\"what\":\"answered\",\"text\":{},\"took_ms\":{ms}", quoted(text)),
        What::Waited(ms) => format!("\"what\":\"waited\",\"took_ms\":{ms}"),
        What::Paused(micros) => format!("\"what\":\"paused\",\"took_ms\":{}", micros / 1000),
    };
    let line = format!("{{\"time_ms\":{time},\"task\":{task},{detail}}}\n");
    let text = unsafe { &mut *RECORD.0.get() };
    for &byte in line.as_bytes() {
        text.bytes[text.written as usize % TEXT] = byte;
        text.written += 1;
    }
}

pub(crate) enum What {
    /// A task started another, from the code at the address.
    Spawned(u64, usize),
    Ended,
    Panicked(String),
    /// A request came, or was answered after so many milliseconds.
    Request(String),
    Answered(String, u64),
    /// A task waited long, for so many milliseconds.
    Waited(u64),
    /// The collector stopped the workers, for so many microseconds.
    Paused(u64),
}

#[inline]
pub(crate) fn on() -> bool {
    super::tasks::tools::listens(super::tasks::tools::EVENTS)
}

pub(crate) fn start() {
    super::tasks::tools::listen(super::tasks::tools::EVENTS, true);
}

/// Keeps what the running task did.
#[cold]
pub(crate) fn add(what: What) {
    add_of(super::tasks::current_id(), what);
}

/// Keeps what a task did.
#[cold]
pub(crate) fn add_of(task: u64, what: What) {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.len() == KEPT {
        last.pop_front();
    }
    record(task, &what);
    last.push_back((Instant::now(), task, what));
}

/// The events as JSON, from the oldest, each with how long before now it
/// was; `place` writes an address. A thread that stopped may hold the list,
/// and then a dump goes without it.
pub(crate) fn json(mut place: impl FnMut(usize) -> String) -> String {
    use super::diagnostics::quoted;
    let Ok(last) = LAST.try_lock() else { return "[]".to_string() };
    let now = Instant::now();
    let event = |(at, task, what): &(Instant, u64, What)| {
        let detail = match what {
            What::Spawned(other, code) => format!("\"what\":\"spawned\",\"other\":{other},\"at\":{}", place(*code)),
            What::Ended => "\"what\":\"ended\"".to_string(),
            What::Panicked(text) => format!("\"what\":\"panicked\",\"text\":{}", quoted(text)),
            What::Request(text) => format!("\"what\":\"request\",\"text\":{}", quoted(text)),
            What::Answered(text, ms) => format!("\"what\":\"answered\",\"text\":{},\"took_ms\":{ms}", quoted(text)),
            What::Waited(ms) => format!("\"what\":\"waited\",\"took_ms\":{ms}"),
            What::Paused(micros) => format!("\"what\":\"paused\",\"took_ms\":{}", micros / 1000),
        };
        format!("{{\"ago_ms\":{},\"task\":{task},{detail}}}", now.duration_since(*at).as_millis())
    };
    format!("[{}]", last.iter().map(event).collect::<Vec<_>>().join(","))
}
