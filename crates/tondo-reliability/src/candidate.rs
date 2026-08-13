//! Final, self-contained G5/T0 candidate bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tondo_conformance::manifest::PinnedFile;
use tondo_conformance::runner::ComposedSuiteResult;
use tondo_conformance::seal::{
    PromotionProofManifest, ProofAdapter, ProofLineage, ProofTarget, verify_promotion_proof,
    verify_promotion_proof_objects,
};

use crate::layer_evidence::{FORMAT as LAYER_EVIDENCE_FORMAT, LayerEvidenceReport};
use crate::provenance::ReportBinding;
use crate::quality::{QualityBaseline, parse_llvm_cov, parse_mutation_report};
use crate::ratchet::RatchetRecord;
use crate::{canonical_json, sha256};

pub const FORMAT: &str = "tondo-conformance-candidate/2";
pub const STATE: &str = "candidate";

const GATES: [&str; 2] = ["G5", "T0"];
const PROOF_MANIFEST_SOURCE: &str = "promotion-proof/manifest.json";
const COVERAGE_SOURCE: &str = "evidence/coverage.json";
const COVERAGE_BINDING_SOURCE: &str = "evidence/coverage-binding.json";
const MUTATION_SOURCE: &str = "evidence/mutation.json";
const MUTATION_BINDING_SOURCE: &str = "evidence/mutation-binding.json";
const LAYER_EVIDENCE_SOURCE: &str = "evidence/layer-evidence.json";
const DOC_TEST_SOURCE: &str = "evidence/doc-test.json";
const DOC_TEST_LINKS_SOURCE: &str = "evidence/doc-test-runtime-links.json";
const DOC_TEST_DOCUMENTS: [&str; 5] = [
    "TONDO_LANGUAGE_SPEC.md",
    "TONDO_TESTING_SPEC.md",
    "TONDO_TOOLCHAIN_SPEC.md",
    "TONDO_STANDARD_LIBRARY_SPEC.md",
    "TONDO_LLM_FORM_SPEC.md",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateManifest {
    pub format: String,
    pub edition: String,
    pub state: String,
    pub lineage: ProofLineage,
    pub target: ProofTarget,
    pub adapter: ProofAdapter,
    pub gates: Vec<String>,
    pub files: Vec<CandidateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateFile {
    pub role: String,
    pub source_path: String,
    pub object: PinnedFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateOutcome {
    Created,
    AlreadyPresent,
}

pub struct CandidateInputs<'a> {
    pub proof: &'a Path,
    pub coverage: &'a Path,
    pub coverage_binding: &'a Path,
    pub mutation: &'a Path,
    pub mutation_binding: &'a Path,
    pub layer_evidence: &'a Path,
    pub doc_test: &'a Path,
    pub doc_test_links: &'a Path,
}

#[derive(Debug, Clone)]
struct CandidateBundle {
    manifest: CandidateManifest,
    manifest_bytes: Vec<u8>,
    objects: BTreeMap<String, Vec<u8>>,
}

/// Seals a fresh proof plus quality and documentation evidence into a candidate.
pub fn seal_candidate(
    root: &Path,
    inputs: &CandidateInputs<'_>,
    output: &Path,
) -> Result<CandidateOutcome, String> {
    for (name, path) in [
        ("promotion proof", inputs.proof),
        ("coverage report", inputs.coverage),
        ("coverage binding", inputs.coverage_binding),
        ("mutation report", inputs.mutation),
        ("mutation binding", inputs.mutation_binding),
        ("layer evidence", inputs.layer_evidence),
        ("doc-test report", inputs.doc_test),
        ("doc-test links", inputs.doc_test_links),
        ("candidate output", output),
    ] {
        require_relative(path, name)?;
    }

    let proof_manifest_path = inputs.proof.join("manifest.json");
    let proof =
        verify_promotion_proof(root, &proof_manifest_path).map_err(|error| error.to_string())?;
    let mut bundle = CandidateBundle {
        manifest: CandidateManifest {
            format: FORMAT.into(),
            edition: proof.edition.clone(),
            state: STATE.into(),
            lineage: proof.lineage.clone(),
            target: proof.target.clone(),
            adapter: proof.adapter.clone(),
            gates: GATES.into_iter().map(str::to_owned).collect(),
            files: Vec::new(),
        },
        manifest_bytes: Vec::new(),
        objects: BTreeMap::new(),
    };

    add_file(
        &mut bundle,
        "promotion-proof-manifest",
        PROOF_MANIFEST_SOURCE,
        read_regular(&root.join(&proof_manifest_path))?,
    )?;
    let unique_objects = proof
        .files
        .iter()
        .map(|file| file.object.path.as_str())
        .collect::<BTreeSet<_>>();
    for object_path in unique_objects {
        add_file(
            &mut bundle,
            "promotion-proof-object",
            &format!("promotion-proof/{object_path}"),
            read_regular(&root.join(inputs.proof).join(object_path))?,
        )?;
    }
    for (role, source, path) in [
        ("coverage-report", COVERAGE_SOURCE, inputs.coverage),
        (
            "coverage-binding",
            COVERAGE_BINDING_SOURCE,
            inputs.coverage_binding,
        ),
        ("mutation-report", MUTATION_SOURCE, inputs.mutation),
        (
            "mutation-binding",
            MUTATION_BINDING_SOURCE,
            inputs.mutation_binding,
        ),
        (
            "layer-evidence",
            LAYER_EVIDENCE_SOURCE,
            inputs.layer_evidence,
        ),
        ("doc-test-report", DOC_TEST_SOURCE, inputs.doc_test),
        (
            "doc-test-links",
            DOC_TEST_LINKS_SOURCE,
            inputs.doc_test_links,
        ),
    ] {
        add_file(&mut bundle, role, source, read_regular(&root.join(path))?)?;
    }
    bundle.manifest.files.sort_by(|left, right| {
        (&left.role, &left.source_path).cmp(&(&right.role, &right.source_path))
    });
    bundle.manifest_bytes = canonical_json(&bundle.manifest)?;
    validate_bundle(&bundle.manifest_bytes, &bundle.objects)?;
    publish(root, output, &bundle)
}

/// Verifies a candidate using only its manifest and object closure.
pub fn verify_candidate(root: &Path, candidate: &Path) -> Result<CandidateManifest, String> {
    require_relative(candidate, "candidate directory")?;
    let directory = resolve_existing_directory(root, candidate)?;
    let manifest_bytes = read_regular(&directory.join("manifest.json"))?;
    let manifest: CandidateManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid candidate JSON: {error}"))?;
    let expected = manifest
        .files
        .iter()
        .map(|file| file.object.path.as_str())
        .collect::<BTreeSet<_>>();
    let object_directory = directory.join("objects");
    let actual = read_file_names(&object_directory)?;
    if actual.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("candidate object closure is missing or contains extra files".into());
    }
    let mut objects = BTreeMap::new();
    for path in expected {
        objects.insert(path.to_owned(), read_regular(&directory.join(path))?);
    }
    validate_bundle(&manifest_bytes, &objects)?;
    Ok(manifest)
}

fn validate_bundle(
    manifest_bytes: &[u8],
    archived_objects: &BTreeMap<String, Vec<u8>>,
) -> Result<CandidateManifest, String> {
    let manifest: CandidateManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("invalid candidate JSON: {error}"))?;
    validate_manifest(&manifest)?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err("candidate manifest is not canonical pretty JSON".into());
    }
    let expected = manifest
        .files
        .iter()
        .map(|file| file.object.path.as_str())
        .collect::<BTreeSet<_>>();
    if archived_objects
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err("candidate object closure is missing or contains extra objects".into());
    }
    let mut sources = BTreeMap::new();
    for file in &manifest.files {
        let bytes = archived_objects
            .get(&file.object.path)
            .ok_or_else(|| format!("candidate omits `{}`", file.object.path))?;
        if sha256(bytes) != file.object.sha256 {
            return Err(format!(
                "candidate object `{}` has an invalid hash",
                file.object.path
            ));
        }
        sources.insert(file.source_path.as_str(), bytes.as_slice());
    }

    let proof_bytes = source(&sources, PROOF_MANIFEST_SOURCE)?;
    let proof: PromotionProofManifest = serde_json::from_slice(proof_bytes)
        .map_err(|error| format!("invalid embedded promotion proof: {error}"))?;
    validate_candidate_file_closure(&manifest, &proof)?;
    let mut proof_objects = BTreeMap::new();
    for file in &proof.files {
        let source_path = format!("promotion-proof/{}", file.object.path);
        proof_objects.insert(
            file.object.path.clone(),
            source(&sources, &source_path)?.to_vec(),
        );
    }
    let proof = verify_promotion_proof_objects(proof_bytes, &proof_objects)
        .map_err(|error| error.to_string())?;
    if manifest.edition != proof.edition
        || manifest.lineage != proof.lineage
        || manifest.target != proof.target
        || manifest.adapter != proof.adapter
    {
        return Err("candidate identity differs from its promotion proof".into());
    }

    let embedded = proof_sources(&proof, &proof_objects)?;
    let ratchet: RatchetRecord = parse_embedded(&embedded, "testing/conformance-ratchet.json")?;
    let result: ComposedSuiteResult = parse_role(&proof, &embedded, "conformance-result")?;
    let layer: LayerEvidenceReport =
        serde_json::from_slice(source(&sources, LAYER_EVIDENCE_SOURCE)?)
            .map_err(|error| format!("invalid layer evidence JSON: {error}"))?;
    validate_layer_gate(&manifest, &ratchet, &result, &layer)?;
    validate_normative_gate(&embedded)?;
    validate_quality_gate(&sources, &embedded, &ratchet, &result)?;
    validate_doc_test_gate(&sources, &embedded)?;
    Ok(manifest)
}

