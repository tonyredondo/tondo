//! Atomic, content-addressed proof that the draft promotion mechanism is closed.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lineage::{
    DRAFT_LINEAGE_NAME, DraftCaseLayerManifest, DraftLineage, DraftLineageManifest,
    validate_active_manifest, validate_case_layer, validate_manifest as validate_lineage_manifest,
};
use crate::manifest::{
    CaseGroup, PinnedFile, SuiteManifest, referenced_files, validate_embedded_suite,
};
use crate::runner::COMPOSED_RESULT_FORMAT;
use crate::{ADAPTER_PROTOCOL, SUITE_NAME, sha256};

pub const PROMOTION_PROOF_FORMAT: &str = "tondo-conformance-promotion-proof/1";
pub const PROMOTION_PROOF_STATE: &str = "promotion-proof";
pub const DEFAULT_PROOF_DIRECTORY: &str = "conformance/proofs";
pub const RATCHET_PATH: &str = "testing/conformance-ratchet.json";

const RATCHET_FORMAT: &str = "tondo-conformance-ratchet/2";
const AUDITED_GAP_SCOPE_SHA256: &str =
    "f28c16dd4b7cc1effeffbfb3238fd1f78c140b2403b1bdb3fee21132dd296bed";
const ADAPTER_PACKAGE: &str = "tondo-reference-adapter";
const ADAPTER_SOURCES: [&str; 10] = [
    "crates/tondo-reference-adapter/Cargo.toml",
    "crates/tondo-reference-adapter/src/determinism.rs",
    "crates/tondo-reference-adapter/src/document.rs",
    "crates/tondo-reference-adapter/src/lib.rs",
    "crates/tondo-reference-adapter/src/main.rs",
    "crates/tondo-reference-adapter/src/maintain.rs",
    "crates/tondo-reference-adapter/src/semantic.rs",
    "crates/tondo-reference-adapter/tests/conformance.rs",
    "crates/tondo-reference-adapter/tests/process_protocol.rs",
    "crates/tondo-reference-adapter/tests/semantic_contracts.rs",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionProofManifest {
    pub format: String,
    pub edition: String,
    pub state: String,
    pub lineage: ProofLineage,
    pub target: ProofTarget,
    pub adapter: ProofAdapter,
    pub files: Vec<ProofFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofLineage {
    pub name: String,
    pub revision: u32,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofTarget {
    pub name: String,
    pub profile: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofAdapter {
    pub package: String,
    pub protocol: String,
    pub implementation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofFile {
    pub role: String,
    pub source_path: String,
    pub object: PinnedFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Debug)]
pub enum SealError {
    Io { path: PathBuf, message: String },
    Json(String),
    Invalid(String),
}

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "cannot access `{}`: {message}", path.display())
            }
            Self::Json(message) => write!(formatter, "invalid promotion-proof JSON: {message}"),
            Self::Invalid(message) => {
                write!(formatter, "invalid conformance promotion proof: {message}")
            }
        }
    }
}

impl Error for SealError {}

