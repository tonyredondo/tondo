//! Channels and `select`: a channel's buffer and the tasks that wait to send
//! and to receive on it, its senders, and a `select` over channels, joins,
//! groups and timers (docs/refactor.md, phase 2).

use super::*;

/// A task waiting on a channel: the value it sends, or where the value it
/// receives goes, and how the wait ended.
pub(super) struct Waiter {
    pub(super) wait: *const Wait,
    pub(super) index: usize,
    pub(super) value: *mut u8,
    pub(super) state: u8,
}

pub(super) const MOVED: u8 = 1;
pub(super) const CLOSED: u8 = 2;
/// A send ended because no receiver was left to take its value.
pub(super) const DISCARDED: u8 = 3;

/// Whether a channel that discards drops what is sent now: no `Receiver`
/// is left, only the senders' holds. With the channel locked.
pub(super) fn discarding(c: &ChannelObject) -> bool {
    c.discard && c.rc.load(Ordering::Acquire) <= c.senders
}

/// A channel (11.13). `rc` counts the `Receiver` handles and the sender
/// objects; `senders` counts the latter, and the channel closes when it
/// reaches zero. A receiver only waits while the buffer is empty, and a
/// sender only while it is full.
#[repr(C)]
pub struct ChannelObject {
    pub(super) rc: AtomicUsize,
    pub(super) lock: AtomicBool,
    pub(super) closed: bool,
    pub(super) senders: usize,
    /// `discardWhenAbandoned`: once no `Receiver` is left, sends drop their
    /// values at once.
    pub(super) discard: bool,
    /// `usize::MAX` when unbounded; 0 for a rendezvous.
    pub(super) capacity: usize,
    pub(super) size: usize,
    pub(super) align: usize,
    pub(super) release: Option<Glue>,
    /// Visits a value for the cycle collector, when it may refer back to
    /// the channel, which is then registered with it.
    pub(super) visit: Option<Visit>,
    /// The buffered values' bytes, `count` values of `size` bytes each.
    pub(super) buffer: VecDeque<u8>,
    pub(super) count: usize,
    pub(super) receiving: VecDeque<*mut Waiter>,
    pub(super) sending: VecDeque<*mut Waiter>,
}

/// A `Sender` handle: one per `channel` or `copy()`, shared by the
/// program's references to it.
#[repr(C)]
pub struct SenderObject {
    pub(super) rc: AtomicUsize,
    pub(super) channel: *mut ChannelObject,
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
    visit: Option<Visit>,
    location: *const Str,
    discard: u8,
) -> *mut ChannelObject {
    if bounded != 0 && capacity < 0 {
        crate::panic_with(
            1,
            &format!("a channel cannot hold {capacity} values: its capacity must not be negative"),
            location,
        );
    }
    LIVE_OBJECTS.made();
    let channel = Box::into_raw(Box::new(ChannelObject {
        rc: AtomicUsize::new(1),
        lock: AtomicBool::new(false),
        closed: false,
        senders: 0,
        discard: discard != 0,
        capacity: if bounded != 0 { capacity as usize } else { usize::MAX },
        size,
        align: align.max(1),
        release,
        visit,
        buffer: VecDeque::new(),
        count: 0,
        receiving: VecDeque::new(),
        sending: VecDeque::new(),
    }));
    if visit.is_some() {
        cycles::remember(channel as *const u8, Kind::Channel);
    }
    channel
}

/// Visits the buffered values for the cycle collector.
pub(crate) unsafe fn visit_channel(visitor: &mut Visitor, channel: *mut u8) {
    let c = unsafe { &*(channel as *const ChannelObject) };
    let mut value = vec![0u64; c.size.div_ceil(8).max(1)];
    for index in 0..c.count {
        let bytes = c.buffer.range(index * c.size..(index + 1) * c.size);
        for (at, &byte) in bytes.enumerate() {
            unsafe { *(value.as_mut_ptr() as *mut u8).add(at) = byte };
        }
        unsafe { visitor.visit(c.visit, value.as_mut_ptr() as *mut u8) };
    }
}