fn validate_layer_gate(
    manifest: &CandidateManifest,
    ratchet: &RatchetRecord,
    result: &ComposedSuiteResult,
    layer: &LayerEvidenceReport,
) -> Result<(), String> {
    if layer.format != LAYER_EVIDENCE_FORMAT
        || layer.lineage != manifest.lineage.name
        || !result.passed
        || result.lineage != manifest.lineage.name
        || result.revision != manifest.lineage.revision
        || result.lineage_manifest_sha256 != manifest.lineage.manifest_sha256
        || result.tree_sha256 != layer.tree_sha256
        || result.input_set_sha256 != layer.input_set_sha256
        || result.inventory_sha256 != layer.inventory_sha256
        || result.revision != layer.revision
        || result.lineage_manifest_sha256 != layer.manifest_sha256
        || !layer.passed
        || ratchet.coverage.tree_sha256.as_deref() != Some(result.tree_sha256.as_str())
        || ratchet.mutation.tree_sha256.as_deref() != Some(result.tree_sha256.as_str())
    {
        return Err("candidate layer, result, ratchet, and source identities differ".into());
    }
    let testing = result
        .case_layers
        .iter()
        .find(|candidate| candidate.id == "testing")
        .ok_or_else(|| "candidate has no executed testing layer".to_owned())?;
    if testing.cases.is_empty() {
        return Err("candidate testing layer is empty".into());
    }
    let observations = layer
        .evidence
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    if observations.len() != layer.evidence.len() {
        return Err("candidate layer evidence contains duplicate observations".into());
    }
    let expected_observations = result
        .case_layers
        .iter()
        .flat_map(|item| &item.cases)
        .flat_map(|item| &item.evidence)
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    if observations.keys().copied().collect::<BTreeSet<_>>() != expected_observations {
        return Err("candidate layer evidence and composed result differ".into());
    }
    for observed in result
        .case_layers
        .iter()
        .flat_map(|item| &item.cases)
        .flat_map(|item| &item.evidence)
    {
        let Some(attested) = observations.get(observed.id.as_str()) else {
            return Err(format!(
                "candidate result evidence `{}` was not freshly attested",
                observed.id
            ));
        };
        if attested.source_sha256 != observed.source_sha256
            || attested.observation_sha256 != observed.observation_sha256
        {
            return Err(format!(
                "candidate result evidence `{}` differs from its attestation",
                observed.id
            ));
        }
    }
    Ok(())
}

fn validate_normative_gate(embedded: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let matrix: Value = parse_embedded(embedded, "testing/coverage-matrix.json")?;
    let requirements = matrix
        .get("requirements")
        .and_then(Value::as_array)
        .ok_or_else(|| "candidate coverage matrix has no requirements".to_owned())?;
    for requirement in requirements {
        let id = text_field(requirement, "id")?;
        let status = text_field(requirement, "status")?;
        let allowed = matches!(status, "covered" | "target-not-applicable")
            || (status == "stdlib-pending" && id.starts_with("TL01-26-"));
        if !allowed {
            return Err(format!("candidate requirement `{id}` remains `{status}`"));
        }
    }
    for namespace in ["TL01", "TT01", "TC01"] {
        if !requirements.iter().any(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with(namespace))
        }) {
            return Err(format!("candidate matrix omits `{namespace}` requirements"));
        }
    }
    Ok(())
}

fn validate_quality_gate(
    sources: &BTreeMap<&str, &[u8]>,
    embedded: &BTreeMap<String, Vec<u8>>,
    ratchet: &RatchetRecord,
    result: &ComposedSuiteResult,
) -> Result<(), String> {
    let baseline = QualityBaseline::from_bytes(
        embedded
            .get("testing/quality-baseline.json")
            .ok_or_else(|| "candidate proof omits the quality baseline".to_owned())?,
    )?;
    let coverage = source(sources, COVERAGE_SOURCE)?;
    let mutation = source(sources, MUTATION_SOURCE)?;
    let coverage_binding: ReportBinding = parse_source(sources, COVERAGE_BINDING_SOURCE)?;
    let mutation_binding: ReportBinding = parse_source(sources, MUTATION_BINDING_SOURCE)?;
    for (name, report, binding, scope) in [
        ("coverage", coverage, &coverage_binding, &ratchet.coverage),
        ("mutation", mutation, &mutation_binding, &ratchet.mutation),
    ] {
        binding.validate()?;
        let provenance_digest = binding.provenance_digest()?;
        if binding.kind != name
            || binding.report_sha256 != sha256(report)
            || scope.report_sha256.as_deref() != Some(binding.report_sha256.as_str())
            || scope.provenance_sha256.as_deref() != Some(provenance_digest.as_str())
            || binding.after.tree_sha256 != result.tree_sha256
            || binding.after.input_set_sha256 != result.input_set_sha256
        {
            return Err(format!(
                "candidate {name} evidence differs from the sealed ratchet"
            ));
        }
    }
    baseline.verify_coverage_report(&parse_llvm_cov(coverage)?)?;
    baseline.verify_mutation_report(&parse_mutation_report(mutation)?)?;
    Ok(())
}

