#[path = "../build_support/standard_identity.rs"]
mod standard_identity;

use standard_identity::{fingerprint, read_inputs};

#[test]
fn standard_source_collection_is_relocatable_and_rejects_missing_inputs() {
    let temporary =
        std::env::temp_dir().join(format!("tondo-standard-identity-{}", std::process::id()));
    std::fs::create_dir(&temporary).unwrap();
    let first = temporary.join("first");
    let relocated = temporary.join("relocated");
    for root in [&first, &relocated] {
        for relative in standard_identity::INPUTS {
            let path = root.join(relative);
            if path.extension().is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, relative.as_bytes()).unwrap();
            } else {
                std::fs::create_dir_all(path).unwrap();
            }
        }
    }
    let expected = fingerprint(&read_inputs(&first).unwrap()).unwrap();
    assert_eq!(
        fingerprint(&read_inputs(&relocated).unwrap()).unwrap(),
        expected
    );
    let added = relocated.join("crates/tondo-compiler/src/new.to");
    std::fs::write(&added, b"fn added() {}\n").unwrap();
    assert_ne!(
        fingerprint(&read_inputs(&relocated).unwrap()).unwrap(),
        expected
    );
    std::fs::remove_file(added).unwrap();
    assert_eq!(
        fingerprint(&read_inputs(&relocated).unwrap()).unwrap(),
        expected
    );
    std::fs::remove_file(relocated.join("Cargo.lock")).unwrap();
    assert!(read_inputs(&relocated).is_err());
    std::fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn embedded_standard_identity_covers_the_current_source_bundle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let inputs = read_inputs(root).unwrap();
    for required in [
        "crates/tondo-compiler/src/bootstrap/io.to",
        "crates/tondo-compiler/src/bootstrap/console_io.to",
        "crates/tondo-compiler/src/process_host.rs",
        "crates/tondo-stdlib/src/io.rs",
        "crates/tondo-vm/src/runtime/execute.rs",
        "Cargo.lock",
    ] {
        assert!(
            inputs.iter().any(|(path, _)| path == required),
            "{required}"
        );
    }
    assert_eq!(
        fingerprint(&inputs).unwrap(),
        tondo_compiler::project::bootstrap_standard_hash()
    );
}

#[test]
fn standard_identity_is_order_independent_and_content_sensitive() {
    let inputs = vec![
        ("a".to_owned(), b"first".to_vec()),
        ("b".to_owned(), b"second".to_vec()),
    ];
    let expected = fingerprint(&inputs).unwrap();
    let mut reordered = inputs.clone();
    reordered.reverse();
    assert_eq!(fingerprint(&reordered).unwrap(), expected);
    let mut changed = inputs.clone();
    changed[0].1.push(0);
    assert_ne!(fingerprint(&changed).unwrap(), expected);
    let mut renamed = inputs.clone();
    renamed[0].0.push('x');
    assert_ne!(fingerprint(&renamed).unwrap(), expected);
    assert_ne!(fingerprint(&inputs[..1]).unwrap(), expected);
    let mut duplicate = inputs;
    duplicate.push(duplicate[0].clone());
    assert!(fingerprint(&duplicate).is_err());
    assert_ne!(
        fingerprint(&[("ab".into(), b"c".to_vec())]).unwrap(),
        fingerprint(&[("a".into(), b"bc".to_vec())]).unwrap()
    );
}
