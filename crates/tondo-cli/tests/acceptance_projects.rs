use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tondo_compiler::test_report::{TestList, TestReport};
use tondo_compiler::test_result::{AggregateStatus, SnapshotStatus};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/projects/testing-acceptance")
}

fn control_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/projects/testing-control")
}

fn temporary_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must follow the Unix epoch")
        .as_nanos();
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tondo-testing-acceptance-{label}-{}-{nonce}-{id}",
        std::process::id()
    ))
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&source, &destination);
        } else {
            fs::copy(source, destination).unwrap();
        }
    }
}

fn project(label: &str) -> PathBuf {
    let destination = temporary_root(label);
    copy_tree(&fixture(), &destination);
    destination
}

fn tondo_test(project: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
    command.current_dir(project);
    command.arg("test").arg("--project").arg(project);
    command.args(arguments);
    command.output().unwrap()
}

fn successful(project: &Path, arguments: &[&str]) -> Output {
    let output = tondo_test(project, arguments);
    assert!(
        output.status.success(),
        "tondo test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn acceptance_project_is_relocatable_and_reports_canonical_observations() {
    let first = project("first");
    let second = project("second");
    let arguments = [
        "--test-format",
        "json",
        "--order",
        "random",
        "--seed",
        "5eed",
        "--jobs",
        "2",
    ];
    let first_output = successful(&first, &arguments);
    let second_output = successful(&second, &arguments);
    let first_report = TestReport::parse(&first_output.stdout).unwrap();
    let second_report = TestReport::parse(&second_output.stdout).unwrap();

    assert_eq!(first_report.tests().len(), 4);
    assert_eq!(
        first_report.canonical_bytes().unwrap(),
        second_report.canonical_bytes().unwrap()
    );
    assert!(
        first_report
            .tests()
            .iter()
            .any(|test| test.id.contains("::unit::"))
    );
    assert!(
        first_report
            .tests()
            .iter()
            .any(|test| test.id.contains("::integration::"))
    );
    assert!(
        first_report
            .tests()
            .iter()
            .any(|test| test.id.contains("shared_service::nested_client"))
    );
    assert!(
        first_report
            .tests()
            .iter()
            .any(|test| test.owners == ["@tondo/testing"])
    );
    for test in first_report.tests() {
        assert_eq!(test.attempts.len(), 1);
        assert_eq!(test.attempts[0].logs.len(), 1);
        assert!(test.attempts[0].tags.contains_key("kind"));
    }
    fs::remove_dir_all(first).unwrap();
    fs::remove_dir_all(second).unwrap();
}

#[test]
fn acceptance_control_project_exposes_fail_now_and_skip_without_visible_context() {
    let project = temporary_root("control");
    copy_tree(&control_fixture(), &project);

    let failure = tondo_test(
        &project,
        &["--exact", "controlledFailure", "--test-format", "json"],
    );
    assert_eq!(failure.status.code(), Some(1));
    let failure = TestReport::parse(&failure.stdout).unwrap();
    assert_eq!(failure.tests()[0].status, AggregateStatus::FailedPanic);
    assert_eq!(
        failure.tests()[0].attempts[0]
            .failure
            .as_ref()
            .unwrap()
            .code
            .as_deref(),
        Some("P0007")
    );

    let skipped = successful(
        &project,
        &["--exact", "controlledSkip", "--test-format", "json"],
    );
    let skipped = TestReport::parse(&skipped.stdout).unwrap();
    assert_eq!(skipped.tests()[0].status, AggregateStatus::Skipped);
    assert_eq!(
        skipped.tests()[0].attempts[0].skip.as_ref().unwrap().reason,
        "controlled skip"
    );

    fs::remove_dir_all(project).unwrap();
}

#[test]
fn acceptance_control_project_publishes_source_attachments_and_snapshots() {
    let project = temporary_root("evidence");
    copy_tree(&control_fixture(), &project);

    let output = successful(
        &project,
        &[
            "--exact",
            "controlledEvidence",
            "--update-snapshots",
            "--test-format",
            "json",
        ],
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    let attempt = &report.tests()[0].attempts[0];
    assert_eq!(attempt.artifacts.len(), 1);
    assert_eq!(attempt.artifacts[0].name, "trace");
    assert_eq!(attempt.artifacts[0].media_type, "text/plain");
    assert_eq!(attempt.snapshots.len(), 1);
    assert_eq!(attempt.snapshots[0].name, "greeting");
    assert_eq!(attempt.snapshots[0].status, SnapshotStatus::Missing);

    fs::remove_dir_all(project).unwrap();
}

#[test]
fn testing_module_is_not_available_to_production_entries() {
    let root = temporary_root("production-boundary");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("main.to");
    fs::write(
        &source,
        b"import std.testing\nfn main() { testing.log(\"forbidden\") }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("check")
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("E1008") && diagnostic.contains("::testing"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn acceptance_project_exercises_public_selection_and_sharding_contracts() {
    let project = project("selection");
    let filter = successful(
        &project,
        &["--list", "--filter", "response", "--test-format", "json"],
    );
    let filter = TestList::parse(&filter.stdout).unwrap();
    assert_eq!(filter.tests().len(), 1);
    assert!(filter.tests()[0].id.contains("receives_response"));

    let glob = successful(
        &project,
        &[
            "--list",
            "--glob",
            "**::shared_service::**",
            "--test-format",
            "json",
        ],
    );
    let glob = TestList::parse(&glob.stdout).unwrap();
    assert_eq!(glob.tests().len(), 2);

    let exact = successful(
        &project,
        &[
            "--list",
            "--exact",
            "acceptance::integration::tests::shared_service",
            "--test-format",
            "json",
        ],
    );
    let exact = TestList::parse(&exact.stdout).unwrap();
    assert_eq!(exact.tests().len(), 2);

    let left = successful(
        &project,
        &["--list", "--shard", "1/2", "--test-format", "json"],
    );
    let right = successful(
        &project,
        &["--list", "--shard", "2/2", "--test-format", "json"],
    );
    let left = TestList::parse(&left.stdout).unwrap();
    let right = TestList::parse(&right.stdout).unwrap();
    let mut partition = left
        .tests()
        .iter()
        .chain(right.tests())
        .map(|test| test.id.clone())
        .collect::<Vec<_>>();
    partition.sort();
    partition.dedup();
    assert_eq!(partition.len(), 4);

    let empty = tondo_test(
        &project,
        &["--list", "--exact", "missing", "--test-format", "json"],
    );
    assert_eq!(empty.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no tests matched the selection"));

    fs::remove_dir_all(project).unwrap();
}

#[test]
fn acceptance_project_publishes_equivalent_json_and_junit_results() {
    let project = project("reports");
    let output = successful(
        &project,
        &[
            "--test-format",
            "json",
            "--report",
            "json=target/acceptance.json",
            "--report",
            "junit=target/acceptance.xml",
        ],
    );
    let stdout = TestReport::parse(&output.stdout).unwrap();
    let stored =
        TestReport::parse(&fs::read(project.join("target/acceptance.json")).unwrap()).unwrap();
    assert_eq!(
        stdout.canonical_bytes().unwrap(),
        stored.canonical_bytes().unwrap()
    );

    let junit = fs::read_to_string(project.join("target/acceptance.xml")).unwrap();
    assert!(junit.contains("tests=\"4\""));
    for test in stdout.tests() {
        assert!(junit.contains(&test.id));
    }

    fs::remove_dir_all(project).unwrap();
}
