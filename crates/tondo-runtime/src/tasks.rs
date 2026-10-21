//! Tasks (spec 11.11, 11.12, 16.14): stackful coroutines that worker
//! threads run from one queue. A task that waits saves its registers on its
//! own stack and switches back to its worker, which runs another task; the
//! code it runs is ordinary compiled code that knows nothing of this.
//!
//! The process's main thread is the first worker and runs `main` as the
//! first task. More workers start, one per core, when the program first
//! spawns a task.

use super::{Glue, LIVE_OBJECTS, Str, flush_stdout, runtime_fault, tondo_rt_panic};
use std::cell::Cell;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

// ---------------------------------------------------------------------------
// Switching stacks
// ---------------------------------------------------------------------------

/// Saves the callee-saved registers on the current stack, stores its pointer
/// in `*from`, and resumes the stack `to` saved the same way.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "sub sp, sp, #176",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "stp d8, d9, [sp, #96]",
        "stp d10, d11, [sp, #112]",
        "stp d12, d13, [sp, #128]",
        "stp d14, d15, [sp, #144]",
        "mov x9, sp",
        "str x9, [x0]",
        "mov sp, x1",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "ldp d8, d9, [sp, #96]",
        "ldp d10, d11, [sp, #112]",
        "ldp d12, d13, [sp, #128]",
        "ldp d14, d15, [sp, #144]",
        "add sp, sp, #176",
        "ret",
    )
}

/// The stack pointer `switch` resumes to start a task in `trampoline`.
unsafe fn initial_stack(top: *mut u8) -> *mut u8 {
    let top = (top as usize & !15) as *mut usize;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            // `ret` pops the trampoline's address and leaves the stack as a
            // call would: 8 bytes past a 16-byte boundary.
            let ret = top.sub(2);
            ret.write(trampoline as *const () as usize);
            let saved = ret.sub(6);
            for i in 0..6 {
                saved.add(i).write(0);
            }
            saved as *mut u8
        }
        #[cfg(target_arch = "aarch64")]
        {
            let saved = top.sub(22);
            for i in 0..22 {
                saved.add(i).write(0);
            }
            // x30, the link register, sits after x29 at offset 88.
            saved.add(11).write(trampoline as *const () as usize);
            saved as *mut u8
        }
    }
}

// ---------------------------------------------------------------------------
// Stacks
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ_WRITE: i32 = 3;
#[cfg(target_os = "linux")]
const MAP_FLAGS: i32 = 0x02 | 0x20 | 0x4000; // PRIVATE | ANONYMOUS | NORESERVE
#[cfg(not(target_os = "linux"))]
const MAP_FLAGS: i32 = 0x02 | 0x1000; // PRIVATE | ANON
/// Covers the largest page size of the supported targets.
const GUARD: usize = 64 * 1024;
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
const MAIN_STACK: usize = 64 * 1024 * 1024;
const TASK_STACK: usize = 8 * 1024 * 1024;

/// A stack mapped on demand, with an inaccessible guard region below it so
/// that an overflow faults instead of corrupting memory.
struct Stack {
    base: *mut u8,
    size: usize,
}

impl Stack {
    fn new(size: usize) -> Stack {
        unsafe {
            let base = mmap(std::ptr::null_mut(), size, PROT_READ_WRITE, MAP_FLAGS, -1, 0);
            if base as isize == -1 {
                runtime_fault("cannot map a task stack");
            }
            mprotect(base, GUARD, PROT_NONE);
            Stack { base, size }
        }
    }

    fn top(&self) -> *mut u8 {
        unsafe { self.base.add(self.size) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        unsafe { munmap(self.base, self.size) };
    }
}

unsafe impl Send for Stack {}

/// Task stacks kept for reuse: mapping and unmapping one per task costs
/// more than the task itself.
static SPARE_STACKS: Mutex<Vec<Stack>> = Mutex::new(Vec::new());
const SPARE_LIMIT: usize = 256;

fn task_stack() -> Stack {
    let spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner()).pop();
    spare.unwrap_or_else(|| Stack::new(TASK_STACK))
}

fn recycle(stack: Stack) {
    let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
    if stack.size == TASK_STACK && spare.len() < SPARE_LIMIT {
        spare.push(stack);
    }
}

