//! Joins: the handle `spawn` gives, which keeps a task's result until `wait`
//! moves it out, and wakes the tasks that wait for it (docs/refactor.md,
//! phase 2).

use super::*;

/// The waits for a task's end: most often one, kept in place, and the rest
/// beside it, so that a join waited for once allocates nothing for it.
#[derive(Default)]
pub(super) struct Waiters {
    first: Option<(*const Wait, usize)>,
    rest: Vec<(*const Wait, usize)>,
}

impl Waiters {
    pub(super) fn push(&mut self, waiter: (*const Wait, usize)) {
        match self.first {
            None => self.first = Some(waiter),
            Some(_) => self.rest.push(waiter),
        }
    }

    /// Every entry of a wait, as `select` makes one for each case.
    pub(super) fn remove(&mut self, wait: *const Wait) {
        self.rest.retain(|&(w, _)| w != wait);
        if self.first.is_some_and(|(w, _)| w == wait) {
            self.first = self.rest.pop();
        }
    }

    pub(super) fn take(&mut self) -> impl Iterator<Item = (*const Wait, usize)> {
        self.first.take().into_iter().chain(std::mem::take(&mut self.rest))
    }
}

/// The handle `spawn` returns (11.11). `rc` counts the program's references;
/// the task itself only touches the object until it marks it done, and the
/// last reference waits for that before freeing it. The result lives here
/// until `wait` moves it out.
#[repr(C)]
pub struct JoinObject {
    pub(super) rc: AtomicUsize,
    pub(super) lock: AtomicBool,
    pub(super) done: bool,
    pub(super) taken: bool,
    pub(super) waiters: Waiters,
    pub(super) result: *mut u8,
    pub(super) size: usize,
    pub(super) align: usize,
    pub(super) release: Option<Glue>,
    /// The group that observes the task's end, and its index there.
    pub(super) group: *mut GroupObject,
    pub(super) index: usize,
    /// The task, alive while it is not done.
    pub(super) task: *mut Task,
    /// Let go with `detach()`: the task holds a reference until it ends.
    pub(super) detached: bool,
    /// Visits the result for the cycle collector, when it may refer back
    /// to the join, which is then registered with it.
    pub(super) visit: Option<Visit>,
    /// Who holds the block: the program, until its references are gone
    /// and the task is done, and the task, until it is freed.
    pub(super) holders: AtomicU8,
    /// The panic the task ended with, which goes on in whoever waits for
    /// it (language 15.9).
    pub(super) panic: Option<Box<crate::panics::Record>>,
}

/// A spawned task's join, the task itself while it is alive, and a result
/// of up to 16 bytes, in one allocation, which the thread that spawned the
/// task usually frees too.
#[repr(C)]
pub(super) struct Spawned {
    pub(super) join: JoinObject,
    pub(super) task: std::mem::MaybeUninit<Task>,
    pub(super) result: [u64; 2],
}

thread_local! {
    /// The blocks of tasks that ended, which this thread's next spawns take,
    /// as Go keeps a goroutine that ended for the next: every block has the
    /// same size, and making one was an allocation of every spawn. Made when
    /// the thread first keeps one; what the runtime keeps for the thread frees
    /// them as it ends (`Kept`).
    static SPARE_BLOCKS: Cell<*mut Vec<*mut Spawned>> = const { Cell::new(std::ptr::null_mut()) };
}

/// This thread's spare blocks.
fn spare_blocks() -> &'static mut Vec<*mut Spawned> {
    let mut blocks = SPARE_BLOCKS.with(Cell::get);
    if blocks.is_null() {
        blocks = Box::into_raw(Box::default());
        kept().blocks.set(blocks);
        SPARE_BLOCKS.with(|cell| cell.set(blocks));
    }
    // Nothing that holds them calls back here.
    unsafe { &mut *blocks }
}

