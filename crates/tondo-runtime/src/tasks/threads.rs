//! The threads of the runtime and those of C that wait for it: how many there
//! are, how the runtime starts one, and how the program waits for those of C
//! and for its tasks to be freed as it ends (docs/refactor.md, phase 2).

use super::*;

/// Threads the runtime started that still run, the main thread included.
pub(crate) static RUNTIME_THREADS: AtomicUsize = AtomicUsize::new(1);
/// Threads of C waiting in `tondo_rt_callback` for the task they started,
/// which went on on a worker; while it runs on theirs, they can call back.
pub(super) static FOREIGN_WAITING: AtomicUsize = AtomicUsize::new(0);
/// Calls from threads of C that have not returned.
pub(super) static FOREIGN_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Threads an executable's process had before its program started, which
/// are not the program's: an emulator's, as qemu's. A library's host has
/// its own, which may call it.
pub(super) static BEFORE: AtomicUsize = AtomicUsize::new(0);

/// Waits, up to a second, for the calls from threads of C still running:
/// what they hold is not leaked when the program ends before them.
pub fn wait_foreign_calls() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while FOREIGN_CALLS.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Waits, up to a second, until every task is freed: a detached task that
/// `main`'s end cancelled is done before its thread lets go of what it held.
pub fn wait_tasks_freed() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while live_tasks() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Starts a runtime thread, counted while it runs.
pub(crate) fn spawn_thread<T: Send + 'static>(
    stack: Option<usize>,
    body: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    try_spawn_thread(stack, body).unwrap_or_else(|_| runtime_fault("cannot start a thread"))
}

/// Starts a runtime thread, or says why the system would not.
pub(crate) fn try_spawn_thread<T: Send + 'static>(
    stack: Option<usize>,
    body: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            crate::diagnostics::retire_here();
            RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
        }
    }
    RUNTIME_THREADS.fetch_add(1, Ordering::SeqCst);
    let mut builder = std::thread::Builder::new();
    if let Some(stack) = stack {
        builder = builder.stack_size(stack);
    }
    let started = builder.spawn(move || {
        let _running = Running;
        body()
    });
    if started.is_err() {
        RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
    }
    started
}

/// How many threads the process has, where the system tells.
#[cfg(target_os = "linux")]
pub(crate) fn os_threads() -> Option<usize> {
    std::fs::read_dir("/proc/self/task").ok().map(|entries| entries.count())
}

#[cfg(target_os = "macos")]
pub(crate) fn os_threads() -> Option<usize> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, port: u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    }
    unsafe {
        let task = mach_task_self_;
        let (mut threads, mut count) = (std::ptr::null_mut(), 0u32);
        if task_threads(task, &mut threads, &mut count) != 0 {
            return None;
        }
        for index in 0..count as usize {
            mach_port_deallocate(task, *threads.add(index));
        }
        vm_deallocate(task, threads as usize, count as usize * 4);
        Some(count as usize)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn os_threads() -> Option<usize> {
    None
}
