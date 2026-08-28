//! Versioned coverage and mutation baselines with non-regression gates.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provenance::QualityProvenance;
use crate::{canonical_json, sha256};

pub const FORMAT: &str = "tondo-quality-baseline/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityBaseline {
    pub format: String,
    pub revision: String,
    pub provenance: QualityProvenance,
    pub coverage: CoverageBaseline,
    pub mutation: MutationBaseline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageBaseline {
    pub tool: String,
    pub command: String,
    pub global: CoverageMetrics,
    pub risk_scopes: Vec<CoverageScope>,
    pub maximum_drop_basis_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageScope {
    pub name: String,
    pub paths: Vec<String>,
    pub metrics: CoverageMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageMetrics {
    pub lines: Metric,
    pub functions: Metric,
    pub regions: Metric,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub count: u64,
    pub covered: u64,
    pub basis_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationBaseline {
    pub tool: String,
    pub command: String,
    pub selected_paths: Vec<String>,
    pub total: u64,
    pub caught: u64,
    pub missed: u64,
    pub timeout: u64,
    pub unviable: u64,
    pub score_basis_points: u32,
    pub minimum_score_basis_points: u32,
    pub survivors: Vec<MutationSurvivor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationSurvivor {
    pub id: String,
    pub classification: String,
    pub rationale: String,
}

impl QualityBaseline {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        Self::from_bytes(&bytes).map_err(|error| format!("invalid `{}`: {error}", path.display()))
    }

    /// Parses a baseline embedded in a self-contained evidence bundle.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let baseline: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid quality baseline JSON: {error}"))?;
        baseline.validate()?;
        Ok(baseline)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!(
                "unsupported quality baseline format `{}`",
                self.format
            ));
        }
        if self.revision.is_empty()
            || self.coverage.tool.is_empty()
            || self.coverage.command.is_empty()
            || self.mutation.tool.is_empty()
            || self.mutation.command.is_empty()
            || self.mutation.selected_paths.is_empty()
        {
            return Err("quality baseline contains incomplete provenance".into());
        }
        self.provenance.validate()?;
        validate_metrics("global", &self.coverage.global)?;
        let mut scope_names = BTreeSet::new();
        for scope in &self.coverage.risk_scopes {
            if scope.name.is_empty()
                || scope.paths.is_empty()
                || !scope_names.insert(scope.name.as_str())
            {
                return Err("coverage risk scopes need unique names and paths".into());
            }
            require_sorted_unique(&format!("{} paths", scope.name), &scope.paths)?;
            validate_metrics(&scope.name, &scope.metrics)?;
        }
        require_sorted_unique("mutation selected paths", &self.mutation.selected_paths)?;
        if self.mutation.total
            != self.mutation.caught
                + self.mutation.missed
                + self.mutation.timeout
                + self.mutation.unviable
        {
            return Err("mutation outcome counts do not sum to the total".into());
        }
        let scored = self.mutation.caught + self.mutation.missed + self.mutation.timeout;
        let expected_score = if scored == 0 {
            0
        } else {
            ((self.mutation.caught * 10_000) / scored) as u32
        };
        if self.mutation.score_basis_points != expected_score
            || self.mutation.minimum_score_basis_points > self.mutation.score_basis_points
        {
            return Err("mutation score or gate is inconsistent with its outcomes".into());
        }
        let mut survivor_ids = BTreeSet::new();
        for survivor in &self.mutation.survivors {
            if survivor.id.is_empty()
                || survivor.rationale.is_empty()
                || !matches!(
                    survivor.classification.as_str(),
                    "missing-test" | "equivalent" | "justified-exclusion"
                )
                || !survivor_ids.insert(survivor.id.as_str())
            {
                return Err("mutation survivors need a unique classified rationale".into());
            }
        }
        if self.mutation.survivors.len() as u64 != self.mutation.missed {
            return Err("every missed mutant must be classified".into());
        }
        Ok(())
    }

    pub fn verify_coverage(&self, observed: &CoverageMetrics) -> Result<(), String> {
        validate_metrics("observed global", observed)?;
        for (name, baseline, current) in [
            (
                "line",
                self.coverage.global.lines.basis_points,
                observed.lines.basis_points,
            ),
            (
                "function",
                self.coverage.global.functions.basis_points,
                observed.functions.basis_points,
            ),
            (
                "region",
                self.coverage.global.regions.basis_points,
                observed.regions.basis_points,
            ),
        ] {
            if current.saturating_add(self.coverage.maximum_drop_basis_points) < baseline {
                return Err(format!(
                    "{name} coverage regressed from {baseline} to {current} basis points"
                ));
            }
        }
        Ok(())
    }

    pub fn verify_coverage_report(&self, observed: &CoverageReport) -> Result<(), String> {
        self.verify_coverage(&observed.global)?;
        let current = observed
            .risk_scopes
            .iter()
            .map(|scope| (scope.name.as_str(), &scope.metrics))
            .collect::<BTreeMap<_, _>>();
        for baseline in &self.coverage.risk_scopes {
            let observed = current
                .get(baseline.name.as_str())
                .ok_or_else(|| format!("coverage report omits risk scope `{}`", baseline.name))?;
            for (name, baseline_metric, observed_metric) in [
                ("line", &baseline.metrics.lines, &observed.lines),
                ("function", &baseline.metrics.functions, &observed.functions),
                ("region", &baseline.metrics.regions, &observed.regions),
            ] {
                if observed_metric
                    .basis_points
                    .saturating_add(self.coverage.maximum_drop_basis_points)
                    < baseline_metric.basis_points
                {
                    return Err(format!(
                        "{} {name} coverage regressed from {} to {} basis points",
                        baseline.name, baseline_metric.basis_points, observed_metric.basis_points
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn verify_mutation_score(&self, observed_basis_points: u32) -> Result<(), String> {
        if observed_basis_points < self.mutation.minimum_score_basis_points {
            Err(format!(
                "mutation score {observed_basis_points} is below the gate {}",
                self.mutation.minimum_score_basis_points
            ))
        } else {
            Ok(())
        }
    }

    pub fn verify_mutation_report(&self, observed: &MutationReport) -> Result<(), String> {
        self.verify_mutation_score(observed.score_basis_points)?;
        if observed.total != self.mutation.total {
            return Err(format!(
                "mutation selection changed from {} to {} outcomes; review and recapture the baseline",
                self.mutation.total, observed.total
            ));
        }
        if observed.unviable > self.mutation.unviable {
            return Err(format!(
                "unviable mutants increased from {} to {}",
                self.mutation.unviable, observed.unviable
            ));
        }
        if observed.caught < self.mutation.caught {
            return Err(format!(
                "caught mutants regressed from {} to {}",
                self.mutation.caught, observed.caught
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageReport {
    pub global: CoverageMetrics,
    pub risk_scopes: Vec<CoverageScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MutationReport {
    pub total: u64,
    pub caught: u64,
    pub missed: u64,
    pub timeout: u64,
    pub unviable: u64,
    pub score_basis_points: u32,
    pub missed_ids: Vec<String>,
}

pub fn capture(
    root: &Path,
    revision: impl Into<String>,
    coverage_bytes: &[u8],
    mutation_bytes: &[u8],
) -> Result<QualityBaseline, String> {
    let coverage = parse_llvm_cov(coverage_bytes)?;
    let mutation = parse_mutation_report(mutation_bytes)?;
    let mut survivors = mutation
        .missed_ids
        .iter()
        .map(|id| MutationSurvivor {
            id: id.clone(),
            classification: "missing-test".into(),
            rationale:
                "The baseline records this surviving mutant; a focused regression must kill it before the gate can rise."
                    .into(),
        })
        .collect::<Vec<_>>();
    survivors.sort_by(|left, right| left.id.cmp(&right.id));
    let baseline = QualityBaseline {
        format: FORMAT.into(),
        revision: revision.into(),
        provenance: QualityProvenance::current(root)?,
        coverage: CoverageBaseline {
            tool: "cargo-llvm-cov 0.8.7".into(),
            command:
                "cargo llvm-cov --workspace --all-targets --json --output-path <coverage.json>"
                    .into(),
            global: coverage.global,
            risk_scopes: coverage.risk_scopes,
            maximum_drop_basis_points: 0,
        },
        mutation: MutationBaseline {
            tool: "cargo-mutants 27.1.0".into(),
            command: "CARGO_INCREMENTAL=0 cargo mutants --workspace --no-config --copy-vcs true --gitignore true --error 'panic!(\"mutated\")' --file <selected-path> --re <one-per-critical-frontier> --baseline run --cargo-test-arg=--lib --jobs 1 --timeout 600 --build-timeout 900 --cargo-arg=--locked --output <directory> --no-shuffle --no-times --colors never --annotations none".into(),
            selected_paths: mutation_paths(),
            total: mutation.total,
            caught: mutation.caught,
            missed: mutation.missed,
            timeout: mutation.timeout,
            unviable: mutation.unviable,
            score_basis_points: mutation.score_basis_points,
            minimum_score_basis_points: mutation.score_basis_points,
            survivors,
        },
    };
    baseline.validate()?;
    Ok(baseline)
}

pub fn parse_llvm_cov(bytes: &[u8]) -> Result<CoverageReport, String> {
    let report: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid llvm-cov JSON: {error}"))?;
    let data = report
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| "llvm-cov report has no data entry".to_owned())?;
    let global = coverage_metrics(
        data.get("totals")
            .ok_or_else(|| "llvm-cov report has no totals".to_owned())?,
    )?;
    let files = data
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| "llvm-cov report has no files".to_owned())?;
    let mut risk_scopes = Vec::new();
    for (name, paths) in risk_scope_paths() {
        let mut lines = (0_u64, 0_u64);
        let mut functions = (0_u64, 0_u64);
        let mut regions = (0_u64, 0_u64);
        let mut matched_paths = BTreeSet::new();
        for file in files {
            let Some(filename) = file.get("filename").and_then(Value::as_str) else {
                continue;
            };
            let normalized = normalize_report_path(filename);
            let matches = paths
                .iter()
                .copied()
                .filter(|path| normalized.starts_with(path))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                continue;
            }
            matched_paths.extend(matches);
            let summary = file
                .get("summary")
                .ok_or_else(|| format!("llvm-cov file `{filename}` has no summary"))?;
            add_metric(&mut lines, summary, "lines")?;
            add_metric(&mut functions, summary, "functions")?;
            add_metric(&mut regions, summary, "regions")?;
        }
        let missing = paths
            .iter()
            .copied()
            .filter(|path| !matched_paths.contains(path))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "llvm-cov report omits configured {name} paths: {}",
                missing.join(", ")
            ));
        }
        let metrics = CoverageMetrics {
            lines: metric(lines.0, lines.1)?,
            functions: metric(functions.0, functions.1)?,
            regions: metric(regions.0, regions.1)?,
        };
        risk_scopes.push(CoverageScope {
            name: name.into(),
            paths: paths.into_iter().map(str::to_owned).collect(),
            metrics,
        });
    }
    Ok(CoverageReport {
        global,
        risk_scopes,
    })
}

pub fn parse_mutation_report(bytes: &[u8]) -> Result<MutationReport, String> {
    let values = match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => flatten_outcomes(&value),
        Err(_) => {
            let mut values = Vec::new();
            for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let value: Value = serde_json::from_slice(line).map_err(|error| {
                    format!("invalid mutation JSON on line {}: {error}", index + 1)
                })?;
                values.extend(flatten_outcomes(&value));
            }
            values
        }
    };
    let mut caught = 0;
    let mut missed = 0;
    let mut timeout = 0;
    let mut unviable = 0;
    let mut missed_ids = Vec::new();
    for value in &values {
        let Some(outcome) = find_outcome(value) else {
            continue;
        };
        let normalized = outcome.to_ascii_lowercase();
        if normalized.contains("caught") || normalized.contains("killed") {
            caught += 1;
        } else if normalized.contains("missed") || normalized.contains("survived") {
            missed += 1;
            missed_ids.push(mutation_id(value)?);
        } else if normalized.contains("timeout") {
            timeout += 1;
        } else if normalized.contains("unviable")
            || normalized.contains("nonviable")
            || normalized.contains("compile")
        {
            unviable += 1;
        }
    }
    missed_ids.sort();
    missed_ids.dedup();
    if missed_ids.len() as u64 != missed {
        return Err("mutation report contains duplicate surviving-mutant identities".into());
    }
    let total = caught + missed + timeout + unviable;
    if total == 0 {
        return Err("mutation report contains no classified mutant outcomes".into());
    }
    let scored = caught + missed + timeout;
    let score_basis_points = if scored == 0 {
        0
    } else {
        ((caught * 10_000) / scored) as u32
    };
    Ok(MutationReport {
        total,
        caught,
        missed,
        timeout,
        unviable,
        score_basis_points,
        missed_ids,
    })
}

/// Returns the portable identity of a quality report.
///
/// Tool output such as llvm-cov contains absolute build paths and cargo-mutants
/// records timings and process details. Bindings identify the parsed
/// observations instead, so equivalent runs produce the same digest while a
/// changed metric, outcome or survivor identity still fails closed.
pub fn canonical_report_bytes(kind: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    match kind {
        "coverage" => canonical_json(&parse_llvm_cov(bytes)?),
        "mutation" => canonical_json(&parse_mutation_report(bytes)?),
        _ => Err(format!("unsupported quality report kind `{kind}`")),
    }
}

pub fn mutation_paths() -> Vec<String> {
    [
        "crates/tondo-compiler/src/project.rs",
        "crates/tondo-conformance/src/document.rs",
        "crates/tondo-vm/src/bytecode.rs",
        "crates/tondo-vm/src/runtime/heap.rs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn coverage_metrics(value: &Value) -> Result<CoverageMetrics, String> {
    Ok(CoverageMetrics {
        lines: json_metric(value, "lines")?,
        functions: json_metric(value, "functions")?,
        regions: json_metric(value, "regions")?,
    })
}

fn json_metric(value: &Value, name: &str) -> Result<Metric, String> {
    let metric = value
        .get(name)
        .ok_or_else(|| format!("coverage summary omits `{name}`"))?;
    let count = metric
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("coverage `{name}` has no count"))?;
    let covered = metric
        .get("covered")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("coverage `{name}` has no covered count"))?;
    super::quality::metric(count, covered)
}

fn add_metric(total: &mut (u64, u64), summary: &Value, name: &str) -> Result<(), String> {
    let metric = json_metric(summary, name)?;
    total.0 = total
        .0
        .checked_add(metric.count)
        .ok_or_else(|| "coverage count overflow".to_owned())?;
    total.1 = total
        .1
        .checked_add(metric.covered)
        .ok_or_else(|| "covered count overflow".to_owned())?;
    Ok(())
}

fn normalize_report_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    for marker in ["/crates/", "/fuzz/"] {
        if let Some(index) = path.find(marker) {
            return path[index + 1..].to_owned();
        }
    }
    path.trim_start_matches("./").to_owned()
}

fn risk_scope_paths() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        (
            "parser",
            vec![
                "crates/tondo-compiler/src/syntax/cst.rs",
                "crates/tondo-compiler/src/syntax/format/",
                "crates/tondo-compiler/src/syntax/lexer.rs",
                "crates/tondo-compiler/src/syntax/parser.rs",
            ],
        ),
        (
            "checkers",
            vec![
                "crates/tondo-compiler/src/hir/availability.rs",
                "crates/tondo-compiler/src/hir/capabilities.rs",
                "crates/tondo-compiler/src/hir/check.rs",
                "crates/tondo-compiler/src/hir/regions.rs",
                "crates/tondo-compiler/src/hir/terminal.rs",
                "crates/tondo-compiler/src/hir/traits.rs",
                "crates/tondo-compiler/src/resolve",
                "crates/tondo-compiler/src/types.rs",
            ],
        ),
        (
            "verifiers",
            vec![
                "crates/tondo-compiler/src/hir/verify.rs",
                "crates/tondo-compiler/src/mir/verify.rs",
                "crates/tondo-vm/src/bytecode/verify.rs",
            ],
        ),
        (
            "heap",
            vec![
                "crates/tondo-vm/src/runtime/heap.rs",
                "crates/tondo-vm/src/runtime/value.rs",
            ],
        ),
        (
            "execution",
            vec![
                "crates/tondo-compiler/src/bytecode/lower.rs",
                "crates/tondo-vm/src/runtime/execute.rs",
            ],
        ),
        (
            "protocols",
            vec![
                "crates/tondo-compiler/src/artifact.rs",
                "crates/tondo-compiler/src/project.rs",
                "crates/tondo-conformance/src/",
                "crates/tondo-reference-adapter/src/",
                "crates/tondo-reliability/src/",
            ],
        ),
    ]
}

