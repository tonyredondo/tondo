use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde_json::Value;
use tondo_conformance::manifest::PinnedFile;
use tondo_conformance::runner::ComposedSuiteResult;
use tondo_conformance::seal::{PromotionProofManifest, verify_promotion_proof_objects};
use tondo_reliability::canonical_json;
use tondo_reliability::layer_evidence::{
    EvidenceObservation, FORMAT as LAYER_EVIDENCE_FORMAT, LayerEvidenceReport,
};
use tondo_reliability::provenance::{QualityProvenance, ReportBinding, Toolchain};
use tondo_reliability::quality::{QualityBaseline, parse_llvm_cov};
use tondo_reliability::ratchet::RatchetRecord;
use tondo_reliability::sha256;

const DOCUMENTS: [&str; 5] = [
    "TONDO_LANGUAGE_SPEC.md",
    "TONDO_TESTING_SPEC.md",
    "TONDO_TOOLCHAIN_SPEC.md",
    "TONDO_STANDARD_LIBRARY_SPEC.md",
    "TONDO_LLM_FORM_SPEC.md",
];

pub struct CandidateFixture {
    pub root: PathBuf,
    pub proof: &'static str,
    pub coverage: &'static str,
    pub coverage_binding: &'static str,
    pub mutation: &'static str,
    pub mutation_binding: &'static str,
    pub layer: &'static str,
    pub doc_test: &'static str,
    pub doc_links: &'static str,
    pub output: &'static str,
}

impl CandidateFixture {
    pub fn new(repository: &Path) -> Self {
        let proof_directory = repository.join("conformance/proofs/revision-23");
        let mut proof: PromotionProofManifest = read_json(&proof_directory.join("manifest.json"));
        let mut sources = proof
            .files
            .iter()
            .map(|file| {
                (
                    file.source_path.clone(),
                    fs::read(proof_directory.join(&file.object.path)).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let result: ComposedSuiteResult = parse_role(&proof, &sources, "conformance-result");
        let baseline: QualityBaseline = parse_source(&sources, "testing/quality-baseline.json");
        let coverage = complete_coverage();
        let observed = parse_llvm_cov(&coverage).unwrap();
        assert!(baseline.verify_coverage_report(&observed).is_ok());
        let mutation = serde_json::to_vec(&serde_json::json!({
            "outcomes": (0..baseline.mutation.total).map(|index| serde_json::json!({
                "id": format!("mutant-{index}"),
                "status": if index < baseline.mutation.caught { "caught" } else { "unviable" }
            })).collect::<Vec<_>>()
        }))
        .unwrap();
        let provenance = QualityProvenance {
            format: tondo_reliability::provenance::FORMAT.into(),
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
        let mut ratchet: RatchetRecord = parse_source(&sources, "testing/conformance-ratchet.json");
        for (scope, binding) in [
            (&mut ratchet.coverage, &coverage_binding),
            (&mut ratchet.mutation, &mutation_binding),
        ] {
            scope.report_sha256 = Some(binding.report_sha256.clone());
            scope.provenance_sha256 = Some(binding.provenance_digest().unwrap());
        }
        sources.insert(
            "testing/conformance-ratchet.json".into(),
            canonical_json(&ratchet).unwrap(),
        );

        let mut proof_objects = BTreeMap::new();
        for file in &mut proof.files {
            let bytes = &sources[&file.source_path];
            let digest = sha256(bytes);
            file.object = PinnedFile {
                path: format!("objects/{digest}"),
                sha256: digest,
            };
            proof_objects.insert(file.object.path.clone(), bytes.clone());
        }
        let proof_manifest = canonical_json(&proof).unwrap();
        verify_promotion_proof_objects(&proof_manifest, &proof_objects).unwrap();

        let layer = LayerEvidenceReport {
            format: LAYER_EVIDENCE_FORMAT.into(),
            lineage: result.lineage.clone(),
            revision: result.revision,
            manifest_sha256: result.lineage_manifest_sha256.clone(),
            inventory_sha256: result.inventory_sha256.clone(),
            tree_sha256: result.tree_sha256.clone(),
            input_set_sha256: result.input_set_sha256.clone(),
            passed: true,
            evidence: result
                .case_layers
                .iter()
                .flat_map(|layer| &layer.cases)
                .flat_map(|case| &case.evidence)
                .map(|item| {
                    (
                        item.id.clone(),
                        EvidenceObservation {
                            id: item.id.clone(),
                            source_sha256: item.source_sha256.clone(),
                            observation_sha256: item.observation_sha256.clone(),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect(),
        };
        let inventory: Value = parse_source(&sources, "testing/inventory.json");
        let evidence = inventory["tests"]
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
            .unwrap();
        let records = DOCUMENTS
            .iter()
            .enumerate()
            .map(|(index, document)| {
                serde_json::json!({
                    "file": document,
                    "fence_byte": index,
                    "category": if index == 0 { "fragment" } else { "syntax" },
                    "edition": "0.1",
                    "fixture": null,
                    "fixture_sha256": null,
                    "production": if index == 0 { vec!["expression"] } else { vec!["module_program"] },
                    "source_sha256": format!("{:064x}", index + 1),
                    "formatted_sha256": format!("{:064x}", index + 11),
                    "parse_ok": true,
                    "typecheck_ok": if index == 0 { Value::Bool(true) } else { Value::Null },
                    "expected_codes": [],
                    "actual_codes": []
                })
            })
            .collect::<Vec<_>>();
        let links = serde_json::json!({
            "format": "tondo-doc-test-runtime-links/1",
            "edition": "0.1",
            "documents": DOCUMENTS,
            "rules": {
                "typed_fences_are_classified": true,
                "syntax_fences_make_no_runtime_claim": true,
                "runtime_evidence_is_public_and_executable": true,
                "documentation_runner_never_executes_examples": true
            },
            "links": [{
                "document": DOCUMENTS[0],
                "fence_byte": 0,
                "source_sha256": format!("{:064x}", 1),
                "behavior": "runtime",
                "evidence": [evidence]
            }]
        });

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tondo-candidate-cli-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("proof/objects")).unwrap();
        fs::create_dir(root.join("inputs")).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
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
                serde_json::to_vec(&records).unwrap(),
            ),
            ("inputs/doc-links.json", serde_json::to_vec(&links).unwrap()),
        ] {
            fs::write(root.join(path), bytes).unwrap();
        }
        Self {
            root,
            proof: "proof",
            coverage: "inputs/coverage.json",
            coverage_binding: "inputs/coverage-binding.json",
            mutation: "inputs/mutation.json",
            mutation_binding: "inputs/mutation-binding.json",
            layer: "inputs/layer.json",
            doc_test: "inputs/doc-test.json",
            doc_links: "inputs/doc-links.json",
            output: "candidate",
        }
    }
}

impl Drop for CandidateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn parse_source<T: DeserializeOwned>(sources: &BTreeMap<String, Vec<u8>>, path: &str) -> T {
    serde_json::from_slice(&sources[path]).unwrap()
}

fn parse_role<T: DeserializeOwned>(
    proof: &PromotionProofManifest,
    sources: &BTreeMap<String, Vec<u8>>,
    role: &str,
) -> T {
    let file = proof.files.iter().find(|file| file.role == role).unwrap();
    parse_source(sources, &file.source_path)
}

fn complete_coverage() -> Vec<u8> {
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
        "crates/tondo-conformance/src/representative.rs",
        "crates/tondo-reference-adapter/src/representative.rs",
        "crates/tondo-reliability/src/representative.rs",
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
