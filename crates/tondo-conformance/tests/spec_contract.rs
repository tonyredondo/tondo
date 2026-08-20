use std::path::PathBuf;

use tondo_conformance::{manifest::SuiteManifest, sha256};

#[test]
fn live_fixture_manifest_is_canonical_and_pinned_by_the_live_suite() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_path = root.join("conformance/0.1/fixtures/tondo-fixture-manifest.txt");
    let fixture = std::fs::read(&fixture_path).expect("the live fixture manifest must be readable");
    let text = std::str::from_utf8(&fixture).expect("the live fixture manifest must be UTF-8");
    let manifest: SuiteManifest = serde_json::from_slice(
        &std::fs::read(root.join("conformance/0.1/manifest.json"))
            .expect("the live suite manifest must be readable"),
    )
    .expect("the live suite manifest must be valid JSON");

    assert!(text.starts_with("tondo-fixture-manifest 0.1\n"));
    assert!(text.ends_with("end\n"));
    assert!(!text.contains('\r'));
    assert_eq!(
        manifest.fixture_manifest.path,
        "conformance/0.1/fixtures/tondo-fixture-manifest.txt"
    );
    assert_eq!(manifest.fixture_manifest.sha256, sha256(&fixture));
}
