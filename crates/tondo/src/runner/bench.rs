//! The benchmarks of `tondo test --bench` (testing 10.4): what the samples
//! of each say, the machine they ran on, how they compare with a baseline,
//! and how people read them.

use super::Benchmark;
use super::json::{Value, int, obj, str};
use std::collections::HashMap;
use std::path::Path;

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

/// The median time of a run in each layout, or `null` without
/// `--layouts`.
fn layouts(medians: &[f64]) -> Value {
    match medians.is_empty() {
        true => Value::Null,
        false => Value::Array(medians.iter().map(|&median| decimal(median)).collect()),
    }
}

/// A measure the processor's counters gave, or `null` without them.
fn counted(value: Option<f64>) -> Value {
    value.map_or(Value::Null, decimal)
}

/// A benchmark in the report (15.11), with how it compares with its
/// baseline, if it has one.
pub(super) fn json(benchmark: &Benchmark, compared: Option<&Compared>) -> Value {
    let stats = stats(&benchmark.samples);
    let mut fields = vec![
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
        ("instructions_per_op", counted(benchmark.instructions)),
        ("cycles_per_op", counted(benchmark.cycles)),
        ("branch_misses_per_op", counted(benchmark.branch_misses)),
        ("l1d_misses_per_op", counted(benchmark.l1d_misses)),
        ("l1i_misses_per_op", counted(benchmark.l1i_misses)),
        ("layouts_ns_per_op", layouts(&benchmark.layouts)),
        ("samples_ns_per_op", Value::Array(benchmark.samples.iter().map(|&s| decimal(s)).collect())),
    ];
    let baseline = compared.map_or(Value::Null, |c| {
        obj(vec![
            ("ns_per_op", decimal(c.median)),
            ("allocations_per_op", decimal(c.allocations)),
            ("bytes_per_op", decimal(c.bytes)),
            ("instructions_per_op", counted(c.instructions)),
            ("instructions_change_percent", counted(c.instructions_change.map(|change| change * 100.0))),
            (
                "counters_change_percent",
                obj(c
                    .counters
                    .iter()
                    .map(|&(name, change)| (name, counted(change.map(|change| change * 100.0))))
                    .collect()),
            ),
            ("change_percent", decimal(c.change * 100.0)),
            ("interval_percent", Value::Array(vec![decimal(c.low * 100.0), decimal(c.high * 100.0)])),
            ("verdict", str(c.verdict)),
            ("regressed", c.regressed.as_ref().map_or(Value::Null, |why| str(why.clone()))),
        ])
    });
    fields.push(("baseline", baseline));
    obj(fields)
}

/// The measures a run compares with, by test and benchmark, and where
/// they come from.
#[derive(Default)]
pub(super) struct Baselines {
    pub measures: HashMap<(String, String), Benchmark>,
    /// What the report says of them: a saved baseline with the machine it
    /// ran on, or a revision.
    pub source: Option<Value>,
}

