use std::fs;
use std::path::PathBuf;

use tondo_conformance::lineage::{LIVE_LINEAGE_PATH, LiveLineage};
use tondo_conformance::runner::run_suite;
use tondo_reference_adapter::ReferenceAdapter;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn complete_checkpoint_suite_matches_in_process() {
    let root = repository_root();
    let lineage =
        LiveLineage::load(&root, LIVE_LINEAGE_PATH).expect("the live lineage must load explicitly");
    let suite = lineage.checkpoint_suite();
    let mut adapter = ReferenceAdapter;
    let result = run_suite(suite, &mut adapter, None)
        .expect("the reference adapter must satisfy its checkpoint suite in process");

    assert_eq!(result.cases.len(), suite.manifest().cases.len());
    let actual = serde_json::to_vec(&result).expect("suite results must have canonical JSON");
    let expected =
        fs::read(root.join("conformance/0.1/results/tondo-reference-0.1.0-tondo-vm-hosted.json"))
            .expect("the published reference result must exist");
    assert_eq!(actual, expected);
}
