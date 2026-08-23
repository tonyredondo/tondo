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

fn rows_by_id(registry: &Value) -> BTreeMap<String, &Value> {
    owners(registry)
        .flat_map(|owner| owner["rows"].as_array().unwrap())
        .map(|row| (row["id"].as_str().unwrap().to_owned(), row))
        .collect()
}

#[test]
fn every_normative_matrix_row_has_an_explicit_conformance_record() {
    let root = root();
    let registry = load(&root, "testing/stdlib-conformance-coordination.json");
    let matrix = load(&root, "testing/stdlib-matrix.json");
    let coordinated = rows_by_id(&registry);

    assert_eq!(registry["status"], "promoted");
    assert_eq!(registry["promotion"]["status"], "promoted");

    let matrix_rows = matrix["rows"].as_array().unwrap();
    assert_eq!(matrix_rows.len(), 385);
    assert_eq!(coordinated.len(), matrix_rows.len());

    for matrix_row in matrix_rows {
        let id = matrix_row["id"].as_str().unwrap();
        let owner_id = matrix_row["owner"].as_str().unwrap();
        let owner_conf = matrix["owners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|owner| owner["id"] == owner_id)
            .unwrap();
        let row = coordinated
            .get(id)
            .unwrap_or_else(|| panic!("missing CONF record for {id}"));
        assert_eq!(row["kind"], matrix_row["kind"]);
        assert_eq!(row["status"], owner_conf["stages"]["CONF"]["status"]);
        assert_eq!(row["reason"], owner_conf["stages"]["CONF"]["reason"]);
        assert_eq!(row["refs"], owner_conf["stages"]["CONF"]["refs"]);
        assert!(!row["refs"].as_array().unwrap().is_empty());
        assert_eq!(row["status"], "verified");
        assert!(row["reason"].is_null());
    }

    assert_eq!(registry["summary"]["rows"], matrix_rows.len());
    assert_eq!(registry["summary"]["verified_rows"], matrix_rows.len());
}

#[test]
fn owner_closure_and_promotion_boundary_are_explicit() {
    let root = root();
    let registry = load(&root, "testing/stdlib-conformance-coordination.json");
    let matrix = load(&root, "testing/stdlib-matrix.json");
    let expected_owners = matrix["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|owner| owner["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let actual_owners = owners(&registry)
        .map(|owner| owner["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_owners, expected_owners);

    for owner in owners(&registry) {
        let rows = owner["rows"].as_array().unwrap();
        let expected_status = "verified";
        assert_eq!(owner["status"], expected_status);
        assert_eq!(owner["evidence"]["status"], expected_status);
        assert!(owner["reason"].is_null());
        assert!(!owner["evidence"]["refs"].as_array().unwrap().is_empty());
        assert!(!owner["evidence"]["commands"].as_array().unwrap().is_empty());
        if expected_status == "verified" {
            assert!(rows.iter().all(|row| row["status"] == "verified"));
        }
    }

    assert_eq!(registry["promotion"]["matrix_status"], "verified");
    assert_eq!(registry["promotion"]["next_coordination"], "STD-A-DIST-001");
}

#[test]
fn conformance_commands_and_codec_observations_are_linked() {
    let root = root();
    let registry = load(&root, "testing/stdlib-conformance-coordination.json");
    let mut codec_owners = BTreeSet::from([
        "std.serialization",
        "std.json",
        "std.messagepack",
        "std.protobuf",
    ]);

    for owner in owners(&registry) {
        for reference in owner["evidence"]["refs"].as_array().unwrap() {
            let path = reference.as_str().unwrap().split('#').next().unwrap();
            assert!(
                root.join(path).exists(),
                "missing CONF reference {reference}"
            );
        }
        for command in owner["evidence"]["commands"].as_array().unwrap() {
            let command = command.as_str().unwrap();
            if let Some(path) = command.strip_prefix("scripts/") {
                assert!(root.join("scripts").join(path).is_file());
            }
        }
        if codec_owners.remove(owner["id"].as_str().unwrap()) {
            assert!(
                owner["evidence"]["cases"]
                    .as_array()
                    .is_some_and(|cases| !cases.is_empty())
            );
            assert!(
                owner["evidence"]["refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|reference| reference
                        .as_str()
                        .unwrap()
                        .contains("stdlib-conformance.json#owners/"))
            );
        }
    }
    assert!(codec_owners.is_empty());

    let async_owner = owners(&registry)
        .find(|owner| owner["id"] == "std.async")
        .unwrap();
    assert_eq!(async_owner["status"], "verified");
    assert!(async_owner["reason"].is_null());
}