/// The channel a sender sends to, for the cycle collector.
pub(crate) unsafe fn visit_sender(visitor: &mut Visitor, sender: *mut u8) {
    let channel = unsafe { (*(sender as *const SenderObject)).channel };
    visitor.edge(channel as *const u8, Kind::Channel, 0, 0);
}

/// Takes the buffered values out of a garbage channel.
pub(crate) unsafe fn clear_channel(channel: *mut u8) -> Option<Taken> {
    let c = unsafe { &mut *(channel as *mut ChannelObject) };
    let release = c.release?;
    let (mut buffer, count, size) = (std::mem::take(&mut c.buffer), std::mem::take(&mut c.count), c.size);
    Some(Box::new(move || {
        let mut value = vec![0u64; size.div_ceil(8).max(1)];
        for _ in 0..count {
            pop_bytes(&mut buffer, value.as_mut_ptr() as *mut u8, size);
            unsafe { release(value.as_mut_ptr() as *mut u8) };
        }
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
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(SenderObject { rc: AtomicUsize::new(1), channel }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_copy(sender: *mut SenderObject) -> *mut SenderObject {
    unsafe { tondo_rt_sender_new((*sender).channel) }
}

/// Moves the value at the front of the buffer, `size` bytes, into `out`.
pub(super) fn pop_bytes(buffer: &mut VecDeque<u8>, out: *mut u8, size: usize) {
    let (front, back) = buffer.as_slices();
    let first = front.len().min(size);
    unsafe {
        std::ptr::copy_nonoverlapping(front.as_ptr(), out, first);
        std::ptr::copy_nonoverlapping(back.as_ptr(), out.add(first), size - first);
    }
    buffer.drain(..size);
}

/// The first waiter of `queue` whose wait it can still claim, removed;
/// waiters another operation claimed first are dropped.
pub(super) unsafe fn claim_first(queue: &mut VecDeque<*mut Waiter>) -> Option<*mut Waiter> {
    while let Some(waiter) = queue.pop_front() {
        if unsafe { (*(*waiter).wait).claim((*waiter).index) } {
            return Some(waiter);
        }
    }
    None
}

/// Ends the waits of a channel that just closed; returns who to notify.
pub(super) unsafe fn close_waits(c: &mut ChannelObject) -> Vec<*mut Task> {
    c.closed = true;
    let mut woken = Vec::new();
    for waiter in c.receiving.drain(..).chain(c.sending.drain(..)) {
        unsafe {
            if (*(*waiter).wait).claim((*waiter).index) {
                (*waiter).state = CLOSED;
                woken.push((*(*waiter).wait).task);
            }
        }
    }
    woken
}

pub(super) const CLOSED_MESSAGE: &str = "cannot send on this channel: it was closed";

/// Moves the value at `value` into the channel if it can without waiting,
/// with the channel locked: `Some(task)` with a receiver to notify, or
/// `None` when it must wait.
pub(super) unsafe fn try_send(c: &mut ChannelObject, value: *mut u8) -> Option<Option<*mut Task>> {
    unsafe {
        if let Some(receiver) = claim_first(&mut c.receiving) {
            std::ptr::copy_nonoverlapping(value, (*receiver).value, c.size);
            (*receiver).state = MOVED;
            let task = (*(*receiver).wait).task;
            crate::debugger::handed(current_id(), (*task).id);
            return Some(Some(task));
        }
        if c.count < c.capacity {
            crate::debugger::sent(c as *const ChannelObject as usize, c.count, current_id());
            c.buffer.extend(std::slice::from_raw_parts(value, c.size));
            c.count += 1;
            return Some(None);
        }
        None
    }
}

/// Takes the next value into `out` if it can without waiting, with the
/// channel locked: `Some((found, task))`, where `found` is false once the
/// channel is closed and empty and `task` is a sender to notify.
pub(super) unsafe fn try_receive(c: &mut ChannelObject, out: *mut u8) -> Option<(bool, Option<*mut Task>)> {
    unsafe {
        if c.count > 0 {
            let channel = c as *const ChannelObject as usize;
            crate::debugger::taken(channel, c.count, current_id());
            pop_bytes(&mut c.buffer, out, c.size);
            c.count -= 1;
            // A sender waiting for room moves its value in.
            let woken = claim_first(&mut c.sending).map(|sender| {
                crate::debugger::sent(channel, c.count, (*(*(*sender).wait).task).id);
                c.buffer.extend(std::slice::from_raw_parts((*sender).value, c.size));
                c.count += 1;
                (*sender).state = MOVED;
                (*(*sender).wait).task
            });
            return Some((true, woken));
        }
        // A rendezvous: take the value from a waiting sender.
        if let Some(sender) = claim_first(&mut c.sending) {
            std::ptr::copy_nonoverlapping((*sender).value, out, c.size);
            (*sender).state = MOVED;
            let task = (*(*sender).wait).task;
            crate::debugger::handed((*task).id, current_id());
            return Some((true, Some(task)));
        }
        if c.closed {
            return Some((false, None));
        }
        None
    }
}

/// `sender.send(value)`: moves the value at `value` into the channel,
/// waiting while it is full; panics with `P0012` if it is closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_send(sender: *mut SenderObject, value: *mut u8, location: *const Str) {
    unsafe {
        let c = &mut *(*sender).channel;
        crate::debugger::channel_used(SENDING, c as *const ChannelObject as usize);
        if stops() {
            drop_unsent(c, value);
            return;
        }
        lock(&c.lock);
        if c.closed {
            unlock(&c.lock);
            drop_unsent(c, value);
            crate::raise(12, CLOSED_MESSAGE, location);
            return;
        }
        if discarding(c) {
            unlock(&c.lock);
            drop_unsent(c, value);
            return;
        }
        if let Some(woken) = try_send(c, value) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return;
        }
        let wait = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&wait), index: 0, value, state: 0 };
        c.sending.push_back(&mut waiter);
        unlock(&c.lock);
        let channel = c as *const ChannelObject as usize;
        if blocked_in(SENDING, channel, || wait.block_on_task()) == CANCELLED {
            lock(&c.lock);
            c.sending.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            drop_unsent(c, value);
            return;
        }
        match std::ptr::read_volatile(&waiter.state) {
            CLOSED => {
                drop_unsent(c, value);
                crate::raise(12, CLOSED_MESSAGE, location);
            }
            DISCARDED => drop_unsent(c, value),
            _ => {}
        }
    }
}

/// `std.internal.channel`'s `offer` (`wait` 1) and `tryOffer`: sends the
/// value, waiting for room only when `wait` is set, and gives 0 when sent,
/// 1 when the channel is full and it did not wait, 2 once the channel is
/// closed or the task cancelled. A value not sent is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_offer(sender: *mut SenderObject, value: *mut u8, wait: u8) -> i64 {
    const SENT: i64 = 0;
    const FULL: i64 = 1;
    const SHUT: i64 = 2;
    unsafe {
        let c = &mut *(*sender).channel;
        if stops() {
            drop_unsent(c, value);
            return SHUT;
        }
        lock(&c.lock);
        if c.closed {
            unlock(&c.lock);
            drop_unsent(c, value);
            return SHUT;
        }
        if discarding(c) {
            unlock(&c.lock);
            drop_unsent(c, value);
            return SENT;
        }
        if let Some(woken) = try_send(c, value) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return SENT;
        }
        if wait == 0 {
            unlock(&c.lock);
            drop_unsent(c, value);
            return FULL;
        }
        let blocked = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&blocked), index: 0, value, state: 0 };
        c.sending.push_back(&mut waiter);
        unlock(&c.lock);
        if blocked.block_on_task() == CANCELLED {
            lock(&c.lock);
            c.sending.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            drop_unsent(c, value);
            return SHUT;
        }
        match std::ptr::read_volatile(&waiter.state) {
            CLOSED => {
                drop_unsent(c, value);
                SHUT
            }
            DISCARDED => {
                drop_unsent(c, value);
                SENT
            }
            _ => SENT,
        }
    }
}

