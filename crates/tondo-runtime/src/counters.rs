//! The processor's counts of what the program runs outside the kernel, on
//! every thread it has, which `tondo test --bench` reads over the samples
//! of a benchmark (testing 10.4): instructions and cycles, and the branches
//! it mispredicts and the reads that miss the first caches of data and of
//! instructions, which explain a time that moves while the instructions do
//! not, as where the code lies moves it. Linux's `perf_event_open`, where
//! the system lets a program count itself; none elsewhere.

/// What the counters count, in the order `Counted` holds them.
pub(crate) const EVENTS: usize = 5;

/// What was counted of each event, scaled to the time it counted; `None`
/// for an event the processor does not count.
pub(crate) type Counted = [Option<f64>; EVENTS];

/// The counters of each thread, by event.
pub(crate) struct Counters {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    files: [Vec<i32>; EVENTS],
}

#[cfg(target_os = "linux")]
mod linux {
    /// `perf_event_attr` as its first version lays it out, which every
    /// kernel takes.
    #[repr(C)]
    #[derive(Default)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
    }

    const HARDWARE: u32 = 0;
    const CACHE: u32 = 3;
    /// Instructions, cycles, mispredicted branches, and reads that miss the
    /// first cache of data and of instructions, by their kind and number.
    pub(super) const EVENTS: [(u32, u64); super::EVENTS] =
        [(HARDWARE, 1), (HARDWARE, 0), (HARDWARE, 5), (CACHE, 1 << 16), (CACHE, 1 | 1 << 16)];
    /// The time the counter was on, and the time it counted, which a
    /// processor with more counters asked than it has shares.
    const TIMES: u64 = 1 | 2;
    const EXCLUDE_KERNEL: u64 = 1 << 5;
    const EXCLUDE_HYPERVISOR: u64 = 1 << 6;
    const CLOSE_ON_EXEC: u64 = 8;

    /// A counter of `event` on the thread `tid`, counting from now.
    pub(super) fn open(tid: i32, (kind, config): (u32, u64)) -> Option<i32> {
        let attr = Attr {
            kind,
            size: size_of::<Attr>() as u32,
            config,
            read_format: TIMES,
            flags: EXCLUDE_KERNEL | EXCLUDE_HYPERVISOR,
            ..Attr::default()
        };
        let file =
            unsafe { libc::syscall(libc::SYS_perf_event_open, &attr as *const Attr, tid, -1i32, -1i32, CLOSE_ON_EXEC) };
        (file >= 0).then_some(file as i32)
    }

    /// What a counter counted, scaled to the time it was on.
    pub(super) fn read(file: i32) -> Option<f64> {
        let mut values = [0u64; 3];
        let read = unsafe { libc::read(file, values.as_mut_ptr().cast(), size_of_val(&values)) };
        if read != size_of_val(&values) as isize {
            return None;
        }
        let [count, enabled, running] = values;
        Some(match running {
            0 => 0.0,
            _ => count as f64 * enabled as f64 / running as f64,
        })
    }
}

impl Counters {
    /// Counters of every thread the program has, counting from now; none
    /// when the system does not let it count its instructions. An event
    /// the processor does not count is left out.
    #[cfg(target_os = "linux")]
    pub(crate) fn start() -> Option<Counters> {
        let mut counters = Counters { files: Default::default() };
        let mut counts = [true; EVENTS];
        for task in std::fs::read_dir("/proc/self/task").ok()?.flatten() {
            let Some(tid) = task.file_name().to_str().and_then(|tid| tid.parse().ok()) else { continue };
            for (at, &event) in linux::EVENTS.iter().enumerate() {
                if !counts[at] {
                    continue;
                }
                match linux::open(tid, event) {
                    Some(file) => counters.files[at].push(file),
                    // A thread that ended meanwhile counts nothing.
                    None if !std::path::Path::new(&format!("/proc/self/task/{tid}")).exists() => {}
                    None if at == 0 => return None,
                    None => counts[at] = false,
                }
            }
        }
        for (files, counts) in counters.files.iter_mut().zip(counts) {
            if !counts {
                files.drain(..).for_each(|file| unsafe {
                    libc::close(file);
                });
            }
        }
        Some(counters)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn start() -> Option<Counters> {
        None
    }

    /// What each event counted since the start, on every thread.
    #[cfg(target_os = "linux")]
    pub(crate) fn read(&self) -> Counted {
        self.files.each_ref().map(|files| match files.is_empty() {
            true => None,
            false => files.iter().map(|&file| linux::read(file)).sum(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn read(&self) -> Counted {
        [None; EVENTS]
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        for &file in self.files.iter().flatten() {
            unsafe { libc::close(file) };
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Where the system lets a program count itself, work counts: where a
    /// counter of this thread opens, as a container's filter of system
    /// calls may refuse whatever `perf_event_paranoid` says.
    #[test]
    fn counts_the_instructions_of_work() {
        let allowed = linux::open(0, linux::EVENTS[0]).map(|file| unsafe { libc::close(file) }).is_some();
        let Some(counters) = Counters::start() else {
            assert!(!allowed, "the system lets the program count itself");
            return;
        };
        let mut sum = 0u64;
        for i in 0..1_000_000u64 {
            sum = std::hint::black_box(sum.wrapping_add(i * i));
        }
        let [instructions, cycles, branches, ..] = counters.read();
        assert!(instructions.unwrap() > 1_000_000.0 && cycles.unwrap() > 0.0, "{instructions:?} {cycles:?} {sum}");
        assert!(branches.is_none_or(|branches| branches < 1_000_000.0), "{branches:?}");
    }
}
