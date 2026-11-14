//! The terminal of `std.cli` (standard library 14.32): one owner of its
//! mode, a stack whose bottom is the mode the program started with. A
//! `cli.Terminal` pushes its mode, and a question the mode it needs while it
//! asks; whatever ends a level, a return, a cancellation or a panic, puts
//! back the one below. Ctrl-Z gives the terminal back as it was before the
//! program stops, and `fg` takes it again. A change of size, and coming
//! back, send the size to draw at to the channels of `cli.redraws()`; keys
//! are read from standard input while the top level asks for them, and
//! decoded here.

use super::native::{self, NativeObject};
use super::tasks::{ChannelObject, SenderObject, tondo_rt_channel_new, tondo_rt_sender_new, tondo_rt_sender_release};
use super::{ArrayHeader, arrays::int_array};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;

/// What a level asks of the terminal: keys as they are pressed or lines,
/// shown or not, and, for the program's own, the alternate screen and
/// whether Ctrl-C and Ctrl-Z are signals. A question keeps what the program
/// chose of those two.
#[derive(Clone, Copy)]
struct Mode {
    echo: bool,
    lines: bool,
    screen: bool,
    signals: Option<bool>,
}

struct Level {
    id: u64,
    mode: Mode,
    program: bool,
    keys: Arc<Keys>,
}

/// The keys pressed for a level, decoded, until it ends.
#[derive(Default)]
struct Keys {
    state: Mutex<KeysState>,
    ready: Notify,
}

#[derive(Default)]
struct KeysState {
    queue: VecDeque<i64>,
    /// Something reads them: until then the program reads the input itself.
    wanted: bool,
    ended: bool,
}

impl Keys {
    fn state(&self) -> MutexGuard<'_, KeysState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn end(&self) {
        self.state().ended = true;
        self.ready.notify_one();
    }
}

struct Stack {
    /// The mode the program started with, kept when the first level came.
    bottom: Option<libc::termios>,
    levels: Vec<Level>,
    /// The alternate screen is shown.
    screen: bool,
    next: u64,
    /// The program is ending, with the terminal given back: the levels its
    /// cleanups end take nothing again.
    ended: bool,
}

static STACK: Mutex<Stack> =
    Mutex::new(Stack { bottom: None, levels: Vec::new(), screen: false, next: 1, ended: false });

fn stack() -> MutexGuard<'static, Stack> {
    STACK.lock().unwrap_or_else(|e| e.into_inner())
}

const ENTER_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_SCREEN: &[u8] = b"\x1b[?1049l";

fn write_out(bytes: &[u8]) {
    unsafe { libc::write(1, bytes.as_ptr().cast(), bytes.len()) };
}

impl Stack {
    /// Sets the terminal to the mode of the top level, or to the bottom's.
    fn apply(&mut self) {
        let Some(bottom) = self.bottom.filter(|_| !self.ended) else { return };
        let mut settings = bottom;
        let screen = self.levels.iter().any(|level| level.mode.screen);
        if let Some(top) = self.levels.last() {
            let signals = self.levels.iter().rev().find_map(|level| level.mode.signals).unwrap_or(true);
            let flags = &mut settings.c_lflag;
            *flags = if top.mode.echo { *flags | libc::ECHO } else { *flags & !libc::ECHO };
            *flags = if signals { *flags | libc::ISIG } else { *flags & !libc::ISIG };
            if top.mode.lines {
                *flags |= libc::ICANON;
            } else {
                // Each key as it comes, Ctrl-V, Ctrl-S and Ctrl-Q too.
                *flags &= !(libc::ICANON | libc::IEXTEN);
                settings.c_iflag &= !libc::IXON;
                settings.c_cc[libc::VMIN] = 1;
                settings.c_cc[libc::VTIME] = 0;
            }
        }
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &settings) };
        if screen != self.screen {
            // What the program wrote goes to the screen it wrote it on.
            super::io::tondo_rt_console_flush();
            write_out(if screen { ENTER_SCREEN } else { LEAVE_SCREEN });
            self.screen = screen;
        }
    }

    /// The terminal as the program found it, the main screen shown, while
    /// a level holds it.
    fn restore(&mut self) {
        if self.levels.is_empty() {
            return;
        }
        if let Some(bottom) = self.bottom {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &bottom) };
        }
        if self.screen {
            write_out(LEAVE_SCREEN);
            self.screen = false;
        }
    }
}

