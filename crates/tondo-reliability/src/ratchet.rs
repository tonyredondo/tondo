//! Incremental evidence gate shared by every draft-conformance wave.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tondo_conformance::lineage::{DRAFT_LINEAGE_NAME, DRAFT_LINEAGE_PATH, DraftLineage};

use crate::inventory;
use crate::matrix;
use crate::quality::{QualityBaseline, parse_llvm_cov, parse_mutation_report};
use crate::{MATRIX_PATH, QUALITY_BASELINE_PATH, canonical_json, check_bytes, sha256};

pub const FORMAT: &str = "tondo-conformance-ratchet/1";
pub const PATH: &str = "testing/conformance-ratchet.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatchetRecord {
    pub format: String,
    pub lineage: String,
    pub revision: u32,
    pub manifest: EvidenceFile,
    pub inventory: EvidenceFile,
    pub matrix: EvidenceFile,
    pub quality_baseline: EvidenceFile,
    pub draft_case_layers: u64,
    pub pending_tasks: Vec<String>,
    pub coverage: ScopeEvidence,
    pub mutation: ScopeEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeEvidence {
    pub status: String,
    pub reason: String,
    pub report_sha256: Option<String>,
}

pub fn build(
    root: &Path,
    coverage_path: Option<&Path>,
    mutants_path: Option<&Path>,
) -> Result<RatchetRecord, String> {
    let lineage = DraftLineage::load(root, Path::new(DRAFT_LINEAGE_PATH))
        .map_err(|error| error.to_string())?;
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    let inventory_ids = inventory
        .tests
        .iter()
        .map(|test| test.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for layer in lineage.case_layers() {
        for case in &layer.cases {
            for evidence in &case.evidence {
                if !inventory_ids.contains(evidence.as_str()) {
                    return Err(format!(
                        "draft case `{}` references unknown inventory evidence `{evidence}`",
                        case.id
                    ));
                }
            }
        }
    }
    check_bytes(
        &root.join(crate::INVENTORY_PATH),
        &canonical_json(&inventory)?,
    )?;
    let matrix = matrix::build(root, &inventory)?;
    check_bytes(&root.join(MATRIX_PATH), &canonical_json(&matrix)?)?;
    let quality_path = root.join(QUALITY_BASELINE_PATH);
    QualityBaseline::load(&quality_path)?;

    let requires_reports = !lineage.manifest().case_layers.is_empty();
    let coverage = scope_evidence(
        root,
        "coverage",
        coverage_path,
        requires_reports,
        |bytes, baseline| {
            let report = parse_llvm_cov(bytes)?;
            baseline.verify_coverage_report(&report)
        },
    )?;
    let mutation = scope_evidence(
        root,
        "mutation",
        mutants_path,
        requires_reports,
        |bytes, baseline| {
            let report = parse_mutation_report(bytes)?;
            baseline.verify_mutation_report(&report)
        },
    )?;

    let record = RatchetRecord {
        format: FORMAT.into(),
        lineage: lineage.manifest().lineage.clone(),
        revision: lineage.manifest().revision,
        manifest: EvidenceFile {
            path: DRAFT_LINEAGE_PATH.into(),
            sha256: lineage.manifest_sha256(),
        },
        inventory: file_evidence(root, crate::INVENTORY_PATH)?,
        matrix: file_evidence(root, MATRIX_PATH)?,
        quality_baseline: file_evidence(root, QUALITY_BASELINE_PATH)?,
        draft_case_layers: lineage.manifest().case_layers.len() as u64,
        pending_tasks: lineage.manifest().pending_tasks.clone(),
        coverage,
        mutation,
    };
    validate(&record)?;
    Ok(record)
}

pub fn validate(record: &RatchetRecord) -> Result<(), String> {
    if record.format != FORMAT || record.lineage != DRAFT_LINEAGE_NAME || record.revision == 0 {
        return Err("ratchet record has an unsupported format, lineage, or revision".into());
    }
    for evidence in [
        (&record.manifest, DRAFT_LINEAGE_PATH),
        (&record.inventory, crate::INVENTORY_PATH),
        (&record.matrix, MATRIX_PATH),
        (&record.quality_baseline, QUALITY_BASELINE_PATH),
    ] {
        if evidence.0.path != evidence.1 || !is_sha256(&evidence.0.sha256) {
            return Err(format!(
                "ratchet evidence path or hash is invalid for {}",
                evidence.1
            ));
        }
    }
    if record
        .pending_tasks
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err("ratchet pending tasks must be sorted and unique".into());
    }
    if record.coverage.status == "not-applicable" && record.coverage.report_sha256.is_some() {
        return Err("not-applicable coverage cannot carry a report hash".into());
    }
    if record.mutation.status == "not-applicable" && record.mutation.report_sha256.is_some() {
        return Err("not-applicable mutation cannot carry a report hash".into());
    }
    for (name, scope) in [
        ("coverage", &record.coverage),
        ("mutation", &record.mutation),
    ] {
        if !matches!(scope.status.as_str(), "not-applicable" | "validated")
            || scope.reason.is_empty()
            || (scope.status == "validated"
                && !scope.report_sha256.as_deref().is_some_and(is_sha256))
        {
            return Err(format!("ratchet {name} scope evidence is incomplete"));
        }
    }
    Ok(())
}