/// Frees spare blocks: a thread's as it ends, and the vector with them
/// unless another thread may still read it.
pub(super) fn free_blocks(blocks: *mut Vec<*mut Spawned>, own: bool) {
    if blocks.is_null() {
        return;
    }
    for block in unsafe { (*blocks).drain(..) } {
        unsafe { std::alloc::dealloc(block.cast(), std::alloc::Layout::new::<Spawned>()) };
    }
    if own {
        SPARE_BLOCKS.with(|cell| {
            if cell.get() == blocks {
                cell.set(std::ptr::null_mut());
            }
        });
        drop(unsafe { Box::from_raw(blocks) });
    }
}

/// How many blocks a thread keeps.
const SPARE_BLOCK_LIMIT: usize = 64;

/// Lets go of a spawned task's block for the program or for the task; the
/// last frees it, or keeps it for the next spawn.
pub(super) unsafe fn let_go(join: *mut JoinObject) {
    unsafe {
        if (*join).holders.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let block = join as *mut Spawned;
        let result = (*join).result;
        if !result.is_null() && result != (&raw mut (*block).result).cast() {
            std::alloc::dealloc(result, std::alloc::Layout::from_size_align((*join).size, (*join).align).unwrap());
        }
        std::ptr::drop_in_place(block);
        let spare = spare_blocks();
        if spare.len() < SPARE_BLOCK_LIMIT {
            spare.push(block);
        } else {
            std::alloc::dealloc(block.cast(), std::alloc::Layout::new::<Spawned>());
        }
    }
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
    visit: Option<Visit>,
) -> *mut JoinObject {
    start_workers();
    let align = align.max(8);
    LIVE_OBJECTS.made();
    let spawned = Spawned {
        join: JoinObject {
            rc: AtomicUsize::new(1),
            lock: AtomicBool::new(false),
            done: false,
            taken: false,
            waiters: Waiters::default(),
            result: std::ptr::null_mut(),
            size,
            align,
            release,
            group: std::ptr::null_mut(),
            index: 0,
            task: std::ptr::null_mut(),
            detached: false,
            visit,
            holders: AtomicU8::new(2),
            panic: None,
        },
        task: std::mem::MaybeUninit::uninit(),
        result: [0; 2],
    };
    let spare = SPARE_BLOCKS.with(Cell::get);
    let block = match unsafe { spare.as_mut() }.and_then(Vec::pop) {
        Some(block) => unsafe {
            block.write(spawned);
            block
        },
        None => Box::into_raw(Box::new(spawned)),
    };
    let join = block as *mut JoinObject;
    unsafe {
        (*join).result = match size {
            0 => std::ptr::null_mut(),
            _ if size <= 16 && align <= 8 => (&raw mut (*block).result).cast(),
            _ => std::alloc::alloc(std::alloc::Layout::from_size_align(size, align).unwrap()),
        };
    }
    if visit.is_some() {
        cycles::remember(join as *const u8, Kind::Join);
    }
    let code = unsafe { *(body.add(16) as *const *const u8) };
    let kind = match kind {
        0 => ResultKind::Nothing,
        1 => ResultKind::Integer,
        2 => ResultKind::Float64,
        3 => ResultKind::Float32,
        _ => ResultKind::Memory,
    };
    let task = new_task(None, body, code, kind, join, unsafe { (&raw mut (*block).task).cast() });
    unsafe {
        (*join).task = task;
        let spawned = crate::trace::spawned(origin());
        tools::spawned(task, code, spawned);
        if let Some(parent) = current().as_ref().filter(|parent| parent.flight != 0) {
            (*task).flight = parent.flight;
            let did = crate::diagnostics::Did::Spawned((*task).id);
            crate::diagnostics::step(parent.flight, parent.id, did, spawned.frames().to_vec());
        }
        (*task).origin = Some(spawned);
    }
    enqueue(task);
    may_switch();
    join
}