/// Sends a value without waiting, from outside any task, as a signal is
/// (`signals`): not when the channel is closed or full, or when the last
/// value it holds is the same, which this one joins.
/// Whether no `Receiver` of a sender's channel is left to take its values.
pub(crate) unsafe fn unheard(sender: *mut SenderObject) -> bool {
    unsafe {
        let c = &*(*sender).channel;
        c.rc.load(Ordering::Acquire) <= c.senders
    }
}

pub(crate) unsafe fn offer_new(sender: *mut SenderObject, value: *mut u8) {
    unsafe {
        let c = &mut *(*sender).channel;
        lock(&c.lock);
        let size = c.size;
        let fresh = std::slice::from_raw_parts(value, size);
        let same = c.count > 0 && c.buffer.range(c.buffer.len() - size..).eq(fresh.iter());
        let woken = if c.closed || same { None } else { try_send(c, value) };
        unlock(&c.lock);
        woken.flatten().into_iter().for_each(notify);
    }
}

/// A value a cancelled task did not send: it was the channel's to release.
pub(super) unsafe fn drop_unsent(c: &ChannelObject, value: *mut u8) {
    if let Some(release) = c.release {
        unsafe { release(value) };
    }
}

/// `receiver.receive()`: moves the next value into `out` and returns 1, or
/// returns 0 once the channel is closed and empty, or if the task is
/// cancelled (in a `defer`, only when it would wait).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_receive(channel: *mut ChannelObject, out: *mut u8) -> u8 {
    unsafe {
        crate::debugger::channel_used(RECEIVING, channel as usize);
        let c = &mut *channel;
        if stops() {
            return 0;
        }
        lock(&c.lock);
        if let Some((found, woken)) = try_receive(c, out) {
            unlock(&c.lock);
            woken.into_iter().for_each(notify);
            return found as u8;
        }
        let wait = Wait::new();
        let mut waiter = Waiter { wait: Arc::as_ptr(&wait), index: 0, value: out, state: 0 };
        c.receiving.push_back(&mut waiter);
        unlock(&c.lock);
        if blocked_in(RECEIVING, channel as usize, || wait.block_on_task()) == CANCELLED {
            lock(&c.lock);
            c.receiving.retain(|w| *w != &raw mut waiter);
            unlock(&c.lock);
            return 0;
        }
        (std::ptr::read_volatile(&waiter.state) == MOVED) as u8
    }
}