/// A level of the stack, which a `cli.Terminal` or a question holds: it
/// ends when its last holder goes, or before with `release`.
struct Taken {
    id: u64,
    /// 0 when it took the terminal, 1 without one, 2 when a `cli.Terminal`
    /// already held it.
    status: i64,
    keys: Arc<Keys>,
}

impl Drop for Taken {
    fn drop(&mut self) {
        release(self.id);
    }
}

/// Whether standard input is the process's own and a terminal.
fn input_is_terminal() -> bool {
    super::io::tondo_rt_console_terminal(2) != 0
}

/// `terminalTake(echo, lines, screen, signals, program)`: a level of the
/// stack, `signals` -1 to keep the program's. One `cli.Terminal` at a time.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_take(
    echo: u8,
    lines: u8,
    screen: u8,
    signals: i64,
    program: u8,
) -> *mut NativeObject {
    let keys = Arc::new(Keys::default());
    let mut stack = stack();
    let status = if !input_is_terminal() {
        1
    } else if program != 0 && stack.levels.iter().any(|level| level.program) {
        2
    } else {
        0
    };
    if status != 0 {
        keys.end();
        return native::new(Taken { id: 0, status, keys });
    }
    if stack.levels.is_empty() {
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut settings) } != 0 {
            keys.end();
            return native::new(Taken { id: 0, status: 1, keys });
        }
        stack.bottom = Some(settings);
        started();
        // Ctrl-Z gives the terminal back before the program stops.
        super::signals::catch(libc::SIGTSTP);
    }
    let id = stack.next;
    stack.next += 1;
    let mode = Mode {
        echo: echo != 0,
        lines: lines != 0,
        screen: screen != 0,
        signals: (signals >= 0).then_some(signals != 0),
    };
    stack.levels.push(Level { id, mode, program: program != 0, keys: keys.clone() });
    stack.apply();
    drop(stack);
    WANTED.notify_waiters();
    native::new(Taken { id, status: 0, keys })
}

/// `terminalStatus`: 0 when the level took the terminal, 1 when there is
/// none, 2 when a `cli.Terminal` already held it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_status(object: *mut NativeObject) -> i64 {
    native::get::<Taken>(object).map_or(1, |taken| taken.status)
}

/// `terminalRelease`: ends the level now; its keys end with it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_release(object: *mut NativeObject) {
    if let Some(taken) = native::get::<Taken>(object) {
        release(taken.id);
    }
}

fn release(id: u64) {
    if id == 0 {
        return;
    }
    let mut stack = stack();
    let Some(at) = stack.levels.iter().position(|level| level.id == id) else { return };
    let level = stack.levels.remove(at);
    level.keys.end();
    stack.apply();
    if stack.levels.is_empty() {
        unsafe { libc::signal(libc::SIGTSTP, libc::SIG_DFL) };
    }
    drop(stack);
    WANTED.notify_waiters();
}

/// Once: the signals that change the terminal are caught. The runtime gives
/// the terminal back wherever the program ends (`give_back`), not from
/// `atexit`, which a program linked without a C compiler does not have.
fn started() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        super::signals::start();
        super::signals::catch(libc::SIGWINCH);
        super::signals::catch(libc::SIGCONT);
    });
}

/// The terminal as the program found it, when the program ends, before
/// what it says of a panic, so that it shows on the main screen: also from
/// a crash, so it does not wait for a lock another thread holds.
pub(crate) fn give_back() {
    if let Ok(mut stack) = STACK.try_lock() {
        stack.restore();
        stack.ended = true;
    }
}

