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
    while (FOREIGN_CALLS.load(Ordering::SeqCst) > 0 || calls_on_their_stacks()) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Whether a call of C's runs on its thread's stack (`Task::native`), which
/// each thread counts for itself.
fn calls_on_their_stacks() -> bool {
    let every = EVERY_KEPT.lock().unwrap_or_else(|e| e.into_inner());
    every.iter().any(|&kept| unsafe { &*(kept as *const Kept) }.calling.load(Ordering::Acquire))
}

/// The next thread's shard of the registry.
static SHARD_TURN: AtomicUsize = AtomicUsize::new(0);

impl Kept {
    /// The number and the shard of the registry of a call on this thread's
    /// stack: numbers from a run of them this thread takes at a time, and
    /// its own shard, which few other threads share.
    pub(super) fn native(&self) -> (u64, u8) {
        const RUN: u64 = 1024;
        let (mut next, mut end) = self.ids.get();
        if next == end {
            next = TASK_IDS.fetch_add(RUN, Ordering::Relaxed);
            end = next + RUN;
        }
        self.ids.set((next + 1, end));
        (next, self.shard)
    }

    /// This thread's block for a call of C's that runs on its stack, made
    /// once.
    pub(super) fn call_block(&self) -> *mut Task {
        if self.call.get().is_null() {
            self.call.set(Box::into_raw(Box::new(std::mem::MaybeUninit::uninit())));
        }
        self.call.get() as *mut Task
    }

