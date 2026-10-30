//! The reports for people (testing 15.13): `--report markdown=<path>` and
//! `--report html=<path>` say what a run found, its tests, its benchmarks
//! and its coverage: in Markdown, as a pull request or a summary of CI
//! shows it, or in one HTML file without scripts, with its charts in SVG.

use super::Benchmark;
use super::bench::{self, Compared};
use std::time::Duration;

/// What a run found, as the reports tell it.
pub(super) struct Document<'a> {
    pub title: String,
    pub failed: bool,
    /// Passed, failed, skipped, blocked and flaky tests.
    pub counts: [usize; 5],
    pub elapsed: Duration,
    /// Each test or suite that failed, with what it ended with.
    pub failures: Vec<(String, String)>,
    /// Each test that skipped, with why.
    pub skipped: Vec<(String, String)>,
    /// Each benchmark by its test, and how it compares with its baseline.
    pub benchmarks: Vec<(String, &'a Benchmark, Option<Compared>)>,
    /// Where the benchmarks ran, and what they compare with.
    pub machine: Option<String>,
    pub baseline: Option<String>,
    pub max_regression: Option<f64>,
    pub regressions: &'a [String],
    /// Each file's lines, functions and branches that ran, of those there
    /// are, and the total last.
    pub coverage: Option<Vec<super::coverage::Row>>,
    /// The files of other systems, which this one does not build, and so
    /// coverage does not count (language 6.8): what it says of them.
    pub elsewhere: Option<String>,
}

impl Document<'_> {
    fn outcome(&self) -> String {
        let [passed, failed, skipped, blocked, flaky] = self.counts;
        let flaky = if flaky > 0 { format!(", {flaky} flaky") } else { String::new() };
        format!(
            "{}: {passed} passed{flaky}, {failed} failed, {skipped} skipped, {blocked} blocked, in {:.2} s",
            if self.failed { "FAILED" } else { "ok" },
            self.elapsed.as_secs_f64()
        )
    }
}

/// A share as a percent, with the counts.
fn share((hit, found): (usize, usize)) -> String {
    match found {
        0 => "-".into(),
        _ => format!("{:.1}% ({hit}/{found})", hit as f64 * 100.0 / found as f64),
    }
}

/// How a benchmark changed: by how much, what that says, and whether it
/// regressed.
fn change(compared: &Option<Compared>, baselined: bool) -> String {
    match compared {
        Some(c) => {
            let regressed = if c.regressed.is_some() { ", regressed" } else { "" };
            format!("{:+.1}% {}{regressed}", c.change * 100.0, c.verdict)
        }
        None if baselined => "new".into(),
        None => String::new(),
    }
}

/// The report in Markdown.
pub(super) fn markdown(document: &Document) -> String {
    let cell = |text: &str| text.replace('|', "\\|");
    let mut out = format!("# {}\n\n**{}**\n", document.title, document.outcome());
    if !document.failures.is_empty() {
        out.push_str("\n## Failures\n");
        for (id, detail) in &document.failures {
            out.push_str(&format!("\n### `{id}`\n\n```text\n{}\n```\n", detail.trim_end()));
        }
    }
    if !document.skipped.is_empty() {
        out.push_str("\n## Skipped\n\n");
        for (id, reason) in &document.skipped {
            out.push_str(&format!("- `{id}`: {}\n", reason.lines().next().unwrap_or_default()));
        }
    }
    if !document.benchmarks.is_empty() {
        out.push_str("\n## Benchmarks\n\n");
        if let Some(machine) = &document.machine {
            out.push_str(&format!("Measured {machine}"));
        }
        match &document.baseline {
            Some(baseline) => out.push_str(&format!(", compared with {baseline}.\n\n")),
            None => out.push_str(".\n\n"),
        }
        let baselined = document.baseline.is_some();
        out.push_str("| Test | Benchmark | Time/op | ± | Allocs/op | B/op |");
        out.push_str(if baselined {
            " Change |\n|---|---|---:|---:|---:|---:|---|\n"
        } else {
            "\n|---|---|---:|---:|---:|---:|\n"
        });
        for (test, benchmark, compared) in &document.benchmarks {
            let stats = bench::stats(&benchmark.samples);
            let mut row = format!(
                "| `{}` | {} | {} | {:.1}% | {} | {} |",
                test.rsplit("::").next().unwrap_or(test),
                cell(&benchmark.name),
                bench::time(stats.median),
                spread(&stats),
                bench::amount(benchmark.allocations),
                bench::amount(benchmark.bytes),
            );
            if baselined {
                row.push_str(&format!(" {} |", change(compared, true)));
            }
            out.push_str(&row);
            out.push('\n');
        }
        if !document.regressions.is_empty() {
            let max = document.max_regression.unwrap_or_default() * 100.0;
            out.push_str(&format!("\n### Regressed beyond {max}%\n\n"));
            for regression in document.regressions {
                out.push_str(&format!("- {}\n", cell(regression)));
            }
        }
    }
    if let Some(coverage) = &document.coverage {
        out.push_str("\n## Coverage\n\n| File | Lines | Functions | Branches |\n|---|---:|---:|---:|\n");
        let last = coverage.len().saturating_sub(1);
        for (index, (file, [lines, functions, branches])) in coverage.iter().enumerate() {
            let name = if index == last { "**Total**".to_string() } else { format!("`{file}`") };
            out.push_str(&format!("| {name} | {} | {} | {} |\n", share(*lines), share(*functions), share(*branches)));
        }
        if let Some(elsewhere) = &document.elsewhere {
            out.push_str(&format!("\n{elsewhere}\n"));
        }
    }
    out
}

