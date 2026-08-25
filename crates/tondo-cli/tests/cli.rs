use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use tondo_compiler::artifact::{BuildArtifact, CAPABILITY_REGISTRY, CompiledInterface, sha256};
use tondo_compiler::project::{
    BOOTSTRAP_STANDARD_PACKAGE, LOCKFILE_FORMAT, MANIFEST_FORMAT, ProjectPlan,
    bootstrap_standard_hash,
};
use tondo_compiler::test_plan::TestProjectPlan;
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

fn markdown_file_with(bytes: &[u8]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "tondo-cli-doc-test-{}-{nonce}-{id}.md",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    path
}

fn remove_json_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                remove_json_nulls(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_json_nulls(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[test]
fn missing_source_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("check")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("a source file is required"),
        "unexpected diagnostic: {stderr}"
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
fn json_public_surface_runs_through_the_cli() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/runtime/m11-std-codecs-001.to");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("run")
        .arg(source)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\ncodecs-ok\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn check_and_run_accept_a_conventional_project_without_json_configuration() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "tondo-conventional-cli-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("src/main.to"),
        b"import std.console\nfn main() { console.print(\"conventional\\n\") }\n",
    )
    .unwrap();
    fs::write(directory.join("tondo.toml"), "[package]\nname = \"demo\"\n").unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "--project"])
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(check.stdout.is_empty());

    let run = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--project"])
        .arg(&directory)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"conventional\n");
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
fn doc_test_publishes_an_ordered_json_array_only_after_all_fences_pass() {
    let markdown = markdown_file_with(
        b"# Examples\n\n~~~tondo\nfn add(a: Int, b: Int): Int { a + b }\n~~~\n\n~~~tondo pseudocode\nnot Tondo\n~~~\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&markdown)
        .output()
        .unwrap();
    fs::remove_file(markdown).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("[{\"file\""));
    assert!(output.stdout.ends_with(b"\n"));
    let records: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["category"], "syntax");
    assert_eq!(records[0]["parse_ok"], true);
    assert_eq!(records[1]["category"], "pseudocode");
    assert_eq!(records[1]["parse_ok"], serde_json::Value::Null);
}

#[test]
fn doc_test_normalizes_crlf_inside_fences_without_changing_source_hash() {
    let lf = markdown_file_with(b"x\n~~~tondo\nlet value = 1\n~~~\n");
    let crlf = markdown_file_with(b"x\r\n~~~tondo\r\nlet value = 1\r\n~~~\r\n");
    let run = |path: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["doc-test", "--edition=0.1"])
            .arg(path)
            .output()
            .unwrap()
    };
    let left = run(&lf);
    let right = run(&crlf);
    fs::remove_file(lf).unwrap();
    fs::remove_file(crlf).unwrap();

    assert!(
        left.status.success(),
        "{}",
        String::from_utf8_lossy(&left.stderr)
    );
    assert!(
        right.status.success(),
        "{}",
        String::from_utf8_lossy(&right.stderr)
    );
    let left: serde_json::Value = serde_json::from_slice(&left.stdout).unwrap();
    let right: serde_json::Value = serde_json::from_slice(&right.stdout).unwrap();
    assert_eq!(left[0]["source_sha256"], right[0]["source_sha256"]);
    assert_ne!(left[0]["fence_byte"], right[0]["fence_byte"]);
}

