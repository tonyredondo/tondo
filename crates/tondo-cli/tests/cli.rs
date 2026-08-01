use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tondo_compiler::artifact::{BuildArtifact, CAPABILITY_REGISTRY, CompiledInterface, sha256};
use tondo_compiler::project::{
    BOOTSTRAP_STANDARD_PACKAGE, LOCKFILE_FORMAT, MANIFEST_FORMAT, ProjectPlan,
    bootstrap_standard_hash,
};
use tondo_compiler::test_report::{SnapshotMode, TestList, TestReport};
use tondo_compiler::test_snapshots::SnapshotStore;

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

fn source_file_with(bytes: &[u8]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("tondo-cli-{}-{nonce}-{id}.to", std::process::id()));
    fs::write(&path, bytes).unwrap();
    path
}

fn source_file() -> std::path::PathBuf {
    source_file_with(b"fn main() {}\n")
}

#[test]
fn missing_source_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("check")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("a source file is required")
    );
}

#[test]
fn check_reaches_the_shared_driver() {
    let source = source_file();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "--diagnostic-format=json"])
        .arg(&source)
        .output()
        .unwrap();
    fs::remove_file(source).unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn help_and_version_are_successful() {
    for argument in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg(argument)
            .output()
            .unwrap();
        assert!(output.status.success(), "{argument} failed");
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn test_command_defaults_to_the_project_manifest() {
    let parsed = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args([
            "test", "--filter", "smoke", "--order", "random", "--seed", "5eed",
        ])
        .output()
        .unwrap();
    assert_eq!(parsed.status.code(), Some(2));
    assert!(parsed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&parsed.stderr).contains("cannot read manifest"));

    let invalid = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--shard", "0/2"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("positive"));
}

fn test_project(source: &[u8]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("tondo-test-cli-{}-{nonce}", std::process::id()));
    fs::create_dir_all(directory.join("tests")).unwrap();
    fs::write(directory.join("tests/smoke.to"), source).unwrap();
    let package_id = "workspace:test-cli@1";
    let source_hash = sha256(source);
    let manifest = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"tests/smoke.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    fs::write(directory.join("tondo.json"), &manifest).unwrap();
    let package_fingerprint = format!(
        "{{\"package_id\":\"{package_id}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\",\"sha256\":\"{source_hash}\"}}],\"interface_hash\":null}}"
    );
    let package_hash = sha256(package_fingerprint.as_bytes());
    let lockfile = format!(
        "{{\"format\":\"{LOCKFILE_FORMAT}\",\"manifest_hash\":\"{}\",\"standard\":{{\"package_id\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"content_hash\":\"{}\"}},\"packages\":[{{\"id\":\"{package_id}\",\"content_hash\":\"{package_hash}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\",\"sha256\":\"{source_hash}\"}}],\"interface\":null}}],\"generator_inputs\":[],\"privileged_units\":[]}}",
        sha256(manifest.as_bytes()),
        bootstrap_standard_hash(),
    );
    fs::write(directory.join("tondo.lock.json"), &lockfile).unwrap();
    let test_plan = format!(
        "{{\"format\":\"tondo-test-plan-draft\",\"project\":{{\"manifest_hash\":\"{}\",\"lockfile_hash\":\"{}\"}},\"repository_root\":\"\",\"roots\":[{{\"class\":\"production\",\"physical_path\":\"tests\",\"logical_path\":\"tests\"}}],\"sources\":[{{\"class\":\"production\",\"package\":\"{package_id}\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\",\"input\":\"source:production:tests/smoke.to\"}}],\"dev_dependencies\":[],\"codeowners\":{{\"mode\":\"auto\"}},\"selector\":{{\"kind\":\"none\"}},\"shard\":null,\"order\":{{\"kind\":\"canonical\"}},\"policy\":{{\"jobs\":1,\"allow_empty\":false,\"fail_fast\":false,\"retry\":0,\"repeat\":1}},\"reporters\":[\"human\",\"json\"],\"artifact_store\":{{\"path\":\"target/test-artifacts\",\"content_addressed\":true,\"max_bytes\":1048576}},\"snapshot_stores\":[],\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[],\"features\":[]}},\"time_catalog\":{{\"package\":\"std\",\"module\":\"time\",\"api\":\"monotonic-v1\"}},\"limits\":{{\"timeout_ms\":1000,\"setup_timeout_ms\":1000,\"teardown_timeout_ms\":1000,\"output_bytes\":65536,\"artifact_bytes\":1048576,\"snapshot_bytes\":1048576,\"memory_bytes\":67108864,\"instructions\":1000000,\"virtual_timers\":1024}}}}",
        sha256(manifest.as_bytes()),
        sha256(lockfile.as_bytes()),
    );
    let project = ProjectPlan::parse(manifest.as_bytes(), lockfile.as_bytes()).unwrap();
    let test_plan = project
        .parse_test_plan(test_plan.as_bytes())
        .unwrap()
        .canonical_bytes()
        .unwrap();
    fs::write(directory.join("tondo.test.json"), test_plan).unwrap();
    directory
}