/// Runs `f`, as `exec`, with the terminal as the program found it, and
/// takes it again if `f` returns.
pub(crate) fn around<T>(f: impl FnOnce() -> T) -> T {
    stack().restore();
    let result = f();
    let mut stack = stack();
    if !stack.levels.is_empty() {
        stack.apply();
    }
    result
}

/// On Ctrl-Z, from the thread of the signals: gives the terminal back and
/// stops the program as the system would, and takes it again on `fg`.
pub(crate) fn suspend() {
    stack().restore();
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        libc::raise(libc::SIGTSTP);
    }
    // Running again, or never stopped, as a program no shell controls is
    // not; it may have let the terminal go meanwhile.
    let mut stack = stack();
    if !stack.levels.is_empty() {
        stack.apply();
        super::signals::catch(libc::SIGTSTP);
    }
}

/// On `fg`, from the thread of the signals: the terminal in the program's
/// mode again. The redraw comes after, at the worker's turn.
pub(crate) fn resume() {
    let mut stack = stack();
    if !stack.levels.is_empty() {
        stack.apply();
    }
}

// ---------------------------------------------------------------------------
// The size, and redrawing
// ---------------------------------------------------------------------------

/// The columns and rows of the terminal of the console's output.
fn size() -> Option<(i64, i64)> {
    if super::io::tondo_rt_console_terminal(0) == 0 {
        return None;
    }
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } != 0 || size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    Some((i64::from(size.ws_col), i64::from(size.ws_row)))
}

/// `terminalSize`: the columns and the rows, or nothing without a terminal.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_size() -> *mut ArrayHeader {
    match size() {
        Some((columns, rows)) => int_array(&[columns, rows]),
        None => int_array(&[]),
    }
}

/// The senders of the channels `cli.redraws()` gave.
static REDRAWS: Mutex<Vec<Sender>> = Mutex::new(Vec::new());
/// Whether one is left, which a change of size may wake a task waiting on.
static LISTENING: AtomicBool = AtomicBool::new(false);

struct Sender(*mut SenderObject);

unsafe impl Send for Sender {}

/// `terminalRedraws`: a channel of the sizes to draw at, a `cli.Size` each,
/// two `Int`s; closed at once without a terminal.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_redraws() -> *mut ChannelObject {
    let channel = unsafe { tondo_rt_channel_new(1, 1, 16, 8, None, None, std::ptr::null(), 0) };
    let sender = unsafe { tondo_rt_sender_new(channel) };
    if size().is_none() {
        unsafe { tondo_rt_sender_release(sender) };
        return channel;
    }
    started();
    REDRAWS.lock().unwrap_or_else(|e| e.into_inner()).push(Sender(sender));
    LISTENING.store(true, Ordering::SeqCst);
    channel
}

/// A change of size, or the program back from a stop: each channel gets
/// the size of now, in place of one it has not given yet.
pub(crate) fn redraw() {
    let Some((columns, rows)) = size() else { return };
    let mut value = [columns, rows];
    let mut senders = REDRAWS.lock().unwrap_or_else(|e| e.into_inner());
    senders.retain(|sender| {
        let heard = unsafe { !super::tasks::unheard(sender.0) };
        if !heard {
            unsafe { tondo_rt_sender_release(sender.0) };
        }
        heard
    });
    for sender in senders.iter() {
        unsafe { super::tasks::offer_latest(sender.0, value.as_mut_ptr().cast()) };
    }
    LISTENING.store(!senders.is_empty(), Ordering::SeqCst);
}

/// Whether a channel of redraws is left: a task waiting on it waits for the
/// person at the terminal, which is no deadlock.
pub(crate) fn listening() -> bool {
    LISTENING.load(Ordering::SeqCst)
}

/// Lets go of the channels, when the program ends.
pub(crate) fn end() {
    for sender in REDRAWS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        unsafe { tondo_rt_sender_release(sender.0) };
    }
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// A level's wish for keys changed, or the top level did.
static WANTED: Notify = Notify::const_new();

/// The keys of a level, which `terminalKey` reads, as a value of its own: a
/// task that forwards them holds it, not the level.
struct KeysOf(Arc<Keys>);

