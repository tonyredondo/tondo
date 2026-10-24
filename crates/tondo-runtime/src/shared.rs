//! The shared collections of `std.sync` (standard library 14.4.4.2), built
//! for little contention: a hash map whose reads take no lock and whose
//! writes lock one stripe of it, a lock-free queue in segments, a lock-free
//! stack, and an array locked by stripes of slots.
//!
//! Memory a concurrent reader may still see is retired through epochs: an
//! operation pins the current epoch while it reads, and what a writer
//! removes is freed once every thread pinned at that point has moved on.

use super::cycles::{self, Kind, Taken, Visit, Visitor};
use super::{
    ArrayHeader, EqualGlue, Glue, LIVE_OBJECTS, MapGlue, MapObject, element, tondo_rt_array_new,
    tondo_rt_array_release, tondo_rt_map_entry, tondo_rt_map_insert, tondo_rt_map_len, tondo_rt_map_new,
    tondo_rt_map_release,
};
use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering, fence};

// ---------------------------------------------------------------------------
// Epochs
// ---------------------------------------------------------------------------

/// A thread that may pin epochs: its pinned epoch, and what it retired.
struct Participant {
    /// `epoch << 1 | 1` while pinned, 0 otherwise.
    state: AtomicUsize,
    garbage: Mutex<Vec<(usize, Garbage)>>,
}

struct Garbage(Box<dyn FnOnce()>);

// Garbage is freed by whichever thread collects it; it only frees memory.
unsafe impl Send for Garbage {}

static EPOCH: AtomicUsize = AtomicUsize::new(0);
static PARTICIPANTS: Mutex<Vec<&'static Participant>> = Mutex::new(Vec::new());

thread_local! {
    static LOCAL: &'static Participant = {
        let participant: &'static Participant =
            Box::leak(Box::new(Participant { state: AtomicUsize::new(0), garbage: Mutex::new(Vec::new()) }));
        PARTICIPANTS.lock().unwrap_or_else(|e| e.into_inner()).push(participant);
        participant
    };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Keeps the current epoch pinned: nothing retired from now on is freed
/// before it is dropped. An operation never waits while pinned.
struct Pin {
    participant: &'static Participant,
}

fn pin() -> Pin {
    let participant = LOCAL.with(|p| *p);
    let depth = DEPTH.with(|d| d.replace(d.get() + 1));
    if depth == 0 {
        participant.state.store(EPOCH.load(Ordering::Relaxed) << 1 | 1, Ordering::Relaxed);
        fence(Ordering::SeqCst);
    }
    Pin { participant }
}

impl Pin {
    /// Frees `garbage` once no thread can still see it.
    fn retire(&self, garbage: impl FnOnce() + 'static) {
        let epoch = EPOCH.load(Ordering::Relaxed);
        self.participant.garbage.lock().unwrap_or_else(|e| e.into_inner()).push((epoch, Garbage(Box::new(garbage))));
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        let depth = DEPTH.with(|d| {
            d.set(d.get() - 1);
            d.get()
        });
        if depth == 0 {
            self.participant.state.store(0, Ordering::Release);
            collect(self.participant, false);
        }
    }
}

/// Advances the epoch when every pinned thread has seen the current one.
fn try_advance() {
    let global = EPOCH.load(Ordering::Relaxed);
    fence(Ordering::SeqCst);
    let participants = PARTICIPANTS.lock().unwrap_or_else(|e| e.into_inner());
    for participant in participants.iter() {
        let state = participant.state.load(Ordering::Relaxed);
        if state & 1 == 1 && state >> 1 != global {
            return;
        }
    }
    drop(participants);
    let _ = EPOCH.compare_exchange(global, global + 1, Ordering::Release, Ordering::Relaxed);
}

/// Frees what a participant retired two epochs ago or earlier, or all of
/// it with `everything`.
fn collect(participant: &'static Participant, everything: bool) {
    let ready: Vec<(usize, Garbage)> = {
        let mut garbage = participant.garbage.lock().unwrap_or_else(|e| e.into_inner());
        if !everything && garbage.len() < 64 {
            return;
        }
        try_advance();
        let global = EPOCH.load(Ordering::Acquire);
        let (ready, keep) = std::mem::take(&mut *garbage).into_iter().partition(|(e, _)| everything || e + 2 <= global);
        *garbage = keep;
        ready
    };
    for (_, garbage) in ready {
        (garbage.0)();
    }
}

/// Frees everything retired; the program has ended.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub fn flush() {
    let participants: Vec<&'static Participant> =
        PARTICIPANTS.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect();
    for participant in participants {
        collect(participant, true);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Keeps what it holds on its own cache line.
#[repr(align(64))]
struct Padded<T>(T);

impl<T> std::ops::Deref for Padded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Waits a little longer each time, then yields the thread.
fn backoff(spins: &mut u32) {
    if *spins < 7 {
        for _ in 0..1 << *spins {
            std::hint::spin_loop();
        }
        *spins += 1;
    } else {
        std::thread::yield_now();
    }
}

fn acquire(flag: &AtomicBool) {
    let mut spins = 0;
    while flag.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while flag.load(Ordering::Relaxed) {
            backoff(&mut spins);
        }
    }
}

fn release(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

/// Copies a value of `size` bytes and takes a reference for the copy.
unsafe fn copy_retained(from: *const u8, to: *mut u8, size: usize, retain: Option<Glue>) {
    unsafe {
        std::ptr::copy_nonoverlapping(from, to, size);
        if let Some(retain) = retain {
            retain(to);
        }
    }
}

/// An index from the start, or from the end when negative; `None` outside.
fn position(index: i64, length: usize) -> Option<usize> {
    let index = if index < 0 { length as i64 + index } else { index };
    (0..length as i64).contains(&index).then_some(index as usize)
}

/// A scratch buffer of `size` bytes aligned for any value.
fn scratch(size: usize) -> Vec<u64> {
    vec![0; size.div_ceil(8).max(1)]
}

/// The number of stripes of locks: several per core.
fn stripe_count() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores * 4).next_power_of_two().clamp(8, 256)
}

