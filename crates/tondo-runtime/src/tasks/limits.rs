//! `tasks.limit(n)` (language 11.17): places that the `within` blocks of
//! one limit share. A task that a block's own task starts, not detached,
//! takes a place in every limit around it, all of them or none, before it
//! is made, and gives each back as it ends. The tasks it starts take none
//! of them: a limit is not inherited.

use super::*;
use crate::native::NativeObject;

/// A limit: how many places it has, and, under its lock, how many are
/// taken and the tasks that wait for one.
pub(crate) struct Limit {
    places: usize,
    lock: AtomicBool,
    state: std::cell::UnsafeCell<Places>,
}

struct Places {
    taken: usize,
    waiting: VecDeque<*const Wait>,
}

// Its state is read and changed under its lock alone.
unsafe impl Send for Limit {}
unsafe impl Sync for Limit {}

impl Limit {
    /// Its state, which only the holder of its lock may touch.
    fn places(&self) -> *mut Places {
        self.state.get()
    }

    /// Takes a place when one is free, under the lock.
    fn take(&self) -> bool {
        lock(&self.lock);
        let places = unsafe { &mut *self.places() };
        let free = places.taken < self.places;
        if free {
            places.taken += 1;
        }
        unlock(&self.lock);
        free
    }

    /// Gives a place back and wakes the first task that waits for one,
    /// which tries again.
    fn give_back(&self) {
        lock(&self.lock);
        let places = unsafe { &mut *self.places() };
        places.taken -= 1;
        let mut woken = None;
        while let Some(wait) = places.waiting.pop_front() {
            if unsafe { (*wait).claim(0) } {
                woken = Some(unsafe { (*wait).task });
                break;
            }
        }
        unlock(&self.lock);
        woken.into_iter().for_each(notify);
    }
}

/// Whether the program made a limit: until it does, a `spawn` and a
/// task's end look at nothing else of limits, one load of this.
pub(super) static USED: AtomicBool = AtomicBool::new(false);

fn limit_of<'a>(object: *mut NativeObject) -> &'a Limit {
    crate::native::get::<Limit>(object).unwrap_or_else(|| runtime_fault("a tasks.Limit that is none"))
}

/// `tasks.limit(places)`: a limit with that many places, on the heap, as
/// tasks and blocks share it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_tasks_limit(places: i64) -> *mut NativeObject {
    let limit = Limit {
        places: places.max(1) as usize,
        lock: AtomicBool::new(false),
        state: std::cell::UnsafeCell::new(Places { taken: 0, waiting: VecDeque::new() }),
    };
    USED.store(true, Ordering::Relaxed);
    crate::native::new(limit)
}

/// What a task holds of limits: the `within tasks.limit(...)` blocks it
/// runs, innermost last, each of whose places the tasks it starts take;
/// and the places it holds, of the limits around the `spawn` that started
/// it, which it gives back as it ends.
#[derive(Default)]
pub(crate) struct Limits {
    entered: Vec<*mut NativeObject>,
    held: Vec<*mut NativeObject>,
}

/// The start of `within limit { ... }` in the running task: the tasks the
/// block starts take a place of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_tasks_limit_enter(limit: *mut NativeObject) {
    let Some(task) = (unsafe { current().as_mut() }) else { return };
    unsafe { crate::native::retain(limit) };
    if task.limits.is_null() {
        task.limits = Box::into_raw(Box::default());
    }
    unsafe { (*task.limits).entered.push(limit) };
}

/// The end of the innermost `within limit { ... }`, however it is left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_tasks_limit_leave() {
    let Some(task) = (unsafe { current().as_mut() }) else { return };
    if let Some(limit) = unsafe { task.limits.as_mut() }.and_then(|limits| limits.entered.pop()) {
        unsafe { crate::native::tondo_rt_native_release(limit) };
    }
}

/// The places a task the running task starts takes, in a program that
/// made a limit; none in a task that runs within no limit.
#[inline(never)]
pub(super) fn take_places() -> *mut Limits {
    let Some(task) = (unsafe { current().as_mut() }) else { return std::ptr::null_mut() };
    match unsafe { task.limits.as_ref() } {
        Some(limits) if !limits.entered.is_empty() => taken(task).map_or(std::ptr::null_mut(), Box::into_raw),
        _ => std::ptr::null_mut(),
    }
}

/// One place of every limit the running task runs within, all or none.
/// While one is full, it holds none of the others, so that two tasks
/// whose blocks nest the same limits in other orders cannot wait for each
/// other; it waits for that one and tries all again. None when it starts
/// a detached task, or is cancelled, or its deadline passes, while it
/// waits: the task it starts then holds no place, and the cancellation
/// ends the spawner at its next wait.
#[inline(never)]
fn taken(task: &mut Task) -> Option<Box<Limits>> {
    if task.detaching {
        return None;
    }
    let limits = unsafe { task.limits.as_ref() }.map(|limits| limits.entered.clone()).unwrap_or_default();
    'again: loop {
        for (index, &object) in limits.iter().enumerate() {
            if limit_of(object).take() {
                continue;
            }
            // Full: give back what was taken, then wait for this one.
            limits[..index].iter().for_each(|&taken| limit_of(taken).give_back());
            let limit = limit_of(object);
            let wait = Wait::new();
            lock(&limit.lock);
            let places = unsafe { &mut *limit.places() };
            if places.taken < limit.places {
                unlock(&limit.lock);
                continue 'again;
            }
            places.waiting.push_back(Arc::as_ptr(&wait));
            unlock(&limit.lock);
            let fired = blocked_in(PLACE, object as usize, || wait.block_on_task());
            if fired == CANCELLED {
                lock(&limit.lock);
                unsafe { (*limit.places()).waiting.retain(|&w| w != Arc::as_ptr(&wait)) };
                unlock(&limit.lock);
                return None;
            }
            continue 'again;
        }
        // Every one taken: the task keeps a reference to each.
        for &object in &limits {
            unsafe { crate::native::retain(object) };
        }
        return Some(Box::new(Limits { entered: Vec::new(), held: limits }));
    }
}

/// Gives back the places a task that ends holds, and lets go of the
/// limits it ran within that unwinding left: nothing to look at in a
/// program that made no limit.
#[inline(always)]
pub(super) fn give_places(task: &mut Task) {
    if USED.load(Ordering::Relaxed) && !task.limits.is_null() {
        let limits = std::mem::replace(&mut task.limits, std::ptr::null_mut());
        given(*unsafe { Box::from_raw(limits) });
    }
}

#[inline(never)]
fn given(limits: Limits) {
    for object in limits.held {
        limit_of(object).give_back();
        unsafe { crate::native::tondo_rt_native_release(object) };
    }
    for object in limits.entered {
        unsafe { crate::native::tondo_rt_native_release(object) };
    }
}

/// What a task waiting for a place waits for, in words: how many places
/// the limit has, all taken.
pub(crate) fn described(object: usize) -> String {
    let limit = limit_of(object as *mut NativeObject);
    format!("a place of a tasks.limit({}), all taken", limit.places)
}
