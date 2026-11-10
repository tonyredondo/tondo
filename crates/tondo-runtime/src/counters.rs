//! The processor's counts of instructions and cycles in user space, of
//! every thread of the program, which `tondo test --bench` reads over the
//! samples of a benchmark (testing 10.4): they do not move with where the
//! code lies, as its time does. Linux's `perf_event_open`, where the
//! system lets a program count itself; none elsewhere.

/// The counters of each thread: instructions, then cycles.
pub(crate) struct Counters {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    files: Vec<i32>,
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
    pub(super) const CYCLES: u64 = 0;
    pub(super) const INSTRUCTIONS: u64 = 1;
    /// The time the counter was on, and the time it counted, which a
    /// processor with more counters asked than it has shares.
    const TIMES: u64 = 1 | 2;
    const EXCLUDE_KERNEL: u64 = 1 << 5;
    const EXCLUDE_HYPERVISOR: u64 = 1 << 6;
    const CLOSE_ON_EXEC: u64 = 8;

    /// A counter of the thread `tid`, counting from now.
    pub(super) fn open(tid: i32, event: u64) -> Option<i32> {
        let attr = Attr {
            kind: HARDWARE,
            size: size_of::<Attr>() as u32,
            config: event,
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
    /// when the system does not let it count.
    #[cfg(target_os = "linux")]
    pub(crate) fn start() -> Option<Counters> {
        let mut counters = Counters { files: Vec::new() };
        for task in std::fs::read_dir("/proc/self/task").ok()?.flatten() {
            let Some(tid) = task.file_name().to_str().and_then(|tid| tid.parse().ok()) else { continue };
            for event in [linux::INSTRUCTIONS, linux::CYCLES] {
                // A thread that ended meanwhile counts nothing.
                match linux::open(tid, event) {
                    Some(file) => counters.files.push(file),
                    None if std::path::Path::new(&format!("/proc/self/task/{tid}")).exists() => return None,
                    None => {}
                }
            }
        }
        Some(counters)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn start() -> Option<Counters> {
        None
    }

    /// The instructions and the cycles counted since the start.
    #[cfg(target_os = "linux")]
    pub(crate) fn read(&self) -> Option<(f64, f64)> {
        let mut total = (0.0, 0.0);
        for pair in self.files.chunks(2) {
            total.0 += linux::read(pair[0])?;
            total.1 += linux::read(*pair.get(1)?)?;
        }
        Some(total)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn read(&self) -> Option<(f64, f64)> {
        None
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        for &file in &self.files {
            unsafe { libc::close(file) };
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Where the system lets a program count itself, work counts.
    #[test]
    fn counts_the_instructions_of_work() {
        let allowed = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
            .ok()
            .and_then(|level| level.trim().parse::<i32>().ok())
            .is_some_and(|level| level <= 2);
        let Some(counters) = Counters::start() else {
            assert!(!allowed, "the system lets the program count itself");
            return;
        };
        let mut sum = 0u64;
        for i in 0..1_000_000u64 {
            sum = std::hint::black_box(sum.wrapping_add(i * i));
        }
        let (instructions, cycles) = counters.read().unwrap();
        assert!(instructions > 1_000_000.0 && cycles > 0.0, "{instructions} {cycles} {sum}");
    }
}