    /// Looks up the bounds of this thread's own stack, where a call of C's
    /// runs (`Task::native`), once, before the call: the system's answer
    /// may allocate, which a signal's handler cannot.
    pub(super) fn find_stack(&self) {
        if self.own_stack.get().1 == 0
            && let Some(bounds) = unsafe { own_stack_bounds() }
        {
            self.own_stack.set(bounds);
        }
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

/// A thread the runtime started, until it is joined: a library's end waits
/// for every one to be gone, its last steps too, which run the library's
/// code (toolchain 7.7). Those that ended are joined as others start.
struct Started {
    thread: libc::pthread_t,
    done: Arc<AtomicBool>,
}

unsafe impl Send for Started {}

static STARTED_THREADS: Mutex<Vec<Started>> = Mutex::new(Vec::new());

/// A thread the runtime started.
pub(crate) struct RuntimeThread {
    done: Arc<AtomicBool>,
}

impl RuntimeThread {
    /// Whether it ended.
    pub(crate) fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Waits for it to end. Once it ended, another start may have joined it
    /// already, and a new thread may have its `pthread_t`: it is known by
    /// its `done`.
    pub(crate) fn join(self) {
        let mut started = STARTED_THREADS.lock().unwrap_or_else(|e| e.into_inner());
        let found = started.iter().position(|s| Arc::ptr_eq(&s.done, &self.done));
        let found = found.map(|at| started.swap_remove(at));
        drop(started);
        if let Some(found) = found {
            unsafe { libc::pthread_join(found.thread, std::ptr::null_mut()) };
        }
    }
}

/// Starts a runtime thread, counted while it runs.
pub(crate) fn spawn_thread(stack: Option<usize>, body: impl FnOnce() + Send + 'static) -> RuntimeThread {
    try_spawn_thread(stack, body).unwrap_or_else(|_| runtime_fault("cannot start a thread"))
}

/// What a new thread runs.
struct Start {
    body: Box<dyn FnOnce() + Send>,
    done: Arc<AtomicBool>,
}

/// Starts a runtime thread, or says why the system would not. A thread of
/// POSIX's: std's `spawn` registers, on the thread that calls it, a
/// thread-local that drops, and in a library a thread of its host that
/// starts one would keep it loaded while it lives.
pub(crate) fn try_spawn_thread(
    stack: Option<usize>,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<RuntimeThread> {
    struct Running(Arc<AtomicBool>);
    impl Drop for Running {
        fn drop(&mut self) {
            crate::diagnostics::retire_here();
            RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
            self.0.store(true, Ordering::Release);
        }
    }
    extern "C" fn begin(start: *mut libc::c_void) -> *mut libc::c_void {
        RUNTIME_THREAD.with(|runtime| runtime.set(true));
        let start = unsafe { Box::from_raw(start as *mut Start) };
        let _running = Running(start.done);
        (start.body)();
        std::ptr::null_mut()
    }
    let done = Arc::new(AtomicBool::new(false));
    let start = Box::into_raw(Box::new(Start { body: Box::new(body), done: done.clone() }));
    RUNTIME_THREADS.fetch_add(1, Ordering::SeqCst);
    let mut thread: libc::pthread_t = unsafe { std::mem::zeroed() };
    let status = unsafe {
        let mut attributes: libc::pthread_attr_t = std::mem::zeroed();
        libc::pthread_attr_init(&mut attributes);
        // As std's, two megabytes unless said.
        let size = stack.unwrap_or(2 << 20).max(libc::PTHREAD_STACK_MIN);
        libc::pthread_attr_setstacksize(&mut attributes, size);
        let status = libc::pthread_create(&mut thread, &attributes, begin, start as *mut libc::c_void);
        libc::pthread_attr_destroy(&mut attributes);
        status
    };
    if status != 0 {
        drop(unsafe { Box::from_raw(start) });
        RUNTIME_THREADS.fetch_sub(1, Ordering::SeqCst);
        return Err(std::io::Error::from_raw_os_error(status));
    }
    let mut started = STARTED_THREADS.lock().unwrap_or_else(|e| e.into_inner());
    let mut at = 0;
    while at < started.len() {
        if started[at].done.load(Ordering::Acquire) {
            let ended = started.swap_remove(at);
            unsafe { libc::pthread_join(ended.thread, std::ptr::null_mut()) };
        } else {
            at += 1;
        }
    }
    started.push(Started { thread, done: done.clone() });
    Ok(RuntimeThread { done })
}

thread_local! {
    /// Set on the threads the runtime started.
    static RUNTIME_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Moves the running task of a call from C, in a library, from the thread
/// of its host onto a worker, before it uses what keeps thread-locals that
/// drop, as tokio does: in glibc each would keep the library loaded while
/// that thread of the host lives. It goes on there as after a wait: the
/// caller reads no thread-local after this but through a function never
/// inlined, which finds the worker's.
#[inline(never)]
pub(crate) fn leave_host_thread() {
    if !crate::library() || RUNTIME_THREAD.with(Cell::get) || exploring().is_some() {
        return;
    }
    let task = current();
    // A task with C's frames on its stack goes on on this thread, as when
    // it waits (`park`).
    if task.is_null() || stays(task) {
        return;
    }
    unsafe {
        let state = &(*task).state;
        if state.compare_exchange(RUNNING, PARKING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        // Notified at once: the thread it leaves schedules it for a worker.
        state.store(NOTIFIED, Ordering::Release);
        switch(&mut (*task).sp, *home());
    }
}

/// Waits until every thread the runtime started is gone, once each was
/// told to end.
pub(crate) fn join_threads() {
    loop {
        let started = std::mem::take(&mut *STARTED_THREADS.lock().unwrap_or_else(|e| e.into_inner()));
        if started.is_empty() {
            return;
        }
        for thread in started {
            unsafe { libc::pthread_join(thread.thread, std::ptr::null_mut()) };
        }
    }
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

/// A thread the runtime starts once, as the monitor, and again when a
/// library that ended is called again; it sleeps until woken, as std's
/// `park` does.
pub(crate) struct ThreadSlot {
    started: AtomicBool,
    woken: Mutex<bool>,
    wake: Condvar,
}

impl ThreadSlot {
    pub(crate) const fn new() -> ThreadSlot {
        ThreadSlot { started: AtomicBool::new(false), woken: Mutex::new(false), wake: Condvar::new() }
    }

    /// Starts the thread with `start`, unless it was.
    pub(crate) fn start_once(&self, start: impl FnOnce() -> RuntimeThread) {
        if self.started.load(Ordering::Acquire) {
            return;
        }
        static STARTING: Mutex<()> = Mutex::new(());
        let _starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        if !self.started.load(Ordering::Acquire) {
            start();
            self.started.store(true, Ordering::Release);
        }
    }

    /// On the thread: sleeps until woken since it last slept.
    pub(crate) fn park(&self) {
        let mut woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        while !*woken {
            woken = self.wake.wait(woken).unwrap_or_else(|e| e.into_inner());
        }
        *woken = false;
    }

    /// On the thread: sleeps until woken since it last slept, or until
    /// `at` if given.
    pub(crate) fn park_until(&self, at: Option<Instant>) {
        let Some(at) = at else { return self.park() };
        let mut woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        while !*woken {
            let Some(left) = at.checked_duration_since(Instant::now()) else { break };
            woken = self.wake.wait_timeout(woken, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        *woken = false;
    }

    pub(crate) fn unpark(&self) {
        *self.woken.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_one();
    }

    /// Forgets the thread, which ended.
    pub(super) fn clear(&self) {
        self.started.store(false, Ordering::Release);
        *self.woken.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

/// What the runtime keeps for a thread, given back when the thread ends:
/// its signal stack and its spare task stacks. Not in thread-locals that
/// drop: a destructor due holds a library loaded, in glibc, for as long as
/// the thread of its host that called it lives. A key of POSIX threads,
/// which a library's end deletes, holds them instead.
pub(crate) struct Kept {
    /// The thread's signal stack, when the runtime made it.
    pub(crate) signal: Cell<*mut libc::c_void>,
    /// Whether the thread's signal stack was looked at.
    pub(crate) guarded: Cell<bool>,
    /// Its spare task stacks (`own_stacks`), as `Worker::stacks`.
    pub(super) stacks: Cell<*mut Vec<Stack>>,
    /// Its spare blocks of spawned tasks (`spare_blocks`).
    pub(super) blocks: Cell<*mut Vec<*mut Spawned>>,
    /// What other parts of the runtime keep for it, each with what frees
    /// it (`keep_until_end`).
    ends: Cell<Vec<(Free, *mut u8)>>,
    /// The key's number when it was put under it.
    generation: Cell<usize>,
    /// The bounds of the thread's own stack, once looked up.
    own_stack: Cell<(usize, usize)>,
    /// The block a call of C's on its stack is a task in, from one call to
    /// the next (`Task::native`), and whether one runs now.
    call: Cell<*mut std::mem::MaybeUninit<Task>>,
    pub(super) calling: AtomicBool,
    /// The numbers its calls take next, from a run the others do not.
    ids: Cell<(u64, u64)>,
    /// Its shard of the registry (`native`).
    shard: u8,
    /// What its `Worker` was given as it ran Tondo code, which it gives
    /// back as it ends (`worker_ended`): read here, as its thread-locals
    /// may be gone by then, as on macOS.
    pub(super) runner: Cell<Option<&'static Runner>>,
    pub(super) in_c: Cell<*const InC>,
    pub(super) waits: Cell<*mut Vec<Arc<Wait>>>,
    pub(super) helds: Cell<*mut Vec<Vec<Held>>>,
    pub(super) turns: Cell<*mut Vec<Arc<dyn Turned>>>,
    pub(super) events: Cell<*mut crate::poll::Events>,
    /// The chunks of regions it keeps for its next ones, linked
    /// (`regions`), which go to the list the threads share as it ends.
    pub(crate) chunks: Cell<*mut u8>,
}

/// What frees a value a thread keeps (`keep_until_end`).
type Free = unsafe fn(*mut u8, bool);

thread_local! {
    static KEPT: Cell<*mut Kept> = const { Cell::new(std::ptr::null_mut()) };
}

/// The key, while a runtime runs, and its number, which a library's end
/// changes.
static KEY: Mutex<Option<libc::pthread_key_t>> = Mutex::new(None);
static GENERATION: AtomicUsize = AtomicUsize::new(1);
/// Every thread's, for a library's end, which frees their spare stacks.
static EVERY_KEPT: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// What the runtime keeps for this thread. Never inlined: a task may
/// resume on another thread, and its caller must not keep this thread's.
#[inline(never)]
pub(crate) fn kept() -> &'static Kept {
    let mut kept = KEPT.with(Cell::get);
    if kept.is_null() {
        let new = Kept {
            signal: Cell::new(std::ptr::null_mut()),
            guarded: Cell::new(false),
            stacks: Cell::new(std::ptr::null_mut()),
            blocks: Cell::new(std::ptr::null_mut()),
            ends: Cell::new(Vec::new()),
            generation: Cell::new(0),
            own_stack: Cell::new((0, 0)),
            call: Cell::new(std::ptr::null_mut()),
            calling: AtomicBool::new(false),
            ids: Cell::new((0, 0)),
            shard: (SHARD_TURN.fetch_add(1, Ordering::Relaxed) % SHARDS) as u8,
            // What the thread was given before this.
            runner: Cell::new(worker(|w| w.runner.get())),
            in_c: Cell::new(worker(|w| w.in_c.get())),
            waits: Cell::new(worker(|w| w.waits.get())),
            helds: Cell::new(worker(|w| w.helds.get())),
            turns: Cell::new(worker(|w| w.turns.get())),
            events: Cell::new(worker(|w| w.events.get())),
            chunks: Cell::new(std::ptr::null_mut()),
        };
        kept = Box::into_raw(Box::new(new));
        KEPT.with(|cell| cell.set(kept));
        EVERY_KEPT.lock().unwrap_or_else(|e| e.into_inner()).push(kept as usize);
    }
    let kept = unsafe { &*kept };
    let generation = GENERATION.load(Ordering::Acquire);
    if kept.generation.get() != generation {
        let mut key = KEY.lock().unwrap_or_else(|e| e.into_inner());
        let key = *key.get_or_insert_with(|| {
            let mut key = 0;
            unsafe { libc::pthread_key_create(&mut key, Some(thread_ended)) };
            key
        });
        unsafe { libc::pthread_setspecific(key, kept as *const Kept as *const libc::c_void) };
        kept.generation.set(generation);
    }
    kept
}

/// What the runtime keeps for the thread, when it made it already and will
/// give it back as the thread ends; never made here, as a caller that
/// counts what a task allocates must not count the runtime's own.
pub(crate) fn kept_if_made() -> Option<&'static Kept> {
    let kept = unsafe { KEPT.with(Cell::get).as_ref() }?;
    (kept.generation.get() == GENERATION.load(Ordering::Acquire)).then_some(kept)
}

/// Frees `value` with `free` when the thread ends, or when a library ends
/// on it: what a part of the runtime keeps for the thread in a thread-local
/// that does not drop. `free(value, true)` runs on the thread and empties
/// the thread-local; at a library's end on another thread,
/// `free(value, false)` frees what `value` holds and leaves `value`, which
/// the thread-local still points to.
pub(crate) fn keep_until_end(free: Free, value: *mut u8) {
    let kept = kept();
    let mut ends = kept.ends.take();
    ends.push((free, value));
    kept.ends.set(ends);
}

/// The destructor of the key, as a thread ends.
unsafe extern "C" fn thread_ended(kept: *mut libc::c_void) {
    // What the profiler's handler reads of the thread may be gone already,
    // as its thread-locals on macOS.
    crate::trace::block_profiler();
    KEPT.with(|cell| cell.set(std::ptr::null_mut()));
    worker_ended(unsafe { &*(kept as *const Kept) });
    give_back(kept as *mut Kept);
}

fn give_back(kept: *mut Kept) {
    EVERY_KEPT.lock().unwrap_or_else(|e| e.into_inner()).retain(|&k| k != kept as usize);
    let kept = unsafe { Box::from_raw(kept) };
    worker(|w| {
        if w.stacks.get() == kept.stacks.get() {
            w.stacks.set(std::ptr::null_mut());
        }
    });
    give_back_stacks(kept.stacks.get());
    free_blocks(kept.blocks.get(), true);
    for (free, value) in kept.ends.take() {
        unsafe { free(value, true) };
    }
    if !kept.call.get().is_null() {
        drop(unsafe { Box::from_raw(kept.call.get()) });
    }
    crate::trace::free_signal_stack(kept.signal.get());
}

/// At a library's end: gives back what this thread kept, frees the spare
/// stacks of the others and the shared ones, and deletes the key. A
/// signal stack another thread has stays, as the system may still deliver
/// a signal on it, and that thread keeps it if it calls the library again.
pub(super) fn give_back_kept() {
    let own = KEPT.with(|cell| cell.replace(std::ptr::null_mut()));
    let key = KEY.lock().unwrap_or_else(|e| e.into_inner()).take();
    if !own.is_null() {
        if let Some(key) = key {
            unsafe { libc::pthread_setspecific(key, std::ptr::null()) };
        }
        give_back(own);
    }
    for &kept in EVERY_KEPT.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let kept = unsafe { &*(kept as *const Kept) };
        if let Some(stacks) = unsafe { kept.stacks.get().as_mut() } {
            stacks.clear();
        }
        free_blocks(kept.blocks.get(), false);
        let ends = kept.ends.take();
        for &(free, value) in &ends {
            unsafe { free(value, false) };
        }
        kept.ends.set(ends);
    }
    SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    if let Some(key) = key {
        unsafe { libc::pthread_key_delete(key) };
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// The bounds of this thread's own stack, as `Kept::find_stack` found
/// them, read without allocating, as in a signal's handler.
pub(super) fn thread_stack() -> Option<(usize, usize)> {
    let kept = unsafe { KEPT.with(Cell::get).as_ref()? };
    Some(kept.own_stack.get()).filter(|bounds| bounds.1 != 0)
}

#[cfg(not(target_os = "macos"))]
unsafe fn own_stack_bounds() -> Option<(usize, usize)> {
    unsafe {
        let mut attributes: libc::pthread_attr_t = std::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attributes) != 0 {
            return None;
        }
        let (mut low, mut size) = (std::ptr::null_mut(), 0usize);
        let found = libc::pthread_attr_getstack(&attributes, &mut low, &mut size) == 0;
        libc::pthread_attr_destroy(&mut attributes);
        found.then(|| (low as usize, low as usize + size))
    }
}

#[cfg(target_os = "macos")]
unsafe fn own_stack_bounds() -> Option<(usize, usize)> {
    unsafe {
        let this = libc::pthread_self();
        let top = libc::pthread_get_stackaddr_np(this) as usize;
        Some((top - libc::pthread_get_stacksize_np(this), top))
    }
}
