//! Samples of what the program allocates, always taken, as Go takes them:
//! one allocation for about every 512 KiB, at random intervals so that no
//! size is favoured, kept with the stack that made it for as long as it
//! lives. A dump shows them: where the live memory was allocated.

use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};

/// The mean number of bytes between two samples.
pub(crate) const RATE: u64 = 512 * 1024;

thread_local! {
    /// Bytes this thread allocates before its next sample, 0 before its
    /// first allocation; and the state of its random numbers.
    static UNTIL: Cell<i64> = const { Cell::new(0) };
    static SEED: Cell<u64> = const { Cell::new(0) };
}

/// The samples alive, in buckets of four by their address. An allocation
/// that finds its bucket full is not kept, and is counted.
const BUCKETS: usize = 4096;
const WAYS: usize = 4;
const EMPTY: usize = 0;
const BUSY: usize = 1;
const FRAMES: usize = 32;

struct Sample {
    size: UnsafeCell<usize>,
    frames: UnsafeCell<[usize; FRAMES]>,
}

unsafe impl Sync for Sample {}

/// How many samples each bucket holds, which a free reads first.
static OCCUPIED: [AtomicU8; BUCKETS] = [const { AtomicU8::new(0) }; BUCKETS];
static ADDRESSES: [AtomicUsize; BUCKETS * WAYS] = [const { AtomicUsize::new(EMPTY) }; BUCKETS * WAYS];
/// Each sample's size and stack: 4 MiB, made when the program is first
/// watched, which a program nobody watches does without.
static SAMPLES: AtomicPtr<Sample> = AtomicPtr::new(std::ptr::null_mut());
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Makes the samples' table, before any sample is taken.
pub(crate) fn prepare() {
    if SAMPLES.load(Ordering::Acquire).is_null() {
        SAMPLES.store(super::zeroed(BUCKETS * WAYS), Ordering::Release);
    }
}

fn samples() -> &'static [Sample] {
    let table = SAMPLES.load(Ordering::Acquire);
    match table.is_null() {
        true => &[],
        false => unsafe { std::slice::from_raw_parts(table, BUCKETS * WAYS) },
    }
}

fn bucket(address: usize) -> usize {
    ((address >> 4).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 52) % BUCKETS
}

/// After an allocation of a thread that counts: takes a sample now and
/// then.
#[inline]
pub(crate) fn allocated(address: usize, size: usize) {
    UNTIL.with(|until| {
        let mut left = until.get();
        if left == 0 {
            left = interval();
        }
        left -= size as i64;
        if left > 0 {
            until.set(left);
            return;
        }
        until.set(interval());
        take(address, size);
    });
}

/// Before memory goes back: a sample of it is no longer alive.
#[inline]
pub(crate) fn freed(address: usize) {
    let bucket = bucket(address);
    if OCCUPIED[bucket].load(Ordering::Relaxed) != 0 {
        forget(bucket, address);
    }
}

