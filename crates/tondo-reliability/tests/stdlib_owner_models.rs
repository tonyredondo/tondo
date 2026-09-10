use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tondo_reliability::workspace_root;

fn root() -> PathBuf {
    workspace_root(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn load(root: &Path, relative: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(relative)).unwrap()).unwrap()
}

fn owner<'a>(registry: &'a Value, id: &str) -> &'a Value {
    registry["owners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == id)
        .unwrap_or_else(|| panic!("missing coordination owner {id}"))
}

#[test]
fn every_public_signature_has_a_declared_model_law() {
    let root = root();
    let registry = load(&root, "testing/stdlib-test-coordination.json");
    let api = load(&root, "testing/stdlib-public-api.json");
    let evidence = load(&root, "testing/stdlib-owner-evidence.json");

    assert_eq!(registry["status"], "open-coordination");
    assert_eq!(
        registry["rules"]["model_laws_do_not_promote_public_implementation"],
        true
    );
    let evidence_owners = evidence["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let registry_owners = registry["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(registry_owners, evidence_owners);

    let mut seen = BTreeSet::new();
    for row in api["rows"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let owner_id = row["owner"].as_str().unwrap();
        let coordinated = owner(&registry, owner_id);
        let public_surface = coordinated["public_api"].as_array().unwrap();
        let entry = public_surface
            .iter()
            .find(|candidate| candidate["id"] == id)
            .unwrap_or_else(|| panic!("signature {id} is not in the owner model"));
        assert_eq!(entry["symbol"], row["symbol"]);
        assert_eq!(entry["signature"], row["signature"]);
        assert_eq!(entry["implementation_status"], row["status"]);
        assert_eq!(entry["missing"], row["missing"]);
        assert!(coordinated["model"]["status"] == "verified");
        assert!(
            coordinated["model"]["laws"]
                .as_array()
                .is_some_and(|laws| !laws.is_empty())
        );
        assert!(seen.insert(id.to_owned()), "duplicate model signature {id}");
    }

    assert_eq!(seen.len(), 298);
    assert_eq!(registry["summary"]["public_signatures"], seen.len());
}

#[test]
fn owners_without_signature_rows_still_model_each_normative_requirement() {
    let root = root();
    let registry = load(&root, "testing/stdlib-test-coordination.json");
    let matrix = load(&root, "testing/stdlib-matrix.json");

    for coordinated in registry["owners"].as_array().unwrap() {
        let public_count = coordinated["public_api"].as_array().unwrap().len();
        let requirement_ids = coordinated["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        let expected_requirements = matrix["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["owner"] == coordinated["id"] && row["kind"] == "requirement")
            .map(|row| row["id"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(requirement_ids, expected_requirements);
        assert!(public_count > 0 || !requirement_ids.is_empty());
        assert!(
            coordinated["model"]["laws"]
                .as_array()
                .is_some_and(|laws| laws.len() >= 3)
        );
    }
}

#[test]
fn component_fuzz_evidence_does_not_promote_complete_owner_coverage() {
    let root = root();
    let registry = load(&root, "testing/stdlib-test-coordination.json");
    let contract = load(&root, "testing/stdlib-fuzz.json");
    let mut verified_components = BTreeSet::new();

    for coordinated in registry["owners"].as_array().unwrap() {
        let id = coordinated["id"].as_str().unwrap();
        let test = &coordinated["test"];
        // The filesystem audit reopened full owner evidence: focused File
        // tests do not establish admission for the other filesystem calls.
        if id == "std.fs" {
            assert_eq!(test["status"], "partial", "test evidence for {id}");
            assert!(
                test["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty())
            );
        } else {
            assert_eq!(test["status"], "verified", "test evidence for {id}");
        }
        assert!(!test["commands"].as_array().unwrap().is_empty());
        assert!(!test["refs"].as_array().unwrap().is_empty());
        for reference in test["refs"].as_array().unwrap() {
            let path = reference.as_str().unwrap().split('#').next().unwrap();
            assert!(
                root.join(path).exists(),
                "missing test reference {reference}"
            );
        }

        let fuzz = &coordinated["fuzz"];
        let status = fuzz["status"].as_str().unwrap();
        assert_eq!(status, "partial");
        assert!(!fuzz["campaigns"].as_array().unwrap().is_empty());
        assert!(!fuzz["refs"].as_array().unwrap().is_empty());
        assert!(
            fuzz["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
        );
        let route = owner(&contract, id);
        assert_eq!(fuzz["reason"], route["reason"]);
        assert_eq!(fuzz["evidence_kind"], route["evidence_kind"]);
        assert_eq!(fuzz["component_status"], route["component_status"]);
        if fuzz["component_status"] == "verified" {
            assert_eq!(fuzz["evidence_kind"], "kernel-invariants");
            verified_components.insert(id);
        }
    }

    assert_eq!(
        verified_components,
        BTreeSet::from([
            "std.format",
            "std.io",
            "std.json",
            "std.math",
            "std.messagepack",
            "std.path",
            "std.protobuf",
            "std.serialization",
            "std.testing",
        ])
    );
    assert_eq!(registry["summary"]["fuzz_verified"], 0);
    assert_eq!(registry["summary"]["fuzz_partial"], 22);
    assert_eq!(
        registry["summary"]["fuzz_component_verified"],
        verified_components.len()
    );
}
