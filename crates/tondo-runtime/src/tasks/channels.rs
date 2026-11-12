//! Channels and `select`: a channel's buffer and the tasks that wait to send
//! and to receive on it, its senders, and a `select` over channels, joins,
//! groups and timers (docs/refactor.md, phase 2).

use super::*;

/// A task waiting on a channel: the value it sends, or where the value it
/// receives goes, and how the wait ended. It lives where the task waits,
/// on its stack, and is linked into the channel's queue there.
#[derive(Clone, Copy)]
pub(super) struct Waiter {
    pub(super) wait: *const Wait,
    pub(super) index: usize,
    pub(super) value: *mut u8,
    pub(super) state: u8,
    /// Whether it is in a queue, and its neighbours there.
    queued: bool,
    before: *mut Waiter,
    after: *mut Waiter,
}

impl Waiter {
    pub(super) fn new(wait: *const Wait, index: usize, value: *mut u8) -> Waiter {
        let null = std::ptr::null_mut();
        Waiter { wait, index, value, state: 0, queued: false, before: null, after: null }
    }
}

/// The tasks waiting on one side of a channel, first to last, linked
/// through their waiters, so that waiting takes no room of the channel's.
/// With the channel locked.
pub(super) struct Queue {
    first: *mut Waiter,
    last: *mut Waiter,
}

impl Queue {
    pub(super) const fn new() -> Queue {
        Queue { first: std::ptr::null_mut(), last: std::ptr::null_mut() }
    }

    pub(super) unsafe fn push(&mut self, waiter: *mut Waiter) {
        unsafe {
            (*waiter).queued = true;
            (*waiter).before = self.last;
            (*waiter).after = std::ptr::null_mut();
            match self.last.is_null() {
                true => self.first = waiter,
                false => (*self.last).after = waiter,
            }
        }
        self.last = waiter;
    }

    pub(super) unsafe fn pop(&mut self) -> Option<*mut Waiter> {
        let first = self.first;
        if first.is_null() {
            return None;
        }
        unsafe { self.remove(first) };
        Some(first)
    }

    /// Takes the waiter out, if it is still in the queue.
    pub(super) unsafe fn remove(&mut self, waiter: *mut Waiter) {
        unsafe {
            if !(*waiter).queued {
                return;
            }
            let (before, after) = ((*waiter).before, (*waiter).after);
            match before.is_null() {
                true => self.first = after,
                false => (*before).after = after,
            }
            match after.is_null() {
                true => self.last = before,
                false => (*after).before = before,
            }
            (*waiter).queued = false;
        }
    }
}

pub(super) const MOVED: u8 = 1;
pub(super) const CLOSED: u8 = 2;
/// A send ended because no receiver was left to take its value.
pub(super) const DISCARDED: u8 = 3;

/// Whether a channel that discards drops what is sent now: no `Receiver`
/// is left, only its sender's hold. With the channel locked.
pub(super) fn discarding(c: &ChannelObject) -> bool {
    c.discard && c.rc.load(Ordering::Acquire) <= c.senders
}

/// A channel (11.13). `rc` counts the `Receiver` handles and its sender's
/// hold; `senders` is 1 while the sender has references, and the channel
/// closes when it reaches zero. A receiver only waits while the buffer is
/// empty, and a sender only while it is full.
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
    /// The buffered values' bytes, `count` values of `size` bytes each,
    /// and the room after the channel that it may hold them in.
    pub(super) buffer: Ring,
    pub(super) count: usize,
    inline: usize,
    pub(super) receiving: Queue,
    pub(super) sending: Queue,
    /// Its `Sender`, which every `copy()` is another reference to: the
    /// channel closes when the last of them goes, as it would when the last
    /// of many senders went, and copying makes nothing.
    pub(super) sender: SenderObject,
}

/// The `Sender` of a channel, inside it.
#[repr(C)]
pub struct SenderObject {
    pub(super) rc: AtomicUsize,
    pub(super) channel: *mut ChannelObject,
}

/// The most room a bounded channel takes for its values when it is made.
const PREALLOCATED: usize = 64 * 1024;

/// The values a channel holds, first to last, as bytes in a ring: in the
/// channel's own allocation when it is bounded and they fit there, as they
/// then always do, or on the heap, where the ring grows as it needs. With
/// the channel locked.
pub(crate) struct Ring {
    data: *mut u8,
    room: usize,
    start: usize,
    len: usize,
    /// Whether `data` is the ring's to free, rather than the channel's.
    owned: bool,
}

unsafe impl Send for Ring {}

