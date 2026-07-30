//! Repository-wide, machine-readable test inventory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tondo_conformance::document::extract_fences;
use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};
use tondo_conformance::manifest::{
    CaseAction, CaseGroup, ConformanceCase, Expectation, LoadedSuite, PinnedFile,
};
use tondo_conformance::protocol::DocCategory;

use crate::{collect_files, logical_path, sha256};

pub const FORMAT: &str = "tondo-test-inventory/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub format: String,
    pub repository: String,
    pub documents: Vec<DocumentRevision>,
    pub summary: InventorySummary,
    pub tests: Vec<TestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRevision {
    pub path: String,
    pub edition: String,
    pub status: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventorySummary {
    pub logical_tests: u64,
    pub repetitions: u64,
    pub physical_sources: u64,
    pub unique_source_hashes: u64,
    pub by_kind: BTreeMap<String, u64>,
    pub by_status: BTreeMap<String, u64>,
    pub by_phase: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestEntry {
    pub id: String,
    pub kind: String,
    pub crate_name: Option<String>,
    pub phase: String,
    pub source: String,
    pub fixture: Option<String>,
    pub group: String,
    pub requirements: Vec<String>,
    pub oracle: String,
    pub repetitions: u32,
    pub source_sha256: String,
    pub target: String,
    pub document: Option<String>,
    pub edition: String,
    pub status: String,
    pub sidecars: Vec<String>,
}

pub fn build(root: &Path) -> Result<Inventory, String> {
    let lineage = DraftLineage::load(root, Path::new(DRAFT_LINEAGE_PATH))
        .map_err(|error| error.to_string())?;
    let suite = lineage.baseline_suite();
    validate_repository_sidecars(root, suite)?;

    let mut tests = Vec::new();
    discover_rust_tests(root, &mut tests)?;
    discover_fixture_tests(root, &mut tests)?;
    discover_conformance_tests(root, suite, &mut tests)?;
    discover_language_fences(root, suite, &mut tests)?;
    discover_pending_testing_fences(root, &mut tests)?;
    discover_fuzz_targets(root, &mut tests)?;
    tests.sort_by(|left, right| left.id.cmp(&right.id));
    require_unique_ids(&tests)?;

    let documents = [
        ("TONDO_LANGUAGE_SPEC.md", "0.1", "draft-normative"),
        ("TONDO_STANDARD_LIBRARY_SPEC.md", "0.1", "draft-pending"),
        ("TONDO_TESTING_SPEC.md", "0.1", "draft-pending"),
        ("TONDO_TOOLCHAIN_SPEC.md", "0.1", "draft-normative"),
        (DRAFT_LINEAGE_PATH, "0.1", "draft-open"),
    ]
    .into_iter()
    .map(|(path, edition, status)| {
        let bytes =
            fs::read(root.join(path)).map_err(|error| format!("cannot read `{path}`: {error}"))?;
        Ok(DocumentRevision {
            path: path.into(),
            edition: edition.into(),
            status: status.into(),
            sha256: sha256(&bytes),
        })
    })
    .collect::<Result<Vec<_>, String>>()?;

    Ok(Inventory {
        format: FORMAT.into(),
        repository: "tonyredondo/tondo".into(),
        documents,
        summary: summarize(&tests),
        tests,
    })
}

pub fn validate(inventory: &Inventory) -> Result<(), String> {
    if inventory.format != FORMAT {
        return Err(format!(
            "unsupported test inventory format `{}`",
            inventory.format
        ));
    }
    require_unique_ids(&inventory.tests)?;
    if inventory.tests.iter().any(|entry| {
        entry.id.is_empty()
            || entry.phase.is_empty()
            || entry.source.is_empty()
            || entry.oracle.is_empty()
            || entry.target.is_empty()
            || entry.edition.is_empty()
            || entry.status.is_empty()
            || entry.repetitions == 0
            || !is_sha256(&entry.source_sha256)
    }) {
        return Err("test inventory contains an incomplete entry".into());
    }
    for entry in &inventory.tests {
        require_sorted_unique(&format!("{} requirements", entry.id), &entry.requirements)?;
        require_sorted_unique(&format!("{} sidecars", entry.id), &entry.sidecars)?;
    }
    if inventory.documents.iter().any(|document| {
        document.path.is_empty()
            || document.edition.is_empty()
            || document.status.is_empty()
            || !is_sha256(&document.sha256)
    }) {
        return Err("test inventory contains an invalid document revision".into());
    }
    let expected = summarize(&inventory.tests);
    if inventory.summary != expected {
        return Err("test inventory summary does not match its entries".into());
    }
    Ok(())
}

fn discover_rust_tests(root: &Path, tests: &mut Vec<TestEntry>) -> Result<(), String> {
    let crates = root.join("crates");
    for path in collect_files(root, &crates)? {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let relative = logical_path(root, &path)?;
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        let contents = std::str::from_utf8(&bytes)
            .map_err(|error| format!("`{relative}` is not valid UTF-8: {error}"))?;
        let crate_name = relative
            .split('/')
            .nth(1)
            .ok_or_else(|| format!("cannot derive crate for `{relative}`"))?
            .to_owned();
        let source_sha256 = sha256(&bytes);
        for name in rust_test_names(contents)? {
            let module = relative
                .strip_prefix(&format!("crates/{crate_name}/"))
                .unwrap_or(&relative)
                .trim_end_matches(".rs")
                .replace('/', "::");
            tests.push(TestEntry {
                id: format!("rust:{crate_name}:{module}:{name}"),
                kind: "rust-test".into(),
                crate_name: Some(crate_name.clone()),
                phase: infer_phase(&relative).into(),
                source: relative.clone(),
                fixture: None,
                group: if relative.contains("/tests/") {
                    "integration"
                } else {
                    "unit"
                }
                .into(),
                requirements: Vec::new(),
                oracle: "rust-assertions".into(),
                repetitions: 1,
                source_sha256: source_sha256.clone(),
                target: "host-rust".into(),
                document: None,
                edition: "host".into(),
                status: "executable".into(),
                sidecars: Vec::new(),
            });
        }
    }
    Ok(())
}

fn rust_test_names(contents: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut pending_test = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "#[test]" {
            pending_test = true;
            continue;
        }
        if !pending_test {
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.is_empty() {
            continue;
        }
        let signature = trimmed
            .strip_prefix("fn ")
            .or_else(|| trimmed.strip_prefix("async fn "))
            .ok_or_else(|| format!("`#[test]` is not followed by a function: `{trimmed}`"))?;
        let end = signature
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(signature.len());
        if end == 0 {
            return Err(format!("cannot derive test name from `{trimmed}`"));
        }
        names.push(signature[..end].to_owned());
        pending_test = false;
    }
    if pending_test {
        return Err("a Rust source ends with an unmatched `#[test]`".into());
    }
    Ok(names)
}

fn discover_fixture_tests(root: &Path, tests: &mut Vec<TestEntry>) -> Result<(), String> {
    let fixture_root = root.join("tests");
    for source in collect_files(root, &fixture_root)? {
        if source.extension().and_then(|extension| extension.to_str()) != Some("to") {
            continue;
        }
        let relative = logical_path(root, &source)?;
        let fixture = relative
            .strip_prefix("tests/")
            .unwrap_or(&relative)
            .trim_end_matches(".to")
            .to_owned();
        let group = fixture.split('/').next().unwrap_or("unknown").to_owned();
        let bytes = fs::read(&source)
            .map_err(|error| format!("cannot read `{}`: {error}", source.display()))?;
        let mut sidecars = adjacent_sidecars(root, &source)?;
        sidecars.sort();
        let requirements = read_lines_if_present(&source.with_extension("codes"))?;
        let oracle = match group.as_str() {
            "compile-fail" | "spec" => "diagnostic-codes+structured-diagnostics",
            "compile-pass" => "compilation-status+diagnostics",
            "runtime" => "exit+stdout+stderr+diagnostics",
            _ => "fixture-sidecars",
        };
        tests.push(TestEntry {
            id: format!("fixture:{fixture}"),
            kind: "fixture".into(),
            crate_name: Some("tondo-compiler".into()),
            phase: infer_phase(&relative).into(),
            source: relative,
            fixture: Some(fixture),
            group,
            requirements,
            oracle: oracle.into(),
            repetitions: 1,
            source_sha256: sha256(&bytes),
            target: "tondo-vm-hosted".into(),
            document: None,
            edition: "0.1".into(),
            status: "executable".into(),
            sidecars,
        });
    }
    Ok(())
}

fn discover_conformance_tests(
    root: &Path,
    suite: &LoadedSuite,
    tests: &mut Vec<TestEntry>,
) -> Result<(), String> {
    for case in &suite.manifest().cases {
        let mut requirements = case.requirements.clone();
        requirements.extend(case.covers.iter().cloned());
        requirements.extend(case.positive_for.iter().cloned());
        requirements.sort();
        requirements.dedup();
        let pinned = case_pinned_files(case);
        let source_sha256 = combined_pinned_hash(&pinned);
        let source = pinned
            .first()
            .map(|file| file.path.clone())
            .unwrap_or_else(|| "conformance/0.1/manifest.json".into());
        let mut sidecars = pinned
            .iter()
            .skip(1)
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        sidecars.sort();
        tests.push(TestEntry {
            id: format!("conformance:{}", case.id),
            kind: "conformance-case".into(),
            crate_name: Some("tondo-conformance".into()),
            phase: conformance_phase(case.group).into(),
            source,
            fixture: Some(case.id.clone()),
            group: case_group(case.group).into(),
            requirements,
            oracle: match &case.expectation {
                Expectation::Exact { .. } => "exact-observation",
                Expectation::OneOf { .. } => "closed-observation-set",
            }
            .into(),
            repetitions: case.repeat,
            source_sha256,
            target: case.target.clone(),
            document: Some("TONDO_LANGUAGE_SPEC.md".into()),
            edition: suite.manifest().edition.clone(),
            status: "executable".into(),
            sidecars,
        });
    }
    let _ = root;
    Ok(())
}

fn discover_language_fences(
    root: &Path,
    suite: &LoadedSuite,
    tests: &mut Vec<TestEntry>,
) -> Result<(), String> {
    let path = root.join("TONDO_LANGUAGE_SPEC.md");
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let errors = suite
        .manifest()
        .registry
        .errors
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let fences = extract_fences(&bytes, &errors).map_err(|error| error.to_string())?;
    for (index, fence) in fences.into_iter().enumerate() {
        let category = doc_category(fence.category);
        let mut requirements = fence.expected_codes;
        requirements.sort();
        tests.push(TestEntry {
            id: format!("document:language-0.1:fence:{:04}", index + 1),
            kind: "spec-fence".into(),
            crate_name: Some("tondo-conformance".into()),
            phase: if category == "pseudocode" {
                "documentation"
            } else {
                "conformance"
            }
            .into(),
            source: "TONDO_LANGUAGE_SPEC.md".into(),
            fixture: fence.fixture,
            group: category.into(),
            requirements,
            oracle: if category == "pseudocode" {
                "documentation-only"
            } else {
                "document-fence-observation"
            }
            .into(),
            repetitions: 1,
            source_sha256: fence.source_sha256,
            target: "tondo-vm-hosted".into(),
            document: Some("TONDO_LANGUAGE_SPEC.md".into()),
            edition: "0.1".into(),
            status: if category == "pseudocode" {
                "non-executable"
            } else {
                "executable"
            }
            .into(),
            sidecars: Vec::new(),
        });
    }
    Ok(())
}

fn discover_pending_testing_fences(root: &Path, tests: &mut Vec<TestEntry>) -> Result<(), String> {
    let path = root.join("TONDO_TESTING_SPEC.md");
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let contents = std::str::from_utf8(&bytes)
        .map_err(|error| format!("TONDO_TESTING_SPEC.md is not valid UTF-8: {error}"))?;
    for (index, fence) in generic_tondo_fences(contents)?.into_iter().enumerate() {
        tests.push(TestEntry {
            id: format!("document:testing-0.1:fence:{:04}", index + 1),
            kind: "draft-contract".into(),
            crate_name: None,
            phase: "testing".into(),
            source: "TONDO_TESTING_SPEC.md".into(),
            fixture: None,
            group: fence.category,
            requirements: Vec::new(),
            oracle: "draft-contract-only".into(),
            repetitions: 1,
            source_sha256: sha256(fence.source.as_bytes()),
            target: "tondo-vm-hosted".into(),
            document: Some("TONDO_TESTING_SPEC.md".into()),
            edition: "0.1".into(),
            status: "draft-pending".into(),
            sidecars: Vec::new(),
        });
    }
    Ok(())
}

fn discover_fuzz_targets(root: &Path, tests: &mut Vec<TestEntry>) -> Result<(), String> {
    let directory = root.join("fuzz/fuzz_targets");
    if !directory.is_dir() {
        return Ok(());
    }
    for path in collect_files(root, &directory)? {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let relative = logical_path(root, &path)?;
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid fuzz target `{}`", path.display()))?;
        tests.push(TestEntry {
            id: format!("fuzz:{name}"),
            kind: "fuzz-target".into(),
            crate_name: None,
            phase: infer_phase(name).into(),
            source: relative,
            fixture: Some(name.into()),
            group: "fuzz".into(),
            requirements: Vec::new(),
            oracle: "panic+resource-limit+invariant".into(),
            repetitions: 1,
            source_sha256: sha256(&bytes),
            target: "host-rust".into(),
            document: None,
            edition: "host".into(),
            status: "campaign".into(),
            sidecars: Vec::new(),
        });
    }
    Ok(())
}

#[derive(Debug)]
struct GenericFence {
    category: String,
    source: String,
}

fn generic_tondo_fences(markdown: &str) -> Result<Vec<GenericFence>, String> {
    let mut result = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("~~~tondo") else {
            continue;
        };
        let category = header
            .split_ascii_whitespace()
            .next()
            .unwrap_or("syntax")
            .to_owned();
        let mut source = String::new();
        let mut closed = false;
        for content in lines.by_ref() {
            if content == "~~~" {
                closed = true;
                break;
            }
            source.push_str(content);
            source.push('\n');
        }
        if !closed {
            return Err("TONDO_TESTING_SPEC.md contains an unclosed Tondo fence".into());
        }
        if source.is_empty() {
            source.push('\n');
        }
        result.push(GenericFence { category, source });
    }
    Ok(result)
}

fn validate_repository_sidecars(root: &Path, suite: &LoadedSuite) -> Result<(), String> {
    validate_fixture_sidecars(root)?;
    validate_conformance_files(root, suite)
}

fn validate_fixture_sidecars(root: &Path) -> Result<(), String> {
    let tests = root.join("tests");
    for path in collect_files(root, &tests)? {
        let extension = path.extension().and_then(|value| value.to_str());
        if matches!(extension, Some("md" | "to")) {
            continue;
        }
        if !matches!(
            extension,
            Some(
                "args-unix"
                    | "args-windows"
                    | "codes"
                    | "jsonl"
                    | "stderr"
                    | "stdout"
                    | "runtime-stderr"
                    | "exit"
                    | "profiles"
            )
        ) {
            return Err(format!("unknown fixture sidecar `{}`", path.display()));
        }
        let source = path.with_extension("to");
        if !source.is_file() {
            return Err(format!(
                "orphan fixture sidecar `{}` has no `{}`",
                path.display(),
                source.display()
            ));
        }
        if matches!(extension, Some("args-unix" | "args-windows")) {
            let counterpart = match extension {
                Some("args-unix") => source.with_extension("args-windows"),
                Some("args-windows") => source.with_extension("args-unix"),
                _ => unreachable!(),
            };
            if !counterpart.is_file() {
                return Err(format!(
                    "platform argument sidecar `{}` has no `{}`",
                    path.display(),
                    counterpart.display()
                ));
            }
            let relative = source
                .strip_prefix(&tests)
                .map_err(|error| error.to_string())?;
            if !relative.starts_with("runtime") {
                return Err(format!(
                    "platform argument sidecar `{}` belongs to a non-runtime fixture",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_conformance_files(root: &Path, suite: &LoadedSuite) -> Result<(), String> {
    let referenced = suite
        .manifest()
        .cases
        .iter()
        .flat_map(case_pinned_files)
        .map(|file| file.path)
        .collect::<BTreeSet<_>>();
    let directory = root.join("conformance/0.1/cases");
    for path in collect_files(root, &directory)? {
        let relative = logical_path(root, &path)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("path `{}` is not valid UTF-8", path.display()))?;
        if referenced.contains(&relative) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("to")
            && source_is_generated_input(root, &path, &referenced)?
        {
            continue;
        }
        let primary = if let Some(stem) = file_name.strip_suffix(".meta.json") {
            Some(path.with_file_name(format!("{stem}.to")))
        } else if let Some(stem) = file_name.strip_suffix(".codes") {
            Some(path.with_file_name(format!("{stem}.to")))
        } else if let Some(stem) = file_name.strip_suffix(".stdout") {
            Some(path.with_file_name(format!("{stem}.to")))
        } else if let Some(stem) = file_name.strip_suffix(".exit") {
            Some(path.with_file_name(format!("{stem}.to")))
        } else if let Some(stem) = file_name.strip_suffix(".formatted") {
            Some(path.with_file_name(format!("{stem}.to")))
        } else if file_name.ends_with(".source") {
            file_name
                .split('.')
                .next()
                .map(|stem| path.with_file_name(format!("{stem}.to")))
        } else {
            None
        };
        if primary.as_ref().is_some_and(|source| source.is_file()) {
            continue;
        }
        return Err(format!(
            "undiscovered conformance file `{relative}` is neither pinned nor a valid sidecar"
        ));
    }
    for path in collect_files(root, &directory)? {
        if path.extension().and_then(|value| value.to_str()) != Some("to") {
            continue;
        }
        let relative = logical_path(root, &path)?;
        if !referenced.contains(&relative) && !source_is_generated_input(root, &path, &referenced)?
        {
            return Err(format!(
                "conformance source `{relative}` is not discovered by the manifest"
            ));
        }
    }
    Ok(())
}

fn source_is_generated_input(
    root: &Path,
    source: &Path,
    referenced: &BTreeSet<String>,
) -> Result<bool, String> {
    if !source.with_extension("meta.json").is_file() {
        return Ok(false);
    }
    let input = source.with_extension("input");
    if !input.is_file() {
        return Ok(false);
    }
    Ok(referenced.contains(&logical_path(root, &input)?))
}

fn case_pinned_files(case: &ConformanceCase) -> Vec<PinnedFile> {
    let mut files = Vec::new();
    match &case.action {
        CaseAction::Source(action) => {
            files.extend(action.sources.iter().map(|source| source.contents.clone()));
        }
        CaseAction::Semantic(action) => {
            files.extend(
                action
                    .source
                    .sources
                    .iter()
                    .map(|source| source.contents.clone()),
            );
        }
        CaseAction::Memory { .. } => {}
        CaseAction::Determinism(action) => {
            files.push(action.manifest.clone());
            files.push(action.lockfile.clone());
            files.extend(action.inputs.iter().map(|input| input.contents.clone()));
        }
        CaseAction::Document(action) => files.push(action.markdown.clone()),
    }
    files.push(case.expectation.pinned_file().clone());
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn combined_pinned_hash(files: &[PinnedFile]) -> String {
    let mut bytes = Vec::new();
    for file in files {
        bytes.extend_from_slice(file.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(file.sha256.as_bytes());
        bytes.push(b'\n');
    }
    sha256(&bytes)
}

fn adjacent_sidecars(root: &Path, source: &Path) -> Result<Vec<String>, String> {
    let parent = source
        .parent()
        .ok_or_else(|| format!("fixture `{}` has no parent", source.display()))?;
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("fixture `{}` has an invalid name", source.display()))?;
    let mut result = Vec::new();
    for entry in fs::read_dir(parent)
        .map_err(|error| format!("cannot read `{}`: {error}", parent.display()))?
    {
        let path = entry
            .map_err(|error| format!("cannot read `{}`: {error}", parent.display()))?
            .path();
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_some_and(|name| name.starts_with(&format!("{stem}."))) && path != source {
            result.push(logical_path(root, &path)?);
        }
    }
    Ok(result)
}

fn read_lines_if_present(path: &Path) -> Result<Vec<String>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let mut lines = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();
    Ok(lines)
}

fn summarize(tests: &[TestEntry]) -> InventorySummary {
    let mut by_kind = BTreeMap::new();
    let mut by_status = BTreeMap::new();
    let mut by_phase = BTreeMap::new();
    let mut physical_sources = BTreeSet::new();
    let mut source_hashes = BTreeSet::new();
    let mut repetitions = 0_u64;
    for entry in tests {
        *by_kind.entry(entry.kind.clone()).or_insert(0) += 1;
        *by_status.entry(entry.status.clone()).or_insert(0) += 1;
        *by_phase.entry(entry.phase.clone()).or_insert(0) += 1;
        physical_sources.insert(entry.source.clone());
        source_hashes.insert(entry.source_sha256.clone());
        repetitions += u64::from(entry.repetitions);
    }
    InventorySummary {
        logical_tests: tests.len() as u64,
        repetitions,
        physical_sources: physical_sources.len() as u64,
        unique_source_hashes: source_hashes.len() as u64,
        by_kind,
        by_status,
        by_phase,
    }
}

fn require_unique_ids(tests: &[TestEntry]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for entry in tests {
        if !ids.insert(entry.id.as_str()) {
            return Err(format!("duplicate test inventory ID `{}`", entry.id));
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn infer_phase(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.contains("lexer") {
        "lexer"
    } else if path.contains("format") {
        "formatter"
    } else if path.contains("frontend") {
        "frontend"
    } else if path.contains("parser") || path.contains("syntax") {
        "parser"
    } else if path.contains("resolve") {
        "resolution"
    } else if path.contains("hir") {
        "hir"
    } else if path.contains("mir") {
        "mir"
    } else if path.contains("bytecode") || path.contains("admission") {
        "bytecode"
    } else if path.contains("runtime") || path.contains("tondo-vm") {
        "runtime"
    } else if path.contains("project")
        || path.contains("artifact")
        || path.contains("protocol")
        || path.contains("manifest")
    {
        "protocol"
    } else if path.contains("conformance") {
        "conformance"
    } else if path.contains("reliability") {
        "reliability"
    } else if path.contains("cli") {
        "cli"
    } else {
        "semantic"
    }
}

fn conformance_phase(group: CaseGroup) -> &'static str {
    match group {
        CaseGroup::LexParseFormat => "frontend",
        CaseGroup::CompilePass | CaseGroup::CompileFail => "semantic",
        CaseGroup::SemanticQueries => "tooling",
        CaseGroup::Runtime => "runtime",
        CaseGroup::Concurrency => "concurrency",
        CaseGroup::Hosted => "host",
        CaseGroup::Memory => "memory",
        CaseGroup::Determinism => "protocol",
        CaseGroup::Documentation => "documentation",
    }
}

fn case_group(group: CaseGroup) -> &'static str {
    match group {
        CaseGroup::LexParseFormat => "lex-parse-format",
        CaseGroup::CompilePass => "compile-pass",
        CaseGroup::CompileFail => "compile-fail",
        CaseGroup::SemanticQueries => "semantic-queries",
        CaseGroup::Runtime => "runtime",
        CaseGroup::Concurrency => "concurrency",
        CaseGroup::Hosted => "hosted",
        CaseGroup::Memory => "memory",
        CaseGroup::Determinism => "determinism",
        CaseGroup::Documentation => "documentation",
    }
}

fn doc_category(category: DocCategory) -> &'static str {
    match category {
        DocCategory::Syntax => "syntax",
        DocCategory::Fragment => "fragment",
        DocCategory::Script => "script",
        DocCategory::CompileFail => "compile-fail",
        DocCategory::Pseudocode => "pseudocode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn rust_test_discovery_rejects_dangling_attributes_and_preserves_names() {
        assert_eq!(
            rust_test_names(
                "\
#[test]
fn first_case() {}

#[test]
#[cfg(unix)]
fn second_case() {}
"
            )
            .unwrap(),
            ["first_case", "second_case"]
        );
        assert!(rust_test_names("#[test]\nconst VALUE: u8 = 1;\n").is_err());
        assert!(rust_test_names("#[test]\n").is_err());
        assert_eq!(
            infer_phase("crates/tondo-compiler/src/syntax/format/document.rs"),
            "formatter"
        );
        assert_eq!(infer_phase("fuzz/fuzz_targets/frontend.rs"), "frontend");
    }

    #[test]
    fn generic_fence_discovery_keeps_future_contracts_non_executable() {
        let fences = generic_tondo_fences(
            "\
~~~tondo script
let value = 1
~~~
~~~rust
ignored
~~~
",
        )
        .unwrap();
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].category, "script");
        assert_eq!(fences[0].source, "let value = 1\n");
    }

    #[test]
    fn pinned_hash_is_order_sensitive_only_after_canonical_sorting() {
        let files = vec![
            PinnedFile {
                path: "a".into(),
                sha256: "1".repeat(64),
            },
            PinnedFile {
                path: "b".into(),
                sha256: "2".repeat(64),
            },
        ];
        assert_eq!(combined_pinned_hash(&files), combined_pinned_hash(&files));
        let mut reversed = files.clone();
        reversed.reverse();
        assert_ne!(
            combined_pinned_hash(&files),
            combined_pinned_hash(&reversed)
        );
    }

    #[test]
    fn orphan_fixture_sidecars_are_rejected() {
        let path = std::env::temp_dir().join(format!(
            "tondo-inventory-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let tests = path.join("tests/compile-fail");
        fs::create_dir_all(&tests).unwrap();
        fs::write(tests.join("orphan.codes"), b"E0001\n").unwrap();
        let error = validate_fixture_sidecars(&path).unwrap_err();
        assert!(error.contains("orphan fixture sidecar"));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn platform_argument_sidecars_must_be_paired_and_runtime_scoped() {
        let path = std::env::temp_dir().join(format!(
            "tondo-inventory-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = path.join("tests/runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join("case.to"), b"fn main() {}\n").unwrap();
        fs::write(runtime.join("case.args-unix"), b"unix\n").unwrap();
        let error = validate_fixture_sidecars(&path).unwrap_err();
        assert!(error.contains("has no"));

        fs::write(runtime.join("case.args-windows"), b"windows\n").unwrap();
        validate_fixture_sidecars(&path).unwrap();

        let compile_pass = path.join("tests/compile-pass");
        fs::create_dir_all(&compile_pass).unwrap();
        fs::write(compile_pass.join("case.to"), b"fn main() {}\n").unwrap();
        fs::write(compile_pass.join("case.args-unix"), b"unix\n").unwrap();
        fs::write(compile_pass.join("case.args-windows"), b"windows\n").unwrap();
        let error = validate_fixture_sidecars(&path).unwrap_err();
        assert!(error.contains("non-runtime fixture"));
        fs::remove_dir_all(path).unwrap();
    }
}