/// Marks the task done and notifies whoever waits for it. The group hears
/// of it under the join's lock, so nobody sees the task done, and frees the
/// group, before that. True when the task was detached.
pub(super) unsafe fn complete(join: *mut JoinObject) -> bool {
    unsafe {
        tools::ended(join as usize);
        let j = &mut *join;
        lock(&j.lock);
        j.done = true;
        let detached = j.detached;
        // Most often one task to wake, kept in place.
        let (mut first, mut rest): (Option<*mut Task>, Vec<*mut Task>) = (None, Vec::new());
        let mut woken = |task: *mut Task| match first {
            None => first = Some(task),
            Some(_) => rest.push(task),
        };
        for (wait, index) in j.waiters.take() {
            if (*wait).claim(index) {
                woken((*wait).task);
            }
        }
        if !j.group.is_null()
            && let Some(task) = group_completed(j.group, j.index)
        {
            woken(task);
        }
        unlock(&j.lock);
        first.into_iter().chain(rest).for_each(notify);
        detached
    }
}

/// The joins of detached tasks that are still running.
pub(super) static DETACHED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `spawn detached`: the task the next spawn starts is detached from its
/// start (11.11).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_spawn_detached() {
    if let Some(task) = unsafe { current().as_mut() } {
        task.detaching = true;
    }
}

/// `job.detach()`: the task runs on without anyone waiting for it, and
/// holds its join until it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_detach(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        let mut detached = DETACHED.lock().unwrap_or_else(|e| e.into_inner());
        lock(&j.lock);
        if !j.done {
            j.detached = true;
            j.rc.fetch_add(1, Ordering::Relaxed);
            detached.push(join as usize);
            // A detached task belongs to no `recover` but its own, and to
            // no deadline.
            if !j.task.is_null() {
                (*j.task).free.store(true, Ordering::Release);
                (*j.task).inherited.store(u64::MAX, Ordering::Release);
            }
        }
        unlock(&j.lock);
    }
}

/// When `main` returns: cancels the detached tasks still running and waits
/// for their cleanup (11.11), including tasks they detach meanwhile.
pub(super) fn end_detached() {
    loop {
        let joins: Vec<usize> = {
            let detached = DETACHED.lock().unwrap_or_else(|e| e.into_inner());
            for &join in detached.iter() {
                unsafe { (*(join as *mut JoinObject)).rc.fetch_add(1, Ordering::Relaxed) };
            }
            detached.clone()
        };
        if joins.is_empty() {
            return;
        }
        for join in joins {
            unsafe {
                cancel_join(join as *mut JoinObject);
                tondo_rt_join_release(join as *mut JoinObject);
            }
        }
    }
}

/// Waits until the task is done; false if the waiting task is cancelled
/// first, unless `fully`.
pub(super) unsafe fn finish(join: *mut JoinObject, fully: bool) -> bool {
    unsafe {
        let j = &mut *join;
        // It comes after the task whenever it asks, which makes no difference.
        tools::note(|| format!("h {}\n", &raw const j.lock as usize));
        lock_unseen(&j.lock);
        if j.done {
            unlock(&j.lock);
            return true;
        }
        let wait = Wait::new();
        j.waiters.push((Arc::as_ptr(&wait), 0));
        unlock(&j.lock);
        if fully {
            blocked_in(JOIN, join as usize, || wait.block_fully());
            return true;
        }
        if blocked_in(JOIN, join as usize, || wait.block_on_task()) != CANCELLED {
            return true;
        }
        lock_unseen(&j.lock);
        j.waiters.remove(Arc::as_ptr(&wait));
        unlock(&j.lock);
        false
    }
}

/// Cancels the task of a join and waits until it is done (11.15).
pub(super) unsafe fn cancel_join(join: *mut JoinObject) {
    unsafe {
        let j = &mut *join;
        lock(&j.lock);
        if !j.done {
            cancel_task(j.task);
        }
        unlock(&j.lock);
        finish(join, true);
    }
}