/// `terminalKeys`: what reads the keys of the level, from now on.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_keys(object: *mut NativeObject) -> *mut NativeObject {
    let keys = native::get::<Taken>(object).map_or_else(
        || {
            let keys = Arc::new(Keys::default());
            keys.end();
            keys
        },
        |taken| taken.keys.clone(),
    );
    if !keys.state().ended {
        keys.state().wanted = true;
        reading();
        WANTED.notify_waiters();
    }
    native::new(KeysOf(keys))
}

/// `terminalKey`: the next key, as `key_code` gives it, or -1 once the
/// level ended or the task is cancelled.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_terminal_key(object: *mut NativeObject) -> i64 {
    let Some(KeysOf(keys)) = native::get::<KeysOf>(object) else { return -1 };
    let keys = keys.clone();
    super::http::wait(async move {
        loop {
            {
                let mut state = keys.state();
                if let Some(key) = state.queue.pop_front() {
                    return key;
                }
                if state.ended {
                    return -1;
                }
            }
            keys.ready.notified().await;
        }
    })
    .unwrap_or(-1)
}

/// Once: the task that reads standard input while the top level wants
/// keys, on the runtime of the host, which a recorded run drives from its
/// own thread, so that the keys are in the recording.
fn reading() {
    static READING: std::sync::Once = std::sync::Once::new();
    READING.call_once(|| {
        super::http::runtime().spawn(read_keys());
    });
}

/// The keys of the top level, when it reads keys.
fn top_wanting(stack: &Stack) -> Option<Arc<Keys>> {
    let top = stack.levels.last()?;
    (!top.mode.lines && top.keys.state().wanted).then(|| top.keys.clone())
}

/// How many bytes standard input has to read now.
fn available() -> usize {
    let mut count: libc::c_int = 0;
    match unsafe { libc::ioctl(0, libc::FIONREAD, &mut count) } {
        0 => count.max(0) as usize,
        _ => 0,
    }
}

/// Reads what standard input has, for the level that wants keys: under the
/// stack's lock, so that a question that comes after takes what is typed
/// after it.
fn take_bytes(pending: &mut Vec<u8>) -> Option<Arc<Keys>> {
    let stack = stack();
    let keys = top_wanting(&stack)?;
    let count = available();
    if count > 0 {
        let start = pending.len();
        pending.resize(start + count, 0);
        let read = unsafe { libc::read(0, pending[start..].as_mut_ptr().cast(), count) };
        pending.truncate(start + read.max(0) as usize);
    }
    Some(keys)
}

async fn read_keys() {
    use tokio::io::{Interest, unix::AsyncFd};
    // Standard input stays open while the program runs.
    let Ok(input) = (unsafe { AsyncFd::register_with_interest(0 as std::os::fd::RawFd, Interest::READABLE) }) else {
        return;
    };
    // Until standard input has bytes to read.
    let bytes_come = || async {
        loop {
            let mut guard = input.readable().await?;
            if available() > 0 {
                return std::io::Result::Ok(());
            }
            guard.clear_ready();
        }
    };
    let mut pending = Vec::new();
    loop {
        let changed = WANTED.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let Some(keys) = take_bytes(&mut pending) else {
            pending.clear();
            changed.await;
            continue;
        };
        let (codes, used) = decode(&pending, false);
        pending.drain(..used);
        let deliver = |codes: Vec<i64>| {
            if !codes.is_empty() {
                keys.state().queue.extend(codes);
                keys.ready.notify_one();
            }
        };
        deliver(codes);
        if !pending.is_empty() {
            // A sequence cut short: the rest comes at once, or a lone
            // Escape is told from the start of one by the pause after it.
            if tokio::time::timeout(std::time::Duration::from_millis(30), bytes_come()).await.is_err() {
                let (codes, used) = decode(&pending, true);
                pending.drain(..used);
                deliver(codes);
            }
            continue;
        }
        tokio::select! {
            came = bytes_come() => if came.is_err() {
                return;
            },
            _ = changed => {}
        }
    }
}

