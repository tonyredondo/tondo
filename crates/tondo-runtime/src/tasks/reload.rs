//! What a live build's reload needs of the tasks (toolchain 7.10): a wait that
//! ends so that its statement starts again, whether every task is still, and the
//! values of channels converted to a type that changed shape (docs/refactor.md,
//! phase 2).

use super::*;

/// Ends the wait of the task `id` so that the statement it waits in starts
/// again: a live build's reload (toolchain 7.10). False without the task
/// or when its wait cannot end so, as one for a file.
pub(crate) fn wait_again(id: u64) -> bool {
    let task = {
        let all = tasks();
        let Some(&task) = all.iter().find(|&&t| unsafe { (*(t as *mut Task)).id } == id) else { return false };
        task as *mut Task
    };
    unsafe {
        let t = &mut *task;
        // The task leaves its wait only after taking this lock, so it sees
        // the flag; a wait that ended first leaves no flag behind.
        lock_unseen(&t.waiting_lock);
        let woken = !t.waiting.is_null() && (*t.waiting).claim(CANCELLED);
        if woken && !t.again.swap(true, Ordering::SeqCst) {
            attend(true);
        }
        unlock(&t.waiting_lock);
        if woken {
            notify(task);
        }
        woken
    }
}

/// Whether no task runs Tondo code or is ready to: each waits, is in a call
/// of C, or has ended.
pub(crate) fn settled() -> bool {
    let callers: Vec<&InC> = CALLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let ready: std::collections::HashSet<usize> =
        queue().runnable.iter().copied().chain(waiting_next()).chain(waiting_local()).map(|t| t as usize).collect();
    let in_c = |task: *mut Task| {
        callers.iter().any(|c| c.depth.load(Ordering::SeqCst) > 0 && c.task.load(Ordering::SeqCst) == task)
    };
    tasks().iter().all(|&t| unsafe {
        let task = &*(t as *mut Task);
        task.finished || !ready.contains(&t) && (task.state.load(Ordering::Acquire) == PARKED || in_c(t as *mut Task))
    })
}

/// A channel's values converted to another type, in a new buffer, which
/// `channel_commit` puts in: none when one does not convert, with those made
/// so far released by `release`.
pub(crate) unsafe fn channel_convert(
    channel: usize,
    convert: &mut dyn FnMut(*const u8, *mut u8) -> bool,
    size: usize,
    align: usize,
    release: Option<crate::Glue>,
) -> Option<VecDeque<u8>> {
    let c = unsafe { &mut *(channel as *mut ChannelObject) };
    lock(&c.lock);
    let (old, new) = (
        std::alloc::Layout::from_size_align(c.size.max(1), c.align).unwrap(),
        std::alloc::Layout::from_size_align(size.max(1), align).unwrap(),
    );
    let (from, to) = unsafe { (std::alloc::alloc(old), std::alloc::alloc(new)) };
    let mut out = VecDeque::new();
    let mut made = 0;
    let mut fine = true;
    for index in 0..c.count {
        let at = index * c.size;
        for byte in 0..c.size {
            unsafe { from.add(byte).write(c.buffer[at + byte]) };
        }
        if !convert(from, to) {
            fine = false;
            break;
        }
        out.extend(unsafe { std::slice::from_raw_parts(to, size) });
        made += 1;
    }
    unlock(&c.lock);
    if !fine {
        for index in 0..made {
            let at = index * size;
            for byte in 0..size {
                unsafe { to.add(byte).write(out[at + byte]) };
            }
            if let Some(release) = release {
                unsafe { release(to) };
            }
        }
    }
    unsafe {
        std::alloc::dealloc(from, old);
        std::alloc::dealloc(to, new);
    }
    fine.then_some(out)
}

/// Puts in a channel's values converted, of another type: what releases and
/// visits them goes with them, and the old values go with what released
/// theirs.
pub(crate) unsafe fn channel_commit(
    channel: usize,
    buffer: VecDeque<u8>,
    size: usize,
    align: usize,
    release: Option<crate::Glue>,
    visit: Option<crate::cycles::Visit>,
) {
    let c = unsafe { &mut *(channel as *mut ChannelObject) };
    lock(&c.lock);
    let old = std::mem::replace(&mut c.buffer, buffer);
    let (old_size, old_align, old_release) = (c.size, c.align, c.release);
    let remembered = c.visit.is_some();
    (c.size, c.align, c.release, c.visit) = (size, align, release, visit);
    unlock(&c.lock);
    match (remembered, visit.is_some()) {
        (false, true) => cycles::remember(channel as *const u8, cycles::Kind::Channel),
        (true, false) => cycles::forget(channel as *const u8),
        _ => {}
    }
    if let Some(release) = old_release
        && old_size > 0
    {
        let layout = std::alloc::Layout::from_size_align(old_size, old_align).unwrap();
        let value = unsafe { std::alloc::alloc(layout) };
        for index in 0..old.len() / old_size {
            for byte in 0..old_size {
                unsafe { value.add(byte).write(old[index * old_size + byte]) };
            }
            unsafe { release(value) };
        }
        unsafe { std::alloc::dealloc(value, layout) };
    }
}