// ---------------------------------------------------------------------------
// Map and set
// ---------------------------------------------------------------------------

/// `sync.Map` and `sync.Set` (a map whose values have no size). Readers walk
/// the buckets without locks; a writer locks the stripe of its bucket.
/// Growing locks every stripe and copies the nodes into a new table.
#[repr(C)]
pub struct SharedMap {
    rc: AtomicUsize,
    table: AtomicPtr<Table>,
    stripes: Box<[Padded<Stripe>]>,
    entry_size: usize,
    value_offset: usize,
    glue: *const MapGlue,
}

/// A lock, and the number of entries in the buckets it guards.
struct Stripe {
    lock: AtomicBool,
    count: AtomicIsize,
}

struct Table {
    buckets: Box<[AtomicPtr<Node>]>,
}

/// An entry: the key, then the value at `value_offset`, after this header.
/// Nodes never change once linked; replacing a value links a new node.
#[repr(C)]
struct Node {
    next: AtomicPtr<Node>,
    hash: u64,
}

const NODE_HEADER: usize = 16;

impl SharedMap {
    fn glue(&self) -> &MapGlue {
        unsafe { &*self.glue }
    }

    fn value_size(&self) -> usize {
        self.entry_size - self.value_offset
    }

    fn node_layout(&self) -> Layout {
        Layout::from_size_align(NODE_HEADER + self.entry_size, 16).unwrap()
    }

    /// A node holding the key and value at those addresses, which move in.
    unsafe fn node(&self, hash: u64, key: *const u8, value: *const u8) -> *mut Node {
        unsafe {
            let node = alloc(self.node_layout()) as *mut Node;
            node.write(Node { next: AtomicPtr::new(std::ptr::null_mut()), hash });
            std::ptr::copy_nonoverlapping(key, key_of(node), self.value_offset);
            std::ptr::copy_nonoverlapping(value, key_of(node).add(self.value_offset), self.value_size());
            node
        }
    }

    unsafe fn value_of(&self, node: *mut Node) -> *mut u8 {
        unsafe { key_of(node).add(self.value_offset) }
    }

    /// Frees a node unlinked from the table; `owned` when it still holds
    /// its key and value.
    fn retire_node(&self, pin: &Pin, node: *mut Node) {
        let (glue, layout, offset) = (self.glue, self.node_layout(), self.value_offset);
        pin.retire(move || unsafe { free_node(node, glue, layout, offset, true) });
    }

    /// The stripe of a bucket.
    fn stripe(&self, bucket: usize) -> &Stripe {
        &self.stripes[bucket % self.stripes.len()]
    }

    /// Locks the stripe of the bucket for `hash`; returns the table, which
    /// cannot grow while the lock is held, and the bucket.
    unsafe fn lock_bucket(&self, hash: u64) -> (&Table, usize) {
        loop {
            let table = self.table.load(Ordering::Acquire);
            let length = unsafe { &*table }.buckets.len();
            let bucket = hash as usize & (length - 1);
            acquire(&self.stripe(bucket).lock);
            if self.table.load(Ordering::Acquire) == table {
                return (unsafe { &*table }, bucket);
            }
            release(&self.stripe(bucket).lock);
        }
    }

    /// The node holding `key` in a bucket, and the link that points to it.
    unsafe fn find(
        &self,
        table: &Table,
        bucket: usize,
        hash: u64,
        key: *const u8,
    ) -> Option<(*const AtomicPtr<Node>, *mut Node)> {
        let mut link: *const AtomicPtr<Node> = &table.buckets[bucket];
        loop {
            let node = unsafe { (*link).load(Ordering::Acquire) };
            if node.is_null() {
                return None;
            }
            unsafe {
                if (*node).hash == hash && (self.glue().key_eq)(key_of(node), key) != 0 {
                    return Some((link, node));
                }
                link = &(*node).next;
            }
        }
    }

    /// Links a new node at the head of its bucket; returns whether the
    /// table should grow.
    unsafe fn link(&self, table: &Table, bucket: usize, node: *mut Node) -> bool {
        unsafe {
            (*node).next.store(table.buckets[bucket].load(Ordering::Relaxed), Ordering::Relaxed);
            table.buckets[bucket].store(node, Ordering::Release);
        }
        let stripe = self.stripe(bucket);
        let count = stripe.count.fetch_add(1, Ordering::Relaxed) + 1;
        let budget = (table.buckets.len() / self.stripes.len()).max(1) * 2;
        count as usize > budget
    }

    /// Doubles the table, unless another writer already replaced `seen`.
    unsafe fn grow(&self, seen: *mut Table) {
        for stripe in self.stripes.iter() {
            acquire(&stripe.lock);
        }
        if self.table.load(Ordering::Acquire) == seen {
            let old = unsafe { &*seen };
            let length = old.buckets.len() * 2;
            let buckets: Box<[AtomicPtr<Node>]> = (0..length).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect();
            for bucket in old.buckets.iter() {
                let mut node = bucket.load(Ordering::Acquire);
                while !node.is_null() {
                    unsafe {
                        // The copy takes over the key and value.
                        let copy = self.node((*node).hash, key_of(node), self.value_of(node));
                        let slot = &buckets[(*node).hash as usize & (length - 1)];
                        (*copy).next.store(slot.load(Ordering::Relaxed), Ordering::Relaxed);
                        slot.store(copy, Ordering::Relaxed);
                        node = (*node).next.load(Ordering::Acquire);
                    }
                }
            }
            self.table.store(Box::into_raw(Box::new(Table { buckets })), Ordering::Release);
            let (glue, layout, offset) = (self.glue, self.node_layout(), self.value_offset);
            let seen = seen as usize;
            pin().retire(move || unsafe { free_table(seen as *mut Table, glue, layout, offset, false) });
        }
        for stripe in self.stripes.iter() {
            release(&stripe.lock);
        }
    }
}

