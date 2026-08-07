use std::path::PathBuf;

use tondo_conformance::sha256;

#[test]
fn canonical_fixture_manifest_fence_has_published_hash() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let specification = std::fs::read_to_string(root.join("TONDO_LANGUAGE_SPEC.md"))
        .expect("the language specification must be readable UTF-8");
    let section = specification
        .split_once("### C.6 Serialización canónica\n")
        .expect("the canonical serialization section must exist")
        .1;
    let fence = section
        .split_once("~~~text\n")
        .expect("the section must contain one text fence")
        .1
        .split_once("~~~\n")
        .expect("the text fence must be closed")
        .0;

    assert!(fence.starts_with("tondo-fixture-manifest 0.1\n"));
    assert!(fence.ends_with("end\n"));
    assert_eq!(
        sha256(fence.as_bytes()),
        "714da31de9e190eed73361d6d3ded585661c9878a355994db1b160f913d529b8"
    );
}
