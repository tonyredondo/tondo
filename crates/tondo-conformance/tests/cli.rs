use std::path::PathBuf;
use std::process::Command;

fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_tondo-conformance")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn validate_command_reports_the_single_draft_identity() {
    let output = Command::new(executable())
        .args([
            "validate",
            "--root",
            repository_root()
                .to_str()
                .expect("repository path must be UTF-8"),
            "--manifest",
            "conformance/draft/manifest.json",
            "--lineage",
            "draft",
        ])
        .output()
        .expect("the conformance runner must execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("identity must be UTF-8"),
        "tondo-draft 0.1 open 14 c1ab0c505fb973d397b27f5681604ad4e2de77a6314eef795035274d8ae3fe6c\n"
    );
}

#[test]
fn validate_command_rejects_a_second_lineage_name() {
    let output = Command::new(executable())
        .args([
            "validate",
            "--root",
            repository_root()
                .to_str()
                .expect("repository path must be UTF-8"),
            "--manifest",
            "conformance/draft/manifest.json",
            "--lineage",
            "checkpoint",
        ])
        .output()
        .expect("the conformance runner must execute");

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .expect("error must be UTF-8")
            .contains("unknown lineage `checkpoint`")
    );
}

#[test]
fn seal_proof_requires_explicit_evidence_and_destination() {
    let output = Command::new(executable())
        .args([
            "seal-proof",
            "--root",
            repository_root()
                .to_str()
                .expect("repository path must be UTF-8"),
            "--manifest",
            "conformance/draft/manifest.json",
            "--lineage",
            "draft",
        ])
        .output()
        .expect("the conformance runner must execute");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .expect("error must be UTF-8")
            .contains("`--result` is required for seal-proof")
    );
}

#[test]
fn verify_proof_command_rejects_an_unsealed_directory() {
    let root =
        std::env::temp_dir().join(format!("tondo-proof-cli-rejection-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("stale test directory must be removable");
    }
    std::fs::create_dir_all(root.join("proof")).expect("test directory must be creatable");
    std::fs::write(root.join("proof/manifest.json"), b"{}\n")
        .expect("test manifest must be writable");
    let output = Command::new(executable())
        .args([
            "verify-proof",
            "--root",
            root.to_str().expect("repository path must be UTF-8"),
            "--proof",
            "proof",
        ])
        .output()
        .expect("the conformance runner must verify");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid promotion-proof JSON"));
    std::fs::remove_dir_all(root).expect("test directory must be removable");
}

#[test]
fn missing_command_is_a_stable_usage_error() {
    let output = Command::new(executable())
        .output()
        .expect("the conformance runner must execute");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("usage must be UTF-8");
    assert!(stderr.starts_with("tondo-conformance: a command is required\n"));
    assert!(stderr.contains("Usage:\n  tondo-conformance validate"));
}
