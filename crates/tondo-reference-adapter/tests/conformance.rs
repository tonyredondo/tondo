use std::path::PathBuf;

use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};
use tondo_conformance::runner::{SuiteResult, compose_suite_result, run_suite};
use tondo_reference_adapter::ReferenceAdapter;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(target_os = "linux")]
#[test]
fn complete_live_suite_passes_in_process_without_a_checked_result_snapshot() {
    let root = repository_root();
    let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH)
        .expect("the draft lineage must load explicitly");
    let suite = lineage.suite();
    let mut adapter = ReferenceAdapter;
    let result = run_suite(suite, &mut adapter, None)
        .expect("the reference adapter must satisfy its bootstrap regression suite in process");

    assert_eq!(result.cases.len(), suite.manifest().cases.len());
    assert!(compose_suite_result(&lineage, result.clone(), b"not json").is_err());
    assert!(result.passed);
    assert_eq!(result.manifest_sha256, suite.manifest_sha256());
    let encoded = serde_json::to_vec(&result).expect("suite results must have canonical JSON");
    let decoded: SuiteResult =
        serde_json::from_slice(&encoded).expect("suite results must round-trip");
    assert_eq!(result, decoded);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn live_suite_identity_loads_on_non_linux_hosts() {
    let root = repository_root();
    let lineage = DraftLineage::load(&root, DRAFT_LINEAGE_PATH)
        .expect("the draft lineage must load explicitly");
    let suite = lineage.suite();

    assert_eq!(suite.manifest().suite, "tondo-conformance-draft");
    assert_eq!(suite.manifest().version, "draft");
    assert!(!suite.manifest().cases.is_empty());
}