#[test]
fn doc_test_rejects_unterminated_fences_without_partial_output() {
    let markdown = markdown_file_with(b"~~~tondo\nlet value = 1\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&markdown)
        .output()
        .unwrap();
    fs::remove_file(markdown).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not closed"));
}

#[test]
fn doc_test_uses_the_normative_fixture_for_typed_fragments() {
    let markdown = markdown_file_with(b"~~~tondo fragment spec.core\nlet value = 1\n~~~\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&markdown)
        .output()
        .unwrap();
    fs::remove_file(markdown).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records[0]["category"], "fragment");
    assert_eq!(records[0]["fixture"], "spec.core");
    assert_eq!(records[0]["typecheck_ok"], true);
}

#[test]
fn doc_test_rejects_unknown_headers_without_partial_output() {
    let markdown = markdown_file_with(b"~~~tondo unknown\nlet value = 1\n~~~\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&markdown)
        .output()
        .unwrap();
    fs::remove_file(markdown).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown Tondo fence header"));
}

#[test]
fn doc_test_requires_the_normative_edition_and_one_markdown_path() {
    let missing_edition = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "README.md"])
        .output()
        .unwrap();
    assert_eq!(missing_edition.status.code(), Some(2));
    assert!(missing_edition.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing_edition.stderr).contains("--edition 0.1"));

    let missing_path = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .output()
        .unwrap();
    assert_eq!(missing_path.status.code(), Some(2));
    assert!(missing_path.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing_path.stderr).contains("Markdown path"));
}

#[test]
fn doc_test_rejects_a_missing_edition_value_before_reading_a_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires `0.1`"));
}

#[test]
fn doc_test_rejects_a_missing_markdown_file_without_partial_output() {
    let path = std::env::temp_dir().join(format!(
        "tondo-cli-doc-test-missing-{}-{}.md",
        std::process::id(),
        TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read documentation Markdown"));
}

#[cfg(unix)]
#[test]
fn doc_test_rejects_non_utf8_paths_at_the_argument_boundary() {
    let invalid = OsString::from_vec(vec![b'd', b'o', b'c', 0xff]);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(invalid)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("valid UTF-8"));
}

#[test]
fn doc_test_reports_a_compile_fail_contract_mismatch_without_output() {
    let markdown =
        markdown_file_with(b"~~~tondo compile-fail E0005\nlet value: Int = \"text\"\n~~~\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["doc-test", "--edition", "0.1"])
        .arg(&markdown)
        .output()
        .unwrap();
    fs::remove_file(markdown).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("did not satisfy"));
}

#[test]
fn doc_test_is_byte_deterministic_and_hashes_canonical_formatting() {
    let markdown = markdown_file_with(b"~~~tondo\nfn add( a:Int,b:Int):Int {a+b}\n~~~\n");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["doc-test", "--edition", "0.1"])
            .arg(&markdown)
            .output()
            .unwrap()
    };
    let first = run();
    let second = run();
    fs::remove_file(markdown).unwrap();

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
    let records: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let formatted = sha256(b"fn add(a: Int, b: Int): Int {\n    a + b\n}\n");
    assert_eq!(
        records[0]["formatted_sha256"],
        formatted.strip_prefix("sha256:").unwrap()
    );
    assert_ne!(records[0]["source_sha256"], records[0]["formatted_sha256"]);
}

#[test]
fn doc_test_reports_exact_container_errors_without_partial_json() {
    let cases = [
        (
            &b"plain\rtext\n"[..],
            "invalid documentation fence at byte 5: isolated CR line ending",
        ),
        (
            &b"ok\n\xff\n"[..],
            "invalid documentation fence at byte 3: Markdown is not valid UTF-8",
        ),
        (
            &b"~~~tondo fragment Bad\n~~~\n"[..],
            "invalid documentation fence at byte 0: invalid fixture name `Bad`",
        ),
        (
            &b"~~~tondo compile-fail E9999\n~~~\n"[..],
            "invalid documentation fence at byte 0: unknown compile-fail code `E9999`",
        ),
    ];

    for (document, expected) in cases {
        let markdown = markdown_file_with(document);
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["doc-test", "--edition", "0.1"])
            .arg(&markdown)
            .output()
            .unwrap();
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        fs::remove_file(markdown).unwrap();

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(
            diagnostic.ends_with(&format!("{expected}\n")),
            "unexpected diagnostic: {diagnostic}"
        );
    }
}

