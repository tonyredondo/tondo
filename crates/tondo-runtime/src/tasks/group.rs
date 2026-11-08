//! Groups of tasks: a `Group` gives the results of the tasks added to it in
//! the order they finish, and with results, the first error cancels the
//! others (docs/refactor.md, phase 2).

use super::*;

/// `Group[T]` (11.12): the joins added, in order, which tasks finished and
/// `next` has not returned yet, and, when `T` is a result, the first task
/// that finished with an error.
#[repr(C)]
pub struct GroupObject {
    pub(super) rc: AtomicUsize,
    pub(super) lock: AtomicBool,
    pub(super) fallible: bool,
    pub(super) joins: Vec<*mut JoinObject>,
    pub(super) taken: Vec<bool>,
    pub(super) finished: VecDeque<usize>,
    pub(super) running: usize,
    pub(super) failed: Option<usize>,
    pub(super) waiter: Option<(*const Wait, usize)>,
}

/// `Group[T]()`; `fallible` when `T` is `R ! E`, whose results start
/// with their tag, `1` for `err`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_group_new(fallible: u8) -> *mut GroupObject {
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(GroupObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        fallible: fallible != 0,
        joins: Vec::new(),
        taken: Vec::new(),
        finished: VecDeque::new(),
        running: 0,
        failed: None,
        waiter: None,
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
        let woken = if j.done {
            group_completed(group, index)
        } else {
            j.group = group;
            j.index = index;
            None
        };
        unlock(&j.lock);
        woken.into_iter().for_each(notify);
    }
}

/// Records that the task at `index` finished; called under its join's
/// lock. Returns the task to notify, if one waits.
pub(super) unsafe fn group_completed(group: *mut GroupObject, index: usize) -> Option<*mut Task> {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        g.finished.push_back(index);
        g.running -= 1;
        let join = g.joins[index];
        if g.fallible && g.failed.is_none() && *(*join).result != 0 {
            g.failed = Some(index);
        }
        let woken = g.waiter.take().filter(|&(wait, i)| (*wait).claim(i)).map(|(wait, _)| (*wait).task);
        unlock(&g.lock);
        woken
    }
}

/// Waits, with the group locked, until a task finishes; returns locked,
/// and false if the waiting task is cancelled.
pub(super) unsafe fn group_block(g: &mut GroupObject) -> bool {
    let wait = Wait::new();
    g.waiter = Some((Arc::as_ptr(&wait), 0));
    unlock(&g.lock);
    let fired = wait.block_on_task();
    lock(&g.lock);
    if g.waiter.is_some_and(|(w, _)| w == Arc::as_ptr(&wait)) {
        g.waiter = None;
    }
    fired != CANCELLED
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

/// Takes the next finished result into `out`, with the group locked:
/// `Some(true)` when there was one, `Some(false)` when no task is left, and
/// `None` when it must wait.
pub(super) unsafe fn group_try_next(g: &mut GroupObject, out: *mut u8) -> Option<bool> {
    if g.taken.iter().all(|t| *t) {
        return Some(false);
    }
    while let Some(index) = g.finished.pop_front() {
        if !g.taken[index] {
            g.taken[index] = true;
            unsafe { take(g.joins[index], out) };
            return Some(true);
        }
    }
    None
}

/// `group.next()`: moves the result of the next task to finish into `out`
/// and returns 1, or returns 0 when no task is left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_next(group: *mut GroupObject, out: *mut u8) -> u8 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        loop {
            if let Some(found) = group_try_next(g, out).filter(|_| !stops()) {
                unlock(&g.lock);
                return found as u8;
            }
            if stops() || !group_block(g) {
                unlock(&g.lock);
                return 0;
            }
        }
    }
}

/// The first part of `group.wait()`: waits until every task finished or,
/// for results, one finished with an error, which cancels the others;
/// returns the index of that task, -1, or -2 if the waiting task is
/// cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_settle(group: *mut GroupObject) -> i64 {
    unsafe {
        let g = &mut *group;
        lock(&g.lock);
        loop {
            if stops() {
                unlock(&g.lock);
                return -2;
            }
            if let Some(index) = g.failed.filter(|&i| !g.taken[i]) {
                unlock(&g.lock);
                tondo_rt_group_cancel(group);
                return index as i64;
            }
            if g.running == 0 {
                unlock(&g.lock);
                return -1;
            }
            if !group_block(g) {
                unlock(&g.lock);
                return -2;
            }
        }
    }
}

/// `group.cancel()`: cancels the tasks still running and waits for them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_group_cancel(group: *mut GroupObject) {
    unsafe {
        for index in 0..(*group).joins.len() {
            cancel_join((&(*group).joins)[index]);
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
        finish(join, true);
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
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
