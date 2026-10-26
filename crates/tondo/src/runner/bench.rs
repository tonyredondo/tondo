//! The benchmarks of `tondo test --bench` (testing 10.4): what the samples
//! of each say, the machine they ran on, and how people read them.

use super::Benchmark;
use super::json::{Value, int, obj, str};

/// What the samples of a benchmark say, in nanoseconds per run.
pub(super) struct Stats {
    pub median: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    /// The median of the distances to the median: the spread, which a
    /// sample slowed by something else does not stretch.
    pub deviation: f64,
}

pub(super) fn stats(samples: &[f64]) -> Stats {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = median(&sorted);
    let mut distances: Vec<f64> = sorted.iter().map(|s| (s - median).abs()).collect();
    distances.sort_by(f64::total_cmp);
    Stats {
        median,
        mean: sorted.iter().sum::<f64>() / sorted.len().max(1) as f64,
        min: sorted.first().copied().unwrap_or_default(),
        max: sorted.last().copied().unwrap_or_default(),
        deviation: self::median(&distances),
    }
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// A number with three decimals, as the report writes measures.
pub(super) fn decimal(value: f64) -> Value {
    Value::Number(format!("{value:.3}"))
}

/// A benchmark in the report (15.11).
pub(super) fn json(benchmark: &Benchmark) -> Value {
    let stats = stats(&benchmark.samples);
    obj(vec![
        ("name", str(benchmark.name.clone())),
        ("runs_per_sample", Value::Int(benchmark.runs as i64)),
        ("samples", int(benchmark.samples.len())),
        (
            "ns_per_op",
            obj(vec![
                ("median", decimal(stats.median)),
                ("mean", decimal(stats.mean)),
                ("min", decimal(stats.min)),
                ("max", decimal(stats.max)),
                ("deviation", decimal(stats.deviation)),
            ]),
        ),
        ("allocations_per_op", decimal(benchmark.allocations)),
        ("bytes_per_op", decimal(benchmark.bytes)),
        ("samples_ns_per_op", Value::Array(benchmark.samples.iter().map(|&s| decimal(s)).collect())),
    ])
}

/// What a run of benchmarks ran on: the time each measured for and the
/// machine, so that runs elsewhere are not taken for the same.
pub(super) fn environment(time: std::time::Duration) -> Value {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    obj(vec![
        ("time_ms", Value::Int(time.as_millis() as i64)),
        ("toolchain", str(format!("tondo {}", crate::driver::VERSION))),
        ("os", str(std::env::consts::OS)),
        ("arch", str(std::env::consts::ARCH)),
        ("cpu", str(cpu())),
        ("cpus", int(cpus)),
    ])
}

/// The model of the processor, as the system names it.
fn cpu() -> String {
    #[cfg(target_os = "linux")]
    {
        let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let named = info.lines().find_map(|l| {
            let (key, value) = l.split_once(':')?;
            matches!(key.trim(), "model name" | "Model" | "Hardware").then(|| value.trim().to_string())
        });
        named.unwrap_or_else(|| std::env::consts::ARCH.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "machdep.cpu.brand_string"]).output();
        output
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| std::env::consts::ARCH.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    std::env::consts::ARCH.to_string()
}

/// A time per run, in the unit that reads best.
pub(super) fn time(nanoseconds: f64) -> String {
    let (value, unit) = match nanoseconds {
        n if n < 1e3 => (n, "ns"),
        n if n < 1e6 => (n / 1e3, "µs"),
        n if n < 1e9 => (n / 1e6, "ms"),
        n => (n / 1e9, "s"),
    };
    let digits = match value {
        v if v < 10.0 => 3,
        v if v < 100.0 => 2,
        _ => 1,
    };
    format!("{value:.digits$} {unit}")
}

/// The lines of a test's benchmarks: each name, its time per run with its
/// spread, and its allocations.
pub(super) fn lines(benchmarks: &[Benchmark]) -> String {
    let width = benchmarks.iter().map(|b| b.name.chars().count()).max().unwrap_or(0);
    let mut text = String::new();
    for benchmark in benchmarks {
        let stats = stats(&benchmark.samples);
        let spread = if stats.median > 0.0 { stats.deviation / stats.median * 100.0 } else { 0.0 };
        text.push_str(&format!(
            "  {:<width$}  {:>11}/op  ±{spread:.1}%  {} allocs/op  {} B/op\n",
            benchmark.name,
            time(stats.median),
            amount(benchmark.allocations),
            amount(benchmark.bytes),
        ));
    }
    text
}

/// A count per run: whole when it is.
fn amount(value: f64) -> String {
    match value.fract() == 0.0 {
        true => format!("{value:.0}"),
        false => format!("{value:.2}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_samples() {
        let stats = stats(&[10.0, 12.0, 11.0, 100.0, 9.0]);
        assert_eq!((stats.median, stats.min, stats.max), (11.0, 9.0, 100.0));
        assert_eq!(stats.mean, 28.4);
        // The slow sample does not stretch the spread.
        assert_eq!(stats.deviation, 1.0);
        assert_eq!(time(152.25), "152.2 ns");
        assert_eq!(time(18_710.0), "18.71 µs");
        assert_eq!(time(2.5e9), "2.500 s");
        assert_eq!(amount(3.0), "3");
        assert_eq!(amount(0.5), "0.50");
    }
}