// ---------------------------------------------------------------------------
// Tasks and the scheduler
// ---------------------------------------------------------------------------

/// A registered `defer` cleanup: its code and the closure it receives.
pub type Cleanup = unsafe extern "C" fn(*mut u8);

/// How the body of a task returns its result, which the compiler knows from
/// the result's representation.
#[derive(Copy, Clone)]
#[repr(u8)]
pub enum ResultKind {
    Nothing = 0,
    Integer = 1,
    Float64 = 2,
    Float32 = 3,
    Memory = 4,
}

struct Task {
    /// The stack pointer saved when the task is not running.
    sp: *mut u8,
    stack: Option<Stack>,
    /// The closure the task runs: a `fn` object, released afterwards.
    body: *mut u8,
    code: *const u8,
    kind: ResultKind,
    join: *mut JoinObject,
    /// Its `defer` cleanups, innermost last (13.7).
    cleanups: Vec<(Cleanup, usize)>,
    /// A lock its worker releases once the task has switched out, so that
    /// nobody resumes it before its registers are saved.
    unlock: *const AtomicBool,
    finished: bool,
    main: bool,
}

struct Queue {
    runnable: VecDeque<*mut Task>,
    /// Workers started, and how many wait for a task.
    workers: usize,
    idle: usize,
}

unsafe impl Send for Queue {}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { runnable: VecDeque::new(), workers: 1, idle: 0 });
static READY: Condvar = Condvar::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static MAIN_STATUS: AtomicUsize = AtomicUsize::new(0);
static MAIN_DONE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static CURRENT: Cell<*mut Task> = const { Cell::new(std::ptr::null_mut()) };
    /// Where the worker's own stack pointer is saved while a task runs.
    static HOME: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// Cleanups registered outside any task, as in unit tests.
    static LOOSE_CLEANUPS: std::cell::RefCell<Vec<(Cleanup, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

// A task may resume on another thread, so these never inline: a caller
// must not keep a thread-local address across a switch.
#[inline(never)]
fn current() -> *mut Task {
    CURRENT.with(|c| c.get())
}

#[inline(never)]
fn home() -> *mut *mut u8 {
    HOME.with(|h| h.as_ptr())
}

/// Makes a task runnable.
fn wake(task: *mut Task) {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    queue.runnable.push_back(task);
    drop(queue);
    READY.notify_one();
}

/// Switches from the current task to its worker. With `unlock`, the worker
/// releases that lock once the task is saved; whoever finds the task in the
/// structure the lock protects can then wake it.
unsafe fn park(unlock: Option<&AtomicBool>) {
    let task = current();
    if task.is_null() {
        runtime_fault("waiting outside a task");
    }
    unsafe {
        (*task).unlock = unlock.map_or(std::ptr::null(), |l| l as *const AtomicBool);
        switch(&mut (*task).sp, *home());
    }
}

/// Runs tasks; on the main thread, until the main task finishes, wherever
/// it ran, and then returns its exit status.
fn work(main: bool) -> i32 {
    loop {
        let task = {
            let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if main && MAIN_DONE.load(Ordering::SeqCst) {
                    return MAIN_STATUS.load(Ordering::SeqCst) as i32;
                }
                if let Some(task) = queue.runnable.pop_front() {
                    break task;
                }
                queue.idle += 1;
                if queue.idle == queue.workers && !MAIN_DONE.load(Ordering::SeqCst) {
                    // Every worker waits and no task can run.
                    drop(queue);
                    deadlock();
                }
                queue = READY.wait(queue).unwrap_or_else(|e| e.into_inner());
                queue.idle -= 1;
            }
        };
        CURRENT.with(|c| c.set(task));
        unsafe { switch(home(), (*task).sp) };
        CURRENT.with(|c| c.set(std::ptr::null_mut()));
        unsafe {
            if let Some(lock) = (*task).unlock.as_ref() {
                (*task).unlock = std::ptr::null();
                lock.store(false, Ordering::Release);
            }
            if (*task).finished {
                let mut task = Box::from_raw(task);
                let was_main = task.main;
                if let Some(stack) = task.stack.take() {
                    recycle(stack);
                }
                drop(task);
                if was_main {
                    MAIN_DONE.store(true, Ordering::SeqCst);
                    // Take the queue's lock so the main thread cannot miss
                    // the notification between its check and its wait.
                    drop(QUEUE.lock().unwrap_or_else(|e| e.into_inner()));
                    READY.notify_all();
                }
            }
        }
    }
}