// The keys of `cli.Key`, as `terminalKey` gives them: the variant above 32
// bits, and below the character or the number of a function key.
const CHAR: i64 = 0;
const CTRL: i64 = 1;
const ALT: i64 = 2;
const ENTER: i64 = 3;
const TAB: i64 = 4;
const BACK_TAB: i64 = 5;
const BACKSPACE: i64 = 6;
const DELETE: i64 = 7;
const INSERT: i64 = 8;
const ESCAPE: i64 = 9;
const UP: i64 = 10;
const DOWN: i64 = 11;
const LEFT: i64 = 12;
const RIGHT: i64 = 13;
const HOME: i64 = 14;
const END: i64 = 15;
const PAGE_UP: i64 = 16;
const PAGE_DOWN: i64 = 17;
const FUNCTION: i64 = 18;

fn key(variant: i64, value: u32) -> i64 {
    (variant << 32) | i64::from(value)
}

/// The keys of the bytes, and how many bytes they took. A sequence cut
/// short at the end is left for more bytes, unless `last`: then an Escape
/// is a key of its own.
fn decode(bytes: &[u8], last: bool) -> (Vec<i64>, usize) {
    let mut keys = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match one(&bytes[at..], last) {
            Some((code, used)) => {
                keys.extend(code);
                at += used;
            }
            None => break,
        }
    }
    (keys, at)
}

/// The key the bytes start with, and how many it took; none for a sequence
/// cut short, and no key for one this does not know.
fn one(bytes: &[u8], last: bool) -> Option<(Option<i64>, usize)> {
    let first = bytes[0];
    let simple = |code| Some((Some(code), 1));
    match first {
        b'\r' | b'\n' => simple(key(ENTER, 0)),
        b'\t' => simple(key(TAB, 0)),
        0x7f | 0x08 => simple(key(BACKSPACE, 0)),
        0x1b => escape(bytes, last),
        0 => simple(key(CTRL, ' ' as u32)),
        1..=26 => simple(key(CTRL, u32::from(b'a' + first - 1))),
        28..=31 => simple(key(CTRL, u32::from(b"\\]^_"[usize::from(first - 28)]))),
        _ => {
            let length = match first {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => 1,
            };
            if bytes.len() < length {
                return if last { Some((None, bytes.len())) } else { None };
            }
            let character = std::str::from_utf8(&bytes[..length]).ok().and_then(|text| text.chars().next());
            Some((character.map(|c| key(CHAR, c as u32)), length))
        }
    }
}