unsafe fn key_of(node: *mut Node) -> *mut u8 {
    unsafe { (node as *mut u8).add(NODE_HEADER) }
}

unsafe fn free_node(node: *mut Node, glue: *const MapGlue, layout: Layout, offset: usize, owned: bool) {
    unsafe {
        if owned {
            if let Some(release) = (*glue).key_release {
                release(key_of(node));
            }
            if let Some(release) = (*glue).value_release {
                release(key_of(node).add(offset));
            }
        }
        dealloc(node as *mut u8, layout);
    }
}

unsafe fn free_table(table: *mut Table, glue: *const MapGlue, layout: Layout, offset: usize, owned: bool) {
    unsafe {
        let table = Box::from_raw(table);
        for bucket in table.buckets.iter() {
            let mut node = bucket.load(Ordering::Relaxed);
            while !node.is_null() {
                let next = (*node).next.load(Ordering::Relaxed);
                free_node(node, glue, layout, offset, owned);
                node = next;
            }
        }
    }
}

/// `sync.Map(entries)` or `sync.Set(keys)`, from a value map or set that
/// it takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_from(
    source: *mut MapObject,
    entry_size: usize,
    value_offset: usize,
    glue: *const MapGlue,
) -> *mut SharedMap {
    let stripes: Box<[Padded<Stripe>]> = (0..stripe_count())
        .map(|_| Padded(Stripe { lock: AtomicBool::new(false), count: AtomicIsize::new(0) }))
        .collect();
    let buckets = (0..stripes.len() * 2).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect();
    LIVE_OBJECTS.made();
    let map = Box::into_raw(Box::new(SharedMap {
        rc: AtomicUsize::new(1),
        table: AtomicPtr::new(Box::into_raw(Box::new(Table { buckets }))),
        stripes,
        entry_size,
        value_offset,
        glue,
    }));
    unsafe {
        let (key, value, old) = (scratch(entry_size), scratch(entry_size), scratch(entry_size));
        let (key, value, old) = (key.as_ptr() as *mut u8, value.as_ptr() as *mut u8, old.as_ptr() as *mut u8);
        let value_size = entry_size - value_offset;
        for index in 0..tondo_rt_map_len(source) {
            let entry = tondo_rt_map_entry(source, index);
            copy_retained(entry, key, value_offset, (*glue).key_retain);
            copy_retained(entry.add(value_offset), value, value_size, (*glue).value_retain);
            tondo_rt_shared_map_insert(map, key, value, old, 0);
        }
        tondo_rt_map_release(source, glue);
        if (*glue).key_visit.is_some() || (*glue).value_visit.is_some() {
            cycles::remember(map as *const u8, Kind::SharedMap);
        }
    }
    map
}

/// Visits the keys and values for the cycle collector.
pub(crate) unsafe fn visit_map(visitor: &mut Visitor, map: *mut u8) {
    unsafe {
        let m = &*(map as *const SharedMap);
        let (key, value) = (m.glue().key_visit, m.glue().value_visit);
        for bucket in (&*m.table.load(Ordering::Acquire)).buckets.iter() {
            let mut node = bucket.load(Ordering::Acquire);
            while !node.is_null() {
                visitor.visit(key, key_of(node));
                visitor.visit(value, m.value_of(node));
                node = (*node).next.load(Ordering::Acquire);
            }
        }
    }
}

/// Takes the entries out of a garbage map.
pub(crate) unsafe fn clear_map(map: *mut u8) -> Option<Taken> {
    let m = unsafe { &*(map as *const SharedMap) };
    let length = unsafe { &*m.table.load(Ordering::Acquire) }.buckets.len();
    let buckets = (0..length).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect();
    let old = m.table.swap(Box::into_raw(Box::new(Table { buckets })), Ordering::AcqRel) as usize;
    for stripe in m.stripes.iter() {
        stripe.count.store(0, Ordering::Relaxed);
    }
    let (glue, layout, offset) = (m.glue, m.node_layout(), m.value_offset);
    Some(Box::new(move || unsafe { free_table(old as *mut Table, glue, layout, offset, true) }))
}

/// `get` (with `out`) or `contains` (without): 1 when the key is present,
/// with a copy of its value in `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_get(map: *mut SharedMap, key: *const u8, out: *mut u8) -> u8 {
    unsafe {
        let m = &*map;
        let hash = (m.glue().hash)(key);
        let _pin = pin();
        let table = &*m.table.load(Ordering::Acquire);
        let bucket = hash as usize & (table.buckets.len() - 1);
        match m.find(table, bucket, hash, key) {
            Some((_, node)) => {
                if !out.is_null() {
                    copy_retained(m.value_of(node), out, m.value_size(), m.glue().value_retain);
                }
                1
            }
            None => 0,
        }
    }
}