#[test]
fn test_command_defaults_to_the_conventional_project_directory() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    fs::remove_file(directory.join("tondo.test.toml")).unwrap();
    let parsed = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test", "--filter", "smoke", "--order", "random", "--seed", "5eed",
        ])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        parsed.status.success(),
        "{}",
        String::from_utf8_lossy(&parsed.stderr)
    );
    assert!(parsed.stdout.is_empty() || String::from_utf8_lossy(&parsed.stdout).contains("PASS"));

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
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::create_dir_all(directory.join("tests")).unwrap();
    fs::write(directory.join("src/main.to"), b"fn main() {}\n").unwrap();
    fs::write(directory.join("tests/smoke.to"), source).unwrap();
    fs::write(
        directory.join("tondo.toml"),
        "[package]\nname = \"cli\"\n[target]\ncapabilities = [\"console\", \"process\", \"clock\", \"environment\"]\n",
    )
    .unwrap();
    let package_id = "workspace:cli@local";
    let production_source = b"fn main() {}\n";
    let production_hash = sha256(production_source);
    let source_hash = sha256(source);
    let production_manifest_text = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[\"console\",\"process\",\"clock\",\"environment\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    let production_manifest_value: serde_json::Value =
        serde_json::from_str(&production_manifest_text).unwrap();
    let production_manifest = serde_json::to_vec(&production_manifest_value).unwrap();
    let manifest_text = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[\"console\",\"process\",\"clock\",\"environment\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}},{{\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"tests\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    let manifest_value: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
    let manifest = serde_json::to_vec(&manifest_value).unwrap();
    let production_package_fingerprint = format!(
        "{{\"package_id\":\"{package_id}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{production_hash}\"}}],\"interface_hash\":null}}"
    );
    let production_package_hash = sha256(production_package_fingerprint.as_bytes());
    let package_fingerprint = format!(
        "{{\"package_id\":\"{package_id}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{production_hash}\"}},{{\"source_set\":\"common\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"tests\",\"sha256\":\"{source_hash}\"}}],\"interface_hash\":null}}"
    );
    let package_hash = sha256(package_fingerprint.as_bytes());
    let production_lockfile = format!(
        "{{\"format\":\"{LOCKFILE_FORMAT}\",\"manifest_hash\":\"{}\",\"standard\":{{\"package_id\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"content_hash\":\"{}\"}},\"packages\":[{{\"id\":\"{package_id}\",\"content_hash\":\"{production_package_hash}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{production_hash}\"}}],\"interface\":null}}],\"generator_inputs\":[],\"privileged_units\":[]}}",
        sha256(&production_manifest),
        bootstrap_standard_hash(),
    );
    let mut production_lock_value: serde_json::Value =
        serde_json::from_str(&production_lockfile).unwrap();
    remove_json_nulls(&mut production_lock_value);
    let lock_toml =
        toml::to_string(&toml::Value::try_from(production_lock_value).unwrap()).unwrap();
    fs::write(directory.join("tondo.lock.toml"), lock_toml).unwrap();
    let normalized_production_lock = serde_json::to_vec(
        &toml::from_str::<toml::Value>(
            &fs::read_to_string(directory.join("tondo.lock.toml")).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut test_lock: serde_json::Value =
        serde_json::from_slice(&normalized_production_lock).unwrap();
    test_lock["manifest_hash"] = sha256(&manifest).into();
    test_lock["packages"][0]["content_hash"] = package_hash.into();
    test_lock["packages"][0]["sources"] = serde_json::json!([
        {
            "source_set": "common",
            "physical_path": "src/main.to",
            "logical_path": "src/main.to",
            "module": "main",
            "sha256": production_hash
        },
        {
            "source_set": "common",
            "physical_path": "tests/smoke.to",
            "logical_path": "tests/smoke.to",
            "module": "tests",
            "sha256": source_hash
        }
    ]);
    let test_lock = serde_json::to_vec(&test_lock).unwrap();
    let project = ProjectPlan::parse(&manifest, &test_lock).unwrap();
    fs::write(
        directory.join("tests/snapshots.json"),
        SnapshotStore::empty(package_id)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
    )
    .unwrap();
    let test_plan = TestProjectPlan::defaults(&project, 1)
        .canonical_bytes()
        .unwrap();
    let mut test_plan_value: serde_json::Value = serde_json::from_slice(&test_plan).unwrap();
    remove_json_nulls(&mut test_plan_value);
    let test_plan_toml = toml::to_string(&toml::Value::try_from(test_plan_value).unwrap()).unwrap();
    fs::write(directory.join("tondo.test.toml"), test_plan_toml).unwrap();
    directory
}

#[test]
fn test_command_executes_a_test_body_through_the_vm_backend() {
    let directory = test_project(b"test smoke { assert(true) }\n");

    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PASS cli::integration::tests::smoke")
    );
}

#[test]
fn test_command_uses_opinionated_defaults_without_a_sidecar() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    fs::remove_file(directory.join("tondo.test.toml")).unwrap();
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
    let explicit = directory.join("custom-plan.toml");
    let sidecar = fs::read_to_string(directory.join("tondo.test.toml")).unwrap();
    fs::write(&explicit, sidecar.replace("retry = 0", "retry = 1")).unwrap();
    fs::remove_file(directory.join("tondo.test.toml")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--test-plan",
            "custom-plan.toml",
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

fn rewrite_test_plan(directory: &std::path::Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut value: serde_json::Value = serde_json::to_value(
        toml::from_str::<toml::Value>(
            &fs::read_to_string(directory.join("tondo.test.toml")).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    edit(&mut value);
    let toml_plan = toml::to_string(&toml::Value::try_from(value).unwrap()).unwrap();
    fs::write(directory.join("tondo.test.toml"), toml_plan).unwrap();
}

#[test]
fn test_command_accepts_an_optional_toml_plan_sidecar() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let explicit = directory.join("custom-plan.toml");
    fs::copy(directory.join("tondo.test.toml"), &explicit).unwrap();
    fs::remove_file(directory.join("tondo.test.toml")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--test-plan",
            "custom-plan.toml",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report.tests().len(), 1);
}

#[test]
fn test_command_cannot_disable_the_closed_sidecar_timeout() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--timeout", "none"])
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
        .args(["test", "--timeout", "20ms"])
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
    rewrite_test_plan(&directory, |plan| {
        plan["snapshot_stores"] = serde_json::json!([{
            "name": "default",
            "path": "tests/snapshots.json",
            "update": false,
            "max_bytes": 1_048_576
        }]);
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--project"])
        .arg(&directory)
        .args(["--update-snapshots", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "snapshot update failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot = fs::read(directory.join("tests/snapshots.json")).unwrap_or_else(|error| {
        panic!(
            "snapshot store was not published: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
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
    rewrite_test_plan(&directory, |plan| {
        plan["snapshot_stores"] = serde_json::json!([{
            "name": "default",
            "path": "tests/snapshots.json",
            "update": false,
            "max_bytes": 1_048_576
        }]);
    });
    fs::remove_file(directory.join("tests/snapshots.json")).unwrap();
    let missing = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--project"])
        .arg(&directory)
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot load snapshot store"));
    fs::remove_dir_all(directory).unwrap();

    let directory = test_project(b"test smoke { assert(true) }\n");
    rewrite_test_plan(&directory, |plan| {
        plan["snapshot_stores"] = serde_json::json!([{
            "name": "default",
            "path": "tests/snapshots.json",
            "update": false,
            "max_bytes": 1_048_576
        }]);
    });
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
        .args(["test", "--project"])
        .arg(&directory)
        .output()
        .unwrap();
    assert_eq!(wrong_package.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&wrong_package.stderr).contains("belongs to package"),
        "unexpected diagnostic: {}",
        String::from_utf8_lossy(&wrong_package.stderr)
    );
    fs::remove_dir_all(directory).unwrap();

    let directory = test_project(b"test smoke { assert(true) }\n");
    rewrite_test_plan(&directory, |plan| {
        plan["snapshot_stores"] = serde_json::json!([{
            "name": "default",
            "path": "tests/snapshots.json",
            "update": false,
            "max_bytes": 1
        }]);
    });
    let too_large = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--project"])
        .arg(&directory)
        .args(["--update-snapshots"])
        .output()
        .unwrap();
    assert_eq!(too_large.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&too_large.stderr).contains("closed 1 byte limit"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_worker_reports_infrastructure_without_leaking_process_errors() {
    let missing = std::env::temp_dir().join(format!(
        "tondo-hidden-worker-missing-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["__test-worker", "--project"])
        .arg(&missing)
        .args(["--entry", "missing"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["format"], "tondo-test-worker-batch/2");
    assert_eq!(response["responses"][0][1]["status"], "infrastructure");
    assert!(
        response["responses"][0][1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot resolve project directory")
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
        String::from_utf8_lossy(&output.stdout).contains("PANIC cli::integration::tests::smoke")
    );
    assert!(String::from_utf8_lossy(&json_bytes).contains("tondo-test-report-0.1/7"));
    assert!(String::from_utf8_lossy(&junit_bytes).contains("<testsuites"));
}

#[test]
fn test_command_executes_retry_rounds_and_preserves_each_attempt() {
    let directory = test_project(b"test smoke { assert(false) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
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
fn diagnostics_are_attached_to_each_retry_and_projected_to_junit() {
    let directory = test_project(b"test smoke { assert(false) }\n");
    let junit = directory.join("target/diagnostics.xml");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--diagnostics",
            "all",
            "--retry",
            "1",
            "--test-format",
            "json",
            "--report",
        ])
        .arg(format!("junit={}", junit.display()))
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap();
    let junit_bytes = fs::read(&junit).unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(output.status.code(), Some(1));
    let attempts = &report.tests()[0].attempts;
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.diagnostics.len() == 3)
    );
    assert_ne!(
        attempts[0].diagnostics[0].attempt_id,
        attempts[1].diagnostics[0].attempt_id
    );
    assert_eq!(
        attempts[0].diagnostics[0].run_id,
        attempts[1].diagnostics[0].run_id
    );
    assert!(String::from_utf8_lossy(&junit_bytes).contains("tondo.diagnostics"));
}

#[test]
fn crash_diagnostics_publish_a_content_addressed_dump_descriptor() {
    let directory = test_project(b"test smoke { panic(\"boom\") }\n");
    let artifacts = directory.join("diagnostic-artifacts");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--diagnostics",
            "crash",
            "--artifacts",
            "diagnostic-artifacts",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}; status={:?}; stdout={:?}; stderr={:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let attempt = &report.tests()[0].attempts[0];
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(attempt.diagnostics.len(), 1);
    assert_eq!(attempt.diagnostics[0].profile, "crash");
    assert_eq!(
        attempt.diagnostics[0].status,
        tondo_compiler::test_result::DiagnosticStatus::Finding
    );
    assert_eq!(
        attempt.diagnostics[0].artifacts[0].name,
        "diagnostic-crash.tdump"
    );
    assert!(
        attempt
            .artifacts
            .iter()
            .any(|artifact| artifact.name == "diagnostic-crash.tdump")
    );
    assert!(artifacts.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_command_repeats_in_fresh_attempts_and_emits_json_lists_and_owners() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    fs::create_dir_all(directory.join(".github")).unwrap();
    fs::write(directory.join(".github/CODEOWNERS"), b"* @tondo\n").unwrap();
    let repeated = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--repeat", "2", "--test-format", "json"])
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
        .args(["test", "--list", "--test-format", "json"])
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
    let directory = test_project(b"test smoke { assert(true) }\n");
    let package_id = "workspace:cli@local";
    let interface_path = directory.join("app.ti");
    let artifact_path = directory.join("app.ta");

    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["check", "--emit-interface"])
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