/// The spread of a benchmark's samples, as a percent of their median.
fn spread(stats: &bench::Stats) -> f64 {
    if stats.median > 0.0 { stats.deviation / stats.median * 100.0 } else { 0.0 }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const STYLE: &str = "body{font:14px/1.5 system-ui,sans-serif;margin:2rem auto;max-width:72rem;padding:0 1rem;color:#1f2328}\
h1{font-size:1.6rem}h2{margin-top:2rem;border-bottom:1px solid #d0d7de}\
.ok{color:#1a7f37}.failed{color:#cf222e}table{border-collapse:collapse;width:100%}\
th,td{padding:.3rem .6rem;border-bottom:1px solid #d0d7de;text-align:left}td.n{text-align:right;font-variant-numeric:tabular-nums}\
pre{background:#f6f8fa;padding:.8rem;overflow:auto}code{font-size:.9em}\
.slower{color:#cf222e}.faster{color:#1a7f37}.same{color:#59636e}";

/// The report as one HTML file.
pub(super) fn html(document: &Document) -> String {
    let mut out = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>{}</title><style>{STYLE}</style></head><body>\n",
        escape(&document.title)
    );
    let class = if document.failed { "failed" } else { "ok" };
    out.push_str(&format!(
        "<h1>{}</h1>\n<p class=\"{class}\"><strong>{}</strong></p>\n",
        escape(&document.title),
        escape(&document.outcome())
    ));
    if !document.failures.is_empty() {
        out.push_str("<h2>Failures</h2>\n");
        for (id, detail) in &document.failures {
            out.push_str(&format!("<h3><code>{}</code></h3>\n<pre>{}</pre>\n", escape(id), escape(detail.trim_end())));
        }
    }
    if !document.skipped.is_empty() {
        out.push_str("<h2>Skipped</h2>\n<ul>\n");
        for (id, reason) in &document.skipped {
            let reason = reason.lines().next().unwrap_or_default();
            out.push_str(&format!("<li><code>{}</code>: {}</li>\n", escape(id), escape(reason)));
        }
        out.push_str("</ul>\n");
    }
    if !document.benchmarks.is_empty() {
        out.push_str("<h2>Benchmarks</h2>\n<p>");
        if let Some(machine) = &document.machine {
            out.push_str(&format!("Measured {}", escape(machine)));
        }
        if let Some(baseline) = &document.baseline {
            out.push_str(&format!(", compared with {}", escape(baseline)));
        }
        out.push_str(".</p>\n<table><tr><th>Test</th><th>Benchmark</th><th>Time/op</th><th>±</th><th>Allocs/op</th><th>B/op</th><th>Samples</th>");
        let baselined = document.baseline.is_some();
        if baselined {
            out.push_str("<th>Change</th><th></th>");
        }
        out.push_str("</tr>\n");
        // The changes' chart shares one scale.
        let reach = document
            .benchmarks
            .iter()
            .filter_map(|(_, _, c)| c.as_ref())
            .flat_map(|c| [c.low.abs(), c.high.abs(), c.change.abs()])
            .fold(0.05f64, f64::max);
        for (test, benchmark, compared) in &document.benchmarks {
            let stats = bench::stats(&benchmark.samples);
            out.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{:.1}%</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td>{}</td>",
                escape(test.rsplit("::").next().unwrap_or(test)),
                escape(&benchmark.name),
                escape(&bench::time(stats.median)),
                spread(&stats),
                bench::amount(benchmark.allocations),
                bench::amount(benchmark.bytes),
                samples(&benchmark.samples, &stats),
            ));
            if baselined {
                let verdict = compared.as_ref().map_or("same", |c| c.verdict);
                out.push_str(&format!(
                    "<td class=\"{verdict}\">{}</td><td>{}</td>",
                    escape(&change(compared, true)),
                    compared.as_ref().map_or(String::new(), |c| moved(c, reach))
                ));
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</table>\n");
        if !document.regressions.is_empty() {
            let max = document.max_regression.unwrap_or_default() * 100.0;
            out.push_str(&format!("<h3 class=\"failed\">Regressed beyond {max}%</h3>\n<ul>\n"));
            for regression in document.regressions {
                out.push_str(&format!("<li>{}</li>\n", escape(regression)));
            }
            out.push_str("</ul>\n");
        }
    }
    if let Some(coverage) = &document.coverage {
        out.push_str("<h2>Coverage</h2>\n<table><tr><th>File</th><th>Lines</th><th></th><th>Functions</th><th>Branches</th></tr>\n");
        let last = coverage.len().saturating_sub(1);
        for (index, (file, [lines, functions, branches])) in coverage.iter().enumerate() {
            let name = if index == last {
                "<strong>Total</strong>".to_string()
            } else {
                format!("<code>{}</code>", escape(file))
            };
            out.push_str(&format!(
                "<tr><td>{name}</td><td class=\"n\">{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>\n",
                share(*lines),
                filled(*lines),
                share(*functions),
                share(*branches)
            ));
        }
        out.push_str("</table>\n");
        if let Some(elsewhere) = &document.elsewhere {
            out.push_str(&format!("<p>{}</p>\n", escape(elsewhere)));
        }
    }
    out.push_str("</body></html>\n");
    out
}