/// `insert`: the key and value move in. Returns 1 when the key was present:
/// a map moves the replaced value to `old`; a set (`keep`) keeps its
/// element and releases the new key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_insert(
    map: *mut SharedMap,
    key: *const u8,
    value: *const u8,
    old: *mut u8,
    keep: u8,
) -> u8 {
    unsafe {
        let m = &*map;
        let hash = (m.glue().hash)(key);
        let pin = pin();
        let (table, bucket) = m.lock_bucket(hash);
        let present = match m.find(table, bucket, hash, key) {
            Some(_) if keep != 0 => {
                if let Some(release) = m.glue().key_release {
                    release(key as *mut u8);
                }
                true
            }
            Some((link, found)) => {
                // Readers may still copy the old value: the old node keeps its
                // reference until it is freed, and the caller gets its own.
                copy_retained(m.value_of(found), old, m.value_size(), m.glue().value_retain);
                let fresh = m.node(hash, key, value);
                (*fresh).next.store((*found).next.load(Ordering::Relaxed), Ordering::Relaxed);
                (*link).store(fresh, Ordering::Release);
                m.retire_node(&pin, found);
                true
            }
            None => {
                let grow = m.link(table, bucket, m.node(hash, key, value));
                release(&m.stripe(bucket).lock);
                if grow {
                    m.grow(table as *const Table as *mut Table);
                }
                return 0;
            }
        };
        release(&m.stripe(bucket).lock);
        present as u8
    }
}

/// `remove`: 1 when the key was present, with its value moved to `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_remove(map: *mut SharedMap, key: *const u8, out: *mut u8) -> u8 {
    unsafe {
        let m = &*map;
        let hash = (m.glue().hash)(key);
        let pin = pin();
        let (table, bucket) = m.lock_bucket(hash);
        let found = m.find(table, bucket, hash, key);
        if let Some((link, node)) = found {
            (*link).store((*node).next.load(Ordering::Relaxed), Ordering::Release);
            m.stripe(bucket).count.fetch_sub(1, Ordering::Relaxed);
            if !out.is_null() {
                copy_retained(m.value_of(node), out, m.value_size(), m.glue().value_retain);
            }
            m.retire_node(&pin, node);
        }
        release(&m.stripe(bucket).lock);
        found.is_some() as u8
    }
}

/// `compareExchange`: compares the value of the key, or its absence when
/// `expected` is null, and when it matches stores `desired`, which moves
/// in, or removes the key when it is null. The value seen is copied to
/// `out`. Returns bit 0 when it exchanged, and bit 1 when a value was seen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_exchange(
    map: *mut SharedMap,
    key: *const u8,
    expected: *const u8,
    desired: *const u8,
    out: *mut u8,
    value_eq: EqualGlue,
) -> u8 {
    unsafe {
        let m = &*map;
        let glue = m.glue();
        let hash = (glue.hash)(key);
        let pin = pin();
        let (table, bucket) = m.lock_bucket(hash);
        let found = m.find(table, bucket, hash, key);
        let matches = match (found, expected.is_null()) {
            (None, true) => true,
            (Some((_, node)), false) => value_eq(m.value_of(node), expected) != 0,
            _ => false,
        };
        let mut code = 0;
        if let Some((_, node)) = found {
            copy_retained(m.value_of(node), out, m.value_size(), glue.value_retain);
            code |= 2;
        }
        let mut grow = false;
        if matches {
            code |= 1;
            match (found, desired.is_null()) {
                (Some((link, node)), true) => {
                    (*link).store((*node).next.load(Ordering::Relaxed), Ordering::Release);
                    m.stripe(bucket).count.fetch_sub(1, Ordering::Relaxed);
                    m.retire_node(&pin, node);
                }
                (Some((link, node)), false) => {
                    let key_copy = scratch(m.value_offset);
                    copy_retained(key_of(node), key_copy.as_ptr() as *mut u8, m.value_offset, glue.key_retain);
                    let fresh = m.node(hash, key_copy.as_ptr() as *const u8, desired);
                    (*fresh).next.store((*node).next.load(Ordering::Relaxed), Ordering::Relaxed);
                    (*link).store(fresh, Ordering::Release);
                    m.retire_node(&pin, node);
                }
                (None, false) => {
                    let key_copy = scratch(m.value_offset);
                    copy_retained(key, key_copy.as_ptr() as *mut u8, m.value_offset, glue.key_retain);
                    grow = m.link(table, bucket, m.node(hash, key_copy.as_ptr() as *const u8, desired));
                }
                (None, true) => {}
            }
        } else if !desired.is_null()
            && let Some(release) = glue.value_release
        {
            release(desired as *mut u8);
        }
        release(&m.stripe(bucket).lock);
        if grow {
            m.grow(table as *const Table as *mut Table);
        }
        code
    }
}

/// The sum of the stripes' counts, taken without locks: exact unless
/// writers run at the same time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_length(map: *mut SharedMap) -> i64 {
    let total: isize = unsafe { (*map).stripes.iter().map(|s| s.count.load(Ordering::Relaxed)).sum() };
    total.max(0) as i64
}

