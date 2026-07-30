//! Versioned regression ledger tied to executable inventory entries.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::inventory::Inventory;

pub const FORMAT: &str = "tondo-regressions/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionLedger {
    pub format: String,
    pub entries: Vec<RegressionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionEntry {
    pub id: String,
    pub confirmed_by: String,
    pub boundary: String,
    pub reproducer: String,
    pub test_id: String,
    pub source: String,
    pub fixed_in: String,
    pub evidence: Vec<String>,
}

impl RegressionLedger {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid `{}`: {error}", path.display()))
    }

    pub fn validate(&self, root: &Path, inventory: &Inventory) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!(
                "unsupported regression ledger format `{}`",
                self.format
            ));
        }
        let tests = inventory
            .tests
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if entry.id.is_empty()
                || entry.confirmed_by.is_empty()
                || entry.boundary.is_empty()
                || entry.reproducer.is_empty()
                || entry.test_id.is_empty()
                || entry.source.is_empty()
                || entry.fixed_in.is_empty()
                || entry.evidence.is_empty()
                || !ids.insert(entry.id.as_str())
            {
                return Err("regression entries require complete, unique identities".into());
            }
            if !entry.evidence.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(format!(
                    "regression `{}` evidence must be sorted and unique",
                    entry.id
                ));
            }
            let test = tests.get(entry.test_id.as_str()).ok_or_else(|| {
                format!(
                    "regression `{}` references unknown test `{}`",
                    entry.id, entry.test_id
                )
            })?;
            if test.status != "executable" || test.source != entry.source {
                return Err(format!(
                    "regression `{}` must reference an executable test in `{}`",
                    entry.id, entry.source
                ));
            }
            if !root.join(&entry.source).is_file() {
                return Err(format!(
                    "regression `{}` source `{}` does not exist",
                    entry.id, entry.source
                ));
            }
        }
        if !self.entries.windows(2).all(|pair| pair[0].id < pair[1].id) {
            return Err("regression entries must be sorted by identity".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::inventory::{InventorySummary, TestEntry};

    static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    fn inventory() -> Inventory {
        let test = TestEntry {
            id: "rust:crate:src::module:regression".into(),
            kind: "rust-test".into(),
            crate_name: Some("crate".into()),
            phase: "reliability".into(),
            source: "src/module.rs".into(),
            fixture: None,
            group: "unit".into(),
            requirements: Vec::new(),
            oracle: "rust-assertions".into(),
            repetitions: 1,
            source_sha256: "0".repeat(64),
            target: "host-rust".into(),
            document: None,
            edition: "host".into(),
            status: "executable".into(),
            sidecars: Vec::new(),
        };
        Inventory {
            format: crate::inventory::FORMAT.into(),
            repository: "test".into(),
            documents: Vec::new(),
            summary: InventorySummary {
                logical_tests: 1,
                repetitions: 1,
                physical_sources: 1,
                unique_source_hashes: 1,
                by_kind: [("rust-test".into(), 1)].into_iter().collect(),
                by_status: [("executable".into(), 1)].into_iter().collect(),
                by_phase: [("reliability".into(), 1)].into_iter().collect(),
            },
            tests: vec![test],
        }
    }

    #[test]
    fn ledger_rejects_unknown_tests_and_unsorted_evidence() {
        let root = tempfile_root();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/module.rs"), "").unwrap();
        let mut ledger = RegressionLedger {
            format: FORMAT.into(),
            entries: vec![RegressionEntry {
                id: "REG-001".into(),
                confirmed_by: "mutation".into(),
                boundary: "public".into(),
                reproducer: "minimal".into(),
                test_id: "rust:crate:src::module:regression".into(),
                source: "src/module.rs".into(),
                fixed_in: "M10.5".into(),
                evidence: vec!["a".into(), "b".into()],
            }],
        };
        ledger.validate(&root, &inventory()).unwrap();
        ledger.entries[0].test_id = "missing".into();
        assert!(ledger.validate(&root, &inventory()).is_err());
        ledger.entries[0].test_id = "rust:crate:src::module:regression".into();
        ledger.entries[0].evidence.reverse();
        assert!(ledger.validate(&root, &inventory()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn tempfile_root() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tondo-regression-ledger-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }
}
