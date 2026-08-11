use std::collections::{BTreeMap, BTreeSet};
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

fn owners(registry: &Value) -> impl Iterator<Item = &Value> {
    registry["owners"].as_array().unwrap().iter()
}

fn owner_map(registry: &Value) -> BTreeMap<String, &Value> {
    owners(registry)
        .map(|owner| (owner["id"].as_str().unwrap().to_owned(), owner))
        .collect()
}

#[test]
fn every_matrix_owner_has_a_documentation_record_and_contract() {
    let root = root();
    let docs = load(&root, "testing/stdlib-documentation.json");
    let matrix = load(&root, "testing/stdlib-matrix.json");
    let conformance = load(&root, "testing/stdlib-conformance-coordination.json");
    let docs_by_owner = owner_map(&docs);
    let matrix_ids = matrix["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|owner| owner["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let conformance_ids = conformance["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|owner| owner["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        docs_by_owner.keys().cloned().collect::<BTreeSet<_>>(),
        matrix_ids
    );
    assert_eq!(
        docs_by_owner.keys().cloned().collect::<BTreeSet<_>>(),
        conformance_ids
    );
    assert_eq!(docs["status"], "closed-coordination");
    assert_eq!(docs["promotion"]["status"], "not-published");

    for owner in owners(&docs) {
        assert_eq!(owner["status"], "documented-draft");
        assert!(
            owner["contract"]
                .as_str()
                .unwrap()
                .starts_with("docs/contracts/")
        );
        assert!(
            owner["docs"]
                .as_array()
                .is_some_and(|docs| !docs.is_empty())
        );
        assert!(
            owner["examples"]
                .as_array()
                .is_some_and(|examples| !examples.is_empty())
        );
        assert!(conformance_ids.contains(owner["id"].as_str().unwrap()));
    }
}

#[test]
fn examples_and_boundaries_are_verifiable_without_a_release_claim() {
    let root = root();
    let docs = load(&root, "testing/stdlib-documentation.json");

    for owner in owners(&docs) {
        let runtime_applicable = owner["runtime_applicable"].as_bool().unwrap();
        let examples = owner["examples"].as_array().unwrap();
        let runtime_examples = examples
            .iter()
            .filter(|example| example["kind"] == "runtime" || example["kind"] == "acceptance")
            .count();
        if runtime_applicable {
            assert!(
                runtime_examples > 0,
                "owner lacks runtime evidence: {}",
                owner["id"]
            );
            assert!(owner["runtime_reason"].is_null());
        } else {
            assert_eq!(runtime_examples, 0);
            assert!(!owner["runtime_reason"].as_str().unwrap().is_empty());
        }

        for example in examples {
            let source = example["source"].as_str().unwrap();
            assert!(root.join(source).is_file(), "missing example {source}");
            if example["kind"] == "runtime" {
                let fixture = root.join(source.strip_suffix(".to").unwrap());
                assert!(
                    fixture.with_extension("exit").is_file(),
                    "missing exit sidecar {source}"
                );
                assert!(
                    fixture.with_extension("stdout").is_file()
                        || fixture.with_extension("codes").is_file()
                );
            }
            let command = example["command"].as_str().unwrap();
            if let Some(script) = command.strip_prefix("scripts/") {
                assert!(root.join("scripts").join(script).is_file());
            }
        }

        assert!(matches!(
            owner["boundary"]["kernel"]["status"].as_str().unwrap(),
            "verified" | "partial" | "pending" | "gap"
        ));
        assert!(matches!(
            owner["boundary"]["bridge"]["status"].as_str().unwrap(),
            "verified" | "partial" | "pending" | "not-applicable"
        ));
        assert!(
            owner["boundary"]["kernel"]["refs"]
                .as_array()
                .is_some_and(|refs| !refs.is_empty())
        );
        assert!(
            owner["boundary"]["bridge"]["refs"]
                .as_array()
                .is_some_and(|refs| !refs.is_empty())
        );
    }

    assert!(docs["owners"].as_array().unwrap().iter().all(|owner| {
        owner["documentation_claim"].as_str().is_some_and(|claim| {
            claim.contains("unpublished draft") && claim.contains("not a release")
        })
    }));
}

#[test]
fn public_api_status_preserves_audited_gaps() {
    let root = root();
    let docs = load(&root, "testing/stdlib-documentation.json");
    let api = load(&root, "testing/stdlib-public-api.json");

    for owner in owners(&docs) {
        let api_rows = api["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["owner"] == owner["id"])
            .collect::<Vec<_>>();
        let public_api = &owner["boundary"]["public_api"];
        let signatures = public_api["signatures"].as_array().unwrap();
        let verified = public_api["verified_signatures"].as_array().unwrap();
        assert_eq!(signatures.len(), api_rows.len());
        assert_eq!(
            verified.len(),
            api_rows
                .iter()
                .filter(|row| row["missing"]
                    .as_array()
                    .is_some_and(|missing| missing.is_empty()))
                .count()
        );
        if api_rows.is_empty() {
            assert!(matches!(
                public_api["status"].as_str().unwrap(),
                "partial" | "not-applicable"
            ));
        } else if verified.len() == signatures.len() {
            assert_eq!(public_api["status"], "complete");
        } else {
            assert_eq!(public_api["status"], "partial");
            assert!(!public_api["reason"].as_str().unwrap().is_empty());
        }
    }

    assert_eq!(docs["summary"]["api_complete"], 14);
    assert_eq!(docs["summary"]["api_partial"], 4);
    assert_eq!(docs["summary"]["api_not_applicable"], 4);
}
