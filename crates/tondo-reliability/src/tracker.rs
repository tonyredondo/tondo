//! Machine-readable validation for the active implementation tracker graph.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const PATH: &str = "testing/tracker-graph.json";
const FORMAT: &str = "tondo-tracker-graph/1";
const TRACKER_PATH: &str = "TONDO_IMPLEMENTATION_TRACKER.md";

/// The sparse canonical graph manifest. Task declarations and completion state
/// remain in the tracker Markdown; this file contains only non-root edges.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub tracker: String,
    #[serde(default)]
    pub task_dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub gate_dependencies: BTreeMap<String, Vec<String>>,
}

/// Derived tracker evidence. Counts, ready work and the topological order are
/// calculated from the active Markdown declarations and the canonical edges.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Report {
    pub format: String,
    pub task_count: usize,
    pub completed_tasks: usize,
    pub pending_tasks: usize,
    pub gate_count: usize,
    pub ready_tasks: Vec<String>,
    pub topological_order: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Task {
    completed: bool,
    line: usize,
}

type Declarations = (BTreeMap<String, Task>, BTreeMap<String, usize>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fence {
    Backtick,
    Tilde,
}

/// Validate the repository tracker and return derived graph evidence.
pub fn lint(root: &Path) -> Result<Report, String> {
    let tracker = root.join(TRACKER_PATH);
    let manifest = root.join(PATH);
    let markdown = fs::read_to_string(&tracker)
        .map_err(|error| format!("cannot read `{}`: {error}", tracker.display()))?;
    let manifest_bytes = fs::read(&manifest)
        .map_err(|error| format!("cannot read `{}`: {error}", manifest.display()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid tracker graph `{}`: {error}", manifest.display()))?;
    lint_documents(&markdown, &manifest)
}

/// Validate Markdown plus a decoded manifest. This small boundary also makes
/// malformed graph cases cheap to test without constructing a workspace.
pub fn lint_documents(markdown: &str, manifest: &Manifest) -> Result<Report, String> {
    validate_manifest_header(manifest)?;
    let (tasks, gates) = parse_active_declarations(markdown)?;
    validate_manifest_keys(manifest, &tasks, &gates)?;

    let mut dependencies = BTreeMap::<String, Vec<String>>::new();
    for id in tasks.keys() {
        let entries = manifest
            .task_dependencies
            .get(id)
            .cloned()
            .unwrap_or_default();
        dependencies.insert(id.clone(), entries);
    }
    for id in gates.keys() {
        let entries = manifest
            .gate_dependencies
            .get(id)
            .cloned()
            .unwrap_or_default();
        dependencies.insert(id.clone(), entries);
    }
    validate_dependencies(&dependencies, &tasks, &gates)?;
    let topological_order = topological_order(&dependencies)?;
    let completed_nodes = completed_nodes(&dependencies, &tasks, &gates);
    let ready_tasks = tasks
        .iter()
        .filter(|(id, task)| {
            !task.completed
                && dependencies
                    .get(*id)
                    .into_iter()
                    .flatten()
                    .all(|dependency| completed_nodes.contains(dependency))
        })
        .map(|(id, _)| id.clone())
        .collect();
    let completed_tasks = tasks.values().filter(|task| task.completed).count();

    Ok(Report {
        format: FORMAT.to_owned(),
        task_count: tasks.len(),
        completed_tasks,
        pending_tasks: tasks.len() - completed_tasks,
        gate_count: gates.len(),
        ready_tasks,
        topological_order,
    })
}

/// Render a stable one-line CLI result without duplicating any tracker counts.
pub fn summary(report: &Report) -> String {
    let ready = if report.ready_tasks.is_empty() {
        String::from("none")
    } else {
        report.ready_tasks.join(", ")
    };
    format!(
        "tracker lint: OK ({} tasks: {} completed, {} pending; {} gates; ready: {ready})",
        report.task_count, report.completed_tasks, report.pending_tasks, report.gate_count
    )
}

fn validate_manifest_header(manifest: &Manifest) -> Result<(), String> {
    if manifest.format != FORMAT {
        return Err(format!(
            "tracker graph format `{}` is unsupported; expected `{FORMAT}`",
            manifest.format
        ));
    }
    if manifest.tracker != TRACKER_PATH {
        return Err(format!(
            "tracker graph points to `{}`, expected `{TRACKER_PATH}`",
            manifest.tracker
        ));
    }
    Ok(())
}

fn parse_active_declarations(markdown: &str) -> Result<Declarations, String> {
    let mut tasks = BTreeMap::new();
    let mut gates = BTreeMap::new();
    let mut fence = None;
    for (index, line) in markdown.lines().enumerate() {
        let line_number = index + 1;
        if let Some(marker) = fence {
            if closes_fence(line, marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = opens_fence(line) {
            fence = Some(marker);
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("## 25.") {
            break;
        }
        if let Some((id, completed)) = task_declaration(trimmed)?
            && let Some(previous) = tasks.insert(
                id.clone(),
                Task {
                    completed,
                    line: line_number,
                },
            )
        {
            return Err(format!(
                "{TRACKER_PATH}:{line_number}: duplicate task `{id}`; first declared at line {}",
                previous.line
            ));
        }
        if let Some(id) = gate_declaration(trimmed)
            && let Some(previous) = gates.insert(id.clone(), line_number)
        {
            return Err(format!(
                "{TRACKER_PATH}:{line_number}: duplicate gate `{id}`; first declared at line {previous}"
            ));
        }
    }
    if fence.is_some() {
        return Err(format!(
            "{TRACKER_PATH}: active section has an unterminated fence"
        ));
    }
    if tasks.is_empty() {
        return Err(format!("{TRACKER_PATH}: no active task declarations found"));
    }
    Ok((tasks, gates))
}

fn task_declaration(line: &str) -> Result<Option<(String, bool)>, String> {
    let (rest, completed) = if let Some(rest) = line.strip_prefix("- [x] **") {
        (rest, true)
    } else if let Some(rest) = line.strip_prefix("- [ ] **") {
        (rest, false)
    } else {
        return Ok(None);
    };
    let end = rest
        .find(" —")
        .or_else(|| rest.find("**"))
        .unwrap_or(rest.len());
    let id = rest[..end].trim();
    if !valid_id(id) {
        return Err(format!(
            "{TRACKER_PATH}: invalid task ID `{id}` in checklist declaration"
        ));
    }
    Ok(Some((id.to_owned(), completed)))
}

fn gate_declaration(line: &str) -> Option<String> {
    let heading = line
        .strip_prefix("### ")
        .or_else(|| line.strip_prefix("## "))?;
    let rest = heading.strip_prefix("Gate ")?;
    let candidate = rest
        .split_whitespace()
        .next()?
        .trim_matches(|value: char| matches!(value, ':' | ',' | '.' | '—' | '-'));
    valid_id(candidate).then(|| candidate.to_owned())
}

fn validate_manifest_keys(
    manifest: &Manifest,
    tasks: &BTreeMap<String, Task>,
    gates: &BTreeMap<String, usize>,
) -> Result<(), String> {
    for id in manifest.task_dependencies.keys() {
        if !tasks.contains_key(id) {
            return Err(format!("tracker graph declares unknown task `{id}`"));
        }
    }
    for id in manifest.gate_dependencies.keys() {
        if !gates.contains_key(id) {
            return Err(format!("tracker graph declares unknown gate `{id}`"));
        }
    }
    Ok(())
}

fn validate_dependencies(
    dependencies: &BTreeMap<String, Vec<String>>,
    tasks: &BTreeMap<String, Task>,
    gates: &BTreeMap<String, usize>,
) -> Result<(), String> {
    let known = tasks
        .keys()
        .chain(gates.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for (node, entries) in dependencies {
        let mut seen = BTreeSet::new();
        for dependency in entries {
            if !valid_id(dependency) {
                return Err(format!(
                    "tracker graph dependency `{dependency}` for `{node}` is not an exact ID"
                ));
            }
            if !known.contains(dependency) {
                return Err(format!(
                    "tracker graph dependency `{dependency}` for `{node}` is unknown; use the exact task or gate ID"
                ));
            }
            if dependency == node {
                return Err(format!("tracker graph node `{node}` depends on itself"));
            }
            if !seen.insert(dependency) {
                return Err(format!(
                    "tracker graph node `{node}` repeats dependency `{dependency}`"
                ));
            }
        }
    }
    Ok(())
}

fn topological_order(dependencies: &BTreeMap<String, Vec<String>>) -> Result<Vec<String>, String> {
    let mut indegree = dependencies
        .iter()
        .map(|(node, entries)| (node.clone(), entries.len()))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for (node, entries) in dependencies {
        for dependency in entries {
            outgoing
                .entry(dependency.clone())
                .or_default()
                .push(node.clone());
        }
    }
    for nodes in outgoing.values_mut() {
        nodes.sort();
    }
    let mut queue = dependencies
        .keys()
        .filter(|node| indegree[*node] == 0)
        .cloned()
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(dependencies.len());
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        if let Some(children) = outgoing.get(&node) {
            for child in children {
                let value = indegree
                    .get_mut(child)
                    .expect("outgoing nodes are present in the graph");
                *value -= 1;
                if *value == 0 {
                    let position = queue
                        .iter()
                        .position(|queued| queued > child)
                        .unwrap_or(queue.len());
                    queue.insert(position, child.clone());
                }
            }
        }
    }
    if order.len() != dependencies.len() {
        return Err("tracker graph contains a dependency cycle".into());
    }
    Ok(order)
}

fn completed_nodes(
    dependencies: &BTreeMap<String, Vec<String>>,
    tasks: &BTreeMap<String, Task>,
    gates: &BTreeMap<String, usize>,
) -> BTreeSet<String> {
    let mut completed = tasks
        .iter()
        .filter(|(_, task)| task.completed)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        for gate in gates.keys() {
            if !completed.contains(gate)
                && dependencies
                    .get(gate)
                    .into_iter()
                    .flatten()
                    .all(|dependency| completed.contains(dependency))
            {
                changed |= completed.insert(gate.clone());
            }
        }
    }
    completed
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && value.as_bytes()[0].is_ascii_uppercase()
        && !value.contains("--")
        && !value.contains("..")
}

fn opens_fence(line: &str) -> Option<Fence> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some(Fence::Backtick)
    } else if trimmed.starts_with("~~~") {
        Some(Fence::Tilde)
    } else {
        None
    }
}

fn closes_fence(line: &str, fence: Fence) -> bool {
    let trimmed = line.trim();
    match fence {
        Fence::Backtick => trimmed.starts_with("```") && trimmed.trim_matches('`').is_empty(),
        Fence::Tilde => trimmed.starts_with("~~~") && trimmed.trim_matches('~').is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn manifest(
        task_dependencies: BTreeMap<String, Vec<String>>,
        gate_dependencies: BTreeMap<String, Vec<String>>,
    ) -> Manifest {
        Manifest {
            format: FORMAT.into(),
            tracker: TRACKER_PATH.into(),
            task_dependencies,
            gate_dependencies,
        }
    }

    #[test]
    fn derives_counts_ready_tasks_and_stable_topological_order() {
        let markdown = "# Tracker\n\n- [x] **BOOT-001 — done**\n- [ ] **NEXT-001 — next**\n- [ ] **LATER-001 — later**\n### Gate G0\n## 25. History\n- [ ] **HIST-001 — ignored**\n";
        let report = lint_documents(
            markdown,
            &manifest(
                BTreeMap::from([(String::from("LATER-001"), vec![String::from("NEXT-001")])]),
                BTreeMap::from([(String::from("G0"), vec![String::from("BOOT-001")])]),
            ),
        )
        .unwrap();
        assert_eq!(report.task_count, 3);
        assert_eq!(report.completed_tasks, 1);
        assert_eq!(report.pending_tasks, 2);
        assert_eq!(report.gate_count, 1);
        assert_eq!(report.ready_tasks, vec!["NEXT-001"]);
        assert_eq!(
            report.topological_order,
            vec!["BOOT-001", "G0", "NEXT-001", "LATER-001"]
        );
        assert!(summary(&report).contains("ready: NEXT-001"));
    }

    #[test]
    fn ignores_fenced_checklists_and_rejects_duplicate_tasks_and_gates() {
        let markdown = "# Tracker\n```text\n- [ ] **FAKE-001 — ignored**\n```\n- [x] **REAL-001 — done**\n- [ ] **REAL-001 — duplicate**\n";
        let error =
            lint_documents(markdown, &manifest(BTreeMap::new(), BTreeMap::new())).unwrap_err();
        assert!(error.contains("duplicate task `REAL-001`"));

        let markdown =
            "# Tracker\n- [x] **REAL-001 — done**\n### Gate G0\n### Gate G0 — duplicate\n";
        let error =
            lint_documents(markdown, &manifest(BTreeMap::new(), BTreeMap::new())).unwrap_err();
        assert!(error.contains("duplicate gate `G0`"));
    }

    #[test]
    fn rejects_invalid_ids_and_missing_active_tasks() {
        let invalid = "# Tracker\n- [ ] **not-valid — bad**\n";
        assert!(
            lint_documents(invalid, &manifest(BTreeMap::new(), BTreeMap::new()))
                .unwrap_err()
                .contains("invalid task ID")
        );
        let empty = "# Tracker\n## 25. History\n";
        assert!(
            lint_documents(empty, &manifest(BTreeMap::new(), BTreeMap::new()))
                .unwrap_err()
                .contains("no active task")
        );
    }

    #[test]
    fn rejects_manifest_header_key_and_dependency_errors() {
        let markdown = "# Tracker\n- [ ] **REAL-001 — pending**\n### Gate G0\n";
        let mut wrong = manifest(BTreeMap::new(), BTreeMap::new());
        wrong.format = "other".into();
        assert!(
            lint_documents(markdown, &wrong)
                .unwrap_err()
                .contains("unsupported")
        );
        let mut wrong = manifest(BTreeMap::new(), BTreeMap::new());
        wrong.tracker = "other.md".into();
        assert!(
            lint_documents(markdown, &wrong)
                .unwrap_err()
                .contains("points to")
        );

        let mut unknown_task = manifest(BTreeMap::new(), BTreeMap::new());
        unknown_task
            .task_dependencies
            .insert("MISSING-001".into(), Vec::new());
        assert!(
            lint_documents(markdown, &unknown_task)
                .unwrap_err()
                .contains("unknown task")
        );
        let mut unknown_gate = manifest(BTreeMap::new(), BTreeMap::new());
        unknown_gate
            .gate_dependencies
            .insert("MISSING".into(), Vec::new());
        assert!(
            lint_documents(markdown, &unknown_gate)
                .unwrap_err()
                .contains("unknown gate")
        );
    }

    #[test]
    fn rejects_non_exact_unknown_duplicate_and_self_dependencies() {
        let markdown =
            "# Tracker\n- [ ] **REAL-001 — pending**\n- [ ] **NEXT-001 — pending**\n### Gate G0\n";
        for dependencies in [
            vec![String::from("REAL")],
            vec![String::from("MISSING-001")],
            vec![String::from("REAL-001"), String::from("REAL-001")],
            vec![String::from("REAL-001")],
        ] {
            let mut manifest = manifest(BTreeMap::new(), BTreeMap::new());
            manifest.task_dependencies.insert(
                if dependencies.len() == 1 && dependencies[0] == "REAL-001" {
                    "REAL-001".into()
                } else {
                    "NEXT-001".into()
                },
                dependencies,
            );
            let error = lint_documents(markdown, &manifest).unwrap_err();
            assert!(error.contains("dependency") || error.contains("depends on itself"));
        }
    }

    #[test]
    fn rejects_cycles_and_accepts_gate_dependencies() {
        let markdown = "# Tracker\n- [x] **DONE-001 — done**\n- [ ] **A-001 — a**\n- [ ] **B-001 — b**\n### Gate G0\n";
        let mut cyclic = manifest(BTreeMap::new(), BTreeMap::new());
        cyclic
            .task_dependencies
            .insert("A-001".into(), vec!["B-001".into()]);
        cyclic
            .task_dependencies
            .insert("B-001".into(), vec!["A-001".into()]);
        assert!(
            lint_documents(markdown, &cyclic)
                .unwrap_err()
                .contains("dependency cycle")
        );

        let mut valid = manifest(BTreeMap::new(), BTreeMap::new());
        valid
            .task_dependencies
            .insert("A-001".into(), vec!["G0".into()]);
        valid
            .gate_dependencies
            .insert("G0".into(), vec!["DONE-001".into()]);
        let report = lint_documents(markdown, &valid).unwrap();
        assert_eq!(report.ready_tasks, vec!["A-001", "B-001"]);
    }

    #[test]
    fn fence_helpers_cover_open_close_and_summary_without_ready_work() {
        assert_eq!(opens_fence("```json"), Some(Fence::Backtick));
        assert_eq!(opens_fence("  ~~~text"), Some(Fence::Tilde));
        assert!(opens_fence("plain").is_none());
        assert!(closes_fence("```", Fence::Backtick));
        assert!(closes_fence(" ~~~", Fence::Tilde));
        assert!(!closes_fence("~~~ text", Fence::Tilde));
        let report = Report {
            format: FORMAT.into(),
            task_count: 1,
            completed_tasks: 1,
            pending_tasks: 0,
            gate_count: 0,
            ready_tasks: Vec::new(),
            topological_order: vec!["DONE-001".into()],
        };
        assert!(summary(&report).contains("ready: none"));
        assert!(valid_id("STD-A-001"));
        assert!(!valid_id("std-a-001"));
        assert!(!valid_id("A--001"));
        assert!(!valid_id(""));
    }

    #[test]
    fn repository_manifest_is_read_through_the_public_lint_boundary() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let report = lint(&root).unwrap();
        assert_eq!(report.format, FORMAT);
        assert!(report.task_count > 600);
    }

    #[test]
    fn filesystem_and_json_errors_are_reported_at_the_boundary() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tondo-tracker-errors-{nonce}"));
        fs::create_dir_all(root.join("testing")).unwrap();
        let missing = lint(&root).unwrap_err();
        assert!(missing.contains("cannot read"));
        fs::write(
            root.join(TRACKER_PATH),
            "# Tracker\n- [x] **REAL-001 — done**\n",
        )
        .unwrap();
        let missing_manifest = lint(&root).unwrap_err();
        assert!(missing_manifest.contains("cannot read"));
        fs::write(root.join(PATH), "not-json").unwrap();
        let invalid_manifest = lint(&root).unwrap_err();
        assert!(invalid_manifest.contains("invalid tracker graph"));
        fs::remove_dir_all(root).unwrap();
    }
}