fn scope_evidence(
    root: &Path,
    name: &str,
    report_path: Option<&Path>,
    required: bool,
    verify: impl FnOnce(&[u8], &QualityBaseline) -> Result<(), String>,
) -> Result<ScopeEvidence, String> {
    let Some(path) = report_path else {
        if required {
            return Err(format!(
                "{name} report is required when draft case layers exist"
            ));
        }
        return Ok(ScopeEvidence {
            status: "not-applicable".into(),
            reason: format!("no executable draft case layer requires a {name} report"),
            report_sha256: None,
        });
    };
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let baseline = QualityBaseline::load(&root.join(QUALITY_BASELINE_PATH))
        .map_err(|error| format!("cannot load quality baseline for {name}: {error}"))?;
    verify(&bytes, &baseline)
        .map_err(|error| format!("{name} report failed the quality gate: {error}"))?;
    Ok(ScopeEvidence {
        status: "validated".into(),
        reason: format!("the supplied {name} report passed the quality non-regression gate"),
        report_sha256: Some(sha256(&bytes)),
    })
}

fn file_evidence(root: &Path, path: &str) -> Result<EvidenceFile, String> {
    let bytes =
        fs::read(root.join(path)).map_err(|error| format!("cannot read {path}: {error}"))?;
    Ok(EvidenceFile {
        path: path.into(),
        sha256: sha256(&bytes),
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn repository_ratchet_requires_reports_for_draft_case_layers() {
        let root = repository_root();
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        assert_eq!(lineage.manifest().case_layers.len(), 2);
        assert!(scope_evidence(&root, "coverage", None, true, |_, _| Ok(())).is_err());
        assert!(scope_evidence(&root, "mutation", None, true, |_, _| Ok(())).is_err());
    }

    fn record() -> RatchetRecord {
        let evidence = |path: &str| EvidenceFile {
            path: path.into(),
            sha256: "a".repeat(64),
        };
        let scope = ScopeEvidence {
            status: "validated".into(),
            reason: "verified".into(),
            report_sha256: Some("b".repeat(64)),
        };
        RatchetRecord {
            format: FORMAT.into(),
            lineage: DRAFT_LINEAGE_NAME.into(),
            revision: 3,
            manifest: evidence(DRAFT_LINEAGE_PATH),
            inventory: evidence(crate::INVENTORY_PATH),
            matrix: evidence(crate::MATRIX_PATH),
            quality_baseline: evidence(crate::QUALITY_BASELINE_PATH),
            draft_case_layers: 2,
            pending_tasks: vec!["M10.6".into()],
            coverage: scope.clone(),
            mutation: scope,
        }
    }

    #[test]
    fn validated_scope_requires_a_report_hash() {
        let mut record = record();
        record.coverage.report_sha256 = None;
        assert!(validate(&record).is_err());
    }

    #[test]
    fn ratchet_validation_rejects_each_identity_and_scope_boundary() {
        let base = record();

        let mut record = base.clone();
        record.format = "future-ratchet".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.lineage = "other-lineage".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.revision = 0;
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.manifest.path = "wrong.json".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.inventory.sha256 = "bad".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.pending_tasks = vec!["B".into(), "A".into()];
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.coverage.status = "not-applicable".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.mutation.status = "not-applicable".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.coverage.status = "future".into();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.coverage.reason.clear();
        assert!(validate(&record).is_err());

        let mut record = base.clone();
        record.coverage.status = "validated".into();
        record.coverage.report_sha256 = Some("bad".into());
        assert!(validate(&record).is_err());

        let mut record = base;
        record.mutation.status = "validated".into();
        record.mutation.report_sha256 = Some("bad".into());
        assert!(validate(&record).is_err());
    }

    #[test]
    fn scope_evidence_distinguishes_required_and_optional_reports() {
        let root = repository_root();
        assert!(scope_evidence(&root, "coverage", None, true, |_, _| Ok(())).is_err());
        let evidence = scope_evidence(&root, "coverage", None, false, |_, _| Ok(())).unwrap();
        assert_eq!(evidence.status, "not-applicable");
        assert!(evidence.report_sha256.is_none());

        let report =
            std::env::temp_dir().join(format!("tondo-ratchet-report-{}", std::process::id()));
        std::fs::write(&report, b"validated report").unwrap();
        let evidence = scope_evidence(&root, "coverage", Some(&report), false, |bytes, _| {
            assert_eq!(bytes, b"validated report");
            Ok(())
        })
        .unwrap();
        assert_eq!(evidence.status, "validated");
        assert_eq!(evidence.report_sha256, Some(sha256(b"validated report")));
        std::fs::remove_file(report).unwrap();
    }
}
