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
        "tondo-draft 0.1 open 9 b5b9a0997f28fca0271651c53304340bf371f5eb7ee8973e67ab7b54bbd81352\n"
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
fn seal_requires_explicit_evidence_and_destination() {
    let output = Command::new(executable())
        .args([
            "seal",
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
            .contains("`--result` is required for seal")
    );
}

#[test]
fn verify_candidate_command_rejects_an_unsealed_directory() {
    let root = std::env::temp_dir().join(format!(
        "tondo-candidate-cli-rejection-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("stale test directory must be removable");
    }
    std::fs::create_dir_all(root.join("candidate")).expect("test directory must be creatable");
    std::fs::write(root.join("candidate/manifest.json"), b"{}\n")
        .expect("test manifest must be writable");
    let output = Command::new(executable())
        .args([
            "verify-candidate",
            "--root",
            root.to_str().expect("repository path must be UTF-8"),
            "--candidate",
            "candidate",
        ])
        .output()
        .expect("the conformance runner must verify");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid candidate JSON"));
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