fn flatten_outcomes(value: &Value) -> Vec<Value> {
    if let Some(outcomes) = value.get("outcomes").and_then(Value::as_array) {
        outcomes.clone()
    } else if let Some(values) = value.as_array() {
        values.clone()
    } else {
        vec![value.clone()]
    }
}

fn find_outcome(value: &Value) -> Option<&str> {
    for key in ["outcome", "summary", "status"] {
        if let Some(outcome) = value.get(key).and_then(Value::as_str) {
            return Some(outcome);
        }
    }
    value.as_object()?.values().find_map(find_outcome)
}

fn mutation_id(value: &Value) -> Result<String, String> {
    for key in ["name", "display_name", "id"] {
        if let Some(name) = find_string_field(value, key) {
            return Ok(name.to_owned());
        }
    }
    let encoded =
        serde_json::to_vec(value).map_err(|error| format!("cannot encode mutant: {error}"))?;
    Ok(format!("mutant:{}", &sha256(&encoded)[..20]))
}

fn find_string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    if let Some(found) = value.get(key).and_then(Value::as_str) {
        return Some(found);
    }
    value.as_object()?.values().find_map(|child| {
        if let Some(values) = child.as_array() {
            values
                .iter()
                .find_map(|value| find_string_field(value, key))
        } else {
            find_string_field(child, key)
        }
    })
}