fn validate_doc_test_gate(
    sources: &BTreeMap<&str, &[u8]>,
    embedded: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let records: Vec<Value> = parse_source(sources, DOC_TEST_SOURCE)?;
    let links: Value = parse_source(sources, DOC_TEST_LINKS_SOURCE)?;
    let inventory: Value = parse_embedded(embedded, "testing/inventory.json")?;
    if records.is_empty()
        || links.get("format").and_then(Value::as_str) != Some("tondo-doc-test-runtime-links/1")
        || links.get("edition").and_then(Value::as_str) != Some("0.1")
        || object_keys(&links)?
            != BTreeSet::from(["documents", "edition", "format", "links", "rules"])
        || links.get("documents")
            != Some(&Value::Array(
                DOC_TEST_DOCUMENTS
                    .iter()
                    .map(|document| Value::String((*document).into()))
                    .collect(),
            ))
        || links.get("rules")
            != Some(&serde_json::json!({
                "typed_fences_are_classified": true,
                "syntax_fences_make_no_runtime_claim": true,
                "runtime_evidence_is_public_and_executable": true,
                "documentation_runner_never_executes_examples": true
            }))
    {
        return Err("candidate doc-test evidence has an invalid identity".into());
    }
    let mut all_keys = BTreeSet::new();
    let mut typed_keys = BTreeSet::new();
    let mut documents = BTreeSet::new();
    let record_keys = BTreeSet::from([
        "actual_codes",
        "category",
        "edition",
        "expected_codes",
        "fence_byte",
        "file",
        "fixture",
        "fixture_sha256",
        "formatted_sha256",
        "parse_ok",
        "production",
        "source_sha256",
        "typecheck_ok",
    ]);
    for record in &records {
        let document = text_field(record, "file")?;
        if record.get("edition").and_then(Value::as_str) != Some("0.1")
            || object_keys(record)? != record_keys
            || !DOC_TEST_DOCUMENTS.contains(&document)
        {
            return Err("candidate contains a failing or foreign doc-test record".into());
        }
        documents.insert(document);
        let key = doc_key(record, "file")?;
        if !all_keys.insert(key.clone()) {
            return Err("candidate contains duplicate doc-test records".into());
        }
        let category = text_field(record, "category")?;
        let parse = record.get("parse_ok");
        let typecheck = record.get("typecheck_ok");
        let formatted = record.get("formatted_sha256");
        let expected = record.get("expected_codes").and_then(Value::as_array);
        let actual = record.get("actual_codes").and_then(Value::as_array);
        let codes_are_valid = [expected, actual].into_iter().all(|codes| {
            codes.is_some_and(|codes| {
                codes
                    .iter()
                    .all(|code| code.as_str().is_some_and(|code| !code.is_empty()))
            })
        });
        let passed = match category {
            "syntax" => {
                parse.and_then(Value::as_bool) == Some(true)
                    && typecheck == Some(&Value::Null)
                    && formatted.and_then(Value::as_str).is_some_and(is_sha256)
            }
            "fragment" | "script" => {
                parse.and_then(Value::as_bool) == Some(true)
                    && typecheck.and_then(Value::as_bool) == Some(true)
                    && formatted.and_then(Value::as_str).is_some_and(is_sha256)
            }
            "compile-fail" => {
                typecheck.and_then(Value::as_bool) == Some(false)
                    && expected.is_some_and(|codes| !codes.is_empty())
                    && expected == actual
                    && match parse.and_then(Value::as_bool) {
                        Some(true) => formatted.and_then(Value::as_str).is_some_and(is_sha256),
                        Some(false) => formatted == Some(&Value::Null),
                        None => false,
                    }
            }
            "pseudocode" => {
                parse == Some(&Value::Null)
                    && typecheck == Some(&Value::Null)
                    && formatted == Some(&Value::Null)
            }
            _ => false,
        } && codes_are_valid;
        if !passed {
            return Err(format!(
                "candidate contains a failing `{category}` doc-test record"
            ));
        }
        if matches!(category, "fragment" | "script") {
            typed_keys.insert(key);
        }
    }
    if documents != DOC_TEST_DOCUMENTS.into_iter().collect::<BTreeSet<_>>() {
        return Err("candidate doc-test report does not cover the normative document set".into());
    }
    let tests = inventory
        .get("tests")
        .and_then(Value::as_array)
        .ok_or_else(|| "candidate inventory has no tests".to_owned())?
        .iter()
        .filter_map(|test| Some((test.get("id")?.as_str()?, test)))
        .collect::<BTreeMap<_, _>>();
    let mut linked_keys = BTreeSet::new();
    for link in links
        .get("links")
        .and_then(Value::as_array)
        .ok_or_else(|| "candidate doc-test registry has no links".to_owned())?
    {
        let key = doc_key(link, "document")?;
        if !linked_keys.insert(key) {
            return Err("candidate contains duplicate doc-test links".into());
        }
        match text_field(link, "behavior")? {
            "runtime" => {
                if object_keys(link)?
                    != BTreeSet::from([
                        "behavior",
                        "document",
                        "evidence",
                        "fence_byte",
                        "source_sha256",
                    ])
                {
                    return Err("runtime doc-test link has an invalid shape".into());
                }
                let evidence = link
                    .get("evidence")
                    .and_then(Value::as_array)
                    .filter(|items| !items.is_empty())
                    .ok_or_else(|| "runtime doc-test link has no evidence".to_owned())?;
                let mut evidence_ids = BTreeSet::new();
                for value in evidence {
                    let id = value
                        .as_str()
                        .ok_or_else(|| "runtime doc-test evidence ID is not a string".to_owned())?;
                    if !evidence_ids.insert(id) {
                        return Err("runtime doc-test link repeats an evidence ID".into());
                    }
                    let test = tests
                        .get(id)
                        .ok_or_else(|| format!("doc-test evidence `{id}` is not inventoried"))?;
                    if test.get("status").and_then(Value::as_str) != Some("executable")
                        || !matches!(
                            test.get("kind").and_then(Value::as_str),
                            Some("conformance-case" | "rust-test")
                        )
                    {
                        return Err(format!("doc-test evidence `{id}` is not executable"));
                    }
                }
            }
            "static-only"
                if object_keys(link)?
                    == BTreeSet::from([
                        "behavior",
                        "document",
                        "evidence",
                        "fence_byte",
                        "reason",
                        "source_sha256",
                    ])
                    && link
                        .get("evidence")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    && link
                        .get("reason")
                        .and_then(Value::as_str)
                        .is_some_and(|reason| !reason.is_empty()) => {}
            _ => return Err("candidate doc-test link has an invalid behavior".into()),
        }
    }
    if typed_keys != linked_keys {
        return Err("candidate typed fences and doc-test links differ".into());
    }
    Ok(())
}

fn validate_manifest(manifest: &CandidateManifest) -> Result<(), String> {
    if manifest.format != FORMAT
        || manifest.edition != "0.1"
        || manifest.state != STATE
        || manifest.lineage.name != "tondo-draft"
        || manifest.lineage.revision == 0
        || !is_sha256(&manifest.lineage.manifest_sha256)
        || manifest.target.name != "tondo-vm-hosted"
        || manifest.target.profile != "hosted"
        || manifest.adapter.package != "tondo-reference-adapter"
        || manifest.gates != GATES
        || manifest.files.is_empty()
    {
        return Err("candidate identity, target, adapter, gates, or closure is invalid".into());
    }
    if manifest
        .files
        .windows(2)
        .any(|pair| (&pair[0].role, &pair[0].source_path) >= (&pair[1].role, &pair[1].source_path))
    {
        return Err("candidate files must be sorted and unique".into());
    }
    let mut sources = BTreeSet::new();
    for file in &manifest.files {
        if file.role.is_empty()
            || file.source_path.is_empty()
            || !sources.insert(file.source_path.as_str())
            || !is_sha256(&file.object.sha256)
            || file.object.path != format!("objects/{}", file.object.sha256)
        {
            return Err("candidate file provenance or object identity is invalid".into());
        }
    }
    for required in [
        PROOF_MANIFEST_SOURCE,
        COVERAGE_SOURCE,
        COVERAGE_BINDING_SOURCE,
        MUTATION_SOURCE,
        MUTATION_BINDING_SOURCE,
        LAYER_EVIDENCE_SOURCE,
        DOC_TEST_SOURCE,
        DOC_TEST_LINKS_SOURCE,
    ] {
        if !sources.contains(required) {
            return Err(format!("candidate omits `{required}`"));
        }
    }
    Ok(())
}

