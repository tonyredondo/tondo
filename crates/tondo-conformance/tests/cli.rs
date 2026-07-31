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
        "tondo-draft 0.1 open 1 7592dfd27dc90363e30a04baca34e1eecf7bde1965975ac4f59a15faf16f9a4c\n"
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
fn seal_is_a_non_mutating_preflight_and_rejects_pending_work() {
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
    let stderr = String::from_utf8(output.stderr).expect("error must be UTF-8");
    assert!(stderr.contains("draft still has pending tasks: M10.6, M10.7, PARSER-STACK-001"));
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