/// The samples of a benchmark as ticks between the fastest and the
/// slowest, the median in black.
fn samples(samples: &[f64], stats: &bench::Stats) -> String {
    let (width, height) = (160.0, 16.0);
    let range = (stats.max - stats.min).max(f64::EPSILON);
    let x = |value: f64| 2.0 + (value - stats.min) / range * (width - 4.0);
    let mut svg = format!(
        "<svg width=\"{width}\" height=\"{height}\" role=\"img\"><title>samples from {} to {}</title>",
        bench::time(stats.min),
        bench::time(stats.max)
    );
    for &sample in samples {
        svg.push_str(&format!("<line x1=\"{0:.1}\" x2=\"{0:.1}\" y1=\"4\" y2=\"12\" stroke=\"#8c959f\"/>", x(sample)));
    }
    svg.push_str(&format!(
        "<line x1=\"{0:.1}\" x2=\"{0:.1}\" y1=\"1\" y2=\"15\" stroke=\"#1f2328\" stroke-width=\"2\"/></svg>",
        x(stats.median)
    ));
    svg
}

/// A change as a bar from zero, with its interval, on a scale that reaches
/// `reach` either way.
fn moved(compared: &Compared, reach: f64) -> String {
    let (width, middle) = (160.0, 80.0);
    let x = |value: f64| middle + value / reach * (middle - 2.0);
    let color = match compared.verdict {
        "slower" => "#cf222e",
        "faster" => "#1a7f37",
        _ => "#8c959f",
    };
    let (start, end) = (x(0.0).min(x(compared.change)), x(0.0).max(x(compared.change)));
    format!(
        "<svg width=\"{width}\" height=\"16\" role=\"img\"><title>{:+.1}%, from {:+.1}% to {:+.1}%</title>\
         <line x1=\"{middle}\" x2=\"{middle}\" y1=\"0\" y2=\"16\" stroke=\"#d0d7de\"/>\
         <rect x=\"{start:.1}\" y=\"4\" width=\"{:.1}\" height=\"8\" fill=\"{color}\"/>\
         <line x1=\"{:.1}\" x2=\"{:.1}\" y1=\"8\" y2=\"8\" stroke=\"#1f2328\"/></svg>",
        compared.change * 100.0,
        compared.low * 100.0,
        compared.high * 100.0,
        (end - start).max(1.0),
        x(compared.low),
        x(compared.high)
    )
}

/// A share as a bar.
fn filled((hit, found): (usize, usize)) -> String {
    let part = if found == 0 { 0.0 } else { hit as f64 / found as f64 };
    format!(
        "<svg width=\"100\" height=\"10\"><rect width=\"100\" height=\"10\" fill=\"#ffebe9\"/><rect width=\"{:.1}\" height=\"10\" fill=\"#1a7f37\"/></svg>",
        part * 100.0
    )
}
