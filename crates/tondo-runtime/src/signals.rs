//! Signals (language 20.10, standard library 19.2): `SIGINT` and `SIGTERM`
//! cancel a program as `job.cancel()` does, and a second one ends it at
//! once; `process.signals` delivers those it names to a channel instead. A
//! handler only writes the signal's number to a pipe, which a thread of its
//! own reads.

use super::tasks::{ChannelObject, SenderObject, tondo_rt_channel_new, tondo_rt_sender_new, tondo_rt_sender_release};
use super::{ArrayHeader, element};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

/// The signals of `process.Signal`, in the order of its variants.
pub(crate) const SIGNALS: [i32; 7] =
    [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2, libc::SIGQUIT, libc::SIGKILL];

/// The pipe's end the handler writes to.
static PIPE: AtomicI32 = AtomicI32::new(-1);
/// The signals the program asked for, as bits of their numbers.
static ROUTED: AtomicU32 = AtomicU32::new(0);
/// The channels `process.signals` gave.
static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());
/// Whether there is one, which a signal may wake a task waiting on.
static LISTENING: AtomicBool = AtomicBool::new(false);

/// A channel's sender, the size of its values and the signals it takes.
struct Subscriber {
    sender: *mut SenderObject,
    size: usize,
    wanted: u32,
}

unsafe impl Send for Subscriber {}

/// A signal came that cancels the program: a second one ends it at once.
static ENDING: AtomicBool = AtomicBool::new(false);
/// In a recorded run, the signals that came and the worker has not taken.
static PENDING: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// What a signal does: goes to the channels that take it, or cancels the
/// program, or ends it.
fn handle(signal: i32) {
    if routed(signal) {
        deliver(signal);
    } else if ENDING.swap(true, Ordering::SeqCst) {
        super::replay::ending();
        unsafe { libc::_exit(128 + signal) };
    } else {
        super::tasks::abandon(128 + signal);
    }
}

/// The worker's turn, in a recorded run: the signals that came since
/// arrive here, where the recording keeps them, and a replay gives them at
/// the same turn (toolchain 7.6).
pub(crate) fn at_turn() {
    static TURNS: AtomicU64 = AtomicU64::new(0);
    let turn = TURNS.fetch_add(1, Ordering::Relaxed);
    super::replay::turn();
    if super::replay::replaying() {
        while let Some(signal) = super::replay::signal_at(turn) {
            handle(signal);
        }
        return;
    }
    let pending = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    for signal in pending {
        super::replay::signal_came(turn, signal);
        handle(signal);
    }
}

extern "C" fn caught(signal: i32) {
    // It may interrupt the program's thread: this write is not the program's.
    let _outside = super::replay::Outside::enter();
    let byte = signal as u8;
    unsafe { libc::write(PIPE.load(Ordering::Relaxed), (&raw const byte).cast(), 1) };
}

fn catch(signal: i32) {
    unsafe { libc::signal(signal, caught as *const () as libc::sighandler_t) };
}

/// Catches `SIGINT` and `SIGTERM` from now on, once.
pub(crate) fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        // A replay takes the signals its recording has, and a recorded run
        // catches them with calls that are not the program's.
        if super::replay::replaying() {
            return;
        }
        let _outside = super::replay::Outside::enter();
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        PIPE.store(fds[1], Ordering::Relaxed);
        let reader = fds[0];
        let watching = std::thread::Builder::new().name("signals".into()).stack_size(64 * 1024).spawn(move || {
            loop {
                let mut byte = 0u8;
                match unsafe { libc::read(reader, (&raw mut byte).cast(), 1) } {
                    1 => {}
                    -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
                    _ => return,
                }
                let signal = byte as i32;
                // `kill -QUIT` asks a program that dumps for one, and it
                // goes on; another says where its tasks are, and ends.
                if signal == libc::SIGQUIT && !routed(signal) {
                    if std::env::var_os("TONDO_DUMP").is_some() {
                        super::diagnostics::dump_on_signal();
                        continue;
                    }
                    super::diagnostics::tasks_to_stderr();
                    unsafe {
                        libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                        libc::raise(libc::SIGQUIT);
                    }
                    continue;
                }
                // A recorded run takes it at the worker's next turn; a
                // replay, where its recording does.
                if super::replay::on() {
                    if !super::replay::replaying() {
                        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(signal);
                        super::tasks::wake_worker();
                    }
                    continue;
                }
                handle(signal);
            }
        });
        if watching.is_ok() {
            catch(libc::SIGINT);
            catch(libc::SIGTERM);
            if !super::replay::on() {
                catch(libc::SIGQUIT);
            }
        }
    });
}

