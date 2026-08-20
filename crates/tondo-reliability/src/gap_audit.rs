//! Closed, requirement-by-requirement audit of open G5 coverage rows.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::inventory::Inventory;
use crate::matrix::{CoverageMatrix, SpecificationIdentity};

pub const FORMAT: &str = "tondo-normative-gap-audit/1";
pub const PATH: &str = "testing/normative-gap-audit.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapAudit {
    pub format: String,
    pub edition: String,
    pub target: String,
    pub documents: Vec<SpecificationIdentity>,
    pub summary: GapAuditSummary,
    pub entries: Vec<GapAuditEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapAuditSummary {
    pub total: u64,
    pub by_outcome: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapAuditEntry {
    pub requirement: String,
    pub text_sha256: String,
    pub outcome: String,
    pub reason: String,
    pub implementation: Vec<String>,
    pub tests: Vec<String>,
    pub follow_up: Option<String>,
}

impl GapAudit {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot decode `{}`: {error}", path.display()))
    }

    pub fn validate(
        &self,
        root: &Path,
        matrix: &CoverageMatrix,
        inventory: &Inventory,
    ) -> Result<(), String> {
        if self.format != FORMAT || self.edition != matrix.edition || self.target != matrix.target {
            return Err("normative gap audit has an unsupported format, edition, or target".into());
        }
        if self.documents != matrix.documents {
            return Err("normative gap audit does not match the current G5 specifications".into());
        }

        let requirements = matrix
            .requirements
            .iter()
            .map(|requirement| (requirement.id.as_str(), requirement))
            .collect::<BTreeMap<_, _>>();
        if self
            .entries
            .windows(2)
            .any(|pair| pair[0].requirement >= pair[1].requirement)
        {
            return Err("normative gap audit entries must be globally sorted and unique".into());
        }
        let executable_tests = inventory
            .tests
            .iter()
            .filter(|test| test.status == "executable")
            .map(|test| test.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut by_outcome = BTreeMap::new();
        let mut observed = BTreeSet::new();
        for entry in &self.entries {
            let requirement = requirements
                .get(entry.requirement.as_str())
                .ok_or_else(|| {
                    format!(
                        "gap audit entry `{}` does not name a current requirement",
                        entry.requirement
                    )
                })?;
            if !observed.insert(entry.requirement.as_str())
                || entry.text_sha256 != requirement.text_sha256
                || entry.reason.trim().is_empty()
                || !entry.reason.contains(&entry.requirement)
            {
                return Err(format!(
                    "gap audit entry `{}` has stale or incomplete identity",
                    entry.requirement
                ));
            }
            match entry.outcome.as_str() {
                "implemented-without-trace" => {
                    if !matches!(
                        requirement.status.as_str(),
                        "toolchain-limit" | "draft-pending" | "covered"
                    ) {
                        return Err(format!(
                            "implemented audit entry `{}` has incompatible matrix status `{}`",
                            entry.requirement, requirement.status
                        ));
                    }
                    if entry.implementation.is_empty()
                        || entry.tests.is_empty()
                        || entry.follow_up.is_some()
                        || entry
                            .implementation
                            .windows(2)
                            .any(|pair| pair[0] >= pair[1])
                        || entry.tests.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err(format!(
                            "implemented audit entry `{}` lacks implementation or test evidence",
                            entry.requirement
                        ));
                    }
                    for path in &entry.implementation {
                        if path.starts_with('/')
                            || path.split('/').any(|component| component == "..")
                            || !root.join(path).is_file()
                        {
                            return Err(format!(
                                "audit entry `{}` names invalid implementation path `{path}`",
                                entry.requirement
                            ));
                        }
                    }
                    for test in &entry.tests {
                        if !executable_tests.contains(test.as_str()) {
                            return Err(format!(
                                "audit entry `{}` names unknown executable test `{test}`",
                                entry.requirement
                            ));
                        }
                    }
                }
                "not-applicable" => {
                    if !matches!(
                        requirement.status.as_str(),
                        "toolchain-limit" | "draft-pending" | "target-not-applicable"
                    ) {
                        return Err(format!(
                            "not-applicable audit entry `{}` has incompatible matrix status `{}`",
                            entry.requirement, requirement.status
                        ));
                    }
                    if !entry.implementation.is_empty()
                        || !entry.tests.is_empty()
                        || entry.follow_up.is_some()
                    {
                        return Err(format!(
                            "not-applicable audit entry `{}` carries executable evidence",
                            entry.requirement
                        ));
                    }
                }
                "absent" => {
                    if !matches!(
                        requirement.status.as_str(),
                        "toolchain-limit" | "draft-pending"
                    ) {
                        return Err(format!(
                            "absent audit entry `{}` cannot resolve as `{}`",
                            entry.requirement, requirement.status
                        ));
                    }
                    let expected = format!("CONF-GAP-IMPL-001:{}", entry.requirement);
                    if !entry.implementation.is_empty()
                        || !entry.tests.is_empty()
                        || entry.follow_up.as_deref() != Some(expected.as_str())
                    {
                        return Err(format!(
                            "absent audit entry `{}` lacks its exact leaf follow-up",
                            entry.requirement
                        ));
                    }
                }
                outcome => {
                    return Err(format!(
                        "gap audit entry `{}` uses unknown outcome `{outcome}`",
                        entry.requirement
                    ));
                }
            }
            *by_outcome.entry(entry.outcome.clone()).or_insert(0) += 1;
        }
        let open = requirements
            .values()
            .filter(|requirement| {
                matches!(
                    requirement.status.as_str(),
                    "toolchain-limit" | "draft-pending"
                )
            })
            .map(|requirement| requirement.id.as_str())
            .collect::<BTreeSet<_>>();
        if !open.is_subset(&observed) {
            return Err("normative gap audit omits an open requirement".into());
        }
        let summary = GapAuditSummary {
            total: self.entries.len() as u64,
            by_outcome,
        };
        if self.summary != summary {
            return Err("normative gap audit summary is inconsistent".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{inventory, matrix};

    fn repository_evidence() -> (std::path::PathBuf, Inventory, CoverageMatrix, GapAudit) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let inventory = inventory::build(&root).unwrap();
        inventory::validate(&inventory).unwrap();
        let matrix = matrix::build(&root, &inventory).unwrap();
        let audit = GapAudit::load(&root.join(PATH)).unwrap();
        (root, inventory, matrix, audit)
    }

    fn implemented_index(audit: &GapAudit) -> usize {
        audit
            .entries
            .iter()
            .position(|entry| entry.outcome == "implemented-without-trace")
            .unwrap()
    }

    fn not_applicable_index(audit: &GapAudit) -> usize {
        audit
            .entries
            .iter()
            .position(|entry| entry.outcome == "not-applicable")
            .unwrap()
    }

    #[test]
    fn repository_audit_classifies_every_open_requirement() {
        let (root, inventory, matrix, audit) = repository_evidence();
        audit.validate(&root, &matrix, &inventory).unwrap();
        assert_eq!(
            audit.summary.total,
            u64::try_from(audit.entries.len()).unwrap()
        );
        assert_eq!(
            audit.summary.by_outcome.values().sum::<u64>(),
            audit.summary.total
        );
        assert_eq!(
            audit
                .entries
                .iter()
                .filter(|entry| entry.outcome == "not-applicable")
                .map(|entry| entry.requirement.as_str())
                .collect::<Vec<_>>(),
            ["TL01-2-2-R001", "TT01-13-R001"]
        );
        assert_eq!(
            audit
                .entries
                .iter()
                .filter(|entry| entry.outcome == "absent")
                .map(|entry| entry.requirement.as_str())
                .collect::<Vec<_>>(),
            [
                "TC01-10-1-4-R001",
                "TL01-10-18-R001",
                "TL01-11-10-R002",
                "TL01-11-10-R003",
                "TL01-11-10-R004",
                "TL01-11-11-R001",
                "TL01-11-12-2-R001",
                "TL01-11-12-2-R002",
                "TL01-12-3-R002",
                "TL01-16-14-R005",
                "TL01-23-12-R001",
                "TL01-23-18-R001",
                "TL01-23-19-R002",
                "TL01-ASYNC-Y-CONCURRENCIA-ESTRUCTURADA-R001",
            ]
        );
    }

    #[test]
    fn audit_identity_and_requirement_set_fail_closed() {
        let (root, inventory, matrix, audit) = repository_evidence();
        let mut invalid = audit.clone();
        invalid.format = "tondo-normative-gap-audit/0".into();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.documents[0].sha256 = "0".repeat(64);
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries.pop();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries.swap(0, 1);
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());
    }

    #[test]
    fn implemented_entries_require_current_paths_and_executable_tests() {
        let (root, inventory, matrix, audit) = repository_evidence();
        let index = implemented_index(&audit);

        let mut invalid = audit.clone();
        invalid.entries[index].text_sha256 = "0".repeat(64);
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[index].reason.clear();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[index].implementation.clear();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[index].implementation[0] = "../outside.rs".into();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[index].tests[0] = "unknown:test".into();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        let duplicate = invalid.entries[index].implementation[0].clone();
        invalid.entries[index].implementation.push(duplicate);
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        let duplicate = invalid.entries[index].tests[0].clone();
        invalid.entries[index].tests.push(duplicate);
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[index].follow_up = Some("CONF-GAP-IMPL-001:wrong".into());
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());
    }

    #[test]
    fn every_outcome_has_one_unambiguous_shape() {
        let (root, inventory, mut matrix, audit) = repository_evidence();
        let implemented = implemented_index(&audit);
        let not_applicable = not_applicable_index(&audit);

        let mut invalid = audit.clone();
        invalid.entries[not_applicable].tests = audit.entries[implemented].tests.clone();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut absent = audit.clone();
        let requirement = absent.entries[implemented].requirement.clone();
        absent.entries[implemented].outcome = "absent".into();
        absent.entries[implemented].implementation.clear();
        absent.entries[implemented].tests.clear();
        absent.entries[implemented].follow_up = Some(format!("CONF-GAP-IMPL-001:{requirement}"));
        *absent
            .summary
            .by_outcome
            .entry("absent".into())
            .or_insert(0) += 1;
        *absent
            .summary
            .by_outcome
            .get_mut("implemented-without-trace")
            .unwrap() -= 1;
        matrix
            .requirements
            .iter_mut()
            .find(|candidate| candidate.id == requirement)
            .unwrap()
            .status = "toolchain-limit".into();
        absent.validate(&root, &matrix, &inventory).unwrap();

        let mut invalid = absent.clone();
        invalid.entries[implemented].follow_up = Some("CONF-GAP-IMPL-001:wrong".into());
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.entries[implemented].outcome = "maybe".into();
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());

        let mut invalid = audit.clone();
        invalid.summary.total -= 1;
        assert!(invalid.validate(&root, &matrix, &inventory).is_err());
    }

    #[test]
    fn reviewed_entries_remain_valid_after_their_matrix_status_resolves() {
        let (root, inventory, mut matrix, audit) = repository_evidence();
        let implemented = implemented_index(&audit);
        let not_applicable = not_applicable_index(&audit);
        let implemented_id = &audit.entries[implemented].requirement;
        let not_applicable_id = &audit.entries[not_applicable].requirement;

        matrix
            .requirements
            .iter_mut()
            .find(|requirement| requirement.id == *implemented_id)
            .unwrap()
            .status = "covered".into();
        matrix
            .requirements
            .iter_mut()
            .find(|requirement| requirement.id == *not_applicable_id)
            .unwrap()
            .status = "target-not-applicable".into();
        audit.validate(&root, &matrix, &inventory).unwrap();

        matrix
            .requirements
            .iter_mut()
            .find(|requirement| requirement.id == *implemented_id)
            .unwrap()
            .status = "stdlib-pending".into();
        assert!(audit.validate(&root, &matrix, &inventory).is_err());
    }
}