/// `receiver.receive(timeout: delay)`: like `receive`, but gives 0 if
/// nothing arrives within `nanos`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_receive_timeout(channel: *mut ChannelObject, out: *mut u8, nanos: i64) -> u8 {
    let mut cases = [
        Case { kind: RECEIVE, found: 0, handle: channel as *mut u8, value: out, nanos: 0 },
        Case { kind: SLEEP, found: 0, handle: std::ptr::null_mut(), value: std::ptr::null_mut(), nanos },
    ];
    let won = unsafe { select_within(&mut cases) };
    (won == 0 && cases[0].found != 0) as u8
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
        woken.into_iter().for_each(notify);
    }
}

/// `isClosed()` of a `Sender` (`sender` is 1) or a `Receiver`; with
/// `sender` 2, `isAbandoned()`: whether no `Receiver` of a sender's channel
/// is left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_channel_closed(handle: *mut u8, sender: u8) -> u8 {
    unsafe {
        let c = if sender != 0 { &*(*(handle as *mut SenderObject)).channel } else { &*(handle as *mut ChannelObject) };
        lock(&c.lock);
        let closed = if sender == 2 { c.rc.load(Ordering::Acquire) <= c.senders } else { c.closed };
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
        LIVE_OBJECTS.gone();
        let c = &mut *sender.channel;
        lock(&c.lock);
        c.senders -= 1;
        let woken = if c.senders == 0 && !c.closed { close_waits(c) } else { Vec::new() };
        unlock(&c.lock);
        woken.into_iter().for_each(notify);
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
            // The last `Receiver` of a channel that discards: the senders
            // waiting for room drop their values, and so do later ones.
            let c = &mut *channel;
            if c.discard {
                lock(&c.lock);
                let mut woken = Vec::new();
                if discarding(c) {
                    while let Some(waiter) = claim_first(&mut c.sending) {
                        (*waiter).state = DISCARDED;
                        woken.push((*(*waiter).wait).task);
                    }
                }
                unlock(&c.lock);
                woken.into_iter().for_each(notify);
            }
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        if (*channel).visit.is_some() {
            cycles::forget(channel as *const u8);
        }
        crate::live::forget_object(channel as usize);
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
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// select
// ---------------------------------------------------------------------------

pub(super) const RECEIVE: u8 = 0;
pub(super) const SEND: u8 = 1;
pub(super) const WAIT: u8 = 2;
pub(super) const NEXT: u8 = 3;
pub(super) const SLEEP: u8 = 4;

/// One operation of a `select` (11.14), as compiled code lays it out:
/// `handle` is the receiver's channel, the sender, the join or the group;
/// `value` the value sent, or where a received value or a result goes;
/// `found` tells whether `receive` or `next` produced one.
#[repr(C)]
pub struct Case {
    pub(super) kind: u8,
    pub(super) found: u8,
    pub(super) handle: *mut u8,
    pub(super) value: *mut u8,
    pub(super) nanos: i64,
}

impl Case {
    /// The lock of the object the operation waits on.
    unsafe fn lock(&self) -> Option<*const AtomicBool> {
        unsafe {
            match self.kind {
                RECEIVE => Some(&raw const (*(self.handle as *mut ChannelObject)).lock),
                SEND => Some(&raw const (*(*(self.handle as *mut SenderObject)).channel).lock),
                WAIT => Some(&raw const (*(self.handle as *mut JoinObject)).lock),
                NEXT => Some(&raw const (*(self.handle as *mut GroupObject)).lock),
                _ => None,
            }
        }
    }
}

/// A small generator for the random choice among ready operations; the
/// seed's, while exploring orders.
pub(super) fn random(bound: usize) -> usize {
    if let Some(seed) = exploring() {
        return scripted(bound, &[]).unwrap_or_else(|| (seeded(seed) % bound as u64) as usize);
    }
    static STATE: AtomicUsize = AtomicUsize::new(0x9e37_79b9_7f4a_7c15);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x % bound
}

/// A wait of `cases[0]` for as long as the timer `cases[1]` says: what is
/// ready already goes first, as a timeout of nothing is ready too.
pub(super) unsafe fn select_within(cases: &mut [Case; 2]) -> i64 {
    match unsafe { select(&mut cases[..1], false, std::ptr::null()) } {
        -1 => unsafe { select(cases, true, std::ptr::null()) },
        won => won,
    }
}

/// `select` over `count` operations at `cases`: completes the first that
/// is ready, chosen at random among those ready, and returns its index;
/// with `blocking` 0 (an `else` arm) returns -1 when none is, and -2 when
/// the task is cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_select(cases: *mut Case, count: usize, blocking: u8, location: *const Str) -> i64 {
    let cases = unsafe { std::slice::from_raw_parts_mut(cases, count) };
    unsafe { select(cases, blocking != 0, location) }
}

