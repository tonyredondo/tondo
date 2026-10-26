//! Coverage (testing 10.3): what each counter of a test build counts, the
//! counts the workers write as they end, and what they add up to, as
//! `tondo-coverage-0.1/1`, as LCOV, and for people.

use super::json::{Value, int, obj, str};
use crate::codegen::Counted;
use std::collections::BTreeMap;
use std::path::Path;

/// Keeps what each counter counts beside the build, as JSON.
pub fn save_counted(path: &Path, counted: &[Counted]) -> Result<(), String> {
    let entries: Vec<serde_json::Value> = counted
        .iter()
        .map(|c| match c {
            Counted::Line { file, line } => serde_json::json!(["line", file, line]),
            Counted::Function { file, line, name } => serde_json::json!(["function", file, line, name]),
            Counted::Way { file, line, column, branch, label } => {
                serde_json::json!(["way", file, line, column, branch.0, branch.1, label])
            }
        })
        .collect();
    let text = serde_json::Value::from(entries).to_string();
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn load_counted(path: &Path) -> Result<Vec<Counted>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let entries: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let number = |v: &serde_json::Value| v.as_u64().unwrap_or_default() as usize;
    Ok(entries
        .iter()
        .map(|e| match e[0].as_str() {
            Some("function") => Counted::Function { file: text(&e[1]), line: number(&e[2]), name: text(&e[3]) },
            Some("way") => Counted::Way {
                file: text(&e[1]),
                line: number(&e[2]),
                column: number(&e[3]),
                branch: (number(&e[4]), number(&e[5])),
                label: text(&e[6]),
            },
            _ => Counted::Line { file: text(&e[1]), line: number(&e[2]) },
        })
        .collect())
}

/// What the counts of one file add up to: each line's runs, each
/// function's calls by its line, and each way of each branch, by where
/// the branch is.
#[derive(Default)]
struct FileCoverage {
    lines: BTreeMap<usize, u64>,
    functions: BTreeMap<usize, (String, u64)>,
    branches: BTreeMap<(usize, usize), Ways>,
}

/// The ways of a branch, by where each starts and what it is called, with
/// their counts.
type Ways = BTreeMap<(usize, usize, String), u64>;

pub struct Coverage {
    files: BTreeMap<String, FileCoverage>,
}

/// Adds up the counts the workers wrote in `counts`, by what the build's
/// counters count, kept in `counted`. Several copies of a generic
/// function count the same lines.
pub fn collect(counts: &Path, counted: &Path) -> Result<Coverage, String> {
    let counted = load_counted(counted)?;
    let mut totals = vec![0u64; counted.len()];
    if let Ok(entries) = std::fs::read_dir(counts) {
        for entry in entries.flatten() {
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            for (total, word) in totals.iter_mut().zip(bytes.as_chunks::<8>().0) {
                *total += u64::from_le_bytes(*word);
            }
        }
    }
    let mut files: BTreeMap<String, FileCoverage> = BTreeMap::new();
    for (counter, total) in counted.into_iter().zip(totals) {
        match counter {
            Counted::Line { file, line } => *files.entry(file).or_default().lines.entry(line).or_default() += total,
            Counted::Function { file, line, name } => {
                let functions = &mut files.entry(file).or_default().functions;
                functions.entry(line).or_insert((name, 0)).1 += total;
            }
            Counted::Way { file, line, column, branch, label } => {
                let branches = &mut files.entry(file).or_default().branches;
                *branches.entry(branch).or_default().entry((line, column, label)).or_default() += total;
            }
        }
    }
    Ok(Coverage { files })
}

/// How many there are and how many ran.
#[derive(Clone, Copy, Default)]
struct Ratio {
    found: usize,
    hit: usize,
}

impl Ratio {
    fn add(&mut self, other: Ratio) {
        self.found += other.found;
        self.hit += other.hit;
    }

    fn json(self) -> Value {
        obj(vec![("found", int(self.found)), ("hit", int(self.hit))])
    }

    fn percent(self) -> String {
        match self.found {
            0 => "-".into(),
            found => format!("{:.1}% ({}/{found})", self.hit as f64 * 100.0 / found as f64, self.hit),
        }
    }
}