fn deadlock() -> ! {
    // Several idle workers may see it at once; one reports it.
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if REPORTED.swap(true, Ordering::SeqCst) {
        loop {
            std::thread::park();
        }
    }
    flush_stdout();
    let _ = writeln!(std::io::stderr(), "panic[P0013]: deadlock: every task is waiting");
    std::process::exit(super::PANIC_EXIT_CODE)
}

/// Starts the other workers, one per core, the first time a task spawns.
fn start_workers() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let count = std::thread::available_parallelism().map_or(1, |n| n.get());
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).workers = count;
    for _ in 1..count {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                work(false);
            })
            .unwrap_or_else(|_| runtime_fault("cannot start a worker thread"));
    }
}

/// Where every task starts: runs its body, completes its `Join` and
/// switches back to its worker for good.
extern "C" fn trampoline() -> ! {
    unsafe {
        let task = current();
        let (body, code, kind, join) = ((*task).body, (*task).code, (*task).kind, (*task).join);
        if (*task).main {
            let main: unsafe extern "C" fn() -> i32 = std::mem::transmute(code);
            MAIN_STATUS.store(main() as usize, Ordering::SeqCst);
        } else {
            let captures = body.add(super::FN_HEADER);
            let out = (*join).result;
            match kind {
                ResultKind::Nothing => {
                    let f: unsafe extern "C" fn(*mut u8) = std::mem::transmute(code);
                    f(captures);
                }
                ResultKind::Integer => {
                    let f: unsafe extern "C" fn(*mut u8) -> u64 = std::mem::transmute(code);
                    let value = f(captures).to_le_bytes();
                    std::ptr::copy_nonoverlapping(value.as_ptr(), out, (*join).size.min(8));
                }
                ResultKind::Float64 => {
                    let f: unsafe extern "C" fn(*mut u8) -> f64 = std::mem::transmute(code);
                    (out as *mut f64).write_unaligned(f(captures));
                }
                ResultKind::Float32 => {
                    let f: unsafe extern "C" fn(*mut u8) -> f32 = std::mem::transmute(code);
                    (out as *mut f32).write_unaligned(f(captures));
                }
                ResultKind::Memory => {
                    let f: unsafe extern "C" fn(*mut u8, *mut u8) = std::mem::transmute(code);
                    f(out, captures);
                }
            }
            super::tondo_rt_fn_release(body as *mut super::FnObject);
            complete(join);
        }
        (*task).finished = true;
        switch(&mut (*task).sp, *home());
    }
    runtime_fault("a finished task resumed")
}

/// Runs `tondo_main` as the first task on the calling thread, which becomes
/// a worker; returns the program's exit status.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub fn run_main(code: unsafe extern "C" fn() -> i32) -> i32 {
    let stack = Stack::new(MAIN_STACK);
    let sp = unsafe { initial_stack(stack.top()) };
    let task = Box::into_raw(Box::new(Task {
        sp,
        stack: Some(stack),
        body: std::ptr::null_mut(),
        code: code as *const u8,
        kind: ResultKind::Integer,
        join: std::ptr::null_mut(),
        cleanups: Vec::new(),
        unlock: std::ptr::null(),
        finished: false,
        main: true,
    }));
    wake(task);
    work(true)
}

// ---------------------------------------------------------------------------
// Cleanups
// ---------------------------------------------------------------------------

fn with_cleanups<R>(f: impl FnOnce(&mut Vec<(Cleanup, usize)>) -> R) -> R {
    let task = current();
    if task.is_null() { LOOSE_CLEANUPS.with(|c| f(&mut c.borrow_mut())) } else { f(unsafe { &mut (*task).cleanups }) }
}

pub fn push_cleanup(code: Cleanup, closure: *mut u8) {
    with_cleanups(|c| c.push((code, closure as usize)));
}

pub fn pop_cleanup() {
    with_cleanups(|c| c.pop());
}

