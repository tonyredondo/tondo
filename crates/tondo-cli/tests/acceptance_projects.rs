use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tondo_compiler::test_report::{TestList, TestReport};
use tondo_compiler::test_result::{AggregateStatus, AttemptPhase, SnapshotStatus};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/projects/testing-acceptance")
}

fn control_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/projects/testing-control")
}

fn flaky_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/projects/testing-flaky")
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

    assert_eq!(first_report.tests().len(), 5);
    assert_eq!(first_report.suites().len(), 3);
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
    let integration_root = first_report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::integration_root"))
        .unwrap();
    assert_eq!(integration_root.attempts[0].virtual_time.len(), 1);
    let virtual_time = &integration_root.attempts[0].virtual_time[0];
    assert_eq!(virtual_time.index, 1);
    assert_eq!(virtual_time.elapsed_ns, "25");
    assert_eq!(virtual_time.automatic_advances, 1);
    assert_eq!(virtual_time.explicit_advances, 0);
    assert_eq!(virtual_time.settles, 1);
    let shared = first_report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::shared_service"))
        .unwrap();
    assert_eq!(shared.owners, ["@tondo/testing"]);
    assert_eq!(shared.attempts.len(), 1);
    assert_eq!(shared.attempts[0].logs, ["shared setup"]);
    let managed = first_report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::managed_service"))
        .unwrap();
    assert_eq!(managed.attempts[0].logs, ["service cleanup"]);
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
    assert!(failure.tests()[0].attempts[0].logs.is_empty());

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
    assert!(skipped.tests()[0].attempts[0].logs.is_empty());

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
    assert_eq!(attempt.snapshots[0].status, SnapshotStatus::Created);
    assert!(project.join("tests/snapshots.json").is_file());

    let checked = successful(
        &project,
        &[
            "--exact",
            "controlledEvidence",
            "--artifacts",
            "target/check-artifacts",
            "--test-format",
            "json",
        ],
    );
    let checked = TestReport::parse(&checked.stdout).unwrap();
    assert_eq!(
        checked.tests()[0].attempts[0].snapshots[0].status,
        SnapshotStatus::Matched
    );

    let control_source = project.join("tests/control.to");
    let mut source = fs::read(&control_source).unwrap();
    source.extend_from_slice(
        b"\nsuite suiteEvidence {\n recordEvidence()\n testing.snapshot(\"setup\", \"shared value\")\n test leaf { assert(true) }\n}\n",
    );
    fs::write(&control_source, source).unwrap();
    let suite_update = successful(
        &project,
        &[
            "--exact",
            "testingControl::integration::tests::suiteEvidence",
            "--update-snapshots",
            "--artifacts",
            "target/suite-artifacts",
            "--test-format",
            "json",
        ],
    );
    let suite_update = TestReport::parse(&suite_update.stdout).unwrap();
    let suite_attempt = &suite_update.suites()[0].attempts[0];
    assert_eq!(suite_attempt.artifacts.len(), 1);
    assert_eq!(suite_attempt.snapshots.len(), 2);
    assert!(
        suite_attempt
            .snapshots
            .iter()
            .all(|snapshot| snapshot.status == SnapshotStatus::Created)
    );
    let suite_check = successful(
        &project,
        &[
            "--exact",
            "testingControl::integration::tests::suiteEvidence",
            "--artifacts",
            "target/suite-check-artifacts",
            "--test-format",
            "json",
        ],
    );
    let suite_check = TestReport::parse(&suite_check.stdout).unwrap();
    assert!(
        suite_check.suites()[0].attempts[0]
            .snapshots
            .iter()
            .all(|snapshot| snapshot.status == SnapshotStatus::Matched)
    );

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
fn suite_setup_failure_blocks_only_its_subtree_and_reports_the_suite() {
    let root = temporary_root("suite-setup-failure");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tondo.toml"),
        b"[package]\nname = \"suite_failure\"\nedition = \"0.1\"\n",
    )
    .unwrap();
    fs::write(
        root.join("tests/lifecycle.to"),
        b"import std.testing\n\nsuite broken {\n assert(false)\n test child { assert(true) }\n}\nsuite broken_cleanup {\n defer assert(false)\n test child { assert(true) }\n}\nsuite nested {\n suite skipped {\n  testing.skip(\"not available\")\n  suite grandchild {\n   test child { assert(true) }\n  }\n }\n test sibling { assert(true) }\n}\ntest sibling { assert(true) }\n",
    )
    .unwrap();

    let output = tondo_test(&root, &["--test-format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let report = TestReport::parse(&output.stdout).unwrap();
    let suite = report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::broken"))
        .unwrap();
    assert_eq!(suite.status, AggregateStatus::FailedPanic);
    assert_eq!(
        suite.attempts[0].phase,
        Some(tondo_compiler::test_result::AttemptPhase::Setup)
    );
    let child = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::broken::child"))
        .unwrap();
    assert_eq!(child.status, AggregateStatus::BlockedSetup);
    assert_eq!(
        child.attempts[0]
            .blocked_by
            .as_ref()
            .map(|blocked| blocked.id.as_str()),
        Some(suite.id.as_str())
    );
    let cleanup_suite = report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::broken_cleanup"))
        .unwrap();
    assert_eq!(cleanup_suite.status, AggregateStatus::FailedPanic);
    assert_eq!(
        cleanup_suite.attempts[0].phase,
        Some(tondo_compiler::test_result::AttemptPhase::Teardown)
    );
    assert_eq!(
        report
            .tests()
            .iter()
            .find(|test| test.id.ends_with("::broken_cleanup::child"))
            .unwrap()
            .status,
        AggregateStatus::Passed
    );
    let skipped_suite = report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::nested::skipped"))
        .unwrap();
    assert_eq!(skipped_suite.status, AggregateStatus::Skipped);
    assert_eq!(skipped_suite.attempts[0].phase, Some(AttemptPhase::Setup));
    let skipped_child = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::nested::skipped::grandchild::child"))
        .unwrap();
    assert_eq!(skipped_child.status, AggregateStatus::BlockedSkip);
    assert_eq!(
        skipped_child.attempts[0]
            .blocked_by
            .as_ref()
            .map(|blocked| blocked.id.as_str()),
        Some(skipped_suite.id.as_str())
    );
    let blocked_suite = report
        .suites()
        .iter()
        .find(|suite| suite.id.ends_with("::nested::skipped::grandchild"))
        .unwrap();
    assert_eq!(blocked_suite.status, AggregateStatus::BlockedSkip);
    assert_eq!(
        blocked_suite.attempts[0]
            .blocked_by
            .as_ref()
            .map(|blocked| blocked.id.as_str()),
        Some(skipped_suite.id.as_str())
    );
    assert_eq!(
        report
            .tests()
            .iter()
            .find(|test| test.id.ends_with("::nested::sibling"))
            .unwrap()
            .status,
        AggregateStatus::Passed
    );
    assert_eq!(
        report
            .tests()
            .iter()
            .find(|test| test.id.ends_with("::sibling"))
            .unwrap()
            .status,
        AggregateStatus::Passed
    );
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
    assert_eq!(partition.len(), 5);

    let left_run = successful(&project, &["--shard", "1/2", "--test-format", "json"]);
    let right_run = successful(&project, &["--shard", "2/2", "--test-format", "json"]);
    let left_run = TestReport::parse(&left_run.stdout).unwrap();
    let right_run = TestReport::parse(&right_run.stdout).unwrap();
    assert!(
        left_run
            .tests()
            .iter()
            .chain(right_run.tests())
            .all(|test| test.status == AggregateStatus::Passed)
    );
    assert_eq!(left_run.tests().len() + right_run.tests().len(), 5);

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
    assert!(junit.contains("tests=\"5\""));
    assert!(junit.contains("name=\"tondo.virtual_time\""));
    assert!(junit.contains("&quot;elapsed_ns&quot;:&quot;25&quot;"));
    assert!(junit.contains("&quot;automatic_advances&quot;:1"));
    for test in stdout.tests() {
        assert!(junit.contains(&test.id));
    }

    fs::remove_dir_all(project).unwrap();
}

#[test]
fn acceptance_project_dogfoods_repeat_with_fresh_attempts() {
    let project = project("repeat");
    let output = successful(
        &project,
        &["--repeat", "3", "--jobs", "2", "--test-format", "json"],
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.metadata().repeat.count, 3);
    for test in report.tests() {
        assert_eq!(test.status, AggregateStatus::Passed);
        assert_eq!(test.attempts.len(), 3);
        assert_eq!(
            test.attempts
                .iter()
                .map(|attempt| attempt.iteration)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(test.attempts.iter().all(|attempt| attempt.round == 0));
    }
    fs::remove_dir_all(project).unwrap();
}

#[cfg(unix)]
#[test]
fn acceptance_project_dogfoods_an_isolated_deterministic_retry() {
    let project = temporary_root("retry");
    copy_tree(&flaky_fixture(), &project);
    let output = successful(
        &project,
        &["--retry", "1", "--allow-flaky", "--test-format", "json"],
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    let test = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::deterministicFlaky"))
        .unwrap();
    assert_eq!(test.status, AggregateStatus::FlakyPass);
    assert_eq!(test.attempts.len(), 2);
    assert_eq!(test.attempts[0].round, 0);
    assert_eq!(test.attempts[1].round, 1);
    assert_eq!(test.attempts[1].logs, ["isolated retry passed"]);
    let sibling = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::stableSibling"))
        .unwrap();
    assert_eq!(sibling.attempts.len(), 2);
    assert!(
        sibling
            .attempts
            .iter()
            .all(|attempt| attempt.logs == ["stable sibling"])
    );
    let suite = report.suites().first().unwrap();
    assert_eq!(suite.attempts.len(), 2);
    assert!(
        suite
            .attempts
            .iter()
            .all(|attempt| attempt.logs == ["suite participation"])
    );
    assert!(!project.join("dogfood-retry.marker").exists());
    fs::remove_dir_all(project).unwrap();
}
