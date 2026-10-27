//! Work spread over every core: the compiler and the linker use it where
//! items are independent, and keep the results in the items' order, so
//! that what they make does not depend on how many cores there are.
//!
//! The threads are made once and wait between calls, since a call often
//! has less work than making threads would cost.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

/// `f` of each item, which it may change, on every core as `map` runs.
pub fn each_mut<T: Send>(items: &mut [T], f: impl Fn(&mut T) + Sync) {
    let cells: Vec<Mutex<&mut T>> = items.iter_mut().map(Mutex::new).collect();
    map(&cells, 1, |cell| f(&mut cell.lock().unwrap_or_else(|e| e.into_inner())));
}

/// `f` of each item, in the items' order, on every core when there are at
/// least `grain` items for each: each thread takes the next item as it
/// finishes one. A call while another runs, as from inside `f`, runs on the
/// calling thread alone.
pub fn map<T: Sync, R: Send>(items: &[T], grain: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let pool = pool();
    let helpers = pool.threads.min(items.len().div_ceil(grain.max(1)).saturating_sub(1));
    if helpers == 0 || pool.busy.swap(true, Ordering::Acquire) {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    let work = || {
        let mut mine = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(index) else { break };
            mine.push((index, f(item)));
        }
        done.lock().unwrap_or_else(|e| e.into_inner()).extend(mine);
    };
    let panicked = pool.run(&work, helpers);
    pool.busy.store(false, Ordering::Release);
    if let Some(payload) = panicked {
        std::panic::resume_unwind(payload);
    }
    let mut done = done.into_inner().unwrap_or_else(|e| e.into_inner());
    done.sort_unstable_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

type Panic = Box<dyn std::any::Any + Send>;

struct Pool {
    /// The threads besides the caller's.
    threads: usize,
    busy: AtomicBool,
    state: Mutex<State>,
    wake: Condvar,
    finished: Condvar,
}

struct State {
    /// The work of the current call, which the caller keeps alive until
    /// every helper is done with it, and how many helpers take part.
    job: Option<&'static (dyn Fn() + Sync)>,
    wanted: usize,
    round: u64,
    running: usize,
    panicked: Option<Panic>,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()) - 1;
        let state = State { job: None, wanted: 0, round: 0, running: 0, panicked: None };
        Pool {
            threads,
            busy: AtomicBool::new(false),
            state: Mutex::new(state),
            wake: Condvar::new(),
            finished: Condvar::new(),
        }
    })
}

impl Pool {
    /// Runs `work` on this thread and on `helpers` others, and waits for all
    /// of them; gives what a panic of any of them carried.
    fn run(&'static self, work: &(dyn Fn() + Sync), helpers: usize) -> Option<Panic> {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            for helper in 0..self.threads {
                std::thread::Builder::new()
                    .name(format!("tondo-worker-{helper}"))
                    .spawn(move || self.serve(helper))
                    .expect("a worker thread");
            }
        });
        // The helpers see `work` only until `running` is back to zero,
        // which happens before this function returns.
        let job: &'static (dyn Fn() + Sync) = unsafe { std::mem::transmute(work) };
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.job = Some(job);
            state.wanted = helpers;
            state.running = helpers;
            state.round += 1;
        }
        self.wake.notify_all();
        let mine = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).err();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.running > 0 {
            state = self.finished.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.job = None;
        mine.or_else(|| state.panicked.take())
    }

    fn serve(&self, helper: usize) {
        let mut seen = 0;
        loop {
            let job = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                // A round that does not want this helper is over for it.
                while state.round == seen || helper >= state.wanted || state.job.is_none() {
                    if helper >= state.wanted {
                        seen = state.round;
                    }
                    state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                seen = state.round;
                state.job.expect("a job")
            };
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).err();
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(payload) = panicked {
                state.panicked.get_or_insert(payload);
            }
            state.running -= 1;
            if state.running == 0 {
                self.finished.notify_all();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_the_order_and_nests() {
        let items: Vec<u64> = (0..10_000).collect();
        let squares = super::map(&items, 1, |&n| n * n);
        assert_eq!(squares, items.iter().map(|n| n * n).collect::<Vec<_>>());
        // A call from inside another runs on its thread.
        let sums = super::map(&items[..64], 1, |&n| super::map(&items[..100], 1, |&m| m + n).iter().sum::<u64>());
        assert_eq!(sums[3], (0..100).map(|m| m + 3).sum::<u64>());
    }

    #[test]
    fn passes_a_panic_on() {
        let items: Vec<u32> = (0..1000).collect();
        let caught = std::panic::catch_unwind(|| super::map(&items, 1, |&n| if n == 500 { panic!("item") } else { n }));
        assert!(caught.is_err());
        // The pool still works afterwards.
        assert_eq!(super::map(&items, 1, |&n| n + 1)[999], 1000);
    }
}
