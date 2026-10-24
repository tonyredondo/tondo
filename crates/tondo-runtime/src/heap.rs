//! Samples of what the program allocates, always taken, as Go takes them:
//! one allocation for about every 512 KiB, at random intervals so that no
//! size is favoured, kept with the stack that made it for as long as it
//! lives. A dump shows them: where the live memory was allocated.

use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

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
static SAMPLES: [Sample; BUCKETS * WAYS] =
    [const { Sample { size: UnsafeCell::new(0), frames: UnsafeCell::new([0; FRAMES]) } }; BUCKETS * WAYS];
static DROPPED: AtomicU64 = AtomicU64::new(0);

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
    let walk = super::trace::walk(super::trace::frame_pointer(), super::tasks::stack_bounds().unwrap_or((0, 0)));
    for (slot, frame) in frames.iter_mut().zip(walk) {
        *slot = frame;
    }
    let bucket = bucket(address);
    for slot in bucket * WAYS..(bucket + 1) * WAYS {
        if ADDRESSES[slot].compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            unsafe {
                *SAMPLES[slot].size.get() = size;
                *SAMPLES[slot].frames.get() = frames;
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
    for slot in &ADDRESSES[bucket * WAYS..(bucket + 1) * WAYS] {
        if slot.compare_exchange(address, EMPTY, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            OCCUPIED[bucket].fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }
}

/// The samples alive now, each its size and the stack that allocated it,
/// innermost first; and how many were not kept.
pub(crate) fn alive() -> (Vec<(usize, Vec<usize>)>, u64) {
    let mut out = Vec::new();
    for (slot, sample) in SAMPLES.iter().enumerate() {
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
