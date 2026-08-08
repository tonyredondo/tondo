use std::path::PathBuf;
use std::process::Command;

use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};

fn executable() -> PathBuf {
    let cargo_path = PathBuf::from(env!("CARGO_BIN_EXE_tondo-conformance"));

    // Cargo's compile-time `CARGO_BIN_EXE_*` value is rooted at the
    // workspace's conventional `target` directory.  A caller may redirect
    // build artifacts with `CARGO_TARGET_DIR` (the reliability gate does so
    // on the SSD), in which case that path is still valid as a fallback but
    // does not name the executable that was actually built.  Resolve the
    // runtime target directory first while retaining the normal Cargo path
    // for CI and direct test invocations.
    let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") else {
        return cargo_path;
    };
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let binary_name = if cfg!(windows) {
        "tondo-conformance.exe"
    } else {
        "tondo-conformance"
    };
    let redirected = PathBuf::from(target_dir).join(profile).join(binary_name);
    if redirected.is_file() {
        redirected
    } else {
        cargo_path
    }
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
    let lineage = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH)
        .expect("the repository draft must remain loadable");
    let expected = format!(
        "tondo-draft 0.1 open {} {}\n",
        lineage.manifest().revision,
        lineage.manifest_sha256()
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("identity must be UTF-8"),
        expected
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