pub(super) unsafe fn select(cases: &mut [Case], blocking: bool, location: *const Str) -> i64 {
    if stops() {
        return -2;
    }
    unsafe {
        // Every object is locked, once and in address order, from checking
        // readiness until the task is registered everywhere.
        let mut locks: Vec<*const AtomicBool> = cases.iter().filter_map(|c| c.lock()).collect();
        locks.sort();
        locks.dedup();
        let lock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| lock(&**l));
        let unlock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| unlock(&**l));
        lock_all(&locks);
        let mut order: Vec<usize> = (0..cases.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, random(i + 1));
        }
        for &index in &order {
            let case = &mut cases[index];
            // A panic that a `recover` stops ends the `select` as a
            // cancellation would, and the code unwinds (language 15.9).
            if case.kind == SEND && (*(*(case.handle as *mut SenderObject)).channel).closed {
                unlock_all(&locks);
                crate::raise(12, CLOSED_MESSAGE, location);
                return -2;
            }
            if let Some(woken) = try_case(case) {
                unlock_all(&locks);
                woken.into_iter().for_each(notify);
                return index as i64;
            }
        }
        if !blocking {
            unlock_all(&locks);
            return -1;
        }
        let wait = Wait::new();
        let mut waiters: Vec<Waiter> = cases
            .iter()
            .enumerate()
            .map(|(index, case)| Waiter { wait: Arc::as_ptr(&wait), index, value: case.value, state: 0 })
            .collect();
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.push_back(waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.push_back(waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.push((Arc::as_ptr(&wait), index)),
                NEXT => (*(case.handle as *mut GroupObject)).waiter = Some((Arc::as_ptr(&wait), index)),
                _ => add_timer(Duration::from_nanos(case.nanos as u64), wait.clone(), index),
            }
        }
        unlock_all(&locks);
        let won = wait.block_on_task();
        // Leave every queue the wait is still in.
        lock_all(&locks);
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.retain(|w| *w != waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.retain(|w| *w != waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.retain(|&(w, _)| w != Arc::as_ptr(&wait)),
                NEXT => {
                    let group = &mut *(case.handle as *mut GroupObject);
                    if group.waiter.is_some_and(|(w, _)| w == Arc::as_ptr(&wait)) {
                        group.waiter = None;
                    }
                }
                _ => {}
            }
        }
        let sleeps = cases.iter().enumerate().filter(|&(i, c)| c.kind == SLEEP && i != won).count();
        if won == CANCELLED {
            unlock_all(&locks);
            timers_ended(sleeps);
            return -2;
        }
        // The operation that won is complete but for what this task does.
        let case = &mut cases[won];
        match case.kind {
            RECEIVE => case.found = (waiters[won].state == MOVED) as u8,
            SEND if waiters[won].state == CLOSED => {
                unlock_all(&locks);
                timers_ended(sleeps);
                crate::raise(12, CLOSED_MESSAGE, location);
                return -2;
            }
            SEND if waiters[won].state == DISCARDED => {
                drop_unsent(&*(*(case.handle as *mut SenderObject)).channel, case.value)
            }
            WAIT => take(case.handle as *mut JoinObject, case.value),
            NEXT => {
                let group = &mut *(case.handle as *mut GroupObject);
                case.found = group_try_next(group, case.value).unwrap_or(false) as u8;
            }
            _ => {}
        }
        unlock_all(&locks);
        if sleeps > 0 {
            timers_ended(sleeps);
        }
        won as i64
    }
}

/// Completes one operation of a `select` if it is ready, with its object
/// locked; returns a task to notify, if any.
pub(super) unsafe fn try_case(case: &mut Case) -> Option<Option<*mut Task>> {
    unsafe {
        match case.kind {
            RECEIVE => {
                let c = &mut *(case.handle as *mut ChannelObject);
                let (found, woken) = try_receive(c, case.value)?;
                case.found = found as u8;
                Some(woken)
            }
            SEND => {
                let c = &mut *(*(case.handle as *mut SenderObject)).channel;
                if discarding(c) {
                    drop_unsent(c, case.value);
                    return Some(None);
                }
                try_send(c, case.value)
            }
            WAIT => {
                let join = case.handle as *mut JoinObject;
                if !(*join).done {
                    return None;
                }
                take(join, case.value);
                Some(None)
            }
            NEXT => {
                let group = &mut *(case.handle as *mut GroupObject);
                case.found = group_try_next(group, case.value)? as u8;
                Some(None)
            }
            _ => (case.nanos <= 0).then_some(None),
        }
    }
}
