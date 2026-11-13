//! A library's runtime (toolchain 7.7): it starts at the first call from C,
//! and `<name>_stop` ends it, after which the library can be unloaded and
//! nothing of the process points into its code. Until then the library
//! holds itself loaded, as a `dlclose` of it would leave its threads, its
//! handlers of signals and what it registered for the end of the process
//! pointing at nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set from the first call until `<name>_stop`.
static STARTED: AtomicBool = AtomicBool::new(false);

/// What a start and an end do, one at a time: whether the compiled code
/// gave its tables, which stay, and the handle that holds the library
/// loaded while it runs.
static STATE: Mutex<State> = Mutex::new(State { tables: false, held: 0 });

struct State {
    tables: bool,
    held: usize,
}

/// Starts a library at the first call from C, and at the first after its
/// end: the runtime reads its settings and the compiled code hands it its
/// tables. What the library wrote and did not deliver yet goes out when the
/// process ends, or when the library ends.
pub(crate) fn start_library(start: unsafe extern "C" fn()) {
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    if !state.tables {
        crate::LIBRARY.store(true, Ordering::Relaxed);
        crate::config::config();
        #[cfg(tondo_staticlib)]
        crate::meta::start();
        crate::diagnostics::start();
        unsafe { start() };
        state.tables = true;
    } else {
        crate::diagnostics::start_again();
    }
    state.held = hold_loaded();
    extern "C" fn flush(_: *mut u8) {
        crate::flush_stdout();
    }
    crate::at_exit(flush);
    STARTED.store(true, Ordering::Release);
}

/// A handle of the library itself, which keeps it loaded, or 0.
fn hold_loaded() -> usize {
    let mut found: libc::Dl_info = unsafe { std::mem::zeroed() };
    let address = &STARTED as *const AtomicBool as *const libc::c_void;
    if unsafe { libc::dladdr(address, &mut found) } == 0 || found.dli_fname.is_null() {
        return 0;
    }
    unsafe { libc::dlopen(found.dli_fname, libc::RTLD_NOW | libc::RTLD_NOLOAD) as usize }
}

/// `<name>_stop`: ends the library's runtime. Its tasks stop at their next
/// wait and run their `defer`s, every thread it started ends, the handlers
/// of signals it installed give way to those there were, and what it left
/// for the end of the process runs now. A call in progress on another
/// thread finishes first. Nothing when it is not running, or when a task
/// of its own asks.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_stop() {
    if !crate::tasks::current().is_null() {
        return;
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    crate::tasks::stop_runtime();
    crate::trace::give_back_handlers();
    crate::shared::flush();
    // What it registered for the end of the process runs now; glibc and
    // macOS then forget it, which musl does not need, as it never unloads.
    crate::run_at_exit();
    unsafe extern "C" {
        fn __cxa_finalize(image: *mut u8);
    }
    unsafe { __cxa_finalize(crate::image()) };
    STARTED.store(false, Ordering::Release);
    let held = std::mem::take(&mut state.held);
    drop(state);
    if held != 0 {
        unsafe { libc::dlclose(held as *mut libc::c_void) };
    }
}