/// Runs the current task's cleanups in LIFO order, each removed first so
/// that a panic inside one never runs it twice.
pub fn run_cleanups() {
    while let Some((code, closure)) = with_cleanups(|c| c.pop()) {
        unsafe { code(closure as *mut u8) };
    }
}

// ---------------------------------------------------------------------------
// Locks held while switching
// ---------------------------------------------------------------------------

/// A spin lock that a parked task's worker releases after the switch.
fn lock(flag: &AtomicBool) {
    while flag.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        std::hint::spin_loop();
    }
}

fn unlock(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Join
// ---------------------------------------------------------------------------

/// The handle `spawn` returns (11.11). `rc` counts the program's references;
/// the task itself only touches the object until it marks it done, and the
/// last reference waits for that before freeing it. The result lives here
/// until `wait` moves it out.
#[repr(C)]
pub struct JoinObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    done: bool,
    taken: bool,
    waiters: Vec<*mut Task>,
    result: *mut u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    /// The group that observes the task's end, and its index there.
    group: *mut GroupObject,
    index: usize,
}

/// Starts a task that calls the closure `body`, a `fn` object it takes,
/// and returns its `Join`. `kind`, `size` and `align` describe the result;
/// `release` frees one that is never taken.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_spawn(
    body: *mut u8,
    kind: u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
) -> *mut JoinObject {
    start_workers();
    let align = align.max(8);
    let result = if size == 0 {
        std::ptr::null_mut()
    } else {
        unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, align).unwrap()) }
    };
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let join = Box::into_raw(Box::new(JoinObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        done: false,
        taken: false,
        waiters: Vec::new(),
        result,
        size,
        align,
        release,
        group: std::ptr::null_mut(),
        index: 0,
    }));
    let code = unsafe { *(body.add(16) as *const *const u8) };
    let kind = match kind {
        0 => ResultKind::Nothing,
        1 => ResultKind::Integer,
        2 => ResultKind::Float64,
        3 => ResultKind::Float32,
        _ => ResultKind::Memory,
    };
    let stack = task_stack();
    let sp = unsafe { initial_stack(stack.top()) };
    let task = Box::into_raw(Box::new(Task {
        sp,
        stack: Some(stack),
        body,
        code,
        kind,
        join,
        cleanups: Vec::new(),
        unlock: std::ptr::null(),
        finished: false,
        main: false,
    }));
    wake(task);
    join
}

/// Marks the task done and wakes whoever waits for it. The group hears of
/// it under the join's lock, so nobody sees the task done, and frees the
/// group, before that.
unsafe fn complete(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        j.done = true;
        let waiters = std::mem::take(&mut j.waiters);
        if !j.group.is_null() {
            group_completed(j.group, j.index);
        }
        unlock(&j.lock);
        for waiter in waiters {
            wake(waiter);
        }
    }
}

/// Waits until the task is done.
unsafe fn finish(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        if j.done {
            unlock(&j.lock);
            return;
        }
        j.waiters.push(current());
        park(Some(&j.lock));
    }
}

/// Moves the result out of a finished task into `out`.
unsafe fn take(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        if (*join).size > 0 {
            std::ptr::copy_nonoverlapping((*join).result, out, (*join).size);
        }
        (*join).taken = true;
    }
}

/// `job.wait()`: waits for the task and moves its result into `out`. The
/// handle is released as usual by its owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_wait(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        finish(join);
        take(join, out);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_handle_retain(handle: *mut u8) {
    unsafe { (*(handle as *const AtomicUsize)).fetch_add(1, Ordering::Relaxed) };
}

/// Releases a `Join`: the last reference waits for a task nobody waited for
/// (11.11) and frees its result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_release(join: *mut JoinObject) {
    if join.is_null() {
        return;
    }
    unsafe {
        if (*join).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        free_join(join);
    }
}