impl Ring {
    pub(crate) const fn new() -> Ring {
        Ring { data: std::ptr::null_mut(), room: 0, start: 0, len: 0, owned: false }
    }

    fn with_room(room: usize) -> Ring {
        let mut ring = Ring::new();
        ring.grow(room);
        ring
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Where the byte `offset` bytes after the first is, for an offset
    /// within the room: a subtraction, where `%` would divide.
    fn at(&self, offset: usize) -> usize {
        let at = self.start + offset;
        if at >= self.room { at - self.room } else { at }
    }

    /// The byte `offset` bytes after the first.
    pub(crate) fn byte(&self, offset: usize) -> u8 {
        unsafe { *self.data.add(self.at(offset)) }
    }

    /// Copies the `size` bytes `offset` bytes after the first to `out`.
    pub(crate) fn read(&self, offset: usize, out: *mut u8, size: usize) {
        if size == 0 {
            return;
        }
        let at = self.at(offset);
        let first = size.min(self.room - at);
        unsafe {
            copy(self.data.add(at), out, first);
            if first < size {
                copy(self.data, out.add(first), size - first);
            }
        }
    }

    /// Adds the `size` bytes at `value` after the last.
    pub(crate) fn push(&mut self, value: *const u8, size: usize) {
        if size == 0 {
            return;
        }
        if self.len + size > self.room {
            self.grow(self.len + size);
        }
        let at = self.at(self.len);
        let first = size.min(self.room - at);
        unsafe {
            copy(value, self.data.add(at), first);
            if first < size {
                copy(value.add(first), self.data, size - first);
            }
        }
        self.len += size;
    }

    /// Moves the first `size` bytes to `out`.
    pub(crate) fn pop(&mut self, out: *mut u8, size: usize) {
        if size == 0 {
            return;
        }
        self.read(0, out, size);
        self.len -= size;
        self.start = if self.len == 0 { 0 } else { self.at(size) };
    }

    /// Moves the bytes to a ring of its own with room for `needed`.
    fn grow(&mut self, needed: usize) {
        let room = needed.max(self.room * 2).max(64);
        let layout = std::alloc::Layout::array::<u8>(room).unwrap();
        let data = unsafe { std::alloc::alloc(layout) };
        if data.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        self.read(0, data, self.len);
        self.free();
        (self.data, self.room, self.start, self.owned) = (data, room, 0, true);
    }

    fn free(&mut self) {
        if self.owned {
            unsafe { std::alloc::dealloc(self.data, std::alloc::Layout::array::<u8>(self.room).unwrap()) };
        }
    }

    /// Its bytes in a ring of its own, which may outlive the channel, and
    /// it empty.
    pub(crate) fn take(&mut self) -> Ring {
        let taken = std::mem::replace(self, Ring::new());
        if taken.owned || taken.len == 0 {
            return taken;
        }
        let mut own = Ring::with_room(taken.len);
        taken.read(0, own.data, taken.len);
        own.len = taken.len;
        own
    }
}

/// Copies `size` bytes: a value of 8, as most are, in one load and one
/// store rather than a call.
#[inline(always)]
unsafe fn copy(from: *const u8, to: *mut u8, size: usize) {
    unsafe {
        match size {
            8 => to.cast::<u64>().write_unaligned(from.cast::<u64>().read_unaligned()),
            _ => std::ptr::copy_nonoverlapping(from, to, size),
        }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        self.free();
    }
}

/// A channel's allocation with `inline` bytes of room for its values after
/// it, and where they start.
fn channel_layout(inline: usize) -> (std::alloc::Layout, usize) {
    std::alloc::Layout::new::<ChannelObject>().extend(std::alloc::Layout::array::<u8>(inline).unwrap()).unwrap()
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
    // Room for what it may hold, as it fills: growing by doubling took seven
    // allocations to fill one of 64. In its own allocation, or, past a
    // limit, from that much on as it needs.
    let wanted = (capacity as usize).saturating_mul(size);
    let inline = if bounded != 0 && wanted <= PREALLOCATED { wanted } else { 0 };
    let (layout, offset) = channel_layout(inline);
    let channel = unsafe { std::alloc::alloc(layout) } as *mut ChannelObject;
    if channel.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    let buffer = if inline > 0 {
        Ring { data: unsafe { (channel as *mut u8).add(offset) }, room: inline, ..Ring::new() }
    } else if bounded != 0 && wanted > 0 {
        Ring::with_room(PREALLOCATED)
    } else {
        Ring::new()
    };
    unsafe {
        channel.write(ChannelObject {
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
            buffer,
            count: 0,
            inline,
            receiving: Queue::new(),
            sending: Queue::new(),
            sender: SenderObject { rc: AtomicUsize::new(0), channel },
        })
    };
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
        c.buffer.read(index * c.size, value.as_mut_ptr() as *mut u8, c.size);
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
    // Releasing the values may free the channel, and the room after it.
    let (mut buffer, count, size) = (c.buffer.take(), std::mem::take(&mut c.count), c.size);
    Some(Box::new(move || {
        let mut value = vec![0u64; size.div_ceil(8).max(1)];
        for _ in 0..count {
            buffer.pop(value.as_mut_ptr() as *mut u8, size);
            unsafe { release(value.as_mut_ptr() as *mut u8) };
        }
    }))
}

/// The `Sender` of a channel just made, which `channel` returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_new(channel: *mut ChannelObject) -> *mut SenderObject {
    unsafe {
        let c = &mut *channel;
        lock(&c.lock);
        c.senders = 1;
        unlock(&c.lock);
        c.rc.fetch_add(1, Ordering::Relaxed);
        c.sender.rc.store(1, Ordering::Relaxed);
        &raw mut c.sender
    }
}

