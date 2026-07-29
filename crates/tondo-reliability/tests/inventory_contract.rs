use std::fs;
use tondo_reliability::inventory::{self, TestEntry};
use tondo_reliability::matrix;
use tondo_reliability::{canonical_json, sha256, workspace_root};

#[test]
fn repository_inventory_and_matrix_are_deterministic_and_self_validating() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let first = inventory::build(&root).unwrap();
    let second = inventory::build(&root).unwrap();
    assert_eq!(first, second);
    inventory::validate(&first).unwrap();
    assert_same_evidence(
        "inventory",
        canonical_json(&first).unwrap(),
        fs::read(root.join("testing/inventory.json")).unwrap(),
    );

    let first_matrix = matrix::build(&root, &first).unwrap();
    let second_matrix = matrix::build(&root, &second).unwrap();
    assert_eq!(first_matrix, second_matrix);
    matrix::validate(&first_matrix).unwrap();
    assert_same_evidence(
        "coverage matrix",
        canonical_json(&first_matrix).unwrap(),
        fs::read(root.join("testing/coverage-matrix.json")).unwrap(),
    );
}

fn assert_same_evidence(name: &str, expected: Vec<u8>, observed: Vec<u8>) {
    assert!(
        expected == observed,
        "{name} differs: expected sha256 {}, observed sha256 {}",
        sha256(&expected),
        sha256(&observed)
    );
}

#[test]
fn duplicate_ids_and_incomplete_entries_are_rejected() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let mut inventory = inventory::build(&root).unwrap();
    inventory.tests.push(inventory.tests[0].clone());
    assert!(inventory::validate(&inventory).is_err());

    let mut inventory = inventory::build(&root).unwrap();
    inventory.tests.push(TestEntry {
        id: "invalid".into(),
        kind: "fixture".into(),
        crate_name: None,
        phase: String::new(),
        source: "missing".into(),
        fixture: None,
        group: "invalid".into(),
        requirements: Vec::new(),
        oracle: String::new(),
        repetitions: 0,
        source_sha256: String::new(),
        target: String::new(),
        document: None,
        edition: String::new(),
        status: String::new(),
        sidecars: Vec::new(),
    });
    assert!(inventory::validate(&inventory).is_err());
}

#[test]
fn command_detects_manifest_drift_without_rewriting_evidence() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let binary = env!("CARGO_BIN_EXE_tondo-reliability");
    let status = std::process::Command::new(binary)
        .args(["inventory", "check", "--root"])
        .arg(&root)
        .status()
        .unwrap();
    assert!(status.success());
}