unsafe fn free_join(join: *mut JoinObject) {
    unsafe {
        finish(join);
        if !(*join).taken
            && let Some(release) = (*join).release
        {
            release((*join).result);
        }
        if !(*join).result.is_null() {
            let layout = std::alloc::Layout::from_size_align((*join).size, (*join).align).unwrap();
            std::alloc::dealloc((*join).result, layout);
        }
        drop(Box::from_raw(join));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

/// `Group[T]` (11.12): the joins added, in order, which tasks finished and
/// `next` has not returned yet, and, when `T` is a result, the first task
/// that finished with an error.
#[repr(C)]
pub struct GroupObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    fallible: bool,
    joins: Vec<*mut JoinObject>,
    taken: Vec<bool>,
    finished: VecDeque<usize>,
    running: usize,
    failed: Option<usize>,
    waiter: *mut Task,
}

/// `Group.new[T]()`; `fallible` when `T` is `R ! E`, whose results start
/// with their tag, `1` for `err`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_group_new(fallible: u8) -> *mut GroupObject {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(GroupObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        fallible: fallible != 0,
        joins: Vec::new(),
        taken: Vec::new(),
        finished: VecDeque::new(),
        running: 0,
        failed: None,
        waiter: std::ptr::null_mut(),
    }))
}

/// `group.add(job)`: the group takes over the join's reference, which the
/// caller retained for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_add(group: *mut GroupObject, join: *mut JoinObject) {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        let index = g.joins.len();
        g.joins.push(join);
        g.taken.push(false);
        g.running += 1;
        unlock(&g.lock);
        let j = &mut *join;
        lock(&j.lock);
        if j.done {
            group_completed(group, index);
        } else {
            j.group = group;
            j.index = index;
        }
        unlock(&j.lock);
    }
}

/// Records that the task at `index` finished; called under its join's lock.
unsafe fn group_completed(group: *mut GroupObject, index: usize) {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        g.finished.push_back(index);
        g.running -= 1;
        let join = g.joins[index];
        if g.fallible && g.failed.is_none() && *(*join).result != 0 {
            g.failed = Some(index);
        }
        let waiter = std::mem::replace(&mut g.waiter, std::ptr::null_mut());
        unlock(&g.lock);
        if !waiter.is_null() {
            wake(waiter);
        }
    }
}

/// Parks the current task until the group's state changes; `g.lock` is
/// held and released by the switch.
unsafe fn group_park(g: &mut GroupObject) {
    g.waiter = current();
    unsafe { park(Some(&g.lock)) };
}

/// `group.length()`: the tasks whose results the group still holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_length(group: *mut GroupObject) -> i64 {
    unsafe {
        let g = &*group;
        lock(&g.lock);
        let count = g.taken.iter().filter(|t| !**t).count();
        unlock(&g.lock);
        count as i64
    }
}

/// `group.next()`: moves the result of the next task to finish into `out`
/// and returns 1, or returns 0 when no task is left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_next(group: *mut GroupObject, out: *mut u8) -> u8 {
    unsafe {
        loop {
            let g = &mut *group;
            lock(&g.lock);
            if g.taken.iter().all(|t| *t) {
                unlock(&g.lock);
                return 0;
            }
            while let Some(index) = g.finished.pop_front() {
                if !g.taken[index] {
                    g.taken[index] = true;
                    let join = g.joins[index];
                    unlock(&g.lock);
                    take(join, out);
                    return 1;
                }
            }
            group_park(g);
        }
    }
}

/// The first part of `group.wait()`: waits until every task finished or,
/// for results, one finished with an error; returns the index of that
/// task, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_settle(group: *mut GroupObject) -> i64 {
    unsafe {
        loop {
            let g = &mut *group;
            lock(&g.lock);
            if let Some(index) = g.failed.filter(|&i| !g.taken[i]) {
                unlock(&g.lock);
                return index as i64;
            }
            if g.running == 0 {
                unlock(&g.lock);
                return -1;
            }
            group_park(g);
        }
    }
}

/// Moves the result of the finished task at `index`, or of the first one
/// not taken yet when `index` is -1, into `out` and returns 1; returns 0
/// when every result was taken.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_take(group: *mut GroupObject, index: i64, out: *mut u8) -> u8 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        let found = if index >= 0 { Some(index as usize) } else { g.taken.iter().position(|t| !*t) };
        let Some(index) = found else {
            unlock(&g.lock);
            return 0;
        };
        g.taken[index] = true;
        let join = g.joins[index];
        unlock(&g.lock);
        finish(join);
        take(join, out);
        1
    }
}