fn validate_candidate_file_closure(
    manifest: &CandidateManifest,
    proof: &PromotionProofManifest,
) -> Result<(), String> {
    let mut expected = BTreeMap::from([
        (PROOF_MANIFEST_SOURCE.to_owned(), "promotion-proof-manifest"),
        (COVERAGE_SOURCE.to_owned(), "coverage-report"),
        (COVERAGE_BINDING_SOURCE.to_owned(), "coverage-binding"),
        (MUTATION_SOURCE.to_owned(), "mutation-report"),
        (MUTATION_BINDING_SOURCE.to_owned(), "mutation-binding"),
        (LAYER_EVIDENCE_SOURCE.to_owned(), "layer-evidence"),
        (DOC_TEST_SOURCE.to_owned(), "doc-test-report"),
        (DOC_TEST_LINKS_SOURCE.to_owned(), "doc-test-links"),
    ]);
    for object_path in proof
        .files
        .iter()
        .map(|file| file.object.path.as_str())
        .collect::<BTreeSet<_>>()
    {
        expected.insert(
            format!("promotion-proof/{object_path}"),
            "promotion-proof-object",
        );
    }
    let actual = manifest
        .files
        .iter()
        .map(|file| (file.source_path.clone(), file.role.as_str()))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err(
            "candidate provenance closure is missing, substituted, or contains extras".into(),
        );
    }
    Ok(())
}

fn proof_sources(
    proof: &PromotionProofManifest,
    archived: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    proof
        .files
        .iter()
        .map(|file| {
            archived
                .get(&file.object.path)
                .cloned()
                .map(|bytes| (file.source_path.clone(), bytes))
                .ok_or_else(|| format!("embedded proof omits `{}`", file.object.path))
        })
        .collect()
}

fn parse_role<T: for<'de> Deserialize<'de>>(
    proof: &PromotionProofManifest,
    embedded: &BTreeMap<String, Vec<u8>>,
    role: &str,
) -> Result<T, String> {
    let matches = proof
        .files
        .iter()
        .filter(|file| file.role == role)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!("embedded proof requires exactly one `{role}`"));
    }
    parse_embedded(embedded, &matches[0].source_path)
}

fn parse_embedded<T: for<'de> Deserialize<'de>>(
    embedded: &BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<T, String> {
    serde_json::from_slice(
        embedded
            .get(path)
            .ok_or_else(|| format!("candidate proof omits `{path}`"))?,
    )
    .map_err(|error| format!("invalid embedded `{path}`: {error}"))
}

fn parse_source<T: for<'de> Deserialize<'de>>(
    sources: &BTreeMap<&str, &[u8]>,
    path: &str,
) -> Result<T, String> {
    serde_json::from_slice(source(sources, path)?)
        .map_err(|error| format!("invalid candidate `{path}`: {error}"))
}

fn source<'a>(sources: &'a BTreeMap<&str, &'a [u8]>, path: &str) -> Result<&'a [u8], String> {
    sources
        .get(path)
        .copied()
        .ok_or_else(|| format!("candidate omits `{path}`"))
}

fn text_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("candidate record has no string `{field}`"))
}

fn object_keys(value: &Value) -> Result<BTreeSet<&str>, String> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .ok_or_else(|| "candidate record is not an object".to_owned())
}

fn doc_key(value: &Value, file_field: &str) -> Result<(String, u64, String), String> {
    let file = text_field(value, file_field)?.to_owned();
    let byte = value
        .get("fence_byte")
        .and_then(Value::as_u64)
        .ok_or_else(|| "doc-test record has no fence byte".to_owned())?;
    let digest = text_field(value, "source_sha256")?.to_owned();
    if !is_sha256(&digest) {
        return Err("doc-test source hash is invalid".into());
    }
    Ok((file, byte, digest))
}

fn add_file(
    bundle: &mut CandidateBundle,
    role: &str,
    source_path: &str,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let digest = sha256(&bytes);
    let object_path = format!("objects/{digest}");
    if let Some(previous) = bundle.objects.insert(object_path.clone(), bytes.clone())
        && previous != bytes
    {
        return Err("two different candidate objects produced the same SHA-256".into());
    }
    bundle.manifest.files.push(CandidateFile {
        role: role.into(),
        source_path: source_path.into(),
        object: PinnedFile {
            path: object_path,
            sha256: digest,
        },
    });
    Ok(())
}

fn publish(
    root: &Path,
    output: &Path,
    bundle: &CandidateBundle,
) -> Result<CandidateOutcome, String> {
    let absolute = root.join(output);
    if absolute.exists() {
        let existing = verify_candidate(root, output)?;
        if existing == bundle.manifest {
            return Ok(CandidateOutcome::AlreadyPresent);
        }
        return Err(format!(
            "candidate destination `{}` already differs",
            output.display()
        ));
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = resolve_directory(root, parent, true)?;
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "candidate output has no UTF-8 name".to_owned())?;
    let temporary = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    if temporary.exists() {
        return Err(format!(
            "candidate temporary destination `{}` exists",
            temporary.display()
        ));
    }
    fs::create_dir(&temporary)
        .map_err(|error| format!("cannot create `{}`: {error}", temporary.display()))?;
    let result: Result<(), String> = (|| {
        let objects = temporary.join("objects");
        fs::create_dir(&objects)
            .map_err(|error| format!("cannot create `{}`: {error}", objects.display()))?;
        for (path, bytes) in &bundle.objects {
            write_new(&temporary.join(path), bytes)?;
        }
        write_new(&temporary.join("manifest.json"), &bundle.manifest_bytes)?;
        fs::rename(&temporary, parent.join(name))
            .map_err(|error| format!("cannot publish candidate: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result?;
    let verified = verify_candidate(root, output)?;
    if verified != bundle.manifest {
        return Err("published candidate differs from its inputs".into());
    }
    Ok(CandidateOutcome::Created)
}

fn resolve_existing_directory(root: &Path, path: &Path) -> Result<PathBuf, String> {
    resolve_directory(root, path, false)
}

fn resolve_directory(root: &Path, path: &Path, create: bool) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace: {error}"))?;
    let absolute = root.join(path);
    if create {
        fs::create_dir_all(&absolute)
            .map_err(|error| format!("cannot create `{}`: {error}", absolute.display()))?;
    }
    let resolved = absolute
        .canonicalize()
        .map_err(|error| format!("cannot resolve `{}`: {error}", absolute.display()))?;
    if !resolved.starts_with(&root) || !resolved.is_dir() {
        return Err(format!("`{}` is not a workspace directory", path.display()));
    }
    Ok(resolved)
}

fn require_relative(path: &Path, name: &str) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "{name} must be a non-empty workspace-relative normal path"
        ));
    }
    Ok(())
}

fn read_regular(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot access `{}`: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("`{}` is not a regular file", path.display()));
    }
    fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))
}