pub fn metric(count: u64, covered: u64) -> Result<Metric, String> {
    if covered > count {
        return Err("covered units exceed total units".into());
    }
    let basis_points = if count == 0 {
        10_000
    } else {
        u32::try_from((covered * 10_000) / count)
            .map_err(|_| "coverage ratio does not fit in u32".to_owned())?
    };
    Ok(Metric {
        count,
        covered,
        basis_points,
    })
}

fn validate_metrics(context: &str, metrics: &CoverageMetrics) -> Result<(), String> {
    for (name, metric) in [
        ("lines", &metrics.lines),
        ("functions", &metrics.functions),
        ("regions", &metrics.regions),
    ] {
        if metric.covered > metric.count || metric.basis_points > 10_000 {
            return Err(format!("{context} {name} coverage is invalid"));
        }
        let expected = if metric.count == 0 {
            10_000
        } else {
            ((metric.covered * 10_000) / metric.count) as u32
        };
        if metric.basis_points != expected {
            return Err(format!(
                "{context} {name} coverage percentage is inconsistent"
            ));
        }
    }
    Ok(())
}

fn require_sorted_unique(context: &str, values: &[String]) -> Result<(), String> {
    if values.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(format!("{context} must be sorted and unique"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_coverage_json(count: u64, covered: u64) -> Value {
        let files = risk_scope_paths()
            .into_iter()
            .flat_map(|(_, paths)| paths)
            .map(|path| {
                let filename = if path.ends_with('/') {
                    format!("/workspace/{path}representative.rs")
                } else {
                    format!("/workspace/{path}")
                };
                serde_json::json!({
                    "filename": filename,
                    "summary": {
                        "lines": {"count": count, "covered": covered},
                        "functions": {"count": count, "covered": covered},
                        "regions": {"count": count, "covered": covered}
                    }
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "data": [{
                "totals": {
                    "lines": {"count": count, "covered": covered},
                    "functions": {"count": count, "covered": covered},
                    "regions": {"count": count, "covered": covered}
                },
                "files": files
            }]
        })
    }

    fn baseline() -> QualityBaseline {
        let metrics = CoverageMetrics {
            lines: metric(100, 80).unwrap(),
            functions: metric(50, 40).unwrap(),
            regions: metric(200, 150).unwrap(),
        };
        QualityBaseline {
            format: FORMAT.into(),
            revision: "test".into(),
            provenance: QualityProvenance {
                format: crate::provenance::FORMAT.into(),
                tree_sha256: "a".repeat(64),
                input_set_sha256: "b".repeat(64),
                file_count: 4,
                flags: Vec::new(),
                toolchain: crate::provenance::Toolchain {
                    rustc: "rustc test".into(),
                    cargo: "cargo test".into(),
                },
            },
            coverage: CoverageBaseline {
                tool: "cargo-llvm-cov".into(),
                command: "cargo llvm-cov".into(),
                global: metrics.clone(),
                risk_scopes: vec![CoverageScope {
                    name: "frontend".into(),
                    paths: vec!["crates/compiler/src/syntax".into()],
                    metrics,
                }],
                maximum_drop_basis_points: 25,
            },
            mutation: MutationBaseline {
                tool: "cargo-mutants".into(),
                command: "cargo mutants".into(),
                selected_paths: vec!["src/parser.rs".into()],
                total: 4,
                caught: 3,
                missed: 1,
                timeout: 0,
                unviable: 0,
                score_basis_points: 7_500,
                minimum_score_basis_points: 7_500,
                survivors: vec![MutationSurvivor {
                    id: "mutant-1".into(),
                    classification: "missing-test".into(),
                    rationale: "tracked by a regression case".into(),
                }],
            },
        }
    }

    #[test]
    fn quality_baseline_is_self_consistent_and_enforces_non_regression() {
        let baseline = baseline();
        baseline.validate().unwrap();
        baseline
            .verify_coverage(&CoverageMetrics {
                lines: metric(100, 80).unwrap(),
                functions: metric(50, 40).unwrap(),
                regions: metric(200, 150).unwrap(),
            })
            .unwrap();
        assert!(
            baseline
                .verify_coverage(&CoverageMetrics {
                    lines: metric(100, 79).unwrap(),
                    functions: metric(50, 40).unwrap(),
                    regions: metric(200, 150).unwrap(),
                })
                .is_err()
        );
        assert!(baseline.verify_mutation_score(7_499).is_err());
        baseline
            .verify_mutation_report(&MutationReport {
                total: 4,
                caught: 3,
                missed: 1,
                timeout: 0,
                unviable: 0,
                score_basis_points: 7_500,
                missed_ids: vec!["mutant-1".into()],
            })
            .unwrap();
        assert!(
            baseline
                .verify_mutation_report(&MutationReport {
                    total: 3,
                    caught: 3,
                    missed: 0,
                    timeout: 0,
                    unviable: 0,
                    score_basis_points: 10_000,
                    missed_ids: Vec::new(),
                })
                .is_err()
        );
    }

    #[test]
    fn llvm_cov_parser_aggregates_named_risk_scopes() {
        let files = risk_scope_paths()
            .into_iter()
            .flat_map(|(_, paths)| paths)
            .map(|path| {
                let filename = if path.ends_with('/') {
                    format!("/workspace/{path}representative.rs")
                } else {
                    format!("/workspace/{path}")
                };
                serde_json::json!({
                    "filename": filename,
                    "summary": {
                        "lines": {"count": 10, "covered": 8},
                        "functions": {"count": 5, "covered": 4},
                        "regions": {"count": 20, "covered": 15}
                    }
                })
            })
            .collect::<Vec<_>>();
        let report = serde_json::json!({
            "data": [{
                "totals": {
                    "lines": {"count": 10, "covered": 8},
                    "functions": {"count": 5, "covered": 4},
                    "regions": {"count": 20, "covered": 15}
                },
                "files": files
            }]
        });
        let parsed = parse_llvm_cov(&serde_json::to_vec(&report).unwrap()).unwrap();
        assert_eq!(parsed.global.lines.basis_points, 8_000);
        let parser = parsed
            .risk_scopes
            .iter()
            .find(|scope| scope.name == "parser")
            .unwrap();
        assert_eq!(parser.metrics.functions.basis_points, 8_000);
        let heap = parsed
            .risk_scopes
            .iter()
            .find(|scope| scope.name == "heap")
            .unwrap();
        assert_eq!(heap.metrics.lines.basis_points, 8_000);

        let mut incomplete = report;
        incomplete["data"][0]["files"].as_array_mut().unwrap().pop();
        assert!(
            parse_llvm_cov(&serde_json::to_vec(&incomplete).unwrap())
                .unwrap_err()
                .contains("omits configured")
        );
    }

    #[test]
    fn risk_scope_paths_are_canonical() {
        for (name, paths) in risk_scope_paths() {
            let paths = paths.into_iter().map(str::to_owned).collect::<Vec<_>>();
            require_sorted_unique(&format!("{name} paths"), &paths).unwrap();
        }
    }

    #[test]
    fn mutation_parser_classifies_every_scored_outcome() {
        let outcomes = serde_json::json!({
            "outcomes": [
                {"outcome": "CaughtMutant", "mutant": {"name": "caught"}},
                {"outcome": "MissedMutant", "mutant": {"name": "survivor"}},
                {"outcome": "Timeout", "mutant": {"name": "slow"}},
                {"outcome": "Unviable", "mutant": {"name": "compile"}}
            ]
        });
        let report = parse_mutation_report(&serde_json::to_vec(&outcomes).unwrap()).unwrap();
        assert_eq!(report.total, 4);
        assert_eq!(report.caught, 1);
        assert_eq!(report.missed, 1);
        assert_eq!(report.timeout, 1);
        assert_eq!(report.unviable, 1);
        assert_eq!(report.score_basis_points, 3_333);
        assert_eq!(report.missed_ids, ["survivor"]);
    }

    #[test]
    fn mutation_identity_search_handles_nested_names_and_hashed_fallbacks() {
        let nested = serde_json::json!({
            "metadata": {"details": [{"id": "nested-survivor"}]}
        });
        assert_eq!(find_string_field(&nested, "id"), Some("nested-survivor"));
        assert_eq!(mutation_id(&nested).unwrap(), "nested-survivor");

        let unnamed = serde_json::json!({"metadata": {"details": [1, true]}});
        let identity = mutation_id(&unnamed).unwrap();
        assert!(identity.starts_with("mutant:"));
        assert_eq!(identity.len(), "mutant:".len() + 20);
    }

    #[test]
    fn baseline_validation_rejects_every_inconsistent_dimension() {
        let mut invalid = baseline();
        invalid.format = "quality/9".into();
        assert!(invalid.validate().unwrap_err().contains("unsupported"));

        let mut invalid = baseline();
        invalid.revision.clear();
        assert!(invalid.validate().unwrap_err().contains("provenance"));

        let mut invalid = baseline();
        invalid.coverage.global.lines.covered = 101;
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("coverage is invalid")
        );

        let mut invalid = baseline();
        invalid.coverage.global.lines.basis_points -= 1;
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("percentage is inconsistent")
        );

        let mut invalid = baseline();
        invalid.coverage.risk_scopes[0].paths.clear();
        assert!(invalid.validate().unwrap_err().contains("unique names"));

        let mut invalid = baseline();
        invalid
            .coverage
            .risk_scopes
            .push(invalid.coverage.risk_scopes[0].clone());
        assert!(invalid.validate().unwrap_err().contains("unique names"));

        let mut invalid = baseline();
        invalid.coverage.risk_scopes[0].paths = vec!["z".into(), "a".into(), "a".into()];
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("sorted and unique")
        );

        let mut invalid = baseline();
        invalid.mutation.selected_paths = vec!["z".into(), "a".into()];
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("sorted and unique")
        );

        let mut invalid = baseline();
        invalid.mutation.total += 1;
        assert!(invalid.validate().unwrap_err().contains("do not sum"));

        let mut only_unviable = baseline();
        only_unviable.mutation.total = 1;
        only_unviable.mutation.caught = 0;
        only_unviable.mutation.missed = 0;
        only_unviable.mutation.timeout = 0;
        only_unviable.mutation.unviable = 1;
        only_unviable.mutation.score_basis_points = 0;
        only_unviable.mutation.minimum_score_basis_points = 0;
        only_unviable.mutation.survivors.clear();
        only_unviable.validate().unwrap();

        let mut invalid = baseline();
        invalid.mutation.score_basis_points = 7_499;
        assert!(invalid.validate().unwrap_err().contains("score or gate"));

        let mut invalid = baseline();
        invalid.mutation.survivors[0].classification = "unknown".into();
        assert!(invalid.validate().unwrap_err().contains("classified"));

        let mut invalid = baseline();
        invalid.mutation.survivors[0].rationale.clear();
        assert!(invalid.validate().unwrap_err().contains("classified"));

        let mut invalid = baseline();
        invalid.mutation.survivors.clear();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("must be classified")
        );
    }

    #[test]
    fn report_gates_identify_global_scope_and_mutation_regressions() {
        let baseline = baseline();
        let observed = CoverageReport {
            global: baseline.coverage.global.clone(),
            risk_scopes: baseline.coverage.risk_scopes.clone(),
        };
        baseline.verify_coverage_report(&observed).unwrap();

        let mut missing = observed.clone();
        missing.risk_scopes.clear();
        assert!(
            baseline
                .verify_coverage_report(&missing)
                .unwrap_err()
                .contains("omits risk scope")
        );

        for dimension in ["lines", "functions", "regions"] {
            let mut regressed = observed.clone();
            let metrics = &mut regressed.risk_scopes[0].metrics;
            let metric = match dimension {
                "lines" => &mut metrics.lines,
                "functions" => &mut metrics.functions,
                "regions" => &mut metrics.regions,
                _ => unreachable!(),
            };
            metric.basis_points -= 26;
            assert!(
                baseline
                    .verify_coverage_report(&regressed)
                    .unwrap_err()
                    .contains(dimension.trim_end_matches('s'))
            );
        }

        for dimension in ["functions", "regions"] {
            let mut regressed = baseline.coverage.global.clone();
            let metric = match dimension {
                "functions" => &mut regressed.functions,
                "regions" => &mut regressed.regions,
                _ => unreachable!(),
            };
            metric.basis_points -= 26;
            assert!(
                baseline
                    .verify_coverage(&regressed)
                    .unwrap_err()
                    .contains(dimension.trim_end_matches('s'))
            );
        }

        let mut more_unviable = MutationReport {
            total: 4,
            caught: 3,
            missed: 0,
            timeout: 0,
            unviable: 1,
            score_basis_points: 10_000,
            missed_ids: Vec::new(),
        };
        assert!(
            baseline
                .verify_mutation_report(&more_unviable)
                .unwrap_err()
                .contains("unviable")
        );
        more_unviable.unviable = 0;
        more_unviable.caught = 2;
        more_unviable.missed = 2;
        more_unviable.score_basis_points = 5_000;
        assert!(
            baseline
                .verify_mutation_report(&more_unviable)
                .unwrap_err()
                .contains("score")
        );
        more_unviable.score_basis_points = 7_500;
        assert!(
            baseline
                .verify_mutation_report(&more_unviable)
                .unwrap_err()
                .contains("caught")
        );
    }

    #[test]
    fn report_parsers_cover_json_lines_fallback_ids_and_malformed_inputs() {
        let coverage = serde_json::to_vec(&complete_coverage_json(10, 9)).unwrap();
        let mutation = br#"
{"status":"Killed","mutant":{"display_name":"caught"}}

{"nested":{"summary":"Survived"},"payload":[{"id":"fallback-id"}]}
{"outcome":"Timeout","id":"slow"}
{"outcome":"CompileFailure","id":"compile"}
{"status":"Ignored"}
"#;
        let parsed = parse_mutation_report(mutation).unwrap();
        assert_eq!(parsed.total, 4);
        assert_eq!(parsed.missed_ids, ["fallback-id"]);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let captured = capture(root, "revision", &coverage, mutation).unwrap();
        assert_eq!(captured.revision, "revision");
        assert_eq!(captured.mutation.selected_paths, mutation_paths());
        assert_eq!(captured.mutation.survivors[0].id, "fallback-id");
        assert_eq!(
            captured.mutation.survivors[0].classification,
            "missing-test"
        );

        let hashed = parse_mutation_report(
            br#"[{"outcome":"MissedMutant","detail":{"location":"src/lib.rs:1"}}]"#,
        )
        .unwrap();
        assert!(hashed.missed_ids[0].starts_with("mutant:"));

        let only_unviable =
            parse_mutation_report(br#"{"outcome":"NonViable","name":"compile"}"#).unwrap();
        assert_eq!(only_unviable.score_basis_points, 0);
        assert!(parse_mutation_report(br#"{"status":"ignored"}"#).is_err());
        assert!(
            parse_mutation_report(b"{broken\n")
                .unwrap_err()
                .contains("line 1")
        );
        assert!(
            parse_mutation_report(
                br#"[{"outcome":"Missed","id":"same"},{"outcome":"Survived","id":"same"}]"#
            )
            .unwrap_err()
            .contains("duplicate")
        );

        assert_eq!(metric(0, 0).unwrap().basis_points, 10_000);
        assert!(metric(1, 2).is_err());
        assert_eq!(
            normalize_report_path(r"C:\repo\crates\x\src\lib.rs"),
            "crates/x/src/lib.rs"
        );
        assert_eq!(normalize_report_path("./relative.rs"), "relative.rs");
        assert_eq!(
            flatten_outcomes(&serde_json::json!([{"status": "caught"}])).len(),
            1
        );
        assert_eq!(
            flatten_outcomes(&serde_json::json!({"status": "caught"})).len(),
            1
        );

        for malformed in [
            serde_json::json!({}),
            serde_json::json!({"data": [{}]}),
            serde_json::json!({"data": [{"totals": {}}]}),
        ] {
            assert!(parse_llvm_cov(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        let mut malformed = complete_coverage_json(10, 9);
        malformed["data"][0]["files"][0]
            .as_object_mut()
            .unwrap()
            .remove("summary");
        assert!(
            parse_llvm_cov(&serde_json::to_vec(&malformed).unwrap())
                .unwrap_err()
                .contains("no summary")
        );
        assert!(QualityBaseline::load(Path::new("/definitely/missing/baseline.json")).is_err());
    }
}