/// `snapshot()`: a value map or set with the entries of one instant, taken
/// with every stripe locked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_snapshot(map: *mut SharedMap) -> *mut MapObject {
    unsafe {
        let m = &*map;
        let glue = m.glue();
        for stripe in m.stripes.iter() {
            acquire(&stripe.lock);
        }
        let mut result = tondo_rt_map_new(m.entry_size, m.value_offset);
        let (key, value, old) = (scratch(m.entry_size), scratch(m.entry_size), scratch(m.entry_size));
        let (key, value, old) = (key.as_ptr() as *mut u8, value.as_ptr() as *mut u8, old.as_ptr() as *mut u8);
        let table = &*m.table.load(Ordering::Acquire);
        for bucket in table.buckets.iter() {
            let mut node = bucket.load(Ordering::Acquire);
            while !node.is_null() {
                copy_retained(key_of(node), key, m.value_offset, glue.key_retain);
                copy_retained(m.value_of(node), value, m.value_size(), glue.value_retain);
                tondo_rt_map_insert(&mut result, key, value, old, m.glue);
                node = (*node).next.load(Ordering::Acquire);
            }
        }
        for stripe in m.stripes.iter() {
            release(&stripe.lock);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_map_release(map: *mut SharedMap) {
    if map.is_null() {
        return;
    }
    unsafe {
        if (*map).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        if (*(*map).glue).key_visit.is_some() || (*(*map).glue).value_visit.is_some() {
            cycles::forget(map as *const u8);
        }
        let m = Box::from_raw(map);
        free_table(m.table.load(Ordering::Relaxed), m.glue, m.node_layout(), m.value_offset, true);
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// `sync.Queue`: a lock-free FIFO of segments. A producer reserves a
/// position with one atomic increment and fills it; a consumer takes the
/// position at the head once it is filled. A slot keeps a reference to its
/// value until its segment is freed, so observers may copy values that
/// consumers took meanwhile.
#[repr(C)]
pub struct SharedQueue {
    rc: AtomicUsize,
    head: Padded<AtomicPtr<Segment>>,
    tail: Padded<AtomicPtr<Segment>>,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    visit: Option<Visit>,
}

struct Segment {
    head: Padded<AtomicUsize>,
    /// The next position to reserve; it may run past the capacity.
    tail: Padded<AtomicUsize>,
    next: AtomicPtr<Segment>,
    capacity: usize,
    /// The position of the first slot in the whole queue.
    base: usize,
    /// Each slot: a filled flag, then the value at offset 8.
    slots: *mut u8,
    slot_size: usize,
}

const FIRST_SEGMENT: usize = 32;
const LARGEST_SEGMENT: usize = 1024;

impl Segment {
    fn new(capacity: usize, base: usize, size: usize) -> *mut Segment {
        let slot_size = 8 + size.div_ceil(8) * 8;
        let slots = unsafe { alloc(Layout::from_size_align(slot_size * capacity, 8).unwrap()) };
        for index in 0..capacity {
            unsafe { (slots.add(index * slot_size) as *mut AtomicU8).write(AtomicU8::new(0)) };
        }
        Box::into_raw(Box::new(Segment {
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
            next: AtomicPtr::new(std::ptr::null_mut()),
            capacity,
            base,
            slots,
            slot_size,
        }))
    }

    fn filled(&self, index: usize) -> &AtomicU8 {
        unsafe { &*(self.slots.add(index * self.slot_size) as *const AtomicU8) }
    }

    fn value(&self, index: usize) -> *mut u8 {
        unsafe { self.slots.add(index * self.slot_size + 8) }
    }

    /// Waits for the producer that reserved a slot to fill it.
    fn wait_filled(&self, index: usize) {
        let mut spins = 0;
        while self.filled(index).load(Ordering::Acquire) == 0 {
            backoff(&mut spins);
        }
    }

    /// The positions reserved in it: those below its tail and capacity.
    fn reserved(&self) -> usize {
        self.tail.load(Ordering::Acquire).min(self.capacity)
    }
}

/// Frees a segment and the values its filled slots hold.
unsafe fn free_segment(segment: *mut Segment, release: Option<Glue>) {
    unsafe {
        let s = Box::from_raw(segment);
        if let Some(release) = release {
            for index in 0..s.capacity {
                if s.filled(index).load(Ordering::Relaxed) != 0 {
                    release(s.value(index));
                }
            }
        }
        dealloc(s.slots, Layout::from_size_align(s.slot_size * s.capacity, 8).unwrap());
    }
}

impl SharedQueue {
    fn push(&self, value: *const u8) {
        let _pin = pin();
        loop {
            let segment = unsafe { &*self.tail.load(Ordering::Acquire) };
            let position = segment.tail.fetch_add(1, Ordering::AcqRel);
            if position < segment.capacity {
                unsafe { std::ptr::copy_nonoverlapping(value, segment.value(position), self.size) };
                segment.filled(position).store(1, Ordering::Release);
                return;
            }
            // Full: move on to the next segment, adding it if needed.
            let mut next = segment.next.load(Ordering::Acquire);
            if next.is_null() {
                let capacity = (segment.capacity * 2).min(LARGEST_SEGMENT);
                let fresh = Segment::new(capacity, segment.base + segment.capacity, self.size);
                next = match segment.next.compare_exchange(
                    std::ptr::null_mut(),
                    fresh,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => fresh,
                    Err(existing) => {
                        unsafe { free_segment(fresh, None) };
                        existing
                    }
                };
            }
            let _ = self.tail.compare_exchange(
                segment as *const Segment as *mut Segment,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }

    /// The head and tail positions of one instant, with their segments.
    fn bounds(&self) -> (*mut Segment, usize, usize) {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            let (first, last) = unsafe { (&*head, &*tail) };
            let start = first.base + first.head.load(Ordering::Acquire).min(first.capacity);
            let end = last.base + last.reserved();
            if self.head.load(Ordering::Acquire) == head && self.tail.load(Ordering::Acquire) == tail {
                return (head, start, end.max(start));
            }
        }
    }

    /// The segment and slot of a position at or after `segment`'s.
    fn locate(mut segment: *mut Segment, position: usize) -> (&'static Segment, usize) {
        loop {
            let s = unsafe { &*segment };
            if position < s.base + s.capacity {
                return (s, position - s.base);
            }
            segment = s.next.load(Ordering::Acquire);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_from(
    source: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    visit: Option<Visit>,
) -> *mut SharedQueue {
    let segment = Segment::new(FIRST_SEGMENT, 0, size);
    LIVE_OBJECTS.made();
    let queue = Box::into_raw(Box::new(SharedQueue {
        rc: AtomicUsize::new(1),
        head: Padded(AtomicPtr::new(segment)),
        tail: Padded(AtomicPtr::new(segment)),
        size,
        retain,
        release,
        visit,
    }));
    if visit.is_some() {
        cycles::remember(queue as *const u8, Kind::SharedQueue);
    }
    unsafe {
        let value = scratch(size);
        for index in 0..(*source).len {
            copy_retained(element(source, size, index), value.as_ptr() as *mut u8, size, retain);
            (*queue).push(value.as_ptr() as *const u8);
        }
        tondo_rt_array_release(source, size, release);
    }
    queue
}

/// `push`: the value moves in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_push(queue: *mut SharedQueue, value: *const u8) {
    unsafe { (*queue).push(value) }
}

/// `popFront`: 1 with the value at the head copied to `out`, or 0 when the
/// queue is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_pop(queue: *mut SharedQueue, out: *mut u8) -> u8 {
    unsafe {
        let q = &*queue;
        let pin = pin();
        loop {
            let pointer = q.head.load(Ordering::Acquire);
            let segment = &*pointer;
            let position = segment.head.load(Ordering::Acquire);
            if position >= segment.capacity {
                let next = segment.next.load(Ordering::Acquire);
                if next.is_null() {
                    return 0;
                }
                if q.head.compare_exchange(pointer, next, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    let release = q.release;
                    let pointer = pointer as usize;
                    pin.retire(move || free_segment(pointer as *mut Segment, release));
                }
                continue;
            }
            if position >= segment.tail.load(Ordering::Acquire) {
                return 0;
            }
            segment.wait_filled(position);
            if segment.head.compare_exchange(position, position + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                copy_retained(segment.value(position), out, q.size, q.retain);
                return 1;
            }
        }
    }
}

/// `get(index)`: a copy of the value at that position from the head, or
/// from the tail when negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_get(queue: *mut SharedQueue, index: i64, out: *mut u8) -> u8 {
    unsafe {
        let q = &*queue;
        let _pin = pin();
        let (head, start, end) = q.bounds();
        let Some(offset) = position(index, end - start) else { return 0 };
        let (segment, slot) = SharedQueue::locate(head, start + offset);
        segment.wait_filled(slot);
        copy_retained(segment.value(slot), out, q.size, q.retain);
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_length(queue: *mut SharedQueue) -> i64 {
    let _pin = pin();
    let (_, start, end) = unsafe { (*queue).bounds() };
    (end - start) as i64
}

/// `snapshot()`: the values from the head to the tail of one instant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_snapshot(queue: *mut SharedQueue) -> *mut ArrayHeader {
    unsafe {
        let q = &*queue;
        let _pin = pin();
        let (head, start, end) = q.bounds();
        let array = tondo_rt_array_new(q.size, end - start);
        let mut segment = head;
        for position in start..end {
            let (s, slot) = SharedQueue::locate(segment, position);
            segment = s as *const Segment as *mut Segment;
            s.wait_filled(slot);
            copy_retained(s.value(slot), element(array, q.size, position - start), q.size, q.retain);
        }
        array
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_queue_release(queue: *mut SharedQueue) {
    if queue.is_null() {
        return;
    }
    unsafe {
        if (*queue).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        if (*queue).visit.is_some() {
            cycles::forget(queue as *const u8);
        }
        let q = Box::from_raw(queue);
        let mut segment = q.head.load(Ordering::Relaxed);
        while !segment.is_null() {
            let next = (*segment).next.load(Ordering::Relaxed);
            free_segment(segment, q.release);
            segment = next;
        }
        LIVE_OBJECTS.gone();
    }
}

/// Visits the values the segments hold for the cycle collector.
pub(crate) unsafe fn visit_queue(visitor: &mut Visitor, queue: *mut u8) {
    unsafe {
        let q = &*(queue as *const SharedQueue);
        let mut segment = q.head.load(Ordering::Acquire);
        while !segment.is_null() {
            let s = &*segment;
            for index in 0..s.capacity {
                if s.filled(index).load(Ordering::Acquire) != 0 {
                    visitor.visit(q.visit, s.value(index));
                }
            }
            segment = s.next.load(Ordering::Acquire);
        }
    }
}

/// Takes the segments out of a garbage queue.
pub(crate) unsafe fn clear_queue(queue: *mut u8) -> Option<Taken> {
    let q = unsafe { &*(queue as *const SharedQueue) };
    let head = q.head.swap(std::ptr::null_mut(), Ordering::AcqRel) as usize;
    q.tail.store(std::ptr::null_mut(), Ordering::Release);
    let release = q.release;
    Some(Box::new(move || unsafe {
        let mut segment = head as *mut Segment;
        while !segment.is_null() {
            let next = (*segment).next.load(Ordering::Relaxed);
            free_segment(segment, release);
            segment = next;
        }
    }))
}

// ---------------------------------------------------------------------------
// Stack
// ---------------------------------------------------------------------------

/// `sync.Stack`: a lock-free (Treiber) stack. Each node records the depth
/// of the stack it tops, which gives the length, and keeps its value until
/// it is freed, so observers may copy values that `pop` took meanwhile.
#[repr(C)]
pub struct SharedStack {
    rc: AtomicUsize,
    top: Padded<AtomicPtr<StackNode>>,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    visit: Option<Visit>,
}

#[repr(C)]
struct StackNode {
    next: *mut StackNode,
    depth: usize,
}

const STACK_HEADER: usize = 16;

impl SharedStack {
    fn layout(&self) -> Layout {
        Layout::from_size_align(STACK_HEADER + self.size, 16).unwrap()
    }

    fn push(&self, value: *const u8) {
        let _pin = pin();
        unsafe {
            let node = alloc(self.layout()) as *mut StackNode;
            std::ptr::copy_nonoverlapping(value, stack_value(node), self.size);
            let mut spins = 0;
            loop {
                let top = self.top.load(Ordering::Acquire);
                node.write(StackNode { next: top, depth: if top.is_null() { 1 } else { (*top).depth + 1 } });
                if self.top.compare_exchange_weak(top, node, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    return;
                }
                backoff(&mut spins);
            }
        }
    }

    /// The nodes from the top, read at one instant.
    fn nodes(&self) -> Vec<*mut StackNode> {
        let mut nodes = Vec::new();
        let mut node = self.top.load(Ordering::Acquire);
        while !node.is_null() {
            nodes.push(node);
            node = unsafe { (*node).next };
        }
        nodes
    }
}

unsafe fn stack_value(node: *mut StackNode) -> *mut u8 {
    unsafe { (node as *mut u8).add(STACK_HEADER) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_from(
    source: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    visit: Option<Visit>,
) -> *mut SharedStack {
    LIVE_OBJECTS.made();
    let stack = Box::into_raw(Box::new(SharedStack {
        rc: AtomicUsize::new(1),
        top: Padded(AtomicPtr::new(std::ptr::null_mut())),
        size,
        retain,
        release,
        visit,
    }));
    if visit.is_some() {
        cycles::remember(stack as *const u8, Kind::SharedStack);
    }
    unsafe {
        let value = scratch(size);
        for index in 0..(*source).len {
            copy_retained(element(source, size, index), value.as_ptr() as *mut u8, size, retain);
            (*stack).push(value.as_ptr() as *const u8);
        }
        tondo_rt_array_release(source, size, release);
    }
    stack
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_push(stack: *mut SharedStack, value: *const u8) {
    unsafe { (*stack).push(value) }
}

/// `pop`: 1 with the top value copied to `out`, or 0 when empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_pop(stack: *mut SharedStack, out: *mut u8) -> u8 {
    unsafe {
        let s = &*stack;
        let pin = pin();
        let mut spins = 0;
        loop {
            let top = s.top.load(Ordering::Acquire);
            if top.is_null() {
                return 0;
            }
            if s.top.compare_exchange_weak(top, (*top).next, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                copy_retained(stack_value(top), out, s.size, s.retain);
                let (release, layout) = (s.release, s.layout());
                let top = top as usize;
                pin.retire(move || {
                    let node = top as *mut StackNode;
                    if let Some(release) = release {
                        release(stack_value(node));
                    }
                    dealloc(node as *mut u8, layout);
                });
                return 1;
            }
            backoff(&mut spins);
        }
    }
}

/// `get(index)`: from the base, or from the top when negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_get(stack: *mut SharedStack, index: i64, out: *mut u8) -> u8 {
    unsafe {
        let s = &*stack;
        let _pin = pin();
        let top = s.top.load(Ordering::Acquire);
        let depth = if top.is_null() { 0 } else { (*top).depth };
        let Some(index) = position(index, depth) else { return 0 };
        let mut node = top;
        for _ in 0..depth - 1 - index {
            node = (*node).next;
        }
        copy_retained(stack_value(node), out, s.size, s.retain);
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_length(stack: *mut SharedStack) -> i64 {
    let _pin = pin();
    let top = unsafe { (*stack).top.load(Ordering::Acquire) };
    if top.is_null() { 0 } else { unsafe { (*top).depth as i64 } }
}

/// `snapshot()`: the values from the base to the top of one instant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_snapshot(stack: *mut SharedStack) -> *mut ArrayHeader {
    unsafe {
        let s = &*stack;
        let _pin = pin();
        let nodes = s.nodes();
        let array = tondo_rt_array_new(s.size, nodes.len());
        for (index, &node) in nodes.iter().rev().enumerate() {
            copy_retained(stack_value(node), element(array, s.size, index), s.size, s.retain);
        }
        array
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_stack_release(stack: *mut SharedStack) {
    if stack.is_null() {
        return;
    }
    unsafe {
        if (*stack).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        if (*stack).visit.is_some() {
            cycles::forget(stack as *const u8);
        }
        let s = Box::from_raw(stack);
        for node in s.nodes() {
            if let Some(release) = s.release {
                release(stack_value(node));
            }
            dealloc(node as *mut u8, s.layout());
        }
        LIVE_OBJECTS.gone();
    }
}

/// Visits the values for the cycle collector.
pub(crate) unsafe fn visit_stack(visitor: &mut Visitor, stack: *mut u8) {
    let s = unsafe { &*(stack as *const SharedStack) };
    for node in s.nodes() {
        unsafe { visitor.visit(s.visit, stack_value(node)) };
    }
}

/// Takes the nodes out of a garbage stack.
pub(crate) unsafe fn clear_stack(stack: *mut u8) -> Option<Taken> {
    let s = unsafe { &*(stack as *const SharedStack) };
    let nodes: Vec<usize> = s.nodes().into_iter().map(|node| node as usize).collect();
    s.top.store(std::ptr::null_mut(), Ordering::Release);
    let (release, layout) = (s.release, s.layout());
    Some(Box::new(move || {
        for node in nodes {
            unsafe {
                if let Some(release) = release {
                    release(stack_value(node as *mut StackNode));
                }
                dealloc(node as *mut u8, layout);
            }
        }
    }))
}

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------

/// `sync.Array`: fixed slots, each guarded by the lock of its stripe, so
/// operations on different slots seldom meet.
#[repr(C)]
pub struct SharedArray {
    rc: AtomicUsize,
    length: usize,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    stripes: Box<[Padded<AtomicBool>]>,
    values: *mut u8,
    visit: Option<Visit>,
}

impl SharedArray {
    fn slot(&self, index: usize) -> *mut u8 {
        unsafe { self.values.add(index * self.size) }
    }

    fn lock(&self, index: usize) -> &AtomicBool {
        &self.stripes[index % self.stripes.len()]
    }

    fn layout(&self) -> Layout {
        Layout::from_size_align((self.size * self.length).max(1), 8).unwrap()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_from(
    source: *mut ArrayHeader,
    size: usize,
    retain: Option<Glue>,
    release: Option<Glue>,
    visit: Option<Visit>,
) -> *mut SharedArray {
    unsafe {
        let length = (*source).len;
        let stripes = (0..stripe_count()).map(|_| Padded(AtomicBool::new(false))).collect();
        let values = alloc(Layout::from_size_align((size * length).max(1), 8).unwrap());
        for index in 0..length {
            copy_retained(element(source, size, index), values.add(index * size), size, retain);
        }
        tondo_rt_array_release(source, size, release);
        LIVE_OBJECTS.made();
        let array = Box::into_raw(Box::new(SharedArray {
            rc: AtomicUsize::new(1),
            length,
            size,
            retain,
            release,
            stripes,
            values,
            visit,
        }));
        if visit.is_some() {
            cycles::remember(array as *const u8, Kind::SharedArray);
        }
        array
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_get(array: *mut SharedArray, index: i64, out: *mut u8) -> u8 {
    unsafe {
        let a = &*array;
        let Some(index) = position(index, a.length) else { return 0 };
        acquire(a.lock(index));
        copy_retained(a.slot(index), out, a.size, a.retain);
        release(a.lock(index));
        1
    }
}

/// `set`: the value moves in and the replaced one moves to `old`; 0, with
/// the value released, outside the array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_set(
    array: *mut SharedArray,
    index: i64,
    value: *const u8,
    old: *mut u8,
) -> u8 {
    unsafe {
        let a = &*array;
        let Some(index) = position(index, a.length) else {
            if let Some(release) = a.release {
                release(value as *mut u8);
            }
            return 0;
        };
        acquire(a.lock(index));
        std::ptr::copy_nonoverlapping(a.slot(index), old, a.size);
        std::ptr::copy_nonoverlapping(value, a.slot(index), a.size);
        release(a.lock(index));
        1
    }
}

/// `compareExchange`: 0 outside the array; 1 when the slot held `expected`
/// and now holds `desired`, which moves in; 2 otherwise. The value seen is
/// copied to `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_exchange(
    array: *mut SharedArray,
    index: i64,
    expected: *const u8,
    desired: *const u8,
    out: *mut u8,
    equal: EqualGlue,
) -> u8 {
    unsafe {
        let a = &*array;
        let drop_desired = || {
            if let Some(release) = a.release {
                release(desired as *mut u8);
            }
        };
        let Some(index) = position(index, a.length) else {
            drop_desired();
            return 0;
        };
        acquire(a.lock(index));
        let code = if equal(a.slot(index), expected) != 0 {
            std::ptr::copy_nonoverlapping(a.slot(index), out, a.size);
            std::ptr::copy_nonoverlapping(desired, a.slot(index), a.size);
            1
        } else {
            copy_retained(a.slot(index), out, a.size, a.retain);
            drop_desired();
            2
        };
        release(a.lock(index));
        code
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_length(array: *mut SharedArray) -> i64 {
    unsafe { (*array).length as i64 }
}

/// `snapshot()`: the values of one instant, taken with every stripe locked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_snapshot(array: *mut SharedArray) -> *mut ArrayHeader {
    unsafe {
        let a = &*array;
        for lock in a.stripes.iter() {
            acquire(lock);
        }
        let result = tondo_rt_array_new(a.size, a.length);
        for index in 0..a.length {
            copy_retained(a.slot(index), element(result, a.size, index), a.size, a.retain);
        }
        for lock in a.stripes.iter() {
            release(lock);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_shared_array_release(array: *mut SharedArray) {
    if array.is_null() {
        return;
    }
    unsafe {
        if (*array).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        if (*array).visit.is_some() {
            cycles::forget(array as *const u8);
        }
        let a = Box::from_raw(array);
        if let Some(release) = a.release {
            for index in 0..a.length {
                release(a.slot(index));
            }
        }
        dealloc(a.values, a.layout());
        LIVE_OBJECTS.gone();
    }
}

/// Visits the values for the cycle collector.
pub(crate) unsafe fn visit_array(visitor: &mut Visitor, array: *mut u8) {
    let a = unsafe { &*(array as *const SharedArray) };
    for index in 0..a.length {
        unsafe { visitor.visit(a.visit, a.slot(index)) };
    }
}

/// Takes the values out of a garbage array.
pub(crate) unsafe fn clear_array(array: *mut u8) -> Option<Taken> {
    let a = unsafe { &mut *(array as *mut SharedArray) };
    let release = a.release.take()?;
    let (size, length) = (a.size, a.length);
    let mut values = vec![0u64; (size * length).div_ceil(8).max(1)];
    unsafe { std::ptr::copy_nonoverlapping(a.values, values.as_mut_ptr() as *mut u8, size * length) };
    Some(Box::new(move || {
        for index in 0..length {
            unsafe { release((values.as_mut_ptr() as *mut u8).add(index * size)) };
        }
    }))
}
