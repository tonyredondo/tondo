//! Deterministic execution attestations for draft conformance case layers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};

use crate::inventory::{self, TestEntry};
use crate::provenance::QualityProvenance;
use crate::{INVENTORY_PATH, canonical_json, sha256};

pub const FORMAT: &str = "tondo-layer-evidence/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerEvidenceReport {
    pub format: String,
    pub lineage: String,
    pub revision: u32,
    pub manifest_sha256: String,
    pub inventory_sha256: String,
    pub tree_sha256: String,
    pub input_set_sha256: String,
    pub passed: bool,
    pub evidence: Vec<EvidenceObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservation {
    pub id: String,
    pub source_sha256: String,
    pub observation_sha256: String,
}

pub fn attest(
    root: &Path,
    test_log_path: &Path,
    before: &QualityProvenance,
) -> Result<LayerEvidenceReport, String> {
    before.validate()?;
    let after = QualityProvenance::current(root)?;
    if before.tree_sha256 != after.tree_sha256
        || before.input_set_sha256 != after.input_set_sha256
        || before.file_count != after.file_count
    {
        return Err("layer evidence input tree changed while tests were running".into());
    }

    let lineage = DraftLineage::load(root, Path::new(DRAFT_LINEAGE_PATH))
        .map_err(|error| error.to_string())?;
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    let inventory_bytes = canonical_json(&inventory)?;
    let tracked_inventory = fs::read(root.join(INVENTORY_PATH))
        .map_err(|error| format!("cannot read `{INVENTORY_PATH}`: {error}"))?;
    if tracked_inventory != inventory_bytes {
        return Err("tracked test inventory is stale".into());
    }

    let by_id = inventory
        .tests
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let required = lineage
        .case_layers()
        .iter()
        .flat_map(|layer| layer.cases.iter())
        .flat_map(|case| case.evidence.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let log = fs::read_to_string(test_log_path)
        .map_err(|error| format!("cannot read `{}`: {error}", test_log_path.display()))?;
    let passed = passed_test_leaves(&log)?;
    let mut evidence = Vec::with_capacity(required.len());
    for id in required {
        let entry = by_id
            .get(id)
            .ok_or_else(|| format!("layer evidence `{id}` is absent from the inventory"))?;
        attest_rust_test(entry, &passed)?;
        let observation_sha256 = sha256(&canonical_json(&(
            entry.id.as_str(),
            entry.source_sha256.as_str(),
            "passed",
        ))?);
        evidence.push(EvidenceObservation {
            id: entry.id.clone(),
            source_sha256: entry.source_sha256.clone(),
            observation_sha256,
        });
    }
    Ok(LayerEvidenceReport {
        format: FORMAT.into(),
        lineage: lineage.manifest().lineage.clone(),
        revision: lineage.manifest().revision,
        manifest_sha256: lineage.manifest_sha256(),
        inventory_sha256: sha256(&tracked_inventory),
        tree_sha256: after.tree_sha256,
        input_set_sha256: after.input_set_sha256,
        passed: true,
        evidence,
    })
}

fn attest_rust_test(entry: &TestEntry, passed: &BTreeMap<&str, u32>) -> Result<(), String> {
    if entry.kind != "rust-test" || entry.status != "executable" {
        return Err(format!(
            "layer evidence `{}` is not an executable Rust test",
            entry.id
        ));
    }
    let leaf = entry
        .id
        .rsplit(':')
        .next()
        .filter(|leaf| !leaf.is_empty())
        .ok_or_else(|| format!("layer evidence `{}` has no test name", entry.id))?;
    match passed.get(leaf).copied().unwrap_or(0) {
        1 => Ok(()),
        count => Err(format!(
            "layer evidence `{}` was observed {count} times as passed; expected exactly once",
            entry.id
        )),
    }
}

fn passed_test_leaves(log: &str) -> Result<BTreeMap<&str, u32>, String> {
    let mut passed = BTreeMap::new();
    for line in log.lines() {
        let Some(name) = line
            .strip_prefix("test ")
            .and_then(|line| line.strip_suffix(" ... ok"))
        else {
            continue;
        };
        let leaf = name.rsplit("::").next().unwrap_or(name);
        *passed.entry(leaf).or_insert(0) += 1;
    }
    if passed.is_empty() {
        return Err("test log contains no passed Rust tests".into());
    }
    Ok(passed)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn test_log_parser_is_exact_and_counts_duplicate_leaf_names() {
        let parsed = passed_test_leaves(
            "test module::tests::one ... ok\n\
             test other::tests::one ... ok\n\
             test module::tests::two ... FAILED\n",
        )
        .unwrap();
        assert_eq!(parsed.get("one"), Some(&2));
        assert_eq!(parsed.get("two"), None);
        assert!(passed_test_leaves("test nothing ... FAILED\n").is_err());
    }

    #[test]
    fn repository_layers_require_one_fresh_pass_per_exact_evidence() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        let names = lineage
            .case_layers()
            .iter()
            .flat_map(|layer| layer.cases.iter())
            .flat_map(|case| case.evidence.iter())
            .map(|id| id.rsplit(':').next().unwrap())
            .collect::<BTreeSet<_>>();
        let log = names
            .iter()
            .map(|name| format!("test module::tests::{name} ... ok\n"))
            .collect::<String>();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tondo-layer-evidence-{}-{nonce}.log",
            std::process::id()
        ));
        fs::write(&path, log).unwrap();
        let before = QualityProvenance::current(&root).unwrap();
        let report = attest(&root, &path, &before).unwrap();
        assert_eq!(report.evidence.len(), names.len());
        assert!(report.passed);

        let mut stale = before;
        stale.tree_sha256 = "0".repeat(64);
        assert!(attest(&root, &path, &stale).is_err());
        fs::remove_file(path).unwrap();
    }
}