fn read_file_names(directory: &Path) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("cannot read `{}`: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("cannot enumerate objects: {error}"))?;
        let file_name = entry.file_name();
        let name = file_name
            .to_str()
            .ok_or_else(|| "candidate object name is not UTF-8".to_owned())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("candidate objects must be regular files".into());
        }
        names.insert(format!("objects/{name}"));
    }
    Ok(names)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create `{}`: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot write `{}`: {error}", path.display()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::provenance::{QualityProvenance, Toolchain};
    use crate::quality::{CoverageBaseline, MutationBaseline};
    use crate::ratchet::{EvidenceFile, ScopeEvidence};

    fn manifest() -> CandidateManifest {
        let mut sources = [
            PROOF_MANIFEST_SOURCE,
            COVERAGE_SOURCE,
            COVERAGE_BINDING_SOURCE,
            MUTATION_SOURCE,
            MUTATION_BINDING_SOURCE,
            LAYER_EVIDENCE_SOURCE,
            DOC_TEST_SOURCE,
            DOC_TEST_LINKS_SOURCE,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, source_path)| CandidateFile {
            role: format!("evidence-{index}"),
            source_path: source_path.into(),
            object: PinnedFile {
                path: format!("objects/{:064x}", index + 1),
                sha256: format!("{:064x}", index + 1),
            },
        })
        .collect::<Vec<_>>();
        sources.sort_by(|left, right| {
            (&left.role, &left.source_path).cmp(&(&right.role, &right.source_path))
        });
        CandidateManifest {
            format: FORMAT.into(),
            edition: "0.1".into(),
            state: STATE.into(),
            lineage: ProofLineage {
                name: "tondo-draft".into(),
                revision: 24,
                manifest_sha256: "a".repeat(64),
            },
            target: ProofTarget {
                name: "tondo-vm-hosted".into(),
                profile: "hosted".into(),
                capabilities: vec!["console".into(), "process".into()],
            },
            adapter: ProofAdapter {
                package: "tondo-reference-adapter".into(),
                protocol: "tondo-conformance-adapter-draft".into(),
                implementation: "tondo-reference".into(),
            },
            gates: vec!["G5".into(), "T0".into()],
            files: sources,
        }
    }

    fn composed_result() -> ComposedSuiteResult {
        serde_json::from_value(serde_json::json!({
            "format":"tondo-conformance-result-draft/2",
            "suite":"tondo-conformance-draft",
            "suite_version":"0.1.0",
            "edition":"0.1",
            "manifest_sha256":"f".repeat(64),
            "adapter":{},
            "target":{"name":"tondo-vm-hosted","profile":"hosted","capabilities":[]},
            "passed":true,
            "cases":[],
            "lineage":"tondo-draft",
            "revision":24,
            "lineage_manifest_sha256":"a".repeat(64),
            "inventory_sha256":"b".repeat(64),
            "tree_sha256":"c".repeat(64),
            "input_set_sha256":"d".repeat(64),
            "case_layers":[{
                "id":"testing",
                "manifest_sha256":"e".repeat(64),
                "cases":[{
                    "id":"runner",
                    "evidence":[{
                        "id":"rust:test:passes",
                        "source_sha256":"1".repeat(64),
                        "observation_sha256":"2".repeat(64)
                    }],
                    "observation_sha256":"3".repeat(64)
                }]
            }]
        }))
        .unwrap()
    }

    fn scope(tree: &str, inputs: &str) -> ScopeEvidence {
        ScopeEvidence {
            status: "validated".into(),
            reason: "test evidence".into(),
            report_sha256: None,
            provenance_sha256: None,
            tree_sha256: Some(tree.into()),
            input_set_sha256: Some(inputs.into()),
        }
    }

    fn ratchet() -> RatchetRecord {
        let evidence = || EvidenceFile {
            path: "testing/evidence.json".into(),
            sha256: "4".repeat(64),
        };
        RatchetRecord {
            format: "tondo-conformance-ratchet/2".into(),
            lineage: "tondo-draft".into(),
            revision: 24,
            manifest: evidence(),
            inventory: evidence(),
            matrix: evidence(),
            gap_audit: evidence(),
            quality_baseline: evidence(),
            draft_case_layers: 1,
            pending_tasks: Vec::new(),
            coverage: scope(&"c".repeat(64), &"d".repeat(64)),
            mutation: scope(&"c".repeat(64), &"d".repeat(64)),
        }
    }

    fn complete_coverage_json() -> Vec<u8> {
        let paths = [
            "crates/tondo-compiler/src/syntax/cst.rs",
            "crates/tondo-compiler/src/syntax/format/representative.rs",
            "crates/tondo-compiler/src/syntax/lexer.rs",
            "crates/tondo-compiler/src/syntax/parser.rs",
            "crates/tondo-compiler/src/hir/availability.rs",
            "crates/tondo-compiler/src/hir/capabilities.rs",
            "crates/tondo-compiler/src/hir/check.rs",
            "crates/tondo-compiler/src/hir/regions.rs",
            "crates/tondo-compiler/src/hir/terminal.rs",
            "crates/tondo-compiler/src/hir/traits.rs",
            "crates/tondo-compiler/src/resolve/representative.rs",
            "crates/tondo-compiler/src/types.rs",
            "crates/tondo-compiler/src/hir/verify.rs",
            "crates/tondo-compiler/src/mir/verify.rs",
            "crates/tondo-vm/src/bytecode/verify.rs",
            "crates/tondo-vm/src/runtime/heap.rs",
            "crates/tondo-vm/src/runtime/value.rs",
            "crates/tondo-compiler/src/bytecode/lower.rs",
            "crates/tondo-vm/src/runtime/execute.rs",
            "crates/tondo-compiler/src/artifact.rs",
            "crates/tondo-compiler/src/project.rs",
            "crates/tondo-conformance/src/lib.rs",
            "crates/tondo-reference-adapter/src/lib.rs",
            "crates/tondo-reliability/src/lib.rs",
        ];
        let metric = serde_json::json!({"count":100,"covered":100});
        serde_json::to_vec(&serde_json::json!({"data":[{
            "totals":{"lines":metric,"functions":metric,"regions":metric},
            "files":paths.into_iter().map(|path| serde_json::json!({
                "filename":format!("/workspace/{path}"),
                "summary":{"lines":metric,"functions":metric,"regions":metric}
            })).collect::<Vec<_>>()
        }]}))
        .unwrap()
    }

    fn closed_candidate_fixture() -> (PathBuf, CandidateInputs<'static>) {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let proof_directory = repository.join("conformance/proofs/revision-23");
        let mut proof: PromotionProofManifest =
            serde_json::from_slice(&fs::read(proof_directory.join("manifest.json")).unwrap())
                .unwrap();
        let mut source_bytes = proof
            .files
            .iter()
            .map(|file| {
                (
                    file.source_path.clone(),
                    fs::read(proof_directory.join(&file.object.path)).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let result: ComposedSuiteResult =
            parse_role(&proof, &source_bytes, "conformance-result").unwrap();
        let baseline =
            QualityBaseline::from_bytes(&source_bytes["testing/quality-baseline.json"]).unwrap();
        let coverage = complete_coverage_json();
        let mutation = serde_json::to_vec(&serde_json::json!({
            "outcomes": (0..baseline.mutation.total).map(|index| {
                serde_json::json!({
                    "id":format!("mutant-{index}"),
                    "status":if index < baseline.mutation.caught { "caught" } else { "unviable" }
                })
            }).collect::<Vec<_>>()
        }))
        .unwrap();
        let provenance = QualityProvenance {
            format: crate::provenance::FORMAT.into(),
            tree_sha256: result.tree_sha256.clone(),
            input_set_sha256: result.input_set_sha256.clone(),
            file_count: 1,
            flags: Vec::new(),
            toolchain: Toolchain {
                rustc: "rustc fixture".into(),
                cargo: "cargo fixture".into(),
            },
        };
        let coverage_binding = ReportBinding::new(
            "coverage",
            &coverage,
            provenance.clone(),
            provenance.clone(),
        )
        .unwrap();
        let mutation_binding =
            ReportBinding::new("mutation", &mutation, provenance.clone(), provenance).unwrap();
        let mut ratchet: RatchetRecord =
            parse_embedded(&source_bytes, "testing/conformance-ratchet.json").unwrap();
        for (scope, binding) in [
            (&mut ratchet.coverage, &coverage_binding),
            (&mut ratchet.mutation, &mutation_binding),
        ] {
            scope.report_sha256 = Some(binding.report_sha256.clone());
            scope.provenance_sha256 = Some(binding.provenance_digest().unwrap());
        }
        source_bytes.insert(
            "testing/conformance-ratchet.json".into(),
            canonical_json(&ratchet).unwrap(),
        );

        let mut proof_objects = BTreeMap::new();
        for file in &mut proof.files {
            let bytes = &source_bytes[&file.source_path];
            let digest = sha256(bytes);
            file.object = PinnedFile {
                path: format!("objects/{digest}"),
                sha256: digest,
            };
            proof_objects.insert(file.object.path.clone(), bytes.clone());
        }
        let proof_manifest = canonical_json(&proof).unwrap();
        verify_promotion_proof_objects(&proof_manifest, &proof_objects).unwrap();

        let mut evidence = BTreeMap::new();
        for observed in result
            .case_layers
            .iter()
            .flat_map(|layer| &layer.cases)
            .flat_map(|case| &case.evidence)
        {
            evidence.insert(
                observed.id.clone(),
                crate::layer_evidence::EvidenceObservation {
                    id: observed.id.clone(),
                    source_sha256: observed.source_sha256.clone(),
                    observation_sha256: observed.observation_sha256.clone(),
                },
            );
        }
        let layer = LayerEvidenceReport {
            format: LAYER_EVIDENCE_FORMAT.into(),
            lineage: result.lineage.clone(),
            revision: result.revision,
            manifest_sha256: result.lineage_manifest_sha256.clone(),
            inventory_sha256: result.inventory_sha256.clone(),
            tree_sha256: result.tree_sha256.clone(),
            input_set_sha256: result.input_set_sha256.clone(),
            passed: true,
            evidence: evidence.into_values().collect(),
        };
        let inventory: Value = parse_embedded(&source_bytes, "testing/inventory.json").unwrap();
        let public_evidence = inventory["tests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|test| {
                test["status"] == "executable"
                    && matches!(
                        test["kind"].as_str(),
                        Some("conformance-case" | "rust-test")
                    )
            })
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let doc_records = DOC_TEST_DOCUMENTS
            .iter()
            .enumerate()
            .map(|(index, document)| {
                serde_json::json!({
                    "file":document,
                    "fence_byte":index,
                    "category":if index == 0 { "fragment" } else { "syntax" },
                    "edition":"0.1",
                    "fixture":null,
                    "fixture_sha256":null,
                    "production":if index == 0 { ["expression"] } else { ["module_program"] },
                    "source_sha256":format!("{:064x}", index + 1),
                    "formatted_sha256":format!("{:064x}", index + 11),
                    "parse_ok":true,
                    "typecheck_ok":if index == 0 { Value::Bool(true) } else { Value::Null },
                    "expected_codes":[],
                    "actual_codes":[]
                })
            })
            .collect::<Vec<_>>();
        let doc_links = serde_json::json!({
            "format":"tondo-doc-test-runtime-links/1",
            "edition":"0.1",
            "documents":DOC_TEST_DOCUMENTS,
            "rules":{
                "typed_fences_are_classified":true,
                "syntax_fences_make_no_runtime_claim":true,
                "runtime_evidence_is_public_and_executable":true,
                "documentation_runner_never_executes_examples":true
            },
            "links":[{
                "document":DOC_TEST_DOCUMENTS[0],
                "fence_byte":0,
                "source_sha256":format!("{:064x}", 1),
                "behavior":"runtime",
                "evidence":[public_evidence]
            }]
        });

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tondo-candidate-closed-fixture-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("proof/objects")).unwrap();
        fs::create_dir(root.join("inputs")).unwrap();
        fs::write(root.join("proof/manifest.json"), proof_manifest).unwrap();
        for (path, bytes) in proof_objects {
            fs::write(root.join("proof").join(path), bytes).unwrap();
        }
        for (path, bytes) in [
            ("inputs/coverage.json", coverage),
            (
                "inputs/coverage-binding.json",
                canonical_json(&coverage_binding).unwrap(),
            ),
            ("inputs/mutation.json", mutation),
            (
                "inputs/mutation-binding.json",
                canonical_json(&mutation_binding).unwrap(),
            ),
            ("inputs/layer.json", canonical_json(&layer).unwrap()),
            (
                "inputs/doc-test.json",
                serde_json::to_vec(&doc_records).unwrap(),
            ),
            (
                "inputs/doc-links.json",
                serde_json::to_vec(&doc_links).unwrap(),
            ),
        ] {
            fs::write(root.join(path), bytes).unwrap();
        }
        let inputs = CandidateInputs {
            proof: Path::new("proof"),
            coverage: Path::new("inputs/coverage.json"),
            coverage_binding: Path::new("inputs/coverage-binding.json"),
            mutation: Path::new("inputs/mutation.json"),
            mutation_binding: Path::new("inputs/mutation-binding.json"),
            layer_evidence: Path::new("inputs/layer.json"),
            doc_test: Path::new("inputs/doc-test.json"),
            doc_test_links: Path::new("inputs/doc-links.json"),
        };
        (root, inputs)
    }

    #[test]
    fn candidate_manifest_is_closed_canonical_and_path_bounded() {
        let candidate = manifest();
        validate_manifest(&candidate).unwrap();
        assert_eq!(
            serde_json::from_slice::<CandidateManifest>(&canonical_json(&candidate).unwrap())
                .unwrap(),
            candidate
        );
        for path in ["", "/absolute", "../escape", "nested/../escape"] {
            assert!(require_relative(Path::new(path), "test path").is_err());
        }

        let mut invalid = candidate.clone();
        invalid.gates.reverse();
        assert!(validate_manifest(&invalid).is_err());
        let mut invalid = candidate.clone();
        invalid.files.pop();
        assert!(validate_manifest(&invalid).is_err());
        let mut invalid = candidate;
        invalid.files.reverse();
        assert!(validate_manifest(&invalid).is_err());
    }

    #[test]
    fn normative_gate_allows_only_separate_stdlib_boundaries() {
        let matrix = serde_json::json!({
            "requirements": [
                {"id":"TL01-1-R001","status":"covered"},
                {"id":"TL01-26-5-R001","status":"stdlib-pending"},
                {"id":"TT01-1-R001","status":"covered"},
                {"id":"TC01-1-R001","status":"target-not-applicable"}
            ]
        });
        let mut embedded = BTreeMap::from([(
            "testing/coverage-matrix.json".into(),
            serde_json::to_vec(&matrix).unwrap(),
        )]);
        validate_normative_gate(&embedded).unwrap();

        embedded.insert(
            "testing/coverage-matrix.json".into(),
            serde_json::to_vec(&serde_json::json!({
                "requirements": [
                    {"id":"TL01-1-R001","status":"covered"},
                    {"id":"TT01-1-R001","status":"draft-pending"},
                    {"id":"TC01-1-R001","status":"covered"}
                ]
            }))
            .unwrap(),
        );
        assert!(validate_normative_gate(&embedded).is_err());
    }

    #[test]
    fn layer_gate_requires_the_exact_fresh_composed_observation_set() {
        let result = composed_result();
        let mut layer = LayerEvidenceReport {
            format: LAYER_EVIDENCE_FORMAT.into(),
            lineage: "tondo-draft".into(),
            revision: 24,
            manifest_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            tree_sha256: "c".repeat(64),
            input_set_sha256: "d".repeat(64),
            passed: true,
            evidence: vec![crate::layer_evidence::EvidenceObservation {
                id: "rust:test:passes".into(),
                source_sha256: "1".repeat(64),
                observation_sha256: "2".repeat(64),
            }],
        };
        validate_layer_gate(&manifest(), &ratchet(), &result, &layer).unwrap();

        layer.evidence.push(layer.evidence[0].clone());
        assert!(validate_layer_gate(&manifest(), &ratchet(), &result, &layer).is_err());
        layer.evidence.pop();
        layer.evidence[0].source_sha256 = "9".repeat(64);
        assert!(validate_layer_gate(&manifest(), &ratchet(), &result, &layer).is_err());
    }

    #[test]
    fn quality_gate_revalidates_raw_reports_bindings_and_baseline() {
        let coverage = complete_coverage_json();
        let mutation = serde_json::to_vec(&serde_json::json!({
            "outcomes":[{"status":"caught"}]
        }))
        .unwrap();
        let observed = parse_llvm_cov(&coverage).unwrap();
        let provenance = QualityProvenance {
            format: crate::provenance::FORMAT.into(),
            tree_sha256: "c".repeat(64),
            input_set_sha256: "d".repeat(64),
            file_count: 1,
            flags: Vec::new(),
            toolchain: Toolchain {
                rustc: "rustc test".into(),
                cargo: "cargo test".into(),
            },
        };
        let coverage_binding = ReportBinding::new(
            "coverage",
            &coverage,
            provenance.clone(),
            provenance.clone(),
        )
        .unwrap();
        let mutation_binding =
            ReportBinding::new("mutation", &mutation, provenance.clone(), provenance).unwrap();
        let baseline = QualityBaseline {
            format: crate::quality::FORMAT.into(),
            revision: "candidate-test".into(),
            provenance: coverage_binding.after.clone(),
            coverage: CoverageBaseline {
                tool: "cargo-llvm-cov".into(),
                command: "cargo llvm-cov".into(),
                global: observed.global.clone(),
                risk_scopes: observed.risk_scopes.clone(),
                maximum_drop_basis_points: 0,
            },
            mutation: MutationBaseline {
                tool: "cargo-mutants".into(),
                command: "cargo mutants".into(),
                selected_paths: vec!["src/lib.rs".into()],
                total: 1,
                caught: 1,
                missed: 0,
                timeout: 0,
                unviable: 0,
                score_basis_points: 10_000,
                minimum_score_basis_points: 10_000,
                survivors: Vec::new(),
            },
        };
        baseline.validate().unwrap();
        let mut ratchet = ratchet();
        ratchet.coverage.report_sha256 = Some(coverage_binding.report_sha256.clone());
        ratchet.coverage.provenance_sha256 = Some(coverage_binding.provenance_digest().unwrap());
        ratchet.mutation.report_sha256 = Some(mutation_binding.report_sha256.clone());
        ratchet.mutation.provenance_sha256 = Some(mutation_binding.provenance_digest().unwrap());
        let source_storage = BTreeMap::from([
            (COVERAGE_SOURCE.to_owned(), coverage),
            (
                COVERAGE_BINDING_SOURCE.to_owned(),
                canonical_json(&coverage_binding).unwrap(),
            ),
            (MUTATION_SOURCE.to_owned(), mutation),
            (
                MUTATION_BINDING_SOURCE.to_owned(),
                canonical_json(&mutation_binding).unwrap(),
            ),
        ]);
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let embedded = BTreeMap::from([(
            "testing/quality-baseline.json".into(),
            canonical_json(&baseline).unwrap(),
        )]);
        validate_quality_gate(&sources, &embedded, &ratchet, &composed_result()).unwrap();

        ratchet.coverage.report_sha256 = Some("0".repeat(64));
        assert!(validate_quality_gate(&sources, &embedded, &ratchet, &composed_result()).is_err());
    }

    #[test]
    fn candidate_provenance_rejects_every_extra_or_substituted_source() {
        let mut candidate = manifest();
        let roles = BTreeMap::from([
            (PROOF_MANIFEST_SOURCE, "promotion-proof-manifest"),
            (COVERAGE_SOURCE, "coverage-report"),
            (COVERAGE_BINDING_SOURCE, "coverage-binding"),
            (MUTATION_SOURCE, "mutation-report"),
            (MUTATION_BINDING_SOURCE, "mutation-binding"),
            (LAYER_EVIDENCE_SOURCE, "layer-evidence"),
            (DOC_TEST_SOURCE, "doc-test-report"),
            (DOC_TEST_LINKS_SOURCE, "doc-test-links"),
        ]);
        for file in &mut candidate.files {
            file.role = roles[file.source_path.as_str()].into();
        }
        candidate.files.sort_by(|left, right| {
            (&left.role, &left.source_path).cmp(&(&right.role, &right.source_path))
        });
        let proof = PromotionProofManifest {
            format: "tondo-conformance-promotion-proof/1".into(),
            edition: candidate.edition.clone(),
            state: "promotion-proof".into(),
            lineage: candidate.lineage.clone(),
            target: candidate.target.clone(),
            adapter: candidate.adapter.clone(),
            files: Vec::new(),
        };
        validate_candidate_file_closure(&candidate, &proof).unwrap();

        candidate.files.push(CandidateFile {
            role: "extra".into(),
            source_path: "evidence/unexpected.json".into(),
            object: PinnedFile {
                path: format!("objects/{}", "f".repeat(64)),
                sha256: "f".repeat(64),
            },
        });
        assert!(validate_candidate_file_closure(&candidate, &proof).is_err());
    }

    #[test]
    fn candidate_seals_verifies_and_rejects_files_outside_its_offline_closure() {
        let (root, inputs) = closed_candidate_fixture();
        let output = Path::new("candidate");
        assert_eq!(
            seal_candidate(&root, &inputs, output).unwrap(),
            CandidateOutcome::Created
        );
        assert_eq!(
            seal_candidate(&root, &inputs, output).unwrap(),
            CandidateOutcome::AlreadyPresent
        );
        assert_eq!(verify_candidate(&root, output).unwrap().gates, GATES);

        fs::write(root.join("candidate/objects/extra"), b"extra").unwrap();
        assert!(verify_candidate(&root, output).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_helpers_reject_malformed_json_missing_objects_and_unsafe_paths() {
        let empty_sources = BTreeMap::new();
        assert!(source(&empty_sources, "missing").is_err());
        assert!(parse_source::<Value>(&empty_sources, "missing").is_err());
        let invalid_sources = BTreeMap::from([("invalid", b"{".as_slice())]);
        assert!(parse_source::<Value>(&invalid_sources, "invalid").is_err());
        assert!(parse_source::<Vec<Value>>(&invalid_sources, "invalid").is_err());
        assert!(parse_source::<ReportBinding>(&invalid_sources, "invalid").is_err());

        let empty_embedded = BTreeMap::new();
        assert!(parse_embedded::<Value>(&empty_embedded, "missing").is_err());
        assert!(parse_embedded::<RatchetRecord>(&empty_embedded, "missing").is_err());
        assert!(parse_embedded::<ComposedSuiteResult>(&empty_embedded, "missing").is_err());
        let invalid_embedded = BTreeMap::from([("invalid".into(), b"{".to_vec())]);
        assert!(parse_embedded::<Value>(&invalid_embedded, "invalid").is_err());
        assert!(parse_embedded::<RatchetRecord>(&invalid_embedded, "invalid").is_err());
        assert!(parse_embedded::<ComposedSuiteResult>(&invalid_embedded, "invalid").is_err());

        let mut proof = PromotionProofManifest {
            format: "tondo-conformance-promotion-proof/1".into(),
            edition: "0.1".into(),
            state: "promotion-proof".into(),
            lineage: manifest().lineage,
            target: manifest().target,
            adapter: manifest().adapter,
            files: Vec::new(),
        };
        assert!(parse_role::<Value>(&proof, &empty_embedded, "missing").is_err());
        proof.files.push(tondo_conformance::seal::ProofFile {
            role: "input".into(),
            source_path: "missing.json".into(),
            object: PinnedFile {
                path: format!("objects/{}", "a".repeat(64)),
                sha256: "a".repeat(64),
            },
        });
        assert!(proof_sources(&proof, &BTreeMap::new()).is_err());

        assert!(text_field(&serde_json::json!({}), "missing").is_err());
        assert!(object_keys(&Value::Null).is_err());
        assert!(doc_key(&serde_json::json!({}), "file").is_err());
        assert!(
            doc_key(
                &serde_json::json!({
                    "file":"spec.md", "source_sha256":"a".repeat(64)
                }),
                "file"
            )
            .is_err()
        );
        assert!(
            doc_key(
                &serde_json::json!({
                    "file":"spec.md", "fence_byte":0, "source_sha256":"invalid"
                }),
                "file"
            )
            .is_err()
        );
        assert!(validate_bundle(b"{", &BTreeMap::new()).is_err());
        assert!(
            validate_normative_gate(&BTreeMap::from([(
                "testing/coverage-matrix.json".into(),
                b"{}".to_vec(),
            )]))
            .is_err()
        );
        assert!(
            validate_quality_gate(
                &empty_sources,
                &empty_embedded,
                &ratchet(),
                &composed_result()
            )
            .is_err()
        );
        let mut no_testing = composed_result();
        no_testing.case_layers.clear();
        let layer = LayerEvidenceReport {
            format: LAYER_EVIDENCE_FORMAT.into(),
            lineage: no_testing.lineage.clone(),
            revision: no_testing.revision,
            manifest_sha256: no_testing.lineage_manifest_sha256.clone(),
            inventory_sha256: no_testing.inventory_sha256.clone(),
            tree_sha256: no_testing.tree_sha256.clone(),
            input_set_sha256: no_testing.input_set_sha256.clone(),
            passed: true,
            evidence: Vec::new(),
        };
        assert!(validate_layer_gate(&manifest(), &ratchet(), &no_testing, &layer).is_err());

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tondo-candidate-helper-errors-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("objects/directory")).unwrap();
        assert!(read_regular(&root.join("missing")).is_err());
        assert!(read_regular(&root).is_err());
        assert!(read_file_names(&root.join("missing")).is_err());
        assert!(read_file_names(&root.join("objects")).is_err());
        let existing = root.join("existing");
        fs::write(&existing, b"existing").unwrap();
        assert!(write_new(&existing, b"replacement").is_err());
        assert!(resolve_existing_directory(&root, Path::new("missing")).is_err());
        assert!(resolve_directory(&existing, Path::new("child"), true).is_err());
        assert!(resolve_existing_directory(&root.join("missing"), Path::new("child")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn doc_test_gate_requires_exact_typed_links_and_executable_evidence() {
        let records = DOC_TEST_DOCUMENTS
            .iter()
            .enumerate()
            .map(|(index, document)| {
                serde_json::json!({
                    "file":document,
                    "fence_byte":42 + index,
                    "category":if index == 0 { "script" } else { "syntax" },
                    "edition":"0.1",
                    "fixture":null,
                    "fixture_sha256":null,
                    "production":if index == 0 { "script" } else { "item" },
                    "source_sha256":format!("{index:064x}"),
                    "formatted_sha256":"b".repeat(64),
                    "parse_ok":true,
                    "typecheck_ok":if index == 0 { Value::Bool(true) } else { Value::Null },
                    "expected_codes":[],
                    "actual_codes":[]
                })
            })
            .collect::<Vec<_>>();
        let links = serde_json::json!({
            "format":"tondo-doc-test-runtime-links/1", "edition":"0.1",
            "documents":DOC_TEST_DOCUMENTS,
            "rules":{
                "typed_fences_are_classified":true,
                "syntax_fences_make_no_runtime_claim":true,
                "runtime_evidence_is_public_and_executable":true,
                "documentation_runner_never_executes_examples":true
            },
            "links":[{
                "document":"TONDO_LANGUAGE_SPEC.md", "fence_byte":42,
                "source_sha256":format!("{:064x}", 0), "behavior":"runtime",
                "evidence":["rust:test:passes"]
            }]
        });
        let inventory = serde_json::json!({"tests":[{
            "id":"rust:test:passes", "status":"executable", "kind":"rust-test"
        }]});
        let mut source_storage = BTreeMap::from([
            (
                DOC_TEST_SOURCE.to_owned(),
                serde_json::to_vec(&records).unwrap(),
            ),
            (
                DOC_TEST_LINKS_SOURCE.to_owned(),
                serde_json::to_vec(&links).unwrap(),
            ),
        ]);
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let embedded = BTreeMap::from([(
            "testing/inventory.json".into(),
            serde_json::to_vec(&inventory).unwrap(),
        )]);
        validate_doc_test_gate(&sources, &embedded).unwrap();

        source_storage.insert(
            DOC_TEST_LINKS_SOURCE.into(),
            serde_json::to_vec(&serde_json::json!({
                "format":"tondo-doc-test-runtime-links/1",
                "edition":"0.1",
                "documents":DOC_TEST_DOCUMENTS,
                "rules":{
                    "typed_fences_are_classified":true,
                    "syntax_fences_make_no_runtime_claim":true,
                    "runtime_evidence_is_public_and_executable":true,
                    "documentation_runner_never_executes_examples":true
                },
                "links":[]
            }))
            .unwrap(),
        );
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert!(validate_doc_test_gate(&sources, &embedded).is_err());

        let mut compile_fail_records = records.clone();
        compile_fail_records[0]["category"] = Value::String("compile-fail".into());
        compile_fail_records[0]["typecheck_ok"] = Value::Bool(false);
        compile_fail_records[0]["expected_codes"] = serde_json::json!(["T9001"]);
        compile_fail_records[0]["actual_codes"] = serde_json::json!(["T9001"]);
        source_storage.insert(
            DOC_TEST_SOURCE.into(),
            serde_json::to_vec(&compile_fail_records).unwrap(),
        );
        source_storage.insert(
            DOC_TEST_LINKS_SOURCE.into(),
            serde_json::to_vec(&links).unwrap(),
        );
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert!(validate_doc_test_gate(&sources, &embedded).is_err());

        source_storage.insert(
            DOC_TEST_SOURCE.into(),
            serde_json::to_vec(&records).unwrap(),
        );
        let missing_inventory = BTreeMap::from([(
            "testing/inventory.json".into(),
            serde_json::to_vec(&serde_json::json!({})).unwrap(),
        )]);
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert!(validate_doc_test_gate(&sources, &missing_inventory).is_err());

        let mut invalid_links = links.clone();
        invalid_links["links"] = Value::Null;
        source_storage.insert(
            DOC_TEST_LINKS_SOURCE.into(),
            serde_json::to_vec(&invalid_links).unwrap(),
        );
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert!(validate_doc_test_gate(&sources, &embedded).is_err());

        for evidence in [
            serde_json::json!([]),
            serde_json::json!([1]),
            serde_json::json!(["missing"]),
        ] {
            let mut invalid_links = links.clone();
            invalid_links["links"][0]["evidence"] = evidence;
            source_storage.insert(
                DOC_TEST_LINKS_SOURCE.into(),
                serde_json::to_vec(&invalid_links).unwrap(),
            );
            let sources = source_storage
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
                .collect();
            assert!(validate_doc_test_gate(&sources, &embedded).is_err());
        }

        let mut ignored = links;
        ignored["links"][0] = serde_json::json!({
            "document":"TONDO_LANGUAGE_SPEC.md",
            "fence_byte":42,
            "source_sha256":format!("{:064x}", 0),
            "behavior":"ignored",
            "reason":""
        });
        source_storage.insert(
            DOC_TEST_LINKS_SOURCE.into(),
            serde_json::to_vec(&ignored).unwrap(),
        );
        let sources = source_storage
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert!(validate_doc_test_gate(&sources, &embedded).is_err());
    }
}