#[test]
fn test_command_executes_a_test_body_through_the_vm_backend() {
    let directory = test_project(b"test smoke { assert(true) }\n");

    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PASS cli::integration::smoke::smoke")
    );
}

#[test]
fn test_command_uses_opinionated_defaults_without_a_sidecar() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    fs::remove_file(directory.join("tondo.test.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(output.status.success());
    assert_eq!(report.metadata().retry.max_additional_rounds, 1);
}

#[test]
fn test_command_accepts_an_explicit_canonical_plan_path() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let explicit = directory.join("custom-plan.json");
    let sidecar = fs::read_to_string(directory.join("tondo.test.json")).unwrap();
    fs::write(&explicit, sidecar.replace("\"retry\":0", "\"retry\":1")).unwrap();
    fs::remove_file(directory.join("tondo.test.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--test-plan",
            "custom-plan.json",
            "--retry",
            "2",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(output.status.success());
    assert_eq!(report.metadata().retry.max_additional_rounds, 2);
}

#[test]
fn test_command_cannot_disable_the_closed_sidecar_timeout() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json", "--timeout", "none"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("closed test-plan"));
}

#[test]
fn test_command_kills_a_recursive_leaf_at_the_wall_clock_boundary() {
    let directory = test_project(b"fn spin() { spin() }\ntest smoke { spin() }\n");
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json", "--timeout", "20ms"])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("TIMEOUT"));
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "worker was not bounded: {elapsed:?}"
    );
}

#[test]
fn update_snapshots_uses_the_sidecar_store_and_publishes_atomically() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let sidecar = fs::read_to_string(directory.join("tondo.test.json")).unwrap();
    let sidecar = sidecar.replace(
        "\"snapshot_stores\":[]",
        "\"snapshot_stores\":[{\"name\":\"default\",\"path\":\"tests/snapshots.json\",\"update\":false,\"max_bytes\":1048576}]",
    );
    fs::write(directory.join("tondo.test.json"), sidecar).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--manifest",
            "tondo.json",
            "--update-snapshots",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    let snapshot = fs::read(directory.join("tests/snapshots.json")).unwrap();
    let report = TestReport::parse(&output.stdout).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(SnapshotStore::parse(&snapshot).unwrap().entries(), &[]);
    assert_eq!(report.metadata().snapshot_policy.mode, SnapshotMode::Update);
    assert_eq!(report.metadata().snapshot_policy.published, Some(true));
}