/// How a benchmark compares with its baseline: the baseline's median and
/// allocations, the change of the median as a fraction, the interval it
/// is in with 95% confidence, what that says, and why it regressed; the
/// baseline's instructions, and their change, when both sides counted
/// them.
pub(super) struct Compared {
    pub median: f64,
    pub allocations: f64,
    pub bytes: f64,
    pub instructions: Option<f64>,
    pub instructions_change: Option<f64>,
    /// How cycles, mispredicted branches and first-cache misses changed,
    /// by their names in the report, where both sides counted them and an
    /// event comes once in a thousand instructions or more.
    pub counters: [(&'static str, Option<f64>); 4],
    pub change: f64,
    pub low: f64,
    pub high: f64,
    pub verdict: &'static str,
    pub regressed: Option<String>,
}

/// A change smaller than this is the same, whatever the interval says.
const NOISE: f64 = 0.02;

/// Compares a benchmark with its baseline; with `max`, a fraction, a run
/// slower, or with `by_instructions` taking more instructions, or
/// allocating more than that much more has regressed.
pub(super) fn compare(new: &Benchmark, base: &Benchmark, max: Option<f64>, by_instructions: bool) -> Compared {
    let (now, before) = (stats(&new.samples).median, stats(&base.samples).median);
    let change = if before > 0.0 { now / before - 1.0 } else { 0.0 };
    let seed = new.name.bytes().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let (low, high) = interval(&new.samples, &base.samples, seed);
    // Measured in several layouts on both sides, a change is one the
    // layouts do not make: every layout now past every one before.
    let span = |layouts: &[f64]| layouts.iter().fold((f64::MAX, 0.0f64), |(low, high), &l| (low.min(l), high.max(l)));
    let verdict = match (new.layouts.len() > 1 && base.layouts.len() > 1, span(&new.layouts), span(&base.layouts)) {
        (true, (now_low, _), (_, before_high)) if now_low > before_high * (1.0 + NOISE) => "slower",
        (true, (_, now_high), (before_low, _)) if now_high < before_low * (1.0 - NOISE) => "faster",
        (true, ..) => "same",
        _ => match change {
            c if low > 0.0 && c > NOISE => "slower",
            c if high < 0.0 && c < -NOISE => "faster",
            _ => "same",
        },
    };
    let grew = |now: f64, before: f64, max: f64| now > before * (1.0 + max) + 1e-9;
    let changed = |now: Option<f64>, before: Option<f64>| match (now, before) {
        (Some(now), Some(before)) if before > 0.0 => Some(now / before - 1.0),
        _ => None,
    };
    let instructions_change = changed(new.instructions, base.instructions);
    // An event rarer than one in a thousand instructions costs little,
    // and its change, of a few, says nothing.
    let often = |now: Option<f64>, before: Option<f64>| {
        let instructions = new.instructions.unwrap_or_default().max(base.instructions.unwrap_or_default());
        changed(now, before)
            .filter(|_| now.unwrap_or_default().max(before.unwrap_or_default()) * 1000.0 >= instructions)
    };
    let counters = [
        ("cycles", changed(new.cycles, base.cycles)),
        ("branch_misses", often(new.branch_misses, base.branch_misses)),
        ("l1d_misses", often(new.l1d_misses, base.l1d_misses)),
        ("l1i_misses", often(new.l1i_misses, base.l1i_misses)),
    ];
    let regressed = max.and_then(|max| {
        let more = |change: f64| format!("{:.1}% more instructions", change * 100.0);
        if by_instructions && instructions_change.is_none() {
            Some("its instructions were not counted on both sides, which judging by them needs".to_string())
        } else if by_instructions && instructions_change.is_some_and(|change| change > max) {
            instructions_change.map(more)
        } else if !by_instructions && verdict == "slower" && change > max {
            Some(format!("{:.1}% slower", change * 100.0))
        } else if grew(new.allocations, base.allocations, max) {
            Some(format!("{} allocs/op, was {}", amount(new.allocations), amount(base.allocations)))
        } else if grew(new.bytes, base.bytes, max) {
            Some(format!("{} B/op, was {}", amount(new.bytes), amount(base.bytes)))
        } else {
            None
        }
    });
    Compared {
        median: before,
        allocations: base.allocations,
        bytes: base.bytes,
        instructions: base.instructions,
        instructions_change,
        counters,
        change,
        low,
        high,
        verdict,
        regressed,
    }
}

/// Where the change of the median is with 95% confidence, as fractions:
/// the ratios of the medians of samples drawn again from each, with
/// repetition, a thousand times, from a seed so that it is the same each
/// time.
fn interval(new: &[f64], base: &[f64], mut seed: u64) -> (f64, f64) {
    if new.is_empty() || base.is_empty() {
        return (0.0, 0.0);
    }
    let mut next = || {
        // SplitMix64.
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut drawn = |samples: &[f64]| {
        let mut again: Vec<f64> =
            (0..samples.len()).map(|_| samples[(next() % samples.len() as u64) as usize]).collect();
        again.sort_by(f64::total_cmp);
        median(&again)
    };
    let mut ratios: Vec<f64> = (0..1000)
        .map(|_| {
            let (now, before) = (drawn(new), drawn(base));
            if before > 0.0 { now / before - 1.0 } else { 0.0 }
        })
        .collect();
    ratios.sort_by(f64::total_cmp);
    (ratios[25], ratios[974])
}

/// A baseline as `--save-baseline` writes it (15.12): the machine and the
/// measures of each benchmark, by test.
pub(super) fn baseline(environment: Value, measured: &[(String, &Benchmark)]) -> Value {
    let benchmarks = measured
        .iter()
        .map(|(test, benchmark)| {
            obj(vec![
                ("test", str(test.clone())),
                ("name", str(benchmark.name.clone())),
                ("runs_per_sample", Value::Int(benchmark.runs as i64)),
                ("allocations_per_op", decimal(benchmark.allocations)),
                ("bytes_per_op", decimal(benchmark.bytes)),
                ("instructions_per_op", counted(benchmark.instructions)),
                ("cycles_per_op", counted(benchmark.cycles)),
                ("branch_misses_per_op", counted(benchmark.branch_misses)),
                ("l1d_misses_per_op", counted(benchmark.l1d_misses)),
                ("l1i_misses_per_op", counted(benchmark.l1i_misses)),
                ("layouts_ns_per_op", layouts(&benchmark.layouts)),
                ("samples_ns_per_op", Value::Array(benchmark.samples.iter().map(|&s| decimal(s)).collect())),
            ])
        })
        .collect();
    obj(vec![
        ("format", str("tondo-bench-baseline-0.1/1")),
        ("environment", environment),
        ("benchmarks", Value::Array(benchmarks)),
    ])
}

/// The measures of a baseline `--save-baseline` wrote, and the machine
/// they ran on.
pub(super) fn read_baseline(path: &Path) -> Result<Baselines, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!("cannot read the baseline {}: {e}; `tondo test --bench --save-baseline` writes one", path.display())
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{} is not a baseline: {e}", path.display()))?;
    if value["format"] != "tondo-bench-baseline-0.1/1" {
        return Err(format!("{} is not a baseline of `tondo test --save-baseline`", path.display()));
    }
    let mut measures = HashMap::new();
    for entry in value["benchmarks"].as_array().into_iter().flatten() {
        let number = |key: &str| entry[key].as_f64().unwrap_or_default();
        let (Some(test), Some(name)) = (entry["test"].as_str(), entry["name"].as_str()) else { continue };
        let samples = entry["samples_ns_per_op"].as_array().into_iter().flatten().filter_map(|s| s.as_f64()).collect();
        let benchmark = Benchmark {
            name: name.to_string(),
            runs: entry["runs_per_sample"].as_u64().unwrap_or(1),
            samples,
            allocations: number("allocations_per_op"),
            bytes: number("bytes_per_op"),
            instructions: entry["instructions_per_op"].as_f64(),
            cycles: entry["cycles_per_op"].as_f64(),
            branch_misses: entry["branch_misses_per_op"].as_f64(),
            l1d_misses: entry["l1d_misses_per_op"].as_f64(),
            l1i_misses: entry["l1i_misses_per_op"].as_f64(),
            layouts: entry["layouts_ns_per_op"].as_array().into_iter().flatten().filter_map(|l| l.as_f64()).collect(),
        };
        measures.insert((test.to_string(), name.to_string()), benchmark);
    }
    let environment = &value["environment"];
    let machine = |key: &str| environment[key].as_str().unwrap_or_default().to_string();
    let source = obj(vec![
        ("kind", str("saved")),
        ("path", str(path.display().to_string())),
        ("toolchain", str(machine("toolchain"))),
        ("os", str(machine("os"))),
        ("arch", str(machine("arch"))),
        ("cpu", str(machine("cpu"))),
    ]);
    Ok(Baselines { measures, source: Some(source) })
}

/// Where benchmarks ran, as people read it.
pub(super) fn machine(time: std::time::Duration) -> String {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    format!(
        "for {} s each on {} ({cpus} threads), {}-{}, by tondo {}",
        time.as_secs_f64(),
        cpu(),
        std::env::consts::ARCH,
        std::env::consts::OS,
        crate::driver::VERSION
    )
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
/// spread, and its allocations; and with a baseline, how much it changed.
pub(super) fn lines(
    test: &str,
    benchmarks: &[Benchmark],
    baselines: &Baselines,
    max: Option<f64>,
    by_instructions: bool,
) -> String {
    let width = benchmarks.iter().map(|b| b.name.chars().count()).max().unwrap_or(0);
    let mut text = String::new();
    for benchmark in benchmarks {
        let stats = stats(&benchmark.samples);
        let spread = if stats.median > 0.0 { stats.deviation / stats.median * 100.0 } else { 0.0 };
        let compared = match baselines.measures.get(&(test.to_string(), benchmark.name.clone())) {
            Some(base) => {
                let compared = compare(benchmark, base, max, by_instructions);
                let regressed = if compared.regressed.is_some() { ", REGRESSED" } else { "" };
                // A change that rounds to nothing has no sign.
                let percent = |change: f64| if (change * 100.0).abs() < 0.05 { 0.0 } else { change * 100.0 };
                let instructions =
                    compared.instructions_change.map_or(String::new(), |c| format!(", {:+.1}% ins", percent(c)));
                // What else the processor counted, where it moved: why a
                // time changed when the instructions did not.
                let moved: String = compared
                    .counters
                    .iter()
                    .filter_map(|&(name, change)| {
                        let change = change.filter(|change| change.abs() >= 0.05)?;
                        Some(format!(", {} {:+.1}%", name.replace('_', " "), change * 100.0))
                    })
                    .collect();
                format!("  {:+.1}% {}{instructions}{moved}{regressed}", compared.change * 100.0, compared.verdict)
            }
            None if baselines.source.is_some() => "  new".to_string(),
            None => String::new(),
        };
        let instructions = benchmark.instructions.map_or(String::new(), |i| format!("  {} ins/op", many(i)));
        let layouts = match benchmark.layouts.len() > 1 {
            true => {
                let (low, high) = benchmark.layouts.iter().fold((f64::MAX, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
                format!("  {} layouts {}–{}", benchmark.layouts.len(), time(low), time(high))
            }
            false => String::new(),
        };
        text.push_str(&format!(
            "  {:<width$}  {:>11}/op  ±{spread:.1}%  {} allocs/op  {} B/op{instructions}{layouts}{compared}\n",
            benchmark.name,
            time(stats.median),
            amount(benchmark.allocations),
            amount(benchmark.bytes),
        ));
    }
    text
}

/// A large count per run, in thousands, millions or billions.
pub(super) fn many(value: f64) -> String {
    match value {
        v if v >= 1e9 => format!("{:.2}G", v / 1e9),
        v if v >= 1e6 => format!("{:.2}M", v / 1e6),
        v if v >= 1e3 => format!("{:.1}k", v / 1e3),
        v => format!("{v:.0}"),
    }
}

/// A count per run: whole when it is.
pub(super) fn amount(value: f64) -> String {
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

    #[test]
    fn compares_with_a_baseline() {
        let measured = |samples: Vec<f64>, allocations: f64| Benchmark {
            name: "parse".into(),
            runs: 100,
            samples,
            allocations,
            bytes: 8.0 * allocations,
            instructions: Some(1000.0),
            cycles: Some(400.0),
            ..Benchmark::default()
        };
        let base = measured((0..50).map(|i| 100.0 + (i % 5) as f64).collect(), 2.0);
        let slower = measured((0..50).map(|i| 120.0 + (i % 5) as f64).collect(), 2.0);
        let compared = compare(&slower, &base, Some(0.05), false);
        assert_eq!(compared.verdict, "slower");
        assert!((compared.change - 0.196).abs() < 0.001, "{}", compared.change);
        assert!(compared.low > 0.1 && compared.high < 0.3, "{} {}", compared.low, compared.high);
        assert_eq!(compared.regressed.as_deref(), Some("19.6% slower"));
        // Noise is the same, and so is a change within the limit.
        let noisy = measured((0..50).map(|i| 100.5 + (i % 5) as f64).collect(), 2.0);
        assert_eq!(compare(&noisy, &base, Some(0.05), false).verdict, "same");
        assert!(compare(&slower, &base, Some(0.5), false).regressed.is_none());
        // An allocation more regresses, however fast.
        let allocating = measured(base.samples.clone(), 3.0);
        assert_eq!(compare(&allocating, &base, Some(0.05), false).regressed.as_deref(), Some("3 allocs/op, was 2"));
        let faster = measured((0..50).map(|i| 50.0 + (i % 5) as f64).collect(), 2.0);
        assert_eq!(compare(&faster, &base, None, false).verdict, "faster");
    }

    /// Measured in several layouts on both sides, a change within what
    /// the layouts make is the same, and one past every layout is not.
    #[test]
    fn judges_by_layouts() {
        let measured = |median: f64, layouts: Vec<f64>| Benchmark {
            name: "loop".into(),
            runs: 10,
            samples: (0..50).map(|i| median + (i % 3) as f64).collect(),
            layouts,
            ..Benchmark::default()
        };
        let base = measured(100.0, vec![100.0, 130.0, 110.0]);
        let lucky = measured(125.0, vec![125.0, 104.0, 128.0]);
        assert_eq!(compare(&lucky, &base, None, false).verdict, "same");
        assert_eq!(compare(&lucky, &measured(100.0, Vec::new()), None, false).verdict, "slower");
        let slower = measured(150.0, vec![150.0, 160.0, 140.0]);
        assert_eq!(compare(&slower, &base, None, false).verdict, "slower");
        let faster = measured(60.0, vec![60.0, 70.0, 80.0]);
        assert_eq!(compare(&faster, &base, None, false).verdict, "faster");
    }

    /// Judged by instructions, a run slower in time with as many
    /// instructions passes, and one with more instructions regresses; one
    /// whose instructions a side did not count cannot be judged by them.
    #[test]
    fn judges_by_instructions() {
        let measured = |samples: Vec<f64>, instructions: Option<f64>| Benchmark {
            name: "parse".into(),
            runs: 100,
            samples,
            allocations: 2.0,
            bytes: 16.0,
            instructions,
            cycles: instructions.map(|i| i / 2.0),
            branch_misses: instructions.map(|i| i / 100.0),
            ..Benchmark::default()
        };
        let base = measured((0..50).map(|i| 100.0 + (i % 5) as f64).collect(), Some(1000.0));
        let slower = measured((0..50).map(|i| 120.0 + (i % 5) as f64).collect(), Some(1001.0));
        let compared = compare(&slower, &base, Some(0.05), true);
        assert_eq!((compared.verdict, compared.regressed.as_deref()), ("slower", None));
        assert!((compared.instructions_change.unwrap() - 0.001).abs() < 1e-9);
        let more = measured(base.samples.clone(), Some(1100.0));
        assert_eq!(compare(&more, &base, Some(0.05), true).regressed.as_deref(), Some("10.0% more instructions"));
        let counters = compare(&more, &base, None, false).counters;
        assert_eq!(counters[1].0, "branch_misses");
        assert!((counters[1].1.unwrap() - 0.1).abs() < 1e-9 && counters[2].1.is_none(), "{counters:?}");
        assert!(compare(&more, &base, Some(0.05), false).regressed.is_none());
        let uncounted = measured(base.samples.clone(), None);
        let why = compare(&uncounted, &base, Some(0.05), true).regressed.unwrap();
        assert!(why.contains("not counted"), "{why}");
        assert_eq!(many(12_345.0), "12.3k");
        assert_eq!(many(1_234_567.0), "1.23M");
        assert_eq!(many(512.0), "512");
    }
}