/// `sender.copy()`: another reference to the channel's sender.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sender_copy(sender: *mut SenderObject) -> *mut SenderObject {
    unsafe { (*sender).rc.fetch_add(1, Ordering::Relaxed) };
    sender
}

/// The first waiter of `queue` whose wait it can still claim, removed;
/// waiters another operation claimed first are dropped.
pub(super) unsafe fn claim_first(queue: &mut Queue) -> Option<*mut Waiter> {
    while let Some(waiter) = unsafe { queue.pop() } {
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
    while let Some(waiter) = unsafe { c.receiving.pop().or_else(|| c.sending.pop()) } {
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
            tools::handed(current_id(), (*task).id);
            return Some(Some(task));
        }
        if c.count < c.capacity {
            tools::sent(c as *const ChannelObject as usize, c.count, current_id());
            c.buffer.push(value, c.size);
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
            tools::taken(channel, c.count, current_id());
            c.buffer.pop(out, c.size);
            c.count -= 1;
            // A sender waiting for room moves its value in.
            let woken = claim_first(&mut c.sending).map(|sender| {
                tools::sent(channel, c.count, (*(*(*sender).wait).task).id);
                c.buffer.push((*sender).value, c.size);
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
            tools::handed((*task).id, current_id());
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
        tools::channel_used(SENDING, c as *const ChannelObject as usize);
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
        let mut waiter = Waiter::new(Arc::as_ptr(&wait), 0, value);
        c.sending.push(&mut waiter);
        unlock(&c.lock);
        let channel = c as *const ChannelObject as usize;
        if blocked_in(SENDING, channel, || wait.block_on_task()) == CANCELLED {
            lock(&c.lock);
            c.sending.remove(&mut waiter);
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
        let mut waiter = Waiter::new(Arc::as_ptr(&blocked), 0, value);
        c.sending.push(&mut waiter);
        unlock(&c.lock);
        if blocked.block_on_task() == CANCELLED {
            lock(&c.lock);
            c.sending.remove(&mut waiter);
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
        let last = c.buffer.len().wrapping_sub(size);
        let same = c.count > 0 && (0..size).all(|at| c.buffer.byte(last + at) == *value.add(at));
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
        tools::channel_used(RECEIVING, channel as usize);
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
        let mut waiter = Waiter::new(Arc::as_ptr(&wait), 0, out);
        c.receiving.push(&mut waiter);
        unlock(&c.lock);
        if blocked_in(RECEIVING, channel as usize, || wait.block_on_task()) == CANCELLED {
            lock(&c.lock);
            c.receiving.remove(&mut waiter);
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
        let channel = (*sender).channel;
        let c = &mut *channel;
        lock(&c.lock);
        c.senders = 0;
        let woken = if !c.closed { close_waits(c) } else { Vec::new() };
        unlock(&c.lock);
        woken.into_iter().for_each(notify);
        tondo_rt_receiver_release(channel);
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
        let c = &mut *channel;
        if let Some(release) = c.release
            && c.size > 0
        {
            let layout = std::alloc::Layout::from_size_align(c.size, c.align).unwrap();
            let value = std::alloc::alloc(layout);
            while c.count > 0 {
                c.buffer.pop(value, c.size);
                c.count -= 1;
                release(value);
            }
            std::alloc::dealloc(value, layout);
        }
        let layout = channel_layout(c.inline).0;
        std::ptr::drop_in_place(channel);
        std::alloc::dealloc(channel as *mut u8, layout);
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

/// What a `select` keeps for each of its cases, as the objects to lock and
/// the order to try them in: on the stack for the few cases a `select`
/// has, on the heap past `FEW`, so that it allocates nothing to wait.
struct Few<T: Copy> {
    inline: [T; FEW],
    len: usize,
    heap: Vec<T>,
}

const FEW: usize = 8;

impl<T: Copy> Few<T> {
    fn new(filler: T, items: impl Iterator<Item = T>) -> Few<T> {
        let mut few = Few { inline: [filler; FEW], len: 0, heap: Vec::new() };
        for item in items {
            if few.heap.is_empty() && few.len < FEW {
                few.inline[few.len] = item;
            } else {
                if few.heap.is_empty() {
                    few.heap.extend_from_slice(&few.inline[..few.len]);
                }
                few.heap.push(item);
            }
            few.len += 1;
        }
        few
    }
}

impl<T: Copy + PartialEq> Few<T> {
    /// Moves the first of each run of equal items to the front: how many
    /// there are.
    fn partition_dedup_len(&mut self) -> usize {
        let items = &mut **self;
        let mut kept = 0;
        for at in 0..items.len() {
            if kept == 0 || items[at] != items[kept - 1] {
                items[kept] = items[at];
                kept += 1;
            }
        }
        kept
    }
}

impl<T: Copy> std::ops::Deref for Few<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        if self.heap.is_empty() { &self.inline[..self.len] } else { &self.heap }
    }
}

impl<T: Copy> std::ops::DerefMut for Few<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        if self.heap.is_empty() { &mut self.inline[..self.len] } else { &mut self.heap }
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
        let mut order = Few::new(0, 0..cases.len());
        for i in (1..order.len()).rev() {
            order.swap(i, random(i + 1));
        }
        // Most often a value waits in one of the channels it receives from:
        // the first in its order that has one is taken with that channel
        // alone locked, and the senders of the others do not wait for it. A
        // channel locked meanwhile, or a case of another kind, leaves the
        // rest to the way below, as does a tool that watches.
        if !super::attended() && !crate::race::on() && !crate::replay::on() {
            for &index in order.iter() {
                let case = &mut cases[index];
                if case.kind != RECEIVE {
                    break;
                }
                let channel = case.handle as *mut ChannelObject;
                if (*channel).lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                    continue;
                }
                let taken = try_case(case);
                unlock(&(*channel).lock);
                if let Some(woken) = taken {
                    woken.into_iter().for_each(notify);
                    return index as i64;
                }
            }
        }
        // Every object is locked, once and in address order, from checking
        // readiness until the task is registered everywhere.
        let mut locks = Few::new(std::ptr::null(), cases.iter().filter_map(|c| c.lock()));
        locks.sort_unstable();
        let unique = locks.partition_dedup_len();
        let locks = &locks[..unique];
        let lock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| lock(&**l));
        let unlock_all = |locks: &[*const AtomicBool]| locks.iter().for_each(|l| unlock(&**l));
        lock_all(locks);
        for &index in order.iter() {
            let case = &mut cases[index];
            // A panic that a `recover` stops ends the `select` as a
            // cancellation would, and the code unwinds (language 15.9).
            if case.kind == SEND && (*(*(case.handle as *mut SenderObject)).channel).closed {
                unlock_all(locks);
                crate::raise(12, CLOSED_MESSAGE, location);
                return -2;
            }
            if let Some(woken) = try_case(case) {
                unlock_all(locks);
                woken.into_iter().for_each(notify);
                return index as i64;
            }
        }
        if !blocking {
            unlock_all(locks);
            return -1;
        }
        let wait = Wait::new();
        let filler = Waiter::new(Arc::as_ptr(&wait), 0, std::ptr::null_mut());
        let mut waiters = Few::new(
            filler,
            cases.iter().enumerate().map(|(index, case)| Waiter::new(Arc::as_ptr(&wait), index, case.value)),
        );
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.push(waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.push(waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.push((Arc::as_ptr(&wait), index)),
                NEXT => (*(case.handle as *mut GroupObject)).waiter = Some((Arc::as_ptr(&wait), index)),
                _ => add_timer(Duration::from_nanos(case.nanos as u64), wait.clone(), index),
            }
        }
        unlock_all(locks);
        let won = wait.block_on_task();
        // Leave every queue the wait is still in.
        lock_all(locks);
        for (index, case) in cases.iter().enumerate() {
            let waiter: *mut Waiter = &mut waiters[index];
            match case.kind {
                RECEIVE => (*(case.handle as *mut ChannelObject)).receiving.remove(waiter),
                SEND => (*(*(case.handle as *mut SenderObject)).channel).sending.remove(waiter),
                WAIT => (*(case.handle as *mut JoinObject)).waiters.remove(Arc::as_ptr(&wait)),
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
            unlock_all(locks);
            timers_ended(sleeps);
            return -2;
        }
        // The operation that won is complete but for what this task does.
        let case = &mut cases[won];
        match case.kind {
            RECEIVE => case.found = (waiters[won].state == MOVED) as u8,
            SEND if waiters[won].state == CLOSED => {
                unlock_all(locks);
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
        unlock_all(locks);
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

#[cfg(test)]
mod tests {
    use super::{Few, Queue, Ring, Waiter};

    /// A ring in room of another's keeps its values in order as they wrap
    /// around it, also split at its end, and moves them to room of its own
    /// when it grows past it, as it is wrapped, or is taken; values of no
    /// bytes take none.
    #[test]
    fn a_ring_keeps_its_values_in_order() {
        fn push(ring: &mut Ring, next: &mut u32) {
            ring.push(next.to_le_bytes().as_ptr(), 4);
            *next += 1;
        }
        fn pop(ring: &mut Ring, first: &mut u32) {
            let mut out = [0u8; 4];
            ring.pop(out.as_mut_ptr(), 4);
            assert_eq!(u32::from_le_bytes(out), *first);
            *first += 1;
        }
        let (mut next, mut first) = (0u32, 0u32);
        for take in [false, true] {
            let mut room = [0u8; 10];
            let mut ring = Ring { data: room.as_mut_ptr(), room: room.len(), ..Ring::new() };
            push(&mut ring, &mut next);
            for _ in 0..7 {
                push(&mut ring, &mut next);
                pop(&mut ring, &mut first);
            }
            ring.push([].as_ptr(), 0);
            assert!(!ring.owned && ring.start != 0);
            let mut ring = match take {
                false => ring,
                true => {
                    let taken = ring.take();
                    assert!(ring.len() == 0 && !ring.owned);
                    taken
                }
            };
            assert_eq!(ring.owned, take);
            for _ in 0..4 {
                push(&mut ring, &mut next);
            }
            assert!(ring.owned && ring.len() == 20);
            let mut second = [0u8; 4];
            ring.read(4, second.as_mut_ptr(), 4);
            assert_eq!(u32::from_le_bytes(second), first + 1);
            assert_eq!(ring.byte(4) as u32, first + 1);
            while ring.len() > 0 {
                pop(&mut ring, &mut first);
            }
        }
        assert_eq!(first, next);
    }

    /// The waiters of a queue leave it first to last, and one taken out
    /// anywhere, or twice, leaves the others linked as they were.
    #[test]
    fn a_queue_keeps_its_order_as_waiters_leave() {
        let mut waiters: Vec<Waiter> =
            (0..5).map(|index| Waiter::new(std::ptr::null(), index, std::ptr::null_mut())).collect();
        let at: Vec<*mut Waiter> = waiters.iter_mut().map(|w| w as *mut Waiter).collect();
        let mut queue = Queue::new();
        let drained = |queue: &mut Queue| {
            let mut out = Vec::new();
            while let Some(waiter) = unsafe { queue.pop() } {
                out.push(unsafe { (*waiter).index });
            }
            out
        };
        unsafe {
            at.iter().for_each(|&w| queue.push(w));
            // The middle, the first and the last; then one already out.
            queue.remove(at[2]);
            queue.remove(at[0]);
            queue.remove(at[4]);
            queue.remove(at[2]);
            assert_eq!(drained(&mut queue), [1, 3]);
            // Empty, it takes waiters again, also those that left it.
            queue.remove(at[1]);
            [at[4], at[0], at[2]].iter().for_each(|&w| queue.push(w));
            assert_eq!(queue.pop().map(|w| (*w).index), Some(4));
            queue.push(at[3]);
            queue.remove(at[3]);
            assert_eq!(drained(&mut queue), [0, 2]);
            assert!(queue.first.is_null() && queue.last.is_null());
        }
    }

    /// A `select` locks each of its objects once: two cases on one channel
    /// lock it once, and every other channel still is locked.
    #[test]
    fn a_select_locks_each_object_once() {
        for (items, unique) in
            [(vec![3, 1, 3, 2, 1, 3], vec![1, 2, 3]), ((0..12).chain(0..12).collect(), (0..12).collect())]
        {
            let mut few = Few::new(0, items.into_iter());
            few.sort_unstable();
            let kept = few.partition_dedup_len();
            assert_eq!(&few[..kept], unique.as_slice());
        }
    }
}