#[derive(Debug, Clone)]
struct PromotionProofBundle {
    manifest: PromotionProofManifest,
    manifest_bytes: Vec<u8>,
    objects: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RatchetRecord {
    format: String,
    lineage: String,
    revision: u32,
    manifest: PinnedFile,
    inventory: PinnedFile,
    matrix: PinnedFile,
    gap_audit: PinnedFile,
    quality_baseline: PinnedFile,
    draft_case_layers: u64,
    pending_tasks: Vec<String>,
    coverage: ScopeEvidence,
    mutation: ScopeEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeEvidence {
    status: String,
    reason: String,
    report_sha256: Option<String>,
    provenance_sha256: Option<String>,
    tree_sha256: Option<String>,
    input_set_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRecord {
    format: String,
    suite: String,
    suite_version: String,
    edition: String,
    manifest_sha256: String,
    adapter: ResultAdapter,
    target: ProofTarget,
    passed: bool,
    cases: Vec<ResultCase>,
    lineage: String,
    revision: u32,
    lineage_manifest_sha256: String,
    inventory_sha256: String,
    tree_sha256: String,
    input_set_sha256: String,
    case_layers: Vec<ResultLayer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultAdapter {
    adapter_protocol: String,
    backend: String,
    compiler: String,
    compiler_version: String,
    implementation: String,
    language_edition: String,
    targets: Vec<ProofTarget>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultCase {
    id: String,
    group: CaseGroup,
    repetitions: u32,
    observation_sha256: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultLayer {
    id: String,
    manifest_sha256: String,
    cases: Vec<ResultLayerCase>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultLayerCase {
    id: String,
    evidence: Vec<ResultLayerEvidence>,
    observation_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultLayerEvidence {
    id: String,
    source_sha256: String,
    observation_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CoverageMatrixRecord {
    format: String,
    edition: String,
    target: String,
    documents: Vec<SpecificationIdentity>,
    summary: CoverageMatrixSummary,
    requirements: Vec<CoverageRequirement>,
}

#[derive(Debug, Deserialize)]
struct CoverageMatrixSummary {
    total: u64,
    by_status: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct CoverageRequirement {
    id: String,
    text_sha256: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GapAuditRecord {
    format: String,
    edition: String,
    target: String,
    documents: Vec<SpecificationIdentity>,
    summary: GapAuditSummary,
    entries: Vec<GapAuditEntry>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecificationIdentity {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GapAuditSummary {
    total: u64,
    by_outcome: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GapAuditEntry {
    requirement: String,
    text_sha256: String,
    outcome: String,
    reason: String,
    implementation: Vec<String>,
    tests: Vec<String>,
    follow_up: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InventoryRecord {
    tests: Vec<InventoryTest>,
}

#[derive(Debug, Deserialize)]
struct InventoryTest {
    id: String,
    status: String,
    source_sha256: String,
}

/// Builds and atomically publishes a self-contained, content-addressed proof of
/// the promotion mechanism. This deliberately does not claim final normative
/// conformance; that requires the complete draft-layer result defined by G5.
/// Existing identical proofs are accepted and differing destinations fail closed.
pub fn seal_promotion_proof(
    lineage: &DraftLineage,
    result_path: &Path,
    output_directory: &Path,
) -> Result<ProofOutcome, SealError> {
    lineage
        .check_sealable()
        .map_err(|error| SealError::Invalid(error.to_string()))?;
    let bundle = build_promotion_proof(lineage, result_path)?;
    let outcome = publish_bundle(lineage.root(), output_directory, &bundle)?;
    let verified = verify_promotion_proof(lineage.root(), &output_directory.join("manifest.json"))?;
    if verified != bundle.manifest {
        return invalid("published promotion proof differs from its input");
    }
    Ok(outcome)
}

/// Verifies an already published promotion proof using only its immutable object
/// closure. Live draft files are deliberately not consulted.
pub fn verify_promotion_proof(
    root: &Path,
    manifest_path: &Path,
) -> Result<PromotionProofManifest, SealError> {
    require_relative_path(manifest_path, "promotion-proof manifest path")?;
    if manifest_path.file_name().and_then(|name| name.to_str()) != Some("manifest.json") {
        return invalid("promotion-proof manifest must be named `manifest.json`");
    }
    let relative_directory = manifest_path.parent().ok_or_else(|| {
        SealError::Invalid("promotion-proof manifest has no parent directory".into())
    })?;
    let directory = resolve_directory(root, relative_directory, false)?;
    let absolute = directory.join("manifest.json");
    let bytes = read_regular(&absolute)?;
    let manifest: PromotionProofManifest =
        serde_json::from_slice(&bytes).map_err(|error| SealError::Json(error.to_string()))?;
    validate_promotion_proof_manifest(&manifest)?;
    if canonical_json(&manifest)? != bytes {
        return invalid("promotion-proof manifest is not canonical pretty JSON");
    }
    let mut objects = BTreeMap::new();
    for file in &manifest.files {
        let path = directory.join(&file.object.path);
        let object = read_regular(&path)?;
        let actual = sha256(&object);
        if actual != file.object.sha256 {
            return invalid(format!(
                "object `{}` has SHA-256 `{actual}`, expected `{}`",
                file.object.path, file.object.sha256
            ));
        }
        objects.insert(file.source_path.clone(), object);
    }
    verify_promotion_proof_directory(&directory, &manifest)?;
    verify_promotion_proof_closure(&manifest, &objects)?;
    Ok(manifest)
}

fn build_promotion_proof(
    lineage: &DraftLineage,
    result_path: &Path,
) -> Result<PromotionProofBundle, SealError> {
    let root = lineage.root();
    let ratchet_bytes = read_regular(&root.join(RATCHET_PATH))?;
    let ratchet: RatchetRecord = serde_json::from_slice(&ratchet_bytes)
        .map_err(|error| SealError::Json(error.to_string()))?;
    validate_ratchet(lineage, &ratchet, root)?;

    let result_bytes = read_regular(result_path)?;
    let result = parse_result(&result_bytes)?;
    validate_result(lineage, &result)?;
    if ratchet.coverage.tree_sha256.as_deref() != Some(result.tree_sha256.as_str())
        || ratchet.coverage.input_set_sha256.as_deref() != Some(result.input_set_sha256.as_str())
    {
        return invalid("composed result was produced from a different quality source tree");
    }

    let mut files = Vec::new();
    let mut objects = BTreeMap::new();
    add_source(
        &mut files,
        &mut objects,
        "draft-manifest",
        lineage.manifest_path().to_string_lossy().as_ref(),
        read_regular(&root.join(lineage.manifest_path()))?,
    )?;
    add_history_sources(&mut files, &mut objects, root, lineage.manifest())?;
    for specification in &lineage.manifest().specifications {
        add_source(
            &mut files,
            &mut objects,
            "specification",
            &specification.path,
            read_regular(&root.join(&specification.path))?,
        )?;
    }
    for layer in &lineage.manifest().case_layers {
        add_source(
            &mut files,
            &mut objects,
            "case-layer",
            &layer.manifest.path,
            read_regular(&root.join(&layer.manifest.path))?,
        )?;
    }
    add_source(
        &mut files,
        &mut objects,
        "regression-manifest",
        &lineage.manifest().baseline.manifest.path,
        read_regular(&root.join(&lineage.manifest().baseline.manifest.path))?,
    )?;
    add_source(
        &mut files,
        &mut objects,
        "regression-specification",
        &lineage.manifest().baseline.specification_snapshot.path,
        read_regular(&root.join(&lineage.manifest().baseline.specification_snapshot.path))?,
    )?;
    let baseline = lineage.baseline_suite();
    let referenced = referenced_files(baseline.manifest())
        .into_iter()
        .map(|pinned| (pinned.path.clone(), pinned))
        .collect::<BTreeMap<_, _>>();
    for pinned in referenced.values() {
        if pinned.path == baseline.manifest().specification.path {
            continue;
        }
        add_source(
            &mut files,
            &mut objects,
            "regression-input",
            &pinned.path,
            baseline.file(pinned).to_vec(),
        )?;
    }
    add_source(
        &mut files,
        &mut objects,
        "conformance-result",
        "generated:tondo-reference-result",
        result_bytes,
    )?;
    add_source(
        &mut files,
        &mut objects,
        "ratchet",
        RATCHET_PATH,
        ratchet_bytes,
    )?;
    for evidence in [
        &ratchet.inventory,
        &ratchet.matrix,
        &ratchet.gap_audit,
        &ratchet.quality_baseline,
    ] {
        add_source(
            &mut files,
            &mut objects,
            "quality-evidence",
            &evidence.path,
            read_regular(&root.join(&evidence.path))?,
        )?;
    }
    for path in ADAPTER_SOURCES {
        add_source(
            &mut files,
            &mut objects,
            "adapter-source",
            path,
            read_regular(&root.join(path))?,
        )?;
    }
    files.sort_by(|left, right| {
        left.role
            .cmp(&right.role)
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    let manifest = PromotionProofManifest {
        format: PROMOTION_PROOF_FORMAT.into(),
        edition: "0.1".into(),
        state: PROMOTION_PROOF_STATE.into(),
        lineage: ProofLineage {
            name: lineage.manifest().lineage.clone(),
            revision: lineage.manifest().revision,
            manifest_sha256: lineage.manifest_sha256(),
        },
        target: result.target,
        adapter: ProofAdapter {
            package: ADAPTER_PACKAGE.into(),
            protocol: result.adapter.adapter_protocol,
            implementation: result.adapter.implementation,
        },
        files,
    };
    validate_promotion_proof_manifest(&manifest)?;
    Ok(PromotionProofBundle {
        manifest_bytes: canonical_json(&manifest)?,
        manifest,
        objects,
    })
}

fn add_history_sources(
    files: &mut Vec<ProofFile>,
    objects: &mut BTreeMap<String, Vec<u8>>,
    root: &Path,
    manifest: &DraftLineageManifest,
) -> Result<(), SealError> {
    let mut child = manifest.clone();
    while let Some(parent) = child.parent.clone() {
        let bytes = read_regular(&root.join(&parent.path))?;
        add_source(files, objects, "draft-history", &parent.path, bytes.clone())?;
        child =
            serde_json::from_slice(&bytes).map_err(|error| SealError::Json(error.to_string()))?;
    }
    Ok(())
}

fn validate_ratchet(
    lineage: &DraftLineage,
    ratchet: &RatchetRecord,
    root: &Path,
) -> Result<(), SealError> {
    if ratchet.format != RATCHET_FORMAT
        || ratchet.lineage != lineage.manifest().lineage
        || ratchet.revision != lineage.manifest().revision
        || ratchet.manifest.path != lineage.manifest_path().to_string_lossy()
        || ratchet.manifest.sha256 != lineage.manifest_sha256()
        || ratchet.draft_case_layers != lineage.manifest().case_layers.len() as u64
        || !ratchet.pending_tasks.is_empty()
    {
        return invalid("ratchet does not describe the exact completed draft");
    }
    for (name, scope) in [
        ("coverage", &ratchet.coverage),
        ("mutation", &ratchet.mutation),
    ] {
        if scope.status != "validated"
            || scope.reason.is_empty()
            || !scope.report_sha256.as_deref().is_some_and(is_sha256)
            || !scope.provenance_sha256.as_deref().is_some_and(is_sha256)
            || !scope.tree_sha256.as_deref().is_some_and(is_sha256)
            || !scope.input_set_sha256.as_deref().is_some_and(is_sha256)
        {
            return invalid(format!("ratchet {name} evidence is not validated"));
        }
    }
    for evidence in [
        &ratchet.inventory,
        &ratchet.matrix,
        &ratchet.gap_audit,
        &ratchet.quality_baseline,
    ] {
        require_relative_path(Path::new(&evidence.path), "ratchet evidence path")?;
        let actual = sha256(&read_regular(&root.join(&evidence.path))?);
        if actual != evidence.sha256 {
            return invalid(format!(
                "ratchet evidence `{}` has SHA-256 `{actual}`, expected `{}`",
                evidence.path, evidence.sha256
            ));
        }
    }
    let matrix = read_regular(&root.join(&ratchet.matrix.path))?;
    validate_coverage_matrix(&matrix)?;
    validate_gap_audit(
        &matrix,
        &read_regular(&root.join(&ratchet.gap_audit.path))?,
        &read_regular(&root.join(&ratchet.inventory.path))?,
    )?;
    if ratchet.coverage.tree_sha256 != ratchet.mutation.tree_sha256
        || ratchet.coverage.input_set_sha256 != ratchet.mutation.input_set_sha256
    {
        return invalid("ratchet quality scopes describe different source trees");
    }
    Ok(())
}

fn validate_coverage_matrix(bytes: &[u8]) -> Result<(), SealError> {
    let matrix: CoverageMatrixRecord =
        serde_json::from_slice(bytes).map_err(|error| SealError::Json(error.to_string()))?;
    if matrix.format != "tondo-normative-coverage/2" {
        return invalid("coverage matrix uses an unsupported format");
    }
    if matrix.summary.total != matrix.requirements.len() as u64 {
        return invalid("coverage matrix summary total is inconsistent");
    }
    let mut by_status = BTreeMap::new();
    for requirement in &matrix.requirements {
        *by_status.entry(requirement.status.clone()).or_insert(0) += 1;
    }
    if matrix.summary.by_status != by_status {
        return invalid("coverage matrix status summary is inconsistent");
    }
    if by_status
        .get("draft-pending")
        .is_some_and(|count| *count != 0)
    {
        return invalid("coverage matrix still contains draft-pending requirements");
    }
    Ok(())
}

fn validate_gap_audit(
    matrix_bytes: &[u8],
    audit_bytes: &[u8],
    inventory_bytes: &[u8],
) -> Result<(), SealError> {
    let matrix: CoverageMatrixRecord =
        serde_json::from_slice(matrix_bytes).map_err(|error| SealError::Json(error.to_string()))?;
    let audit: GapAuditRecord =
        serde_json::from_slice(audit_bytes).map_err(|error| SealError::Json(error.to_string()))?;
    let inventory: InventoryRecord = serde_json::from_slice(inventory_bytes)
        .map_err(|error| SealError::Json(error.to_string()))?;
    if audit.format != "tondo-normative-gap-audit/1"
        || audit.edition != matrix.edition
        || audit.target != matrix.target
        || audit.documents != matrix.documents
    {
        return invalid("normative gap audit differs from the coverage matrix identity");
    }

    let requirements = matrix
        .requirements
        .iter()
        .map(|requirement| (requirement.id.as_str(), requirement))
        .collect::<BTreeMap<_, _>>();
    if audit
        .entries
        .windows(2)
        .any(|pair| pair[0].requirement >= pair[1].requirement)
    {
        return invalid("normative gap audit entries are not globally sorted and unique");
    }
    let mut audited_scope = String::new();
    for entry in &audit.entries {
        audited_scope.push_str(&entry.requirement);
        audited_scope.push('\t');
        audited_scope.push_str(&entry.text_sha256);
        audited_scope.push('\n');
    }
    if sha256(audited_scope.as_bytes()) != AUDITED_GAP_SCOPE_SHA256 {
        return invalid("normative gap audit differs from its reviewed requirement set");
    }
    let executable = inventory
        .tests
        .iter()
        .filter(|test| test.status == "executable")
        .map(|test| test.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut outcomes = BTreeMap::new();
    let mut observed = BTreeSet::new();
    for entry in &audit.entries {
        let requirement = requirements
            .get(entry.requirement.as_str())
            .ok_or_else(|| {
                SealError::Invalid(format!(
                    "gap audit entry `{}` is not a current requirement",
                    entry.requirement
                ))
            })?;
        if !observed.insert(entry.requirement.as_str())
            || entry.text_sha256 != requirement.text_sha256
            || entry.reason.trim().is_empty()
            || !entry.reason.contains(&entry.requirement)
        {
            return invalid(format!(
                "gap audit entry `{}` has stale or incomplete identity",
                entry.requirement
            ));
        }
        match entry.outcome.as_str() {
            "implemented-without-trace" => {
                if !matches!(
                    requirement.status.as_str(),
                    "toolchain-limit" | "draft-pending" | "covered"
                ) || entry.implementation.is_empty()
                    || entry.tests.is_empty()
                    || entry.follow_up.is_some()
                    || entry
                        .implementation
                        .windows(2)
                        .any(|pair| pair[0] >= pair[1])
                    || entry.tests.windows(2).any(|pair| pair[0] >= pair[1])
                    || entry.implementation.iter().any(|path| {
                        path.is_empty()
                            || Path::new(path).is_absolute()
                            || Path::new(path)
                                .components()
                                .any(|component| component == Component::ParentDir)
                    })
                    || entry
                        .tests
                        .iter()
                        .any(|test| !executable.contains(test.as_str()))
                {
                    return invalid(format!(
                        "implemented audit entry `{}` lacks closed evidence",
                        entry.requirement
                    ));
                }
            }
            "not-applicable" => {
                if !matches!(
                    requirement.status.as_str(),
                    "toolchain-limit" | "draft-pending" | "target-not-applicable"
                ) || !entry.implementation.is_empty()
                    || !entry.tests.is_empty()
                    || entry.follow_up.is_some()
                {
                    return invalid(format!(
                        "not-applicable audit entry `{}` carries evidence",
                        entry.requirement
                    ));
                }
            }
            "absent" => {
                let expected = format!("CONF-GAP-IMPL-001:{}", entry.requirement);
                if !matches!(
                    requirement.status.as_str(),
                    "toolchain-limit" | "draft-pending"
                ) || !entry.implementation.is_empty()
                    || !entry.tests.is_empty()
                    || entry.follow_up.as_deref() != Some(expected.as_str())
                {
                    return invalid(format!(
                        "absent audit entry `{}` lacks its exact leaf follow-up",
                        entry.requirement
                    ));
                }
            }
            _ => {
                return invalid(format!(
                    "gap audit entry `{}` has an unknown outcome",
                    entry.requirement
                ));
            }
        }
        *outcomes.entry(entry.outcome.clone()).or_insert(0) += 1;
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
    if !open.is_subset(&observed)
        || audit.summary.total != audit.entries.len() as u64
        || audit.summary.by_outcome != outcomes
    {
        return invalid("normative gap audit summary or requirement closure is inconsistent");
    }
    Ok(())
}

fn parse_result(bytes: &[u8]) -> Result<ResultRecord, SealError> {
    serde_json::from_slice(bytes).map_err(|error| SealError::Json(error.to_string()))
}

fn validate_result(lineage: &DraftLineage, result: &ResultRecord) -> Result<(), SealError> {
    validate_result_contract(
        &lineage.manifest().edition,
        lineage.baseline_suite().manifest_sha256().as_str(),
        lineage.baseline_suite().manifest(),
        result,
    )?;
    let inventory = read_regular(&lineage.root().join("testing/inventory.json"))?;
    validate_layer_results(
        lineage.manifest(),
        lineage.case_layers(),
        &inventory,
        result,
    )
}

fn validate_result_contract(
    edition: &str,
    manifest_sha256: &str,
    suite: &SuiteManifest,
    result: &ResultRecord,
) -> Result<(), SealError> {
    if result.format != COMPOSED_RESULT_FORMAT
        || result.suite != SUITE_NAME
        || result.suite_version != suite.version
        || result.edition != edition
        || result.manifest_sha256 != manifest_sha256
        || !result.passed
        || result.adapter.adapter_protocol != ADAPTER_PROTOCOL
        || result.adapter.backend != "bytecode-vm"
        || result.adapter.compiler != "tondo-bootstrap/draft"
        || result.adapter.compiler_version != "draft"
        || result.adapter.implementation != "tondo-reference"
        || result.adapter.language_edition != edition
        || result.adapter.targets != [result.target.clone()]
        || result.target.name != "tondo-vm-hosted"
        || result.target.profile != "hosted"
        || result.target.capabilities != ["console", "process"]
        || result
            .target
            .capabilities
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return invalid(
            "conformance result does not match the draft regression and reference adapter",
        );
    }
    if result.cases.len() != suite.cases.len() {
        return invalid("conformance result does not contain the complete regression case set");
    }
    for (observed, expected) in result.cases.iter().zip(&suite.cases) {
        if observed.id != expected.id
            || observed.group != expected.group
            || observed.repetitions != expected.repeat
            || observed.observation_sha256.len() != expected.repeat as usize
            || observed
                .observation_sha256
                .iter()
                .any(|hash| !is_sha256(hash))
        {
            return invalid(format!(
                "conformance result case `{}` differs from the pinned regression manifest",
                expected.id
            ));
        }
    }
    Ok(())
}

fn validate_layer_results(
    draft: &DraftLineageManifest,
    layers: &[DraftCaseLayerManifest],
    inventory_bytes: &[u8],
    result: &ResultRecord,
) -> Result<(), SealError> {
    if result.lineage != draft.lineage
        || result.revision != draft.revision
        || result.lineage_manifest_sha256 != sha256(&canonical_json(draft)?)
        || result.inventory_sha256 != sha256(inventory_bytes)
        || !is_sha256(&result.tree_sha256)
        || !is_sha256(&result.input_set_sha256)
        || result.case_layers.len() != layers.len()
        || layers.len() != draft.case_layers.len()
    {
        return invalid("composed result does not match the active draft identity");
    }
    let inventory: InventoryRecord = serde_json::from_slice(inventory_bytes)
        .map_err(|error| SealError::Json(error.to_string()))?;
    let inventory = inventory
        .tests
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut expected_evidence = BTreeSet::new();
    for ((descriptor, layer), observed_layer) in draft
        .case_layers
        .iter()
        .zip(layers)
        .zip(&result.case_layers)
    {
        if observed_layer.id != layer.layer
            || observed_layer.id != descriptor.id
            || observed_layer.manifest_sha256 != descriptor.manifest.sha256
            || observed_layer.cases.len() != layer.cases.len()
        {
            return invalid(format!(
                "composed layer `{}` differs from the draft",
                descriptor.id
            ));
        }
        for (case, observed_case) in layer.cases.iter().zip(&observed_layer.cases) {
            if observed_case.id != case.id
                || observed_case.evidence.len() != case.evidence.len()
                || !is_sha256(&observed_case.observation_sha256)
            {
                return invalid(format!(
                    "composed layer case `{}/{}` differs from the draft",
                    layer.layer, case.id
                ));
            }
            for (id, observed) in case.evidence.iter().zip(&observed_case.evidence) {
                let entry = inventory.get(id.as_str()).ok_or_else(|| {
                    SealError::Invalid(format!("composed evidence `{id}` is not inventoried"))
                })?;
                if observed.id != *id
                    || observed.source_sha256 != entry.source_sha256
                    || entry.status != "executable"
                    || !is_sha256(&observed.observation_sha256)
                {
                    return invalid(format!(
                        "composed evidence `{id}` is not executable or exact"
                    ));
                }
                expected_evidence.insert(id.as_str());
            }
            let encoded = serde_json::to_vec(&observed_case.evidence)
                .map_err(|error| SealError::Json(error.to_string()))?;
            if observed_case.observation_sha256 != sha256(&encoded) {
                return invalid(format!(
                    "composed layer case `{}/{}` has a forged observation hash",
                    layer.layer, case.id
                ));
            }
        }
    }
    let observed_evidence = result
        .case_layers
        .iter()
        .flat_map(|layer| layer.cases.iter())
        .flat_map(|case| case.evidence.iter())
        .map(|evidence| evidence.id.as_str())
        .collect::<BTreeSet<_>>();
    if observed_evidence != expected_evidence {
        return invalid("composed result has missing, extra, or duplicated layer evidence");
    }
    Ok(())
}

fn add_source(
    files: &mut Vec<ProofFile>,
    objects: &mut BTreeMap<String, Vec<u8>>,
    role: &str,
    source_path: &str,
    bytes: Vec<u8>,
) -> Result<(), SealError> {
    if role.is_empty() || source_path.is_empty() {
        return invalid("promotion-proof file role and source path must be non-empty");
    }
    let digest = sha256(&bytes);
    if let Some(previous) = objects.insert(digest.clone(), bytes.clone())
        && previous != bytes
    {
        return invalid("two different objects produced the same SHA-256");
    }
    files.push(ProofFile {
        role: role.into(),
        source_path: source_path.into(),
        object: PinnedFile {
            path: format!("objects/{digest}"),
            sha256: digest,
        },
    });
    Ok(())
}

fn validate_promotion_proof_manifest(manifest: &PromotionProofManifest) -> Result<(), SealError> {
    if manifest.format != PROMOTION_PROOF_FORMAT
        || manifest.edition != "0.1"
        || manifest.state != PROMOTION_PROOF_STATE
        || manifest.lineage.name != DRAFT_LINEAGE_NAME
        || manifest.lineage.revision == 0
        || !is_sha256(&manifest.lineage.manifest_sha256)
        || manifest.target.name != "tondo-vm-hosted"
        || manifest.target.profile != "hosted"
        || manifest.target.capabilities != ["console", "process"]
        || manifest
            .target
            .capabilities
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || manifest.adapter.package != ADAPTER_PACKAGE
        || manifest.adapter.protocol != ADAPTER_PROTOCOL
        || manifest.adapter.implementation != "tondo-reference"
        || manifest.files.is_empty()
    {
        return invalid("promotion-proof identity, target, adapter, or file closure is invalid");
    }
    if manifest
        .files
        .windows(2)
        .any(|pair| (&pair[0].role, &pair[0].source_path) >= (&pair[1].role, &pair[1].source_path))
    {
        return invalid("promotion-proof files must be sorted and unique");
    }
    let mut sources = BTreeSet::new();
    for file in &manifest.files {
        if file.role.is_empty()
            || file.source_path.is_empty()
            || !sources.insert(file.source_path.as_str())
            || !is_sha256(&file.object.sha256)
            || file.object.path != format!("objects/{}", file.object.sha256)
        {
            return invalid("promotion-proof file provenance or object identity is invalid");
        }
    }
    Ok(())
}

fn verify_promotion_proof_closure(
    manifest: &PromotionProofManifest,
    objects: &BTreeMap<String, Vec<u8>>,
) -> Result<(), SealError> {
    let draft_path = one_role(manifest, "draft-manifest")?;
    let draft_bytes = objects[draft_path.source_path.as_str()].as_slice();
    let draft: DraftLineageManifest =
        serde_json::from_slice(draft_bytes).map_err(|error| SealError::Json(error.to_string()))?;
    validate_active_manifest(&draft).map_err(|error| SealError::Invalid(error.to_string()))?;
    if canonical_json(&draft)? != draft_bytes
        || draft.lineage != manifest.lineage.name
        || draft.edition != manifest.edition
        || draft.revision != manifest.lineage.revision
        || sha256(draft_bytes) != manifest.lineage.manifest_sha256
    {
        return invalid("embedded draft manifest differs from the sealed lineage identity");
    }
    let history_paths = validate_embedded_history(&draft, objects)?;

    let ratchet_file = one_role(manifest, "ratchet")?;
    let ratchet: RatchetRecord =
        serde_json::from_slice(&objects[ratchet_file.source_path.as_str()])
            .map_err(|error| SealError::Json(error.to_string()))?;
    if ratchet.lineage != draft.lineage
        || ratchet.revision != draft.revision
        || ratchet.manifest.sha256 != manifest.lineage.manifest_sha256
        || ratchet.draft_case_layers != draft.case_layers.len() as u64
        || !ratchet.pending_tasks.is_empty()
        || ratchet.coverage.status != "validated"
        || ratchet.mutation.status != "validated"
        || !ratchet
            .coverage
            .provenance_sha256
            .as_deref()
            .is_some_and(is_sha256)
        || !ratchet
            .mutation
            .provenance_sha256
            .as_deref()
            .is_some_and(is_sha256)
        || !ratchet
            .coverage
            .tree_sha256
            .as_deref()
            .is_some_and(is_sha256)
        || !ratchet
            .mutation
            .tree_sha256
            .as_deref()
            .is_some_and(is_sha256)
        || ratchet.coverage.tree_sha256 != ratchet.mutation.tree_sha256
        || ratchet.coverage.input_set_sha256 != ratchet.mutation.input_set_sha256
    {
        return invalid("embedded ratchet differs from the sealed draft");
    }
    for evidence in [
        &ratchet.inventory,
        &ratchet.matrix,
        &ratchet.gap_audit,
        &ratchet.quality_baseline,
    ] {
        let bytes = objects.get(evidence.path.as_str()).ok_or_else(|| {
            SealError::Invalid(format!("promotion proof omits `{}`", evidence.path))
        })?;
        if sha256(bytes) != evidence.sha256 {
            return invalid(format!(
                "promotion-proof evidence `{}` differs from the ratchet",
                evidence.path
            ));
        }
    }
    let matrix = objects
        .get(ratchet.matrix.path.as_str())
        .ok_or_else(|| SealError::Invalid("promotion proof omits its coverage matrix".into()))?;
    validate_coverage_matrix(matrix)?;
    validate_gap_audit(
        matrix,
        objects
            .get(ratchet.gap_audit.path.as_str())
            .ok_or_else(|| {
                SealError::Invalid("promotion proof omits its normative gap audit".into())
            })?,
        objects
            .get(ratchet.inventory.path.as_str())
            .ok_or_else(|| SealError::Invalid("promotion proof omits its test inventory".into()))?,
    )?;
    for specification in &draft.specifications {
        require_object(objects, specification)?;
    }
    let mut embedded_layers = Vec::with_capacity(draft.case_layers.len());
    for layer in &draft.case_layers {
        require_object(objects, &layer.manifest)?;
        let bytes = &objects[layer.manifest.path.as_str()];
        let embedded: DraftCaseLayerManifest =
            serde_json::from_slice(bytes).map_err(|error| SealError::Json(error.to_string()))?;
        validate_case_layer(layer, &embedded)
            .map_err(|error| SealError::Invalid(error.to_string()))?;
        if canonical_json(&embedded)? != *bytes {
            return invalid(format!(
                "embedded case layer `{}` is not canonical pretty JSON",
                layer.id
            ));
        }
        embedded_layers.push(embedded);
    }
    require_object(objects, &draft.baseline.manifest)?;
    require_object(objects, &draft.baseline.specification_snapshot)?;

    let result_file = one_role(manifest, "conformance-result")?;
    let result = parse_result(&objects[result_file.source_path.as_str()])?;
    let baseline_bytes = objects
        .get(draft.baseline.manifest.path.as_str())
        .ok_or_else(|| {
            SealError::Invalid("promotion proof omits its regression manifest".into())
        })?;
    let baseline: SuiteManifest = serde_json::from_slice(baseline_bytes)
        .map_err(|error| SealError::Json(error.to_string()))?;
    let mut baseline_pinned = BTreeMap::new();
    for pinned in referenced_files(&baseline) {
        if pinned.path == baseline.specification.path {
            let snapshot = objects
                .get(draft.baseline.specification_snapshot.path.as_str())
                .ok_or_else(|| {
                    SealError::Invalid("promotion proof omits its regression specification".into())
                })?;
            if sha256(snapshot) != pinned.sha256 {
                return invalid("regression specification differs from the suite manifest");
            }
            baseline_pinned.insert(pinned.path, snapshot.clone());
        } else {
            require_object(objects, &pinned)?;
            baseline_pinned.insert(pinned.path.clone(), objects[&pinned.path].clone());
        }
    }
    validate_embedded_suite(&baseline, baseline_bytes, &baseline_pinned)
        .map_err(|error| SealError::Invalid(error.to_string()))?;
    validate_result_contract(
        &draft.edition,
        &draft.baseline.manifest.sha256,
        &baseline,
        &result,
    )?;
    validate_layer_results(
        &draft,
        &embedded_layers,
        objects
            .get(ratchet.inventory.path.as_str())
            .ok_or_else(|| SealError::Invalid("promotion proof omits its test inventory".into()))?,
        &result,
    )?;
    if ratchet.coverage.tree_sha256.as_deref() != Some(result.tree_sha256.as_str())
        || ratchet.coverage.input_set_sha256.as_deref() != Some(result.input_set_sha256.as_str())
    {
        return invalid("embedded conformance result differs from the quality source tree");
    }
    if result.target != manifest.target
        || result.adapter.adapter_protocol != manifest.adapter.protocol
        || result.adapter.implementation != manifest.adapter.implementation
    {
        return invalid("embedded conformance result differs from the promotion-proof identity");
    }
    for path in ADAPTER_SOURCES {
        if !objects.contains_key(path) {
            return invalid(format!("promotion proof omits adapter source `{path}`"));
        }
    }
    validate_exact_provenance(
        manifest,
        &draft_path.source_path,
        &draft,
        &ratchet,
        &baseline,
        &history_paths,
    )?;
    Ok(())
}

fn validate_embedded_history(
    manifest: &DraftLineageManifest,
    objects: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<String>, SealError> {
    let mut child = manifest.clone();
    let mut paths = Vec::new();
    while let Some(parent_file) = child.parent.clone() {
        require_object(objects, &parent_file)?;
        let parent_bytes = &objects[parent_file.path.as_str()];
        let parent: DraftLineageManifest = serde_json::from_slice(parent_bytes)
            .map_err(|error| SealError::Json(error.to_string()))?;
        validate_lineage_manifest(&parent)
            .map_err(|error| SealError::Invalid(error.to_string()))?;
        if canonical_json(&parent)? != *parent_bytes
            || parent.revision.checked_add(1) != Some(child.revision)
            || parent.lineage != child.lineage
            || parent.edition != child.edition
            || parent.baseline != child.baseline
        {
            return invalid("embedded draft history is not the immediate canonical parent chain");
        }
        paths.push(parent_file.path);
        child = parent;
    }
    if child.revision != 1 {
        return invalid("embedded draft history does not terminate at revision 1");
    }
    Ok(paths)
}

fn validate_exact_provenance(
    manifest: &PromotionProofManifest,
    draft_manifest_path: &str,
    draft: &DraftLineageManifest,
    ratchet: &RatchetRecord,
    baseline: &SuiteManifest,
    history_paths: &[String],
) -> Result<(), SealError> {
    let mut expected = BTreeSet::new();
    let mut add = |role: &str, path: &str| {
        expected.insert((role.to_owned(), path.to_owned()));
    };
    add("draft-manifest", draft_manifest_path);
    for path in history_paths {
        add("draft-history", path);
    }
    for specification in &draft.specifications {
        add("specification", &specification.path);
    }
    for layer in &draft.case_layers {
        add("case-layer", &layer.manifest.path);
    }
    add("regression-manifest", &draft.baseline.manifest.path);
    add(
        "regression-specification",
        &draft.baseline.specification_snapshot.path,
    );
    for pinned in referenced_files(baseline) {
        if pinned.path != baseline.specification.path {
            add("regression-input", &pinned.path);
        }
    }
    add("conformance-result", "generated:tondo-reference-result");
    add("ratchet", RATCHET_PATH);
    for evidence in [
        &ratchet.inventory,
        &ratchet.matrix,
        &ratchet.gap_audit,
        &ratchet.quality_baseline,
    ] {
        add("quality-evidence", &evidence.path);
    }
    for path in ADAPTER_SOURCES {
        add("adapter-source", path);
    }
    let observed = manifest
        .files
        .iter()
        .map(|file| (file.role.clone(), file.source_path.clone()))
        .collect::<BTreeSet<_>>();
    if observed != expected {
        return invalid("promotion-proof roles or source provenance differ from the exact closure");
    }
    Ok(())
}

fn one_role<'a>(
    manifest: &'a PromotionProofManifest,
    role: &str,
) -> Result<&'a ProofFile, SealError> {
    let mut matches = manifest.files.iter().filter(|file| file.role == role);
    let file = matches
        .next()
        .ok_or_else(|| SealError::Invalid(format!("promotion proof omits role `{role}`")))?;
    if matches.next().is_some() {
        return invalid(format!("promotion proof repeats singleton role `{role}`"));
    }
    Ok(file)
}

fn require_object(
    objects: &BTreeMap<String, Vec<u8>>,
    pinned: &PinnedFile,
) -> Result<(), SealError> {
    let bytes = objects
        .get(pinned.path.as_str())
        .ok_or_else(|| SealError::Invalid(format!("promotion proof omits `{}`", pinned.path)))?;
    if sha256(bytes) != pinned.sha256 {
        return invalid(format!(
            "promotion-proof object `{}` differs from its draft pin",
            pinned.path
        ));
    }
    Ok(())
}

fn publish_bundle(
    root: &Path,
    output_directory: &Path,
    bundle: &PromotionProofBundle,
) -> Result<ProofOutcome, SealError> {
    require_relative_path(output_directory, "promotion-proof output directory")?;
    let relative_parent = output_directory.parent().ok_or_else(|| {
        SealError::Invalid("promotion-proof output has no parent directory".into())
    })?;
    let parent = resolve_directory(root, relative_parent, true)?;
    let destination = parent.join(
        output_directory
            .file_name()
            .ok_or_else(|| SealError::Invalid("promotion-proof output has no file name".into()))?,
    );
    let manifest_path = output_directory.join("manifest.json");
    if destination.exists() {
        let existing = verify_promotion_proof(root, &manifest_path)?;
        if existing == bundle.manifest
            && read_regular(&destination.join("manifest.json"))? == bundle.manifest_bytes
        {
            return Ok(ProofOutcome::AlreadyPresent);
        }
        return invalid("promotion-proof destination already contains a different proof");
    }
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SealError::Invalid("promotion-proof output name is not UTF-8".into()))?;
    let staging = parent.join(format!(".{name}.staging-{}", std::process::id()));
    fs::create_dir(&staging).map_err(|error| io(&staging, error))?;
    let result = (|| {
        let object_directory = staging.join("objects");
        fs::create_dir(&object_directory).map_err(|error| io(&object_directory, error))?;
        for (digest, bytes) in &bundle.objects {
            write_new(&object_directory.join(digest), bytes)?;
        }
        write_new(&staging.join("manifest.json"), &bundle.manifest_bytes)?;
        fs::rename(&staging, &destination).map_err(|error| io(&destination, error))?;
        sync_directory(&parent)?;
        Ok(ProofOutcome::Created)
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn verify_promotion_proof_directory(
    directory: &Path,
    manifest: &PromotionProofManifest,
) -> Result<(), SealError> {
    let mut entries = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(|error| io(directory, error))? {
        let entry = entry.map_err(|error| io(directory, error))?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| SealError::Invalid("promotion-proof entry name is not UTF-8".into()))?
            .to_owned();
        entries.insert(name);
    }
    if entries != BTreeSet::from(["manifest.json".into(), "objects".into()]) {
        return invalid("promotion-proof directory contains files outside its manifest closure");
    }
    let expected = manifest
        .files
        .iter()
        .map(|file| file.object.sha256.clone())
        .collect::<BTreeSet<_>>();
    let object_directory = directory.join("objects");
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(&object_directory).map_err(|error| io(&object_directory, error))? {
        let entry = entry.map_err(|error| io(&object_directory, error))?;
        let metadata = entry
            .file_type()
            .map_err(|error| io(&entry.path(), error))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| SealError::Invalid("promotion-proof object name is not UTF-8".into()))?;
        if !metadata.is_file() || !actual.insert(name.to_owned()) {
            return invalid("promotion-proof object directory contains an invalid entry");
        }
    }
    if actual != expected {
        return invalid("promotion-proof object directory differs from the manifest closure");
    }
    Ok(())
}

fn resolve_directory(root: &Path, relative: &Path, create: bool) -> Result<PathBuf, SealError> {
    let root_metadata = fs::symlink_metadata(root).map_err(|error| io(root, error))?;
    if !root_metadata.file_type().is_dir() {
        return invalid("promotion-proof root is not a directory");
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return invalid("promotion-proof directory contains a non-normal component");
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return invalid(format!("`{}` is not a directory", current.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&current).map_err(|error| io(&current, error))?;
            }
            Err(error) => return Err(io(&current, error)),
        }
    }
    Ok(current)
}

fn sync_directory(path: &Path) -> Result<(), SealError> {
    #[cfg(unix)]
    {
        let directory = fs::File::open(path).map_err(|error| io(path, error))?;
        directory.sync_all().map_err(|error| io(path, error))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SealError> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|error| SealError::Json(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_regular(path: &Path) -> Result<Vec<u8>, SealError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if !metadata.file_type().is_file() {
        return invalid(format!("`{}` is not a regular file", path.display()));
    }
    fs::read(path).map_err(|error| io(path, error))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), SealError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io(path, error))?;
    file.write_all(bytes).map_err(|error| io(path, error))?;
    file.sync_all().map_err(|error| io(path, error))
}

fn require_relative_path(path: &Path, name: &str) -> Result<(), SealError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid(format!(
            "{name} must contain only relative normal components"
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn io(path: &Path, error: std::io::Error) -> SealError {
    SealError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, SealError> {
    Err(SealError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::lineage::DRAFT_LINEAGE_PATH;

    static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
    static REPOSITORY_BUNDLE: OnceLock<PromotionProofBundle> = OnceLock::new();

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn temporary_root() -> PathBuf {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("tondo-candidate-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        root
    }

    fn repository_bundle() -> PromotionProofBundle {
        REPOSITORY_BUNDLE
            .get_or_init(build_repository_bundle)
            .clone()
    }

    fn build_repository_bundle() -> PromotionProofBundle {
        let root = repository_fixture();
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        let bundle = build_promotion_proof(
            &lineage,
            &root.join("conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json"),
        )
        .unwrap();
        fs::remove_dir_all(root).unwrap();
        bundle
    }

    fn repository_fixture() -> PathBuf {
        let source = repository_root();
        let root = temporary_root();
        copy_tree(&source.join("conformance"), &root.join("conformance"));
        for path in [
            "TONDO_LANGUAGE_SPEC.md",
            "TONDO_STANDARD_LIBRARY_SPEC.md",
            "TONDO_TESTING_SPEC.md",
            "TONDO_TOOLCHAIN_SPEC.md",
            "testing/inventory.json",
            "testing/coverage-matrix.json",
            "testing/normative-gap-audit.json",
            "testing/quality-baseline.json",
            RATCHET_PATH,
        ] {
            copy_file(&source, &root, path);
        }
        for path in ADAPTER_SOURCES {
            copy_file(&source, &root, path);
        }
        // Seal tests exercise the proof transport with a deliberately closed
        // matrix. The repository draft remains open while ABI migrations are
        // pending; converting only this isolated fixture avoids asserting a
        // false completion claim in tracked evidence.
        let matrix_path = root.join("testing/coverage-matrix.json");
        let mut matrix: serde_json::Value =
            serde_json::from_slice(&fs::read(&matrix_path).unwrap()).unwrap();
        let mut promoted = BTreeSet::new();
        for requirement in matrix["requirements"].as_array_mut().unwrap() {
            if requirement["status"] == "draft-pending" {
                promoted.insert(requirement["id"].as_str().unwrap().to_owned());
                requirement["status"] = serde_json::Value::String("covered".into());
            }
        }
        let by_status = matrix["summary"]["by_status"].as_object_mut().unwrap();
        by_status.remove("draft-pending");
        let covered = by_status
            .get("covered")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        by_status.insert(
            "covered".into(),
            serde_json::Value::from(covered + promoted.len() as u64),
        );
        fs::write(&matrix_path, serde_json::to_vec_pretty(&matrix).unwrap()).unwrap();
        let mut ratchet: RatchetRecord =
            serde_json::from_slice(&fs::read(root.join(RATCHET_PATH)).unwrap()).unwrap();
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        ratchet.lineage = lineage.manifest().lineage.clone();
        ratchet.revision = lineage.manifest().revision;
        ratchet.manifest.path = DRAFT_LINEAGE_PATH.into();
        ratchet.manifest.sha256 = lineage.manifest_sha256();
        ratchet.draft_case_layers = lineage.manifest().case_layers.len() as u64;
        ratchet.pending_tasks = lineage.manifest().pending_tasks.clone();
        for scope in [&mut ratchet.coverage, &mut ratchet.mutation] {
            scope.tree_sha256 = Some("d".repeat(64));
            scope.input_set_sha256 = Some("e".repeat(64));
        }
        for evidence in [
            &mut ratchet.inventory,
            &mut ratchet.matrix,
            &mut ratchet.gap_audit,
            &mut ratchet.quality_baseline,
        ] {
            evidence.sha256 = sha256(&fs::read(root.join(&evidence.path)).unwrap());
        }
        fs::write(root.join(RATCHET_PATH), canonical_json(&ratchet).unwrap()).unwrap();
        write_composed_result(&root, &lineage, &ratchet);
        root
    }

    fn write_composed_result(root: &Path, lineage: &DraftLineage, ratchet: &RatchetRecord) {
        let result_path =
            root.join("conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json");
        let baseline: crate::runner::SuiteResult =
            serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
        let inventory_bytes = fs::read(root.join(&ratchet.inventory.path)).unwrap();
        let inventory: serde_json::Value = serde_json::from_slice(&inventory_bytes).unwrap();
        let sources = inventory["tests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["id"].as_str().unwrap(),
                    entry["source_sha256"].as_str().unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let ids = lineage
            .case_layers()
            .iter()
            .flat_map(|layer| layer.cases.iter())
            .flat_map(|case| case.evidence.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let report = serde_json::json!({
            "format": "tondo-layer-evidence/1",
            "lineage": lineage.manifest().lineage,
            "revision": lineage.manifest().revision,
            "manifest_sha256": lineage.manifest_sha256(),
            "inventory_sha256": sha256(&inventory_bytes),
            "tree_sha256": ratchet.coverage.tree_sha256.as_deref().unwrap(),
            "input_set_sha256": ratchet.coverage.input_set_sha256.as_deref().unwrap(),
            "passed": true,
            "evidence": ids.iter().map(|id| serde_json::json!({
                "id": id,
                "source_sha256": sources[id],
                "observation_sha256": sha256(id.as_bytes()),
            })).collect::<Vec<_>>(),
        });
        let composed = crate::runner::compose_suite_result(
            lineage,
            baseline,
            &serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        fs::write(&result_path, serde_json::to_vec(&composed).unwrap()).unwrap();
    }

    fn copy_file(source: &Path, destination: &Path, relative: &str) {
        let output = destination.join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(source.join(relative), output).unwrap();
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let output = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &output);
            } else {
                fs::copy(entry.path(), output).unwrap();
            }
        }
    }

    #[test]
    fn candidate_is_canonical_self_contained_and_idempotently_published() {
        let bundle = repository_bundle();
        let root = temporary_root();
        assert_eq!(
            publish_bundle(&root, Path::new("candidate"), &bundle).unwrap(),
            ProofOutcome::Created
        );
        let verified = verify_promotion_proof(&root, Path::new("candidate/manifest.json")).unwrap();
        assert_eq!(verified, bundle.manifest);
        assert_eq!(
            publish_bundle(&root, Path::new("candidate"), &bundle).unwrap(),
            ProofOutcome::AlreadyPresent
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_seal_promotes_and_reverifies_the_isolated_completed_draft() {
        let root = repository_fixture();
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        let result =
            root.join("conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json");
        assert_eq!(
            seal_promotion_proof(&lineage, &result, Path::new("nested/candidate")).unwrap(),
            ProofOutcome::Created
        );
        assert_eq!(
            seal_promotion_proof(&lineage, &result, Path::new("nested/candidate")).unwrap(),
            ProofOutcome::AlreadyPresent
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_rejects_mixed_history_tampering_and_partial_destinations() {
        let bundle = repository_bundle();
        let root = temporary_root();
        let mut mixed = bundle.clone();
        mixed.manifest.lineage.revision += 1;
        mixed.manifest_bytes = canonical_json(&mixed.manifest).unwrap();
        publish_bundle(&root, Path::new("mixed"), &mixed).unwrap();
        assert!(verify_promotion_proof(&root, Path::new("mixed/manifest.json")).is_err());

        publish_bundle(&root, Path::new("tampered"), &bundle).unwrap();
        let object = bundle.manifest.files[0].object.sha256.clone();
        fs::write(root.join("tampered/objects").join(object), b"tampered").unwrap();
        assert!(verify_promotion_proof(&root, Path::new("tampered/manifest.json")).is_err());

        publish_bundle(&root, Path::new("extra"), &bundle).unwrap();
        fs::write(root.join("extra/untracked"), b"untracked").unwrap();
        assert!(verify_promotion_proof(&root, Path::new("extra/manifest.json")).is_err());

        fs::write(root.join("not-a-directory"), b"occupied").unwrap();
        assert!(publish_bundle(&root, Path::new("not-a-directory/candidate"), &bundle).is_err());
        assert!(!root.join("not-a-directory/candidate").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_paths_hashes_and_error_vocabulary_are_closed() {
        let bundle = repository_bundle();
        assert_eq!(bundle.manifest.format, PROMOTION_PROOF_FORMAT);
        assert!(bundle.manifest.files.windows(2).all(|pair| {
            (&pair[0].role, &pair[0].source_path) < (&pair[1].role, &pair[1].source_path)
        }));
        assert!(require_relative_path(Path::new("candidate/manifest.json"), "path").is_ok());
        for path in ["", "/absolute", "../escape", "nested/../escape"] {
            assert!(require_relative_path(Path::new(path), "path").is_err());
        }
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"A".repeat(64)));
        assert!(
            SealError::Invalid("reason".into())
                .to_string()
                .contains("reason")
        );
        assert!(verify_promotion_proof(Path::new("missing"), Path::new("candidate.json")).is_err());
    }

    #[test]
    fn candidate_validators_reject_identity_provenance_and_conflicts() {
        let bundle = repository_bundle();

        let mut invalid_manifest = bundle.manifest.clone();
        invalid_manifest.format = "future-candidate".into();
        assert!(validate_promotion_proof_manifest(&invalid_manifest).is_err());

        let mut unsorted = bundle.manifest.clone();
        unsorted.files.reverse();
        assert!(validate_promotion_proof_manifest(&unsorted).is_err());

        let mut invalid_file = bundle.manifest.clone();
        invalid_file.files[0].source_path.clear();
        assert!(validate_promotion_proof_manifest(&invalid_file).is_err());

        assert!(
            add_source(
                &mut Vec::new(),
                &mut BTreeMap::new(),
                "",
                "source",
                Vec::new(),
            )
            .is_err()
        );
        assert!(one_role(&bundle.manifest, "absent").is_err());
        let mut repeated = bundle.manifest.clone();
        let singleton = one_role(&repeated, "draft-manifest").unwrap().clone();
        repeated.files.push(singleton);
        assert!(one_role(&repeated, "draft-manifest").is_err());

        let absent = PinnedFile {
            path: "absent".into(),
            sha256: "a".repeat(64),
        };
        assert!(require_object(&BTreeMap::new(), &absent).is_err());
        let objects = BTreeMap::from([("present".into(), b"bytes".to_vec())]);
        let mismatched = PinnedFile {
            path: "present".into(),
            sha256: "b".repeat(64),
        };
        assert!(require_object(&objects, &mismatched).is_err());

        let root = temporary_root();
        publish_bundle(&root, Path::new("candidate"), &bundle).unwrap();
        let mut different = bundle.clone();
        different.manifest.state = "different".into();
        different.manifest_bytes = canonical_json(&different.manifest).unwrap();
        assert!(publish_bundle(&root, Path::new("candidate"), &different).is_err());
        assert!(verify_promotion_proof(&root, Path::new("missing/manifest.json")).is_err());
        assert!(read_regular(&root).is_err());
        fs::remove_dir_all(root).unwrap();

        let io_error = SealError::Io {
            path: PathBuf::from("evidence"),
            message: "unavailable".into(),
        };
        assert!(io_error.to_string().contains("evidence"));
    }

    #[test]
    fn ratchet_result_and_object_closure_fail_closed() {
        let root = repository_fixture();
        let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH).unwrap();
        let ratchet_bytes = fs::read(root.join(RATCHET_PATH)).unwrap();
        let ratchet: RatchetRecord = serde_json::from_slice(&ratchet_bytes).unwrap();
        let matrix_bytes = fs::read(root.join(&ratchet.matrix.path)).unwrap();
        let audit_bytes = fs::read(root.join(&ratchet.gap_audit.path)).unwrap();
        let inventory_bytes = fs::read(root.join(&ratchet.inventory.path)).unwrap();
        validate_coverage_matrix(&matrix_bytes).unwrap();
        validate_gap_audit(&matrix_bytes, &audit_bytes, &inventory_bytes).unwrap();

        let mut stale_audit: serde_json::Value = serde_json::from_slice(&audit_bytes).unwrap();
        stale_audit["entries"][0]["text_sha256"] = "0".repeat(64).into();
        assert!(
            validate_gap_audit(
                &matrix_bytes,
                &serde_json::to_vec(&stale_audit).unwrap(),
                &inventory_bytes,
            )
            .is_err()
        );

        let mut unknown_test: serde_json::Value = serde_json::from_slice(&audit_bytes).unwrap();
        unknown_test["entries"][0]["tests"][0] = "unknown:test".into();
        assert!(
            validate_gap_audit(
                &matrix_bytes,
                &serde_json::to_vec(&unknown_test).unwrap(),
                &inventory_bytes,
            )
            .is_err()
        );

        let mut bad_audit_summary: serde_json::Value =
            serde_json::from_slice(&audit_bytes).unwrap();
        bad_audit_summary["summary"]["total"] = 0.into();
        assert!(
            validate_gap_audit(
                &matrix_bytes,
                &serde_json::to_vec(&bad_audit_summary).unwrap(),
                &inventory_bytes,
            )
            .is_err()
        );
        let mut pending_matrix: serde_json::Value = serde_json::from_slice(&matrix_bytes).unwrap();
        let original_status = pending_matrix["requirements"][0]["status"]
            .as_str()
            .unwrap()
            .to_owned();
        pending_matrix["requirements"][0]["status"] = "draft-pending".into();
        let original_count = pending_matrix["summary"]["by_status"][&original_status]
            .as_u64()
            .unwrap();
        pending_matrix["summary"]["by_status"][&original_status] = (original_count - 1).into();
        pending_matrix["summary"]["by_status"]["draft-pending"] = 1.into();
        assert!(validate_coverage_matrix(&serde_json::to_vec(&pending_matrix).unwrap()).is_err());

        let mut old_format: serde_json::Value = serde_json::from_slice(&matrix_bytes).unwrap();
        old_format["format"] = "tondo-normative-coverage/1".into();
        assert!(validate_coverage_matrix(&serde_json::to_vec(&old_format).unwrap()).is_err());

        let mut bad_total: serde_json::Value = serde_json::from_slice(&matrix_bytes).unwrap();
        bad_total["summary"]["total"] = 0.into();
        assert!(validate_coverage_matrix(&serde_json::to_vec(&bad_total).unwrap()).is_err());

        let mut bad_summary: serde_json::Value = serde_json::from_slice(&matrix_bytes).unwrap();
        bad_summary["summary"]["by_status"]["covered"] = 0.into();
        assert!(validate_coverage_matrix(&serde_json::to_vec(&bad_summary).unwrap()).is_err());

        let mut incomplete = ratchet.clone();
        incomplete.pending_tasks.push("pending".into());
        assert!(validate_ratchet(&lineage, &incomplete, &root).is_err());

        let mut unvalidated = ratchet.clone();
        unvalidated.coverage.status = "unvalidated".into();
        assert!(validate_ratchet(&lineage, &unvalidated, &root).is_err());

        let mut mismatched = ratchet;
        mismatched.inventory.sha256 = "a".repeat(64);
        assert!(validate_ratchet(&lineage, &mismatched, &root).is_err());

        let result_bytes = fs::read(
            root.join("conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json"),
        )
        .unwrap();
        let mut result = parse_result(&result_bytes).unwrap();
        result.passed = false;
        assert!(validate_result(&lineage, &result).is_err());

        let result = parse_result(&result_bytes).unwrap();
        let mut missing = result.clone();
        missing.cases.pop();
        assert!(validate_result(&lineage, &missing).is_err());

        let mut duplicated = result.clone();
        duplicated.cases.push(duplicated.cases[0].clone());
        assert!(validate_result(&lineage, &duplicated).is_err());

        let mut reordered = result.clone();
        reordered.cases.swap(0, 1);
        assert!(validate_result(&lineage, &reordered).is_err());

        let mut wrong_group = result.clone();
        wrong_group.cases[0].group = CaseGroup::Runtime;
        assert!(validate_result(&lineage, &wrong_group).is_err());

        let mut wrong_repetitions = result.clone();
        wrong_repetitions.cases[0].repetitions += 1;
        assert!(validate_result(&lineage, &wrong_repetitions).is_err());

        let mut missing_observation = result.clone();
        missing_observation.cases[0].observation_sha256.clear();
        assert!(validate_result(&lineage, &missing_observation).is_err());

        let mut invalid_observation = result.clone();
        invalid_observation.cases[0].observation_sha256[0] = "not-a-hash".into();
        assert!(validate_result(&lineage, &invalid_observation).is_err());

        let mut missing_layer = result.clone();
        missing_layer.case_layers.pop();
        assert!(validate_result(&lineage, &missing_layer).is_err());

        let mut reordered_layers = result.clone();
        reordered_layers.case_layers.swap(0, 1);
        assert!(validate_result(&lineage, &reordered_layers).is_err());

        let mut missing_layer_evidence = result.clone();
        missing_layer_evidence.case_layers[0].cases[0]
            .evidence
            .pop();
        assert!(validate_result(&lineage, &missing_layer_evidence).is_err());

        let mut wrong_layer_source = result.clone();
        wrong_layer_source.case_layers[0].cases[0].evidence[0].source_sha256 = "f".repeat(64);
        assert!(validate_result(&lineage, &wrong_layer_source).is_err());

        let mut wrong_tree = result.clone();
        wrong_tree.tree_sha256 = "not-a-hash".into();
        assert!(validate_result(&lineage, &wrong_tree).is_err());

        let mut unknown_field: serde_json::Value = serde_json::from_slice(&result_bytes).unwrap();
        unknown_field["unexpected"] = true.into();
        assert!(parse_result(&serde_json::to_vec(&unknown_field).unwrap()).is_err());

        let bundle = build_promotion_proof(
            &lineage,
            &root.join("conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json"),
        )
        .unwrap();
        let source_objects = || {
            bundle
                .manifest
                .files
                .iter()
                .map(|file| {
                    (
                        file.source_path.clone(),
                        bundle.objects[&file.object.sha256].clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        let mut objects = source_objects();
        objects.remove(ADAPTER_SOURCES[0]);
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let history = bundle
            .manifest
            .files
            .iter()
            .find(|file| file.role == "draft-history")
            .unwrap();
        let mut objects = source_objects();
        objects.remove(&history.source_path);
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let mut wrong_role = bundle.manifest.clone();
        wrong_role
            .files
            .iter_mut()
            .find(|file| file.role == "adapter-source")
            .unwrap()
            .role = "unexpected-source".into();
        assert!(verify_promotion_proof_closure(&wrong_role, &source_objects()).is_err());

        let mut objects = source_objects();
        let regression_input = bundle
            .manifest
            .files
            .iter()
            .find(|file| file.role == "regression-input")
            .unwrap();
        objects.remove(&regression_input.source_path);
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let mut objects = source_objects();
        objects.insert("testing/inventory.json".into(), b"changed".to_vec());
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let mut objects = source_objects();
        let mut failed_result: serde_json::Value = serde_json::from_slice(&result_bytes).unwrap();
        failed_result["passed"] = false.into();
        objects.insert(
            "generated:tondo-reference-result".into(),
            serde_json::to_vec(&failed_result).unwrap(),
        );
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let mut objects = source_objects();
        let mut partial_result: serde_json::Value = serde_json::from_slice(&result_bytes).unwrap();
        partial_result["cases"].as_array_mut().unwrap().pop();
        objects.insert(
            "generated:tondo-reference-result".into(),
            serde_json::to_vec(&partial_result).unwrap(),
        );
        assert!(verify_promotion_proof_closure(&bundle.manifest, &objects).is_err());

        let destination = temporary_root();
        publish_bundle(&destination, Path::new("missing-object"), &bundle).unwrap();
        let object = bundle.manifest.files[0].object.sha256.clone();
        fs::remove_file(destination.join("missing-object/objects").join(object)).unwrap();
        assert!(
            verify_promotion_proof(&destination, Path::new("missing-object/manifest.json"))
                .is_err()
        );
        fs::remove_dir_all(destination).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