/// Releases a `Group`: the last reference waits for its remaining tasks
/// and frees their results.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_release(group: *mut GroupObject) {
    if group.is_null() {
        return;
    }
    unsafe {
        if (*group).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        // Each join finishes before it is freed, and with it the last use
        // of the group by its task; tasks still running read `joins`.
        for index in 0..(*group).joins.len() {
            free_join((&(*group).joins)[index]);
        }
        drop(Box::from_raw(group));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// A task waiting on a channel, on its own stack: the value it sends, or
/// where the value it receives goes, and how the wait ended.
struct Waiter {
    task: *mut Task,
    value: *mut u8,
    state: u8,
}

const WAITING: u8 = 0;
const MOVED: u8 = 1;
const CLOSED: u8 = 2;

/// A channel (11.13). `rc` counts the `Receiver` handles and the sender
/// objects; `senders` counts the latter, and the channel closes when it
/// reaches zero. A receiver only waits while the buffer is empty, and a
/// sender only while it is full.
#[repr(C)]
pub struct ChannelObject {
    rc: AtomicUsize,
    lock: AtomicBool,
    closed: bool,
    senders: usize,
    /// `usize::MAX` when unbounded; 0 for a rendezvous.
    capacity: usize,
    size: usize,
    align: usize,
    release: Option<Glue>,
    /// The buffered values' bytes, `count` values of `size` bytes each.
    buffer: VecDeque<u8>,
    count: usize,
    receiving: VecDeque<*mut Waiter>,
    sending: VecDeque<*mut Waiter>,
}

/// A `Sender` handle: one per `channel` or `copy()`, shared by the
/// program's references to it.
#[repr(C)]
pub struct SenderObject {
    rc: AtomicUsize,
    channel: *mut ChannelObject,
}

/// `channel[T](capacity)` without its sender: the `Receiver`. A negative
/// capacity panics with `P0001`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_new(
    capacity: i64,
    bounded: u8,
    size: usize,
    align: usize,
    release: Option<Glue>,
    location: *const Str,
) -> *mut ChannelObject {
    if bounded != 0 && capacity < 0 {
        unsafe { tondo_rt_panic(1, std::ptr::null(), location) };
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(ChannelObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        closed: false,
        senders: 0,
        capacity: if bounded != 0 { capacity as usize } else { usize::MAX },
        size,
        align: align.max(1),
        release,
        buffer: VecDeque::new(),
        count: 0,
        receiving: VecDeque::new(),
        sending: VecDeque::new(),
    }))
}

/// A new `Sender` of the channel: the one `channel` returns, or `copy()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_new(channel: *mut ChannelObject) -> *mut SenderObject {
    unsafe {
        let c = &mut *channel;
        lock(&c.lock);
        c.senders += 1;
        unlock(&c.lock);
        c.rc.fetch_add(1, Ordering::Relaxed);
    }
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(SenderObject { rc: AtomicUsize::new(1), channel }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_copy(sender: *mut SenderObject) -> *mut SenderObject {
    unsafe { tondo_rt_sender_new((*sender).channel) }
}

/// Moves the value at the front of the buffer, `size` bytes, into `out`.
fn pop_bytes(buffer: &mut VecDeque<u8>, out: *mut u8, size: usize) {
    let (front, back) = buffer.as_slices();
    let first = front.len().min(size);
    unsafe {
        std::ptr::copy_nonoverlapping(front.as_ptr(), out, first);
        std::ptr::copy_nonoverlapping(back.as_ptr(), out.add(first), size - first);
    }
    buffer.drain(..size);
}

/// Ends the waits of a channel that just closed; returns who to wake.
fn close_waits(c: &mut ChannelObject) -> Vec<*mut Task> {
    c.closed = true;
    let waiters = c.receiving.drain(..).chain(c.sending.drain(..));
    waiters
        .map(|w| unsafe {
            (*w).state = CLOSED;
            (*w).task
        })
        .collect()
}

fn closed_panic(location: *const Str) -> ! {
    unsafe { tondo_rt_panic(12, std::ptr::null(), location) }
}

/// `sender.send(value)`: moves the value at `value` into the channel,
/// waiting while it is full; panics with `P0012` if it is closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_send(sender: *mut SenderObject, value: *mut u8, location: *const Str) {
    unsafe {
        let c = &mut *(*sender).channel;
        lock(&c.lock);
        if c.closed {
            unlock(&c.lock);
            closed_panic(location);
        }
        if let Some(receiver) = c.receiving.pop_front() {
            std::ptr::copy_nonoverlapping(value, (*receiver).value, c.size);
            (*receiver).state = MOVED;
            let task = (*receiver).task;
            unlock(&c.lock);
            wake(task);
            return;
        }
        if c.count < c.capacity {
            c.buffer.extend(std::slice::from_raw_parts(value, c.size));
            c.count += 1;
            unlock(&c.lock);
            return;
        }
        let mut waiter = Waiter { task: current(), value, state: WAITING };
        c.sending.push_back(&mut waiter);
        park(Some(&c.lock));
        if std::ptr::read_volatile(&waiter.state) == CLOSED {
            closed_panic(location);
        }
    }
}