/// Bytes until the next sample: exponential, with `RATE` as the mean.
fn interval() -> i64 {
    SEED.with(|seed| {
        let mut x = seed.get();
        if x == 0 {
            x = (seed as *const Cell<u64> as u64 ^ super::recorder::now()) | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        seed.set(x);
        let uniform = ((x >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        (-uniform.ln() * RATE as f64).max(1.0) as i64
    })
}

#[cold]
fn take(address: usize, size: usize) {
    let mut frames = [0; FRAMES];
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let walk = super::trace::walk(super::trace::through(super::trace::frame_pointer(), bounds), bounds);
    for (slot, frame) in frames.iter_mut().zip(walk) {
        *slot = frame;
    }
    let bucket = bucket(address);
    let samples = samples();
    for slot in (bucket * WAYS..(bucket + 1) * WAYS).filter(|&slot| slot < samples.len()) {
        if ADDRESSES[slot].compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            unsafe {
                *samples[slot].size.get() = size;
                *samples[slot].frames.get() = frames;
            }
            ADDRESSES[slot].store(address, Ordering::Release);
            OCCUPIED[bucket].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

#[cold]
fn forget(bucket: usize, address: usize) {
    // A read first: a compare-and-swap that fails still takes the line
    // from every other core, and most frees of a bucket's addresses are
    // not its samples.
    for slot in &ADDRESSES[bucket * WAYS..(bucket + 1) * WAYS] {
        if slot.load(Ordering::Relaxed) == address
            && slot.compare_exchange(address, EMPTY, Ordering::AcqRel, Ordering::Relaxed).is_ok()
        {
            OCCUPIED[bucket].fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }
}

/// The samples alive now, each its size and the stack that allocated it,
/// innermost first; and how many were not kept.
pub(crate) fn alive() -> (Vec<(usize, Vec<usize>)>, u64) {
    let mut out = Vec::new();
    for (slot, sample) in samples().iter().enumerate() {
        let before = ADDRESSES[slot].load(Ordering::Acquire);
        if before == EMPTY || before == BUSY {
            continue;
        }
        let (size, frames) = unsafe { (*sample.size.get(), *sample.frames.get()) };
        std::sync::atomic::fence(Ordering::Acquire);
        if ADDRESSES[slot].load(Ordering::Relaxed) == before {
            out.push((size, frames.into_iter().take_while(|&f| f != 0).collect()));
        }
    }
    (out, DROPPED.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Every object, in a debug build
// ---------------------------------------------------------------------------

/// In a debug build that is watched, every live allocation the program's
/// code made, by address: its size and the call of compiled code that
/// made it, whose type the table of traces names. A dump finds who holds
/// each one from them.
static OBJECTS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const SHARDS: usize = 16;
static OBJECTS: [std::sync::Mutex<std::collections::BTreeMap<usize, (usize, usize)>>; SHARDS] =
    [const { std::sync::Mutex::new(std::collections::BTreeMap::new()) }; SHARDS];

thread_local! {
    /// Inside the registry, whose own allocations do not count.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

/// A debug build tracks every object from now on.
pub(crate) fn track_objects() {
    OBJECTS_ON.store(true, Ordering::Relaxed);
}

fn shard(address: usize) -> std::sync::MutexGuard<'static, std::collections::BTreeMap<usize, (usize, usize)>> {
    OBJECTS[(address >> 4) % SHARDS].lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn track(address: usize, size: usize) {
    if !OBJECTS_ON.load(Ordering::Relaxed) || TRACKING.with(|t| t.replace(true)) {
        return;
    }
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let fp = super::trace::through(super::trace::frame_pointer(), bounds);
    let site = super::trace::walk(fp, bounds).find(|&a| super::trace::describe(a).is_some_and(|d| d.at.is_some()));
    if let Some(site) = site {
        shard(address).insert(address, (size, site));
    }
    TRACKING.with(|t| t.set(false));
}

pub(crate) fn untrack(address: usize) {
    if !OBJECTS_ON.load(Ordering::Relaxed) || TRACKING.with(|t| t.replace(true)) {
        return;
    }
    shard(address).remove(&address);
    TRACKING.with(|t| t.set(false));
}

/// What a dump knows of every object: its address, size and the call that
/// made it, with the first bytes of a string; which objects each points
/// to; and which ones the tasks' stacks point to. Pointers are found by
/// reading every word, so a number that equals an address counts too.
pub(crate) struct Objects {
    pub list: Vec<(usize, usize, usize, Option<String>)>,
    pub edges: Vec<(usize, usize)>,
    pub roots: Vec<(u64, usize)>,
}

pub(crate) fn objects() -> Option<Objects> {
    if !OBJECTS_ON.load(Ordering::Relaxed) {
        return None;
    }
    TRACKING.with(|t| t.set(true));
    let mut all: Vec<(usize, (usize, usize))> = Vec::new();
    for shard in &OBJECTS {
        all.extend(shard.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(&a, &v)| (a, v)));
    }
    all.sort_unstable_by_key(|&(address, _)| address);
    let index = |word: usize| all.binary_search_by_key(&word, |&(a, _)| a).ok();
    let mut edges = Vec::new();
    let mut list = Vec::with_capacity(all.len());
    for (from, &(address, (size, site))) in all.iter().enumerate() {
        for offset in (0..size / 8).map(|w| w * 8) {
            let word = unsafe { *((address + offset) as *const usize) };
            if let Some(to) = index(word).filter(|&to| to != from) {
                edges.push((from, to));
            }
        }
        // A string's header is its count and length, then its bytes.
        let is_string = super::trace::describe(site).and_then(|d| d.ty) == Some("String");
        let text = is_string.then(|| {
            let length = unsafe { *((address + 8) as *const usize) }.min(size.saturating_sub(16)).min(200);
            let bytes = unsafe { std::slice::from_raw_parts((address + 16) as *const u8, length) };
            String::from_utf8_lossy(bytes).into_owned()
        });
        list.push((address, size, site, text));
    }
    let mut roots = Vec::new();
    for (task, low, high) in super::tasks::stacks() {
        for slot in (low.next_multiple_of(8)..high.saturating_sub(8)).step_by(8) {
            let word = unsafe { *(slot as *const usize) };
            if let Some(to) = index(word) {
                roots.push((task, to));
            }
        }
    }
    roots.sort_unstable();
    roots.dedup();
    TRACKING.with(|t| t.set(false));
    Some(Objects { list, edges, roots })
}