/// Moves the result out of a finished task into `out`.
pub(super) unsafe fn take(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        if (*join).size > 0 {
            std::ptr::copy_nonoverlapping((*join).result, out, (*join).size);
        }
        (*join).taken = true;
    }
}

/// `job.wait()`: waits for the task and moves its result into `out`. The
/// handle is released as usual by its owner. A cancelled waiter gets zeros.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_wait(join: *mut JoinObject, out: *mut u8) {
    unsafe {
        if !stops() && finish(join, false) {
            if let Some(panic) = (*join).panic.take() {
                // The task's panic goes on here (language 15.9).
                std::ptr::write_bytes(out, 0, (*join).size);
                crate::panics::rethrow(panic);
                return;
            }
            tools::joined(join as usize);
            take(join, out);
        } else {
            std::ptr::write_bytes(out, 0, (*join).size);
        }
    }
}

/// `job.wait(timeout: delay)`: the result into `out` and 1, or 0 after
/// cancelling the task when it does not finish within `nanos`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_wait_timeout(join: *mut JoinObject, out: *mut u8, nanos: i64) -> u8 {
    let mut cases = [
        Case { kind: WAIT, found: 0, handle: join as *mut u8, value: out, nanos: 0 },
        Case { kind: SLEEP, found: 0, handle: std::ptr::null_mut(), value: std::ptr::null_mut(), nanos },
    ];
    match unsafe { select_within(&mut cases) } {
        0 => 1,
        1 => {
            unsafe { cancel_join(join) };
            0
        }
        _ => 0,
    }
}

/// `job.cancel()`: cancels the task and waits until it is done.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_join_cancel(join: *mut JoinObject) {
    unsafe { cancel_join(join) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_handle_retain(handle: *mut u8) {
    if !handle.is_null() {
        unsafe { (*(handle as *const AtomicUsize)).fetch_add(1, Ordering::Relaxed) };
    }
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

/// Visits a finished task's result for the cycle collector; a task still
/// running holds its join.
pub(crate) unsafe fn visit_join(visitor: &mut Visitor, join: *mut u8) {
    let j = unsafe { &*(join as *const JoinObject) };
    if !j.done {
        visitor.hold();
    } else if !j.taken {
        unsafe { visitor.visit(j.visit, j.result) };
    }
}

/// Takes the result out of a garbage join.
pub(crate) unsafe fn clear_join(join: *mut u8) -> Option<Taken> {
    let j = unsafe { &mut *(join as *mut JoinObject) };
    if !j.done || j.taken {
        return None;
    }
    let release = j.release?;
    j.taken = true;
    let mut result = vec![0u64; j.size.div_ceil(8).max(1)];
    unsafe { std::ptr::copy_nonoverlapping(j.result, result.as_mut_ptr() as *mut u8, j.size) };
    Some(Box::new(move || unsafe { release(result.as_mut_ptr() as *mut u8) }))
}

/// Visits a group's joins for the cycle collector; tasks still running
/// hold their group.
pub(crate) unsafe fn visit_group(visitor: &mut Visitor, group: *mut u8) {
    let g = unsafe { &*(group as *const GroupObject) };
    if g.running > 0 {
        visitor.hold();
    }
    for entry in &g.entries {
        visitor.edge(entry.join as *const u8, Kind::Join, 0, 0);
    }
}

pub(super) unsafe fn free_join(join: *mut JoinObject) {
    unsafe {
        if (*join).visit.is_some() {
            cycles::forget(join as *const u8);
        }
        // A cancelled task cancels the tasks it still owns (11.15), and so
        // does one that unwinds from a panic (language 15.9).
        if cancelled() || crate::panics::cleaning() {
            cancel_join(join);
        }
        finish(join, true);
        if let Some(panic) = (*join).panic.take() {
            crate::panics::rethrow(panic);
        }
        if !(*join).taken
            && let Some(release) = (*join).release
        {
            release((*join).result);
        }
        LIVE_OBJECTS.gone();
        let_go(join);
    }
}