#[test]
fn snapshot_store_inputs_are_validated_before_worker_execution() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let sidecar = fs::read_to_string(directory.join("tondo.test.json")).unwrap();
    let sidecar = sidecar.replace(
        "\"snapshot_stores\":[]",
        "\"snapshot_stores\":[{\"name\":\"default\",\"path\":\"tests/snapshots.json\",\"update\":false,\"max_bytes\":1048576}]",
    );
    fs::write(directory.join("tondo.test.json"), &sidecar).unwrap();
    let missing = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot load snapshot store"));
    fs::remove_dir_all(directory).unwrap();

    let directory = test_project(b"test smoke { assert(true) }\n");
    let sidecar = fs::read_to_string(directory.join("tondo.test.json")).unwrap();
    let sidecar = sidecar.replace(
        "\"snapshot_stores\":[]",
        "\"snapshot_stores\":[{\"name\":\"default\",\"path\":\"tests/snapshots.json\",\"update\":false,\"max_bytes\":1048576}]",
    );
    fs::write(directory.join("tondo.test.json"), &sidecar).unwrap();
    fs::write(
        directory.join("tests/snapshots.json"),
        SnapshotStore::empty("workspace:other@1")
            .unwrap()
            .canonical_bytes()
            .unwrap(),
    )
    .unwrap();
    let wrong_package = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json"])
        .output()
        .unwrap();
    assert_eq!(wrong_package.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&wrong_package.stderr).contains("belongs to package"));
    fs::remove_dir_all(directory).unwrap();

    let directory = test_project(b"test smoke { assert(true) }\n");
    let sidecar = fs::read_to_string(directory.join("tondo.test.json")).unwrap();
    let sidecar = sidecar.replace(
        "\"snapshot_stores\":[]",
        "\"snapshot_stores\":[{\"name\":\"default\",\"path\":\"tests/snapshots.json\",\"update\":false,\"max_bytes\":1}]",
    );
    fs::write(directory.join("tondo.test.json"), &sidecar).unwrap();
    let too_large = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--manifest", "tondo.json", "--update-snapshots"])
        .output()
        .unwrap();
    assert_eq!(too_large.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&too_large.stderr).contains("closed 1 byte limit"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_worker_reports_infrastructure_without_leaking_process_errors() {
    let missing = std::env::temp_dir().join(format!(
        "tondo-hidden-worker-missing-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["__test-worker", "--manifest"])
        .arg(&missing)
        .args(["--entry", "missing"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "infrastructure");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot read manifest")
    );
}

#[test]
fn test_command_reports_failures_and_publishes_json_and_junit() {
    let directory = test_project(b"test smoke { assert(false) }\n");
    let json = directory.join("target/report.json");
    let junit = directory.join("target/report.xml");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--manifest",
            "tondo.json",
            "--report",
            "json=target/report.json",
            "--report",
            "junit=target/report.xml",
        ])
        .output()
        .unwrap();
    let json_bytes = fs::read(&json).unwrap();
    let junit_bytes = fs::read(&junit).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("FAIL cli::integration::smoke::smoke")
    );
    assert!(String::from_utf8_lossy(&json_bytes).contains("tondo-test-report-0.1/7"));
    assert!(String::from_utf8_lossy(&junit_bytes).contains("<testsuites"));
}

#[test]
fn test_command_executes_retry_rounds_and_preserves_each_attempt() {
    let directory = test_project(b"test smoke { assert(false) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--manifest",
            "tondo.json",
            "--retry",
            "1",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(output.status.code(), Some(1));
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.tests()[0].attempts.len(), 2);
    assert_eq!(report.tests()[0].attempts[0].round, 0);
    assert_eq!(report.tests()[0].attempts[1].round, 1);
    assert_eq!(report.metadata().retry.rounds.len(), 1);
}

#[test]
fn test_command_repeats_in_fresh_attempts_and_emits_json_lists_and_owners() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    fs::create_dir_all(directory.join(".github")).unwrap();
    fs::write(directory.join(".github/CODEOWNERS"), b"* @tondo\n").unwrap();
    let repeated = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--manifest",
            "tondo.json",
            "--repeat",
            "2",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let report = TestReport::parse(&repeated.stdout).unwrap();
    assert_eq!(report.tests()[0].attempts.len(), 2);
    assert_eq!(report.tests()[0].owners, ["@tondo"]);

    let listed = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--manifest",
            "tondo.json",
            "--list",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let list = TestList::parse(&listed.stdout).unwrap();
    assert_eq!(list.tests()[0].owners, ["@tondo"]);
}

#[test]
fn fmt_writes_canonical_source_to_stdout_without_modifying_the_file() {
    let original = b"fn main(){let values=[1,2]\n}\n";
    let source = source_file_with(original);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("fmt")
        .arg(&source)
        .output()
        .unwrap();
    let persisted = fs::read(&source).unwrap();
    fs::remove_file(source).unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, b"fn main() {\n    let values = [1, 2]\n}\n");
    assert_eq!(persisted, original);
}

#[test]
fn fmt_check_is_silent_and_succeeds_only_for_a_fixed_point() {
    let unformatted = source_file_with(b"fn main( ){}\n");
    let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["fmt", "--check"])
        .arg(&unformatted)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.is_empty());

    fs::write(&unformatted, b"fn main() {}\n").unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["fmt", "--check"])
        .arg(&unformatted)
        .output()
        .unwrap();
    fs::remove_file(unformatted).unwrap();

    assert!(accepted.status.success());
    assert!(accepted.stdout.is_empty());
    assert!(accepted.stderr.is_empty());
}