impl FileCoverage {
    fn ratios(&self) -> [Ratio; 3] {
        let ratio = |counts: Vec<u64>| Ratio { found: counts.len(), hit: counts.iter().filter(|&&c| c > 0).count() };
        [
            ratio(self.lines.values().copied().collect()),
            ratio(self.functions.values().map(|(_, c)| *c).collect()),
            ratio(self.branches.values().flat_map(|ways| ways.values().copied()).collect()),
        ]
    }
}

impl Coverage {
    fn totals(&self) -> [Ratio; 3] {
        let mut totals = [Ratio::default(); 3];
        for file in self.files.values() {
            for (total, ratio) in totals.iter_mut().zip(file.ratios()) {
                total.add(ratio);
            }
        }
        totals
    }

    /// `tondo-coverage-0.1/1` (testing 15.10).
    pub fn json(&self) -> Value {
        let count = |n: u64| Value::Int(n as i64);
        let files = self
            .files
            .iter()
            .map(|(name, file)| {
                let lines = file.lines.iter().map(|(&line, &c)| obj(vec![("line", int(line)), ("count", count(c))]));
                let functions = file.functions.iter().map(|(&line, (name, c))| {
                    obj(vec![("name", str(name.clone())), ("line", int(line)), ("count", count(*c))])
                });
                let branches = file.branches.iter().flat_map(|(&(b_line, b_column), ways)| {
                    ways.iter().map(move |((line, column, label), &c)| {
                        let branch = obj(vec![("line", int(b_line)), ("column", int(b_column))]);
                        obj(vec![
                            ("branch", branch),
                            ("line", int(*line)),
                            ("column", int(*column)),
                            ("label", str(label.clone())),
                            ("count", count(c)),
                        ])
                    })
                });
                obj(vec![
                    ("file", str(name.clone())),
                    ("lines", Value::Array(lines.collect())),
                    ("functions", Value::Array(functions.collect())),
                    ("branches", Value::Array(branches.collect())),
                ])
            })
            .collect();
        let [lines, functions, branches] = self.totals();
        obj(vec![
            ("format", str("tondo-coverage-0.1/1")),
            ("files", Value::Array(files)),
            (
                "summary",
                obj(vec![("lines", lines.json()), ("functions", functions.json()), ("branches", branches.json())]),
            ),
        ])
    }

    /// The same as LCOV's tracefile, which other tools read.
    pub fn lcov(&self) -> String {
        let mut out = String::from("TN:\n");
        for (name, file) in &self.files {
            out.push_str(&format!("SF:{name}\n"));
            for (line, (function, _)) in &file.functions {
                out.push_str(&format!("FN:{line},{function}\n"));
            }
            for (function, count) in file.functions.values() {
                out.push_str(&format!("FNDA:{count},{function}\n"));
            }
            let [lines, functions, branches] = file.ratios();
            out.push_str(&format!("FNF:{}\nFNH:{}\n", functions.found, functions.hit));
            for (block, ((line, _), ways)) in file.branches.iter().enumerate() {
                for (way, count) in ways.values().enumerate() {
                    out.push_str(&format!("BRDA:{line},{block},{way},{count}\n"));
                }
            }
            out.push_str(&format!("BRF:{}\nBRH:{}\n", branches.found, branches.hit));
            for (line, count) in &file.lines {
                out.push_str(&format!("DA:{line},{count}\n"));
            }
            out.push_str(&format!("LF:{}\nLH:{}\nend_of_record\n", lines.found, lines.hit));
        }
        out
    }

    /// What `--coverage` prints after the tests: each file's share of
    /// lines, functions and branches that ran, and the whole project's.
    pub fn summary(&self) -> String {
        let width = self.files.keys().map(|name| name.chars().count()).max().unwrap_or(0).max(5);
        let row = |name: &str, [lines, functions, branches]: [Ratio; 3]| {
            format!(
                "  {name:width$}  lines {:<16}  functions {:<14}  branches {}\n",
                lines.percent(),
                functions.percent(),
                branches.percent()
            )
        };
        let mut out = String::from("\ncoverage:\n");
        for (name, file) in &self.files {
            out.push_str(&row(name, file.ratios()));
        }
        out.push_str(&row("total", self.totals()));
        out
    }
}
