use std::path::Path;

use serde::Serialize;
use serde_json::json;
use tondo_compiler::artifact::{
    DeclaredBuildInputs, FeatureName, SourceSetId, sha256 as compiler_sha256,
};
use tondo_compiler::project::{
    BOOTSTRAP_STANDARD_PACKAGE, LOCKFILE_FORMAT, MANIFEST_FORMAT, ProjectInputKind, ProjectPlan,
};
use tondo_reliability::quality::{QualityBaseline, parse_llvm_cov, parse_mutation_report};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the reliability crate must live in the workspace")
}

#[test]
fn public_quality_protocols_accept_realistic_nested_reports() {
    let root = repository_root();
    let baseline = QualityBaseline::load(&root.join("testing/quality-baseline.json")).unwrap();

    let metrics = json!({
        "lines": {"count": 1, "covered": 1},
        "functions": {"count": 1, "covered": 1},
        "regions": {"count": 1, "covered": 1}
    });
    let files = baseline
        .coverage
        .risk_scopes
        .iter()
        .flat_map(|scope| scope.paths.iter())
        .map(|path| {
            let filename = if path.ends_with('/') {
                format!("/workspace/{path}representative.rs")
            } else {
                format!("/workspace/{path}")
            };
            json!({"filename": filename, "summary": metrics})
        })
        .collect::<Vec<_>>();
    let coverage = json!({
        "data": [{
            "totals": {
                "lines": {"count": 1, "covered": 1},
                "functions": {"count": 1, "covered": 1},
                "regions": {"count": 1, "covered": 1}
            },
            "files": files
        }]
    });
    let coverage = parse_llvm_cov(&serde_json::to_vec(&coverage).unwrap()).unwrap();
    assert_eq!(coverage.global.lines.basis_points, 10_000);
    assert_eq!(
        coverage.risk_scopes.len(),
        baseline.coverage.risk_scopes.len()
    );

    let mutation = json!({
        "outcomes": [
            {"outcome": "CaughtMutant"},
            {"outcome": "MissedMutant", "metadata": {"nested": {"id": "survivor"}}}
        ]
    });
    let mutation = parse_mutation_report(&serde_json::to_vec(&mutation).unwrap()).unwrap();
    assert_eq!(mutation.total, 2);
    assert_eq!(mutation.caught, 1);
    assert_eq!(mutation.missed_ids, ["survivor"]);
}

#[test]
fn public_artifact_identity_accessors_preserve_canonical_values() {
    let feature = FeatureName::new("runtime").unwrap();
    let source_set = SourceSetId::new("@3:app#core").unwrap();
    let inputs = DeclaredBuildInputs::new(
        [feature.clone()].into_iter().collect(),
        [source_set.clone()].into_iter().collect(),
    );

    assert_eq!(feature.as_str(), "runtime");
    assert_eq!(source_set.as_str(), "@3:app#core");
    assert!(inputs.features().contains(&feature));
    assert!(inputs.source_sets().contains(&source_set));
    assert!(inputs.manifest_hash().is_none());
    assert!(inputs.lockfile_hash().is_none());
    assert!(inputs.generator_inputs().is_empty());
    assert!(inputs.dependency_interfaces().is_empty());
    assert!(!inputs.require_dependency_interfaces());
}

#[test]
fn public_project_plan_consumes_canonical_manifest_and_lockfile() {
    let package = "workspace:app@1";
    let source = b"fn main() {}\n";
    #[derive(Clone, Serialize)]
    struct SourceFingerprint {
        source_set: String,
        physical_path: String,
        logical_path: String,
        module: String,
        sha256: String,
    }
    let source_fingerprint = SourceFingerprint {
        source_set: "common".into(),
        physical_path: "app/src/main.to".into(),
        logical_path: "src/main.to".into(),
        module: "main".into(),
        sha256: compiler_sha256(source),
    };
    let manifest = json!({
        "format": MANIFEST_FORMAT,
        "target": {
            "name": "tondo-vm-hosted",
            "profile": "hosted",
            "capability_registry": "tondo-capabilities-draft",
            "capabilities": ["console"],
            "features": ["fast"]
        },
        "root": {
            "package": package,
            "source": "app/src/main.to",
            "form": "module"
        },
        "standard": BOOTSTRAP_STANDARD_PACKAGE,
        "packages": [{
            "id": package,
            "local_name": "app",
            "edition": "0.1",
            "dependencies": [],
            "source_sets": [{
                "id": "common",
                "sources": [{
                    "physical_path": "app/src/main.to",
                    "logical_path": "src/main.to",
                    "module": "main"
                }]
            }]
        }],
        "generator_inputs": [],
        "privileged_units": []
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    #[derive(Serialize)]
    struct PackageFingerprint {
        package_id: String,
        dependencies: Vec<serde_json::Value>,
        sources: Vec<SourceFingerprint>,
        interface_hash: Option<String>,
    }
    let package_fingerprint = PackageFingerprint {
        package_id: package.into(),
        dependencies: Vec::new(),
        sources: vec![source_fingerprint.clone()],
        interface_hash: None,
    };
    let lockfile = json!({
        "format": LOCKFILE_FORMAT,
        "manifest_hash": compiler_sha256(&manifest_bytes),
        "standard": {
            "package_id": BOOTSTRAP_STANDARD_PACKAGE,
            "content_hash": tondo_compiler::project::bootstrap_standard_hash()
        },
        "packages": [{
            "id": package,
            "content_hash": compiler_sha256(&serde_json::to_vec(&package_fingerprint).unwrap()),
            "dependencies": [],
            "sources": [serde_json::to_value(source_fingerprint).unwrap()],
            "interface": null
        }],
        "generator_inputs": [],
        "privileged_units": []
    });

    let plan = ProjectPlan::parse(&manifest_bytes, &serde_json::to_vec(&lockfile).unwrap())
        .expect("canonical project records should produce a plan");
    assert_eq!(plan.target_name(), "tondo-vm-hosted");
    assert_eq!(plan.profile().as_str(), "hosted");
    assert_eq!(
        plan.selected_source_paths().collect::<Vec<_>>(),
        ["app/src/main.to"]
    );
    assert_eq!(plan.required_inputs().count(), 1);
    assert_eq!(
        plan.required_inputs().next().unwrap().kind(),
        ProjectInputKind::Source
    );
    assert_eq!(plan.features().len(), 1);
    assert_eq!(plan.selected_source_sets().len(), 1);
}