#[test]
fn fmt_rejects_invalid_source_without_partial_stdout() {
    let source = source_file_with(b"enum Empty {}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["fmt", "--diagnostic-format=json"])
        .arg(&source)
        .output()
        .unwrap();
    fs::remove_file(source).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("\"code\":\"E0004\"")
    );
}

#[test]
fn run_executes_sync_main_and_preserves_runtime_exit_classes() {
    let success = source_file_with(b"fn main() {\n    assert(true)\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("run")
        .arg(&success)
        .output()
        .unwrap();
    fs::remove_file(success).unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let panicking = source_file_with(b"fn main() {\n    panic(\"boom\")\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostic-format=json"])
        .arg(&panicking)
        .output()
        .unwrap();
    fs::remove_file(panicking).unwrap();
    assert_eq!(output.status.code(), Some(101));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("\"code\":\"P0008\"")
    );
}

#[test]
fn run_reports_a_missing_hosted_entry() {
    let source = source_file_with(b"fn helper() {}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostic-format=json"])
        .arg(&source)
        .output()
        .unwrap();
    fs::remove_file(source).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("\"code\":\"E1806\"")
    );
}

#[test]
fn run_writes_console_print_to_stdout_without_an_implicit_newline() {
    let source = source_file_with(
        b"import std.console\nfn main() {\n    console.print(\"Hello\")\n    console.print(\", Tondo!\")\n}\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("run")
        .arg(&source)
        .output()
        .unwrap();
    fs::remove_file(source).unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"Hello, Tondo!");
    assert!(output.stderr.is_empty());
}

#[test]
fn project_check_uses_the_default_lockfile_and_emits_canonical_products() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("tondo-project-{}-{nonce}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    let source = b"fn main() {}\n";
    fs::write(directory.join("src/main.to"), source).unwrap();
    let package_id = "workspace:cli@1";
    let source_hash = sha256(source);
    let manifest = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[\"console\",\"process\",\"clock\",\"environment\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    fs::write(directory.join("tondo.json"), &manifest).unwrap();
    let package_fingerprint = format!(
        "{{\"package_id\":\"{package_id}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{source_hash}\"}}],\"interface_hash\":null}}"
    );
    let package_hash = sha256(package_fingerprint.as_bytes());
    let lockfile = format!(
        "{{\"format\":\"{LOCKFILE_FORMAT}\",\"manifest_hash\":\"{}\",\"standard\":{{\"package_id\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"content_hash\":\"{}\"}},\"packages\":[{{\"id\":\"{package_id}\",\"content_hash\":\"{package_hash}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{source_hash}\"}}],\"interface\":null}}],\"generator_inputs\":[],\"privileged_units\":[]}}",
        sha256(manifest.as_bytes()),
        bootstrap_standard_hash(),
    );
    fs::write(directory.join("tondo.lock.json"), lockfile).unwrap();
    let interface_path = directory.join("app.ti");
    let artifact_path = directory.join("app.ta");

    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["check", "--manifest", "tondo.json", "--emit-interface"])
        .arg(&interface_path)
        .arg("--emit-artifact")
        .arg(&artifact_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let interface = CompiledInterface::decode(&fs::read(&interface_path).unwrap()).unwrap();
    let artifact = BuildArtifact::decode(&fs::read(&artifact_path).unwrap()).unwrap();
    assert_eq!(interface.package_id(), package_id);
    assert_eq!(interface.target(), "tondo-vm-hosted");
    assert_eq!(artifact.source_form(), "module");
    assert_eq!(artifact.interface_hash(), interface.content_hash().unwrap());
    assert!(artifact.reproducible());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn source_io_and_product_write_failures_have_stable_exit_classes() {
    let missing = std::env::temp_dir().join(format!(
        "missing-tondo-cli-{}-{}.to",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("check")
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read source"));

    let source = source_file();
    let directory = std::env::temp_dir().join(format!(
        "tondo-cli-product-dir-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("check")
        .arg(&source)
        .arg("--emit-interface")
        .arg(&directory)
        .output()
        .unwrap();
    fs::remove_file(source).unwrap();
    fs::remove_dir(directory).unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot write interface"));
}
