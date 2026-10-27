//! Interrupting `tondo test` (testing 7.10). The first `SIGINT`, or the
//! `SIGTERM` an editor ends a run with, stops the dispatch of tests and
//! asks each worker to stop with `SIGTERM`: it cancels its tasks, which run
//! their cleanups, and ends. A worker still running after `GRACE`, or at a
//! second signal, is killed with what it started. The run then writes no
//! report and exits with `4`, or with `3` when a worker had to be killed.

use std::process::{Child, Command};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the workers have to stop; longer than a worker gives its own
/// tasks after a panic.
const GRACE: Duration = Duration::from_secs(10);

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Under `--fuzz`, while workers search: the files that stop them, which
/// the first interruption makes instead of stopping the run.
static SOFT: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
static FORCED: AtomicBool = AtomicBool::new(false);
/// The workers alive, by process id, which is also their group's.
static WORKERS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Whether the run was interrupted.
pub(super) fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// From now on the first interruption makes these files, which end the
/// search of `--fuzz` and let the run end as it does; none to end that.
pub(super) fn soften(stops: Vec<std::path::PathBuf>) {
    *SOFT.lock().unwrap_or_else(|e| e.into_inner()) = stops;
}

/// Whether a worker had to be killed.
pub(super) fn forced() -> bool {
    FORCED.load(Ordering::SeqCst)
}

fn workers() -> std::sync::MutexGuard<'static, Vec<u32>> {
    WORKERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a worker, unless the run was interrupted.
pub(super) fn start(command: &mut Command) -> Option<std::io::Result<Child>> {
    let mut workers = workers();
    if requested() {
        return None;
    }
    let child = command.spawn();
    if let Ok(child) = &child {
        workers.push(child.id());
    }
    Some(child)
}

/// A worker ended: `killed` when the interruption had to end it.
pub(super) fn ended(child: &Child, killed: bool) {
    workers().retain(|&pid| pid != child.id());
    if killed && requested() {
        FORCED.store(true, Ordering::SeqCst);
    }
}

/// Handles `SIGINT` and `SIGTERM` from now on, once for the invocation.
#[cfg(unix)]
pub(super) fn listen() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(listening);
}

#[cfg(unix)]
fn listening() {
    static PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn interrupted(_: i32) {
        let byte = 1u8;
        unsafe { libc::write(PIPE.load(Ordering::Relaxed), (&raw const byte).cast(), 1) };
    }
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    PIPE.store(fds[1], Ordering::Relaxed);
    let next = move || {
        let mut byte = 0u8;
        while unsafe { libc::read(fds[0], (&raw mut byte).cast(), 1) } != 1 {}
    };
    let signal = |signal: i32| {
        for &pid in workers().iter() {
            // The second time, its group: what the worker started.
            let target = if signal == libc::SIGKILL { -(pid as i32) } else { pid as i32 };
            unsafe { libc::kill(target, signal) };
        }
    };
    std::thread::spawn(move || {
        next();
        // A search ends, and the run ends as it does.
        let stops = std::mem::take(&mut *SOFT.lock().unwrap_or_else(|e| e.into_inner()));
        if !stops.is_empty() {
            for stop in &stops {
                let _ = std::fs::write(stop, "");
            }
            eprintln!("\nstopping the search; interrupt again to end the run now");
            next();
        }
        REQUESTED.store(true, Ordering::SeqCst);
        eprintln!("\ninterrupted: the running tests are being cancelled; interrupt again to end them now");
        signal(libc::SIGTERM);
        std::thread::spawn(move || {
            std::thread::sleep(GRACE);
            signal(libc::SIGKILL);
        });
        next();
        signal(libc::SIGKILL);
    });
    for signal in [libc::SIGINT, libc::SIGTERM] {
        unsafe { libc::signal(signal, interrupted as *const () as libc::sighandler_t) };
    }
}

#[cfg(not(unix))]
pub(super) fn listen() {}