/// Whether a channel still takes the signal: those whose receivers the
/// program let go are let go first, and a signal only they took cancels
/// the program again.
fn routed(signal: i32) -> bool {
    let mut subscribers = lock();
    subscribers.retain(|subscriber| {
        let heard = unsafe { !super::tasks::unheard(subscriber.sender) };
        if !heard {
            unsafe { tondo_rt_sender_release(subscriber.sender) };
        }
        heard
    });
    let mask = subscribers.iter().fold(0, |mask, subscriber| mask | subscriber.wanted);
    ROUTED.store(mask, Ordering::SeqCst);
    LISTENING.store(!subscribers.is_empty(), Ordering::SeqCst);
    mask & (1 << signal) != 0
}

/// Sends the signal to each channel that takes it.
fn deliver(signal: i32) {
    let Some(tag) = SIGNALS.iter().position(|&s| s == signal) else { return };
    for subscriber in lock().iter().filter(|s| s.wanted & (1 << signal) != 0) {
        let mut value = vec![0u8; subscriber.size.max(1)];
        value[0] = tag as u8;
        unsafe { super::tasks::offer_new(subscriber.sender, value.as_mut_ptr()) };
    }
}

fn lock() -> std::sync::MutexGuard<'static, Vec<Subscriber>> {
    SUBSCRIBERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `process.signals(wanted)`: a channel of the signals of the array, each
/// `stride` bytes, whose values are `size` bytes aligned to `align`. They
/// no longer cancel or end the program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_signals(
    wanted: *mut ArrayHeader,
    stride: i64,
    size: i64,
    align: i64,
) -> *mut ChannelObject {
    start();
    let count = if wanted.is_null() { 0 } else { unsafe { (*wanted).len } };
    let mut mask = 0u32;
    for index in 0..count {
        let tag = unsafe { *element(wanted, stride as usize, index) } as usize;
        // `Kill` ends a process before it can answer: it is never received.
        if let Some(&signal) = SIGNALS.get(tag).filter(|&&signal| signal != libc::SIGKILL) {
            mask |= 1 << signal;
        }
    }
    let channel =
        unsafe { tondo_rt_channel_new(16, 1, size as usize, align as usize, None, None, std::ptr::null(), 0) };
    let sender = unsafe { tondo_rt_sender_new(channel) };
    lock().push(Subscriber { sender, size: size as usize, wanted: mask });
    LISTENING.store(true, Ordering::SeqCst);
    let before = ROUTED.fetch_or(mask, Ordering::SeqCst);
    for signal in SIGNALS {
        if mask & !before & (1 << signal) != 0 {
            catch(signal);
        }
    }
    channel
}

/// Ends the program as the signal of `process.Signal` number `tag` does
/// when nothing listens for it, once a server that listened for it
/// drained: its tasks are cancelled, the caller's too, and it ends with
/// 128 + the signal.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_signal_end(tag: i64) {
    if let Some(&signal) = SIGNALS.get(tag as usize) {
        super::tasks::abandon_all(128 + signal);
    }
}

/// Whether a channel of signals exists: then every task waiting is not a
/// deadlock, as a signal may come.
pub(crate) fn listening() -> bool {
    LISTENING.load(Ordering::SeqCst)
}

/// Lets go of the channels, when the program ends.
pub(crate) fn end() {
    for subscriber in lock().drain(..) {
        unsafe { tondo_rt_sender_release(subscriber.sender) };
    }
}