/// A key that starts with Escape: an arrow or another key of a sequence,
/// Alt with a character, or Escape alone.
fn escape(bytes: &[u8], last: bool) -> Option<(Option<i64>, usize)> {
    let Some(&second) = bytes.get(1) else {
        return if last { Some((Some(key(ESCAPE, 0)), 1)) } else { None };
    };
    match second {
        b'[' => {
            // CSI: parameters, then the byte that ends it.
            let Some(end) = bytes[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                return if last { Some((Some(key(ALT, '[' as u32)), 2)) } else { None };
            };
            let parameters = std::str::from_utf8(&bytes[2..2 + end]).unwrap_or("");
            let number = parameters.split(';').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1);
            let code = match bytes[2 + end] {
                b'A' => Some(key(UP, 0)),
                b'B' => Some(key(DOWN, 0)),
                b'C' => Some(key(RIGHT, 0)),
                b'D' => Some(key(LEFT, 0)),
                b'H' => Some(key(HOME, 0)),
                b'F' => Some(key(END, 0)),
                b'Z' => Some(key(BACK_TAB, 0)),
                b'P' => Some(key(FUNCTION, 1)),
                b'Q' => Some(key(FUNCTION, 2)),
                b'R' => Some(key(FUNCTION, 3)),
                b'S' => Some(key(FUNCTION, 4)),
                b'~' => match number {
                    1 | 7 => Some(key(HOME, 0)),
                    2 => Some(key(INSERT, 0)),
                    3 => Some(key(DELETE, 0)),
                    4 | 8 => Some(key(END, 0)),
                    5 => Some(key(PAGE_UP, 0)),
                    6 => Some(key(PAGE_DOWN, 0)),
                    11..=15 => Some(key(FUNCTION, number - 10)),
                    17..=21 => Some(key(FUNCTION, number - 11)),
                    23..=24 => Some(key(FUNCTION, number - 12)),
                    _ => None,
                },
                _ => None,
            };
            Some((code, 3 + end))
        }
        b'O' => {
            let Some(&third) = bytes.get(2) else {
                return if last { Some((Some(key(ALT, 'O' as u32)), 2)) } else { None };
            };
            let code = match third {
                b'A' => Some(key(UP, 0)),
                b'B' => Some(key(DOWN, 0)),
                b'C' => Some(key(RIGHT, 0)),
                b'D' => Some(key(LEFT, 0)),
                b'H' => Some(key(HOME, 0)),
                b'F' => Some(key(END, 0)),
                b'P'..=b'S' => Some(key(FUNCTION, u32::from(third - b'P') + 1)),
                _ => None,
            };
            Some((code, 3))
        }
        // Escape, then a key that is not a character: Escape alone.
        0x1b | 0..=31 | 0x7f => Some((Some(key(ESCAPE, 0)), 1)),
        _ => match one(&bytes[1..], last)? {
            (Some(code), used) if code >> 32 == CHAR => Some((Some(key(ALT, code as u32)), used + 1)),
            _ => Some((Some(key(ESCAPE, 0)), 1)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<(i64, u32)> {
        decode(bytes, true).0.into_iter().map(|code| (code >> 32, code as u32)).collect()
    }

    #[test]
    fn sequences_become_keys() {
        assert_eq!(keys(b"a\x1b[A\x1b[B\x1bOC\x1b[D"), [(CHAR, 'a' as u32), (UP, 0), (DOWN, 0), (RIGHT, 0), (LEFT, 0)]);
        assert_eq!(keys(b"\x1b[1;5A"), [(UP, 0)], "Ctrl with an arrow is the arrow");
        assert_eq!(keys(b"\x1b[3~\x1b[5~\x1b[6~\x1b[2~"), [(DELETE, 0), (PAGE_UP, 0), (PAGE_DOWN, 0), (INSERT, 0)]);
        assert_eq!(keys(b"\x1bOP\x1b[15~\x1b[24~"), [(FUNCTION, 1), (FUNCTION, 5), (FUNCTION, 12)]);
        assert_eq!(keys(b"\x1b[H\x1b[F\x1b[1~\x1b[4~"), [(HOME, 0), (END, 0), (HOME, 0), (END, 0)]);
        assert_eq!(keys(b"\r\t\x7f\x1b[Z"), [(ENTER, 0), (TAB, 0), (BACKSPACE, 0), (BACK_TAB, 0)]);
        assert_eq!(keys(b"\x03\x1a\x00"), [(CTRL, 'c' as u32), (CTRL, 'z' as u32), (CTRL, ' ' as u32)]);
        assert_eq!(keys(b"\x1bx\x1b\xc3\xb1"), [(ALT, 'x' as u32), (ALT, 'ñ' as u32)]);
        assert_eq!(keys("ñ€".as_bytes()), [(CHAR, 'ñ' as u32), (CHAR, '€' as u32)]);
        assert_eq!(keys(b"\x1b"), [(ESCAPE, 0)]);
        assert_eq!(keys(b"\x1b\x1b[A"), [(ESCAPE, 0), (UP, 0)]);
    }

    #[test]
    fn a_sequence_cut_short_waits_for_the_rest() {
        assert_eq!(decode(b"a\x1b", false), (vec![key(CHAR, 'a' as u32)], 1));
        assert_eq!(decode(b"\x1b[1;", false).1, 0);
        assert_eq!(decode(b"\xc3", false).1, 0);
        assert_eq!(decode(b"\x1b[", true).0, [key(ALT, '[' as u32)]);
    }
}