/// `receiver.receive()`: moves the next value into `out` and returns 1, or
/// returns 0 once the channel is closed and empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_receive(channel: *mut ChannelObject, out: *mut u8) -> u8 {
    unsafe {
        let c = &mut *channel;
        lock(&c.lock);
        if c.count > 0 {
            pop_bytes(&mut c.buffer, out, c.size);
            c.count -= 1;
            // A sender waiting for room moves its value in.
            if let Some(sender) = c.sending.pop_front() {
                c.buffer.extend(std::slice::from_raw_parts((*sender).value, c.size));
                c.count += 1;
                (*sender).state = MOVED;
                let task = (*sender).task;
                unlock(&c.lock);
                wake(task);
                return 1;
            }
            unlock(&c.lock);
            return 1;
        }
        // A rendezvous: take the value from a waiting sender.
        if let Some(sender) = c.sending.pop_front() {
            std::ptr::copy_nonoverlapping((*sender).value, out, c.size);
            (*sender).state = MOVED;
            let task = (*sender).task;
            unlock(&c.lock);
            wake(task);
            return 1;
        }
        if c.closed {
            unlock(&c.lock);
            return 0;
        }
        let mut waiter = Waiter { task: current(), value: out, state: WAITING };
        c.receiving.push_back(&mut waiter);
        park(Some(&c.lock));
        (std::ptr::read_volatile(&waiter.state) == MOVED) as u8
    }
}

/// `sender.close()`: later sends panic, and receivers get what is buffered
/// and then `none`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_close(sender: *mut SenderObject) {
    unsafe {
        let c = &mut *(*sender).channel;
        lock(&c.lock);
        let woken = if c.closed { Vec::new() } else { close_waits(c) };
        unlock(&c.lock);
        woken.into_iter().for_each(wake);
    }
}

/// `isClosed()` of a `Sender` (`sender` is 1) or a `Receiver`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_closed(handle: *mut u8, sender: u8) -> u8 {
    unsafe {
        let c = if sender != 0 { &*(*(handle as *mut SenderObject)).channel } else { &*(handle as *mut ChannelObject) };
        lock(&c.lock);
        let closed = c.closed;
        unlock(&c.lock);
        closed as u8
    }
}

/// Releases a `Sender`; the channel closes when its last one goes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_release(sender: *mut SenderObject) {
    if sender.is_null() {
        return;
    }
    unsafe {
        if (*sender).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let sender = Box::from_raw(sender);
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
        let c = &mut *sender.channel;
        lock(&c.lock);
        c.senders -= 1;
        let woken = if c.senders == 0 && !c.closed { close_waits(c) } else { Vec::new() };
        unlock(&c.lock);
        woken.into_iter().for_each(wake);
        tondo_rt_receiver_release(sender.channel);
    }
}

/// Releases a `Receiver`, or a sender's hold on its channel; the last one
/// frees the channel and the values still buffered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_receiver_release(channel: *mut ChannelObject) {
    if channel.is_null() {
        return;
    }
    unsafe {
        if (*channel).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let mut c = Box::from_raw(channel);
        if let Some(release) = c.release
            && c.size > 0
        {
            let layout = std::alloc::Layout::from_size_align(c.size, c.align).unwrap();
            let value = std::alloc::alloc(layout);
            while c.count > 0 {
                pop_bytes(&mut c.buffer, value, c.size);
                c.count -= 1;
                release(value);
            }
            std::alloc::dealloc(value, layout);
        }
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}
