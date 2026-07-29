use std::path::PathBuf;
use std::process::Command;

fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_tondo-conformance")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn validate_command_reports_the_published_suite_identity() {
    let output = Command::new(executable())
        .args([
            "validate",
            "--root",
            repository_root()
                .to_str()
                .expect("repository path must be UTF-8"),
            "--manifest",
            "conformance/0.1/manifest.json",
        ])
        .output()
        .expect("the conformance runner must execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("identity must be UTF-8"),
        "tondo-conformance-0.1 0.1.0 67f12434001d5d9d17b0f2181afe3ec38cb07d6207e431cca164ec4854f0148b\n"
    );
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
