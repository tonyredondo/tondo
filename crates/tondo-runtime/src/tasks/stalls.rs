//! `TONDO_RT_STALLS`, a seed, with `TONDO_CHECK_SCHEDULER` (toolchain 7.17):
//! one thread at a time stops where the seed says, as a busy machine
//! deschedules one thread while the others run, where the cases' runner
//! stops every thread at once (`TONDO_CASES_STALLS`). Six places, none on
//! the path a task's wake takes: the monitor before it looks, the monitor
//! between reading a held worker's count and marking it, a worker that
//! took a task from the queue every worker takes from, or from another
//! worker, before it runs it, a worker another stood in for, back from its
//! call of C before it gives that one back, a job of the host that ended,
//! before it gives its result to the task that waits for it, and a task
//! that ended a process's stages, before it looks whether they were
//! reaped. Each thread stops three times at most.

use super::*;
use std::cell::Cell;

/// Where a thread may stop.
#[derive(Clone, Copy)]
pub(crate) enum Point {
    /// The monitor, before a look.
    Look,
    /// The monitor, between reading a held worker's count and marking it:
    /// the task may end meanwhile, and the mark land on a worker gone idle.
    Mark,
    /// A worker that took a task from the shared queue or another's, before
    /// it runs it.
    Take,
    /// A worker back from a call of C in which another stood in for it,
    /// before it gives that one back: as many as return together stop there.
    Leave,
    /// A job of the host that ended, before it claims the wait of the task
    /// that waits for it: a deadline or a cancel may end the wait meanwhile,
    /// and the job then drops its result (`io::host_cancellable`).
    Give,
    /// A task that ended the stages of a process at a cancel, a deadline or
    /// an alarm, before it looks whether the job that waits for them reaped
    /// them: they may end meanwhile, and it then goes on without a wait
    /// (`process::Reaped`).
    Kill,
}

impl Point {
    /// The passes over which a thread makes its stops at this point, about
    /// three: the monitor looks once a millisecond, marks a worker seldom,
    /// a worker takes from the shared queue now and then, one comes back
    /// from a call another stood in for seldom, a job of the host ends now
    /// and then, and a task ends a process's stages seldom.
    fn passes(self) -> u64 {
        match self {
            Point::Look => 1000,
            Point::Mark => 4,
            Point::Take => 32,
            Point::Leave => 4,
            Point::Give => 8,
            Point::Kill => 8,
        }
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static SEED: AtomicU64 = AtomicU64::new(0);

/// How long a stop lasts, from 5 to 50 ms. A stopped monitor delays what it
/// does by that much, and the checker's rules that wait for it leave five
/// hundred looks (`checker`): a stop must stay far under that, or the
/// checker would judge the stop.
const SHORTEST_MS: u64 = 5;
const LONGEST_MS: u64 = 50;
/// The stops a thread makes at most.
const STOPS: u32 = 3;

/// Turns the stops on when the checker starts, with a seed given.
pub(super) fn start() {
    let given = crate::config::config().stalls.as_deref().and_then(|seed| seed.to_str()?.trim().parse::<u64>().ok());
    if let Some(seed) = given {
        SEED.store(seed, Ordering::Relaxed);
        ON.store(true, Ordering::Relaxed);
    }
}

/// Stops this thread here, if the seed says so.
#[inline]
pub(crate) fn here(point: Point) {
    if ON.load(Ordering::Relaxed) {
        stop(point);
    }
}

thread_local! {
    /// This thread's passes at each point, and the stops it has made.
    static PASSES: Cell<[u64; 6]> = const { Cell::new([0; 6]) };
    static STOPPED: Cell<u32> = const { Cell::new(0) };
}

#[inline(never)]
fn stop(point: Point) {
    let at = point as usize;
    let pass = PASSES.with(|passes| {
        let mut all = passes.get();
        all[at] += 1;
        passes.set(all);
        all[at]
    });
    if pass > point.passes() || STOPPED.with(Cell::get) >= STOPS {
        return;
    }
    // A worker by its slot, the monitor apart: the same in every run.
    let thread = own_next().map_or(u64::MAX, |next| {
        (next as *const Next as usize - NEXTS.as_ptr() as usize) as u64 / std::mem::size_of::<Next>() as u64
    });
    let drawn = mix(SEED.load(Ordering::Relaxed) ^ (at as u64) << 56 ^ thread << 32 ^ pass);
    if drawn % point.passes() >= 3 {
        return;
    }
    STOPPED.with(|stopped| stopped.set(stopped.get() + 1));
    let lasting = SHORTEST_MS + mix(drawn) % (LONGEST_MS - SHORTEST_MS + 1);
    std::thread::sleep(Duration::from_millis(lasting));
}

/// SplitMix64's mix of one number.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::{LONGEST_MS, Point, SHORTEST_MS, mix};

    /// A seed gives each thread about three stops at each point, and each
    /// lasts from 5 to 50 ms.
    #[test]
    fn a_seed_gives_a_few_stops_of_a_bounded_length() {
        for point in [Point::Look, Point::Mark, Point::Take, Point::Give, Point::Kill] {
            let at = point as usize;
            let mut total = 0;
            for seed in 0..100u64 {
                let stops = (1..=point.passes())
                    .filter(|&pass| mix(seed ^ (at as u64) << 56 ^ 3 << 32 ^ pass) % point.passes() < 3)
                    .count();
                total += stops;
                let lasting = SHORTEST_MS + mix(seed) % (LONGEST_MS - SHORTEST_MS + 1);
                assert!((SHORTEST_MS..=LONGEST_MS).contains(&lasting));
            }
            // About three in a hundred seeds' worth of passes each.
            assert!((150..=450).contains(&total), "{total} stops over 100 seeds at a point");
        }
    }
}
