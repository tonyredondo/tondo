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
use tondo_compiler::test_result::AggregateStatus;
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

fn project_with_source_and_threads(bytes: &[u8]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "tondo-cli-executor-{}-{nonce}-{id}",
        std::process::id()
    ));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/main.to"), bytes).unwrap();
    fs::write(
        directory.join("tondo.toml"),
        "[package]\nname = \"executordemo\"\n\n[target]\ncapabilities = [\"console\", \"clock\", \"environment\", \"filesystem\", \"threads\"]\n",
    )
    .unwrap();
    directory
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
fn std_executor_public_surface_runs_through_the_cli() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/runtime/m11-std-executor-impl-001.to");
    let project = project_with_source_and_threads(&fs::read(source).unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--project"])
        .arg(&project)
        .output()
        .unwrap();
    fs::remove_dir_all(project).unwrap();

    assert!(
        output.status.success(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"executor-ok\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn std_executor_public_surface_with_diagnostics_runs_through_the_cli() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/runtime/m11-std-executor-impl-001.to");
    let project = project_with_source_and_threads(&fs::read(source).unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostics=all", "--project"])
        .arg(&project)
        .output()
        .unwrap();
    fs::remove_dir_all(project).unwrap();

    assert!(
        output.status.success(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"executor-ok\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn std_executor_blocking_worker_can_request_a_host_call() {
    let project = project_with_source_and_threads(
        br#"import std.console
import std.executor

fn blocking_host(): Int ! console.ConsoleError {
    console.print("blocking-host\n")?
    42
}

fn main(): !(executor.ExecutorError | executor.SubmitError | console.ConsoleError) {
    let pool = executor.blockingPool(1, 1)?
    assert(pool.run(blocking_host)? == 42)
    pool.shutdown()
    console.println("executor-host-ok")?
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--project"])
        .arg(&project)
        .output()
        .unwrap();
    fs::remove_dir_all(project).unwrap();

    assert!(
        output.status.success(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"blocking-host\nexecutor-host-ok\n");
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
        b"import std.console\nfn main(): !console.ConsoleError { console.print(\"conventional\\n\")? }\n",
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
    test_project_with_threads(source, false)
}

fn test_project_with_threads(source: &[u8], threads: bool) -> std::path::PathBuf {
    let mut capabilities = vec!["console", "clock", "environment"];
    if threads {
        capabilities.push("threads");
    }
    test_project_with_capabilities(source, &capabilities)
}

#[cfg(target_os = "linux")]
fn process_isolation_root() -> std::path::PathBuf {
    std::env::var_os("TONDO_TEST_PROCESS_CGROUP")
        .map(std::path::PathBuf::from)
        .expect("process lifecycle tests require an explicit delegated TONDO_TEST_PROCESS_CGROUP")
}

#[test]
fn generation_ids_are_public_records_and_replay_their_exact_uint64_fields() {
    let directory = test_project_with_capabilities(
        br#"import std.testing

fn replay(id: testing.GenerationId): UInt64 {
    var generator = testing.Generator.forCase(id.seed, id.caseIndex)
    testing.assertOk(generator.nextUInt())
}

test publicRecord {
    var generator = testing.Generator.forCase(18446744073709551615, 18446744073709551615)
    let id: testing.GenerationId = generator.id()
    assert(id.seed == 18446744073709551615)
    assert(id.caseIndex == 18446744073709551615)
    let copied = id
    assert(replay(copied) == testing.assertOk(generator.nextUInt()))
    assert(id.seed == copied.seed)
    let manual = testing.GenerationId { seed: 7, caseIndex: 3 }
    let changed = manual with { caseIndex: 4 }
    assert(manual.caseIndex == 3)
    assert(changed.seed == 7)
    assert(changed.caseIndex == 4)
    assert(replay(manual) == replay(manual))
}
"#,
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.summary().selected, 1);
    assert_eq!(report.summary().passed, 1);
    for body in [
        "let id = testing.Generator.new(7).id()\n _ = id.unknownField",
        "_ = testing.GenerationId { seed: 7 }",
    ] {
        fs::write(
            directory.join("tests/smoke.to"),
            format!("import std.testing\ntest invalid {{\n{body}\n}}\n"),
        )
        .unwrap();
        let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            rejected.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(rejected.stdout.is_empty());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("E1102"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tolerance_errors_are_public_exhaustive_variants_with_exact_numeric_causes() {
    let directory = test_project_with_capabilities(
        br#"import std.testing

fn constrainedDisplay[T: Discard + Display](value: T): String {
    value.display()
}

fn classify(error: testing.FloatToleranceError): Int {
    match error {
        testing.FloatToleranceError.Negative => 1
        testing.FloatToleranceError.NonFinite => 2
        testing.FloatToleranceError.Overflow => 3
    }
}

fn code(absolute: Float, relative: Float): Int {
    match testing.FloatTolerance.from(absolute, relative) {
        ok(_) => 0
        err(error) => classify(error)
    }
}

test toleranceErrors {
    assert(code(-1.0, 0.0) == 1)
    assert(code(0.0, -1.0) == 1)
    assert(code(0.0 / 0.0, 0.0) == 2)
    assert(code(1.0 / 0.0, 0.0) == 2)
    assert(code(0.0, -1.0 / 0.0) == 2)
    assert(code(1.7976931348623157e308, 1.7976931348623157e308) == 3)
    assert(code(-0.0, 0.0) == 0)
    assert(classify(testing.FloatToleranceError.Overflow) == 3)
    _ = testing.assertOk(testing.FloatTolerance.from(-0.0, 0.0))
    let error = match testing.FloatTolerance.from(-1.0, 0.0) {
        err(error) => error
        ok(_) => testing.failNow("negative tolerance was accepted")
    }
    assert(classify(error) == 1)
    assert("{error}" == "FloatToleranceError.Negative")
    assert(Display.display(error) == "FloatToleranceError.Negative")
    assert(constrainedDisplay(error) == "FloatToleranceError.Negative")
    let errors = [testing.FloatToleranceError.NonFinite, testing.FloatToleranceError.Overflow]
    assert("{errors}" == "[FloatToleranceError.NonFinite, FloatToleranceError.Overflow]")
}
"#,
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.summary().passed, 1);
    fs::write(
        directory.join("tests/smoke.to"),
        br#"import std.testing
fn incomplete(error: testing.FloatToleranceError): Int {
    match error {
        testing.FloatToleranceError.Negative => 1
        testing.FloatToleranceError.NonFinite => 2
    }
}
test invalid {
    _ = incomplete(testing.FloatToleranceError.Overflow)
}
"#,
    )
    .unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("E1204"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    for (source, diagnostic) in [
        (
            "import std.testing\ntest invalid {\n    _ = testing.FloatToleranceError.Negative.display()\n}\n",
            "E1102",
        ),
        (
            "enum FloatToleranceError { Negative }\ntest invalid {\n    _ = Display.display(FloatToleranceError.Negative)\n}\n",
            "E1105",
        ),
    ] {
        fs::write(directory.join("tests/smoke.to"), source).unwrap();
        let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(1));
        assert!(rejected.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(diagnostic),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn temporary_errors_are_public_variants_and_valid_directories_remain_affine() {
    let directory = test_project_with_capabilities(
        br#"import std.testing

fn classify(error: testing.TempError): Int {
    match error {
        testing.TempError.InvalidPrefix => 1
        testing.TempError.Unavailable => 2
        testing.TempError.PermissionDenied => 3
        testing.TempError.LimitExceeded => 4
        testing.TempError.IoError => 5
    }
}

test temporaryErrors {
    match testing.tempDirectory("bad/prefix") {
        ok(directory) => {
            testing.TempDirectory.cleanup(directory)
            assert(false)
        }
        err(error) => {
            assert(classify(error) == 1)
            assert(Display.display(error) == "TempError.InvalidPrefix")
        }
    }
    assert(classify(testing.TempError.Unavailable) == 2)
    assert(classify(testing.TempError.PermissionDenied) == 3)
    assert(classify(testing.TempError.LimitExceeded) == 4)
    assert(classify(testing.TempError.IoError) == 5)
    let directory = testing.assertOk(testing.tempDirectory("nominal"))
    defer testing.TempDirectory.cleanup(directory)
}
"#,
        &["filesystem"],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    let roots = directory.join("target/.tondo-test-root");
    assert!(fs::read_dir(roots).unwrap().next().is_none());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generation_errors_are_public_and_failed_requests_preserve_the_stream() {
    let directory = test_project_with_capabilities(
        br#"import std.testing

fn classify(error: testing.GenerationError): Int {
    match error {
        testing.GenerationError.InvalidBounds => 1
        testing.GenerationError.LimitExceeded => 2
        testing.GenerationError.Exhausted => 3
    }
}

test generationErrors {
    var generator = testing.Generator.new(42)
    let before = generator.drawCount()
    let bounds = testing.assertErr(generator.nextInt(2, 1))
    assert(classify(bounds) == 1)
    assert(Display.display(bounds) == "GenerationError.InvalidBounds")
    assert(classify(testing.assertErr(generator.nextText(-1))) == 1)
    assert(classify(testing.assertErr(generator.nextText(1048577))) == 2)
    match generator.nextBytes(1048577) {
        ok(_) => assert(false)
        err(error) => assert(classify(error) == 2)
    }
    assert(generator.drawCount() == before)
    assert(classify(testing.GenerationError.Exhausted) == 3)
    _ = testing.assertOk(generator.nextUInt())
    assert(generator.drawCount() == before + 1)
    let original = 13
    let candidates = testing.assertOk(testing.shrink(ref original))
    assert(candidates.length() == 4)
}
"#,
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn shrink_candidates_executes_qualified_and_constrained_calls_with_explicit_limits() {
    let directory = test_project_with_capabilities(
        r#"import std.testing

fn limited[T: Discard + Shrink + Equatable](value: T, limit: Int): Array[T] ! testing.GenerationError {
    value.candidates(limit)
}

test candidates {
    let value = 13
    assert(testing.assertOk(Shrink.candidates(value, 2)) == [6, 3])
    assert(testing.assertOk(testing.Shrink.candidates(value, 2)) == [6, 3])
    assert(testing.assertOk(limited(value, 3)) == [6, 3, 1])
    assert(testing.assertOk(Shrink.candidates(value, 0)).length() == 0)
    assert(testing.assertOk(Shrink.candidates("é🦀", 2)) == ["", "é"])
    assert(testing.assertErr(Shrink.candidates(value, -1)) == testing.GenerationError.InvalidBounds)
    assert(testing.assertErr(Shrink.candidates(value, 4097)) == testing.GenerationError.LimitExceeded)
}
"#.as_bytes(),
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    for (source, diagnostic) in [
        (
            "test noImport {\n    match Shrink.candidates(13, 2) {\n        ok(values) => assert(values == [6, 3])\n        err(_) => assert(false)\n    }\n}\n",
            None,
        ),
        (
            "test invalid {\n    _ = Shrink.candidates(true, 2)\n}\n",
            Some("E1105"),
        ),
        (
            "test invalid {\n    let value = 13\n    _ = value.candidates(2)\n}\n",
            Some("E1102"),
        ),
    ] {
        fs::write(directory.join("tests/smoke.to"), source).unwrap();
        let checked = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        if let Some(diagnostic) = diagnostic {
            assert_eq!(checked.status.code(), Some(1));
            assert!(checked.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&checked.stderr).contains(diagnostic),
                "{}",
                String::from_utf8_lossy(&checked.stderr)
            );
        } else {
            assert!(
                checked.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
            assert_eq!(
                TestReport::parse(&checked.stdout).unwrap().summary().passed,
                1
            );
        }
    }
    fs::write(
        directory.join("src/main.to"),
        "fn main() {\n    _ = Shrink.candidates(13, 2)\n}\n",
    )
    .unwrap();
    let production = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["check", "src/main.to"])
        .output()
        .unwrap();
    assert_eq!(production.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&production.stderr).contains("E2003"),
        "{}",
        String::from_utf8_lossy(&production.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn text_diffs_are_public_records_with_matchable_hunks_and_bounded_rendering() {
    let directory = test_project_with_capabilities(
        r#"import std.testing

test publicDiff {
    let diff: testing.TextDiff = testing.diffText("same\nold\n", "same\nnew\n")
    assert(not diff.equal)
    assert(diff.expectedBytes == 9)
    assert(diff.actualBytes == 9)
    assert(not diff.truncated)
    assert(diff.hunks.length() == 3)
    match diff.hunks[0] {
        testing.TextDiffHunk.Equal(text) => assert(text == "same\n")
        testing.TextDiffHunk.Delete(_) => assert(false)
        testing.TextDiffHunk.Insert(_) => assert(false)
    }
    match diff.hunks[1] {
        testing.TextDiffHunk.Delete(text) => assert(text == "old\n")
        _ => assert(false)
    }
    match diff.hunks[2] {
        testing.TextDiffHunk.Insert(text) => assert(text == "new\n")
        _ => assert(false)
    }
    assert(diff.render() == "--- expected\n+++ actual\n same\n-old\n+new\n")
    assert(testing.TextDiff.render(diff) == diff.render())
    let manual = testing.TextDiff {
        equal: false
        hunks: [testing.TextDiffHunk.Delete("é\r\n"), testing.TextDiffHunk.Insert("🦀")]
        expectedBytes: 4
        actualBytes: 4
        truncated: false
    }
    assert(manual.render() == "--- expected\n+++ actual\n-é\r\n+🦀\n")
    let changed = manual with { truncated: true }
    assert(not manual.truncated)
    assert(changed.render() == "--- expected\n+++ actual\n-é\r\n+🦀\n... truncated ...\n")
    let equal = testing.diffText("é\r\n", "é\r\n")
    assert(equal.equal)
    assert(equal.hunks.length() == 0)
    var large = "é"
    for index in 0..20 {
        large = "{large}{large}"
    }
    let oversized = manual with { hunks: [testing.TextDiffHunk.Insert(large)] }
    assert(oversized.render() == "--- expected\n+++ actual\n... truncated ...\n")
    assert(not oversized.truncated)
    var hunks = [testing.TextDiffHunk.Equal("")]
    for index in 0..4096 {
        hunks.push(testing.TextDiffHunk.Insert(""))?
    }
    let crowded = manual with { hunks: hunks }
    assert(crowded.render() == "--- expected\n+++ actual\n... truncated ...\n")
}
"#
        .as_bytes(),
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    for (declarations, body, code) in [
        ("", "_ = testing.TextDiff { equal: true }", "E1102"),
        (
            "",
            "let diff = testing.diffText(\"\", \"\")\n _ = diff.unknownField",
            "E1102",
        ),
        (
            "",
            "match testing.TextDiffHunk.Equal(\"\") {\n testing.TextDiffHunk.Equal(_) => {}\n }",
            "E1204",
        ),
        (
            "type TextDiff = { equal: Bool }",
            "let diff = TextDiff { equal: true }\n _ = diff.render()",
            "E1102",
        ),
    ] {
        fs::write(
            directory.join("tests/smoke.to"),
            format!("import std.testing\n{declarations}\ntest invalid {{\n{body}\n}}\n"),
        )
        .unwrap();
        let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            rejected.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(rejected.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(code),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn process_tests_require_containment_before_dispatch_and_output_publication() {
    let directory = test_project_with_capabilities(
        b"test smoke { assert(true) }\n",
        &["process", "filesystem"],
    );
    fs::write(directory.join("results.json"), b"prior json").unwrap();
    let snapshots = fs::read(directory.join("tests/snapshots.json")).unwrap();
    for options in [
        vec![],
        vec!["--process-cgroup", directory.to_str().unwrap()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args([
                "test",
                "--test-format",
                "json",
                "--report",
                "json=results.json",
            ])
            .args(options)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(3),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("process isolation unavailable"));
        assert!(output.stdout.is_empty());
        assert_eq!(
            fs::read(directory.join("results.json")).unwrap(),
            b"prior json"
        );
        assert_eq!(
            fs::read(directory.join("tests/snapshots.json")).unwrap(),
            snapshots
        );
        assert!(!directory.join("target/.tondo-test-root").exists());
    }
    // Compilation and discovery do not execute any process-capable code.
    let listed = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list"])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(String::from_utf8_lossy(&listed.stdout).contains("smoke"));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn process_provider_failure_stops_dispatch_and_preserves_complete_outputs() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let root = process_isolation_root();
    let directory = test_project_with_capabilities(
        br#"import std.process
test active {
    _ = process.command("/bin/sh", "-c", "echo ready > ready; exec /bin/sleep 60").run()?
}
test later {
    _ = process.command("/bin/sh", "-c", "echo started > later").run()?
}
"#,
        &["process"],
    );
    fs::write(directory.join("results.json"), b"prior json").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--process-cgroup"])
        .arg(&root)
        .args(["--report", "json=results.json", "--test-format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let prefix = format!("tondo-worker-{}-", child.id());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("ready").exists() && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Revoke only the fresh group belonging to this invocation. The stopped
    // coordinator must observe the provider failure when it resumes.
    let revoked = (|| -> Result<(), String> {
        if !directory.join("ready").exists() {
            return Err("fixture did not become ready".into());
        }
        let stopped = Command::new("kill")
            .args(["-STOP", &child.id().to_string()])
            .status()
            .map_err(|e| e.to_string())?;
        if !stopped.success() {
            return Err("cannot stop fixture coordinator".into());
        }
        let groups = fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        if groups.len() != 1 {
            return Err(format!(
                "expected one owned active group, got {}",
                groups.len()
            ));
        }
        let group = &groups[0];
        fs::write(group.join("cgroup.kill"), b"1").map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events =
                fs::read_to_string(group.join("cgroup.events")).map_err(|e| e.to_string())?;
            if events.lines().any(|line| line == "populated 0") {
                break;
            }
            if Instant::now() >= deadline {
                return Err("fixture group did not become empty".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        fs::remove_dir(group).map_err(|e| e.to_string())
    })();
    let _ = Command::new("kill")
        .args(["-CONT", &child.id().to_string()])
        .status();
    if revoked.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        revoked.is_ok(),
        "{revoked:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !directory.join("later").exists(),
        "dispatch continued after losing the process provider"
    );
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read(directory.join("results.json")).unwrap(),
        b"prior json"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("process cleanup failed"));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn process_containment_removes_redirected_descendants_after_success_failure_and_timeout() {
    use std::process::Stdio;
    let root = process_isolation_root();
    for (terminal, expected) in [
        ("", "passed"),
        ("assert(false)", "failed-panic"),
        ("for {}", "timeout"),
    ] {
        let source = format!(
            r#"import std.process
test child {{
    _ = process.command("/bin/sh", "-c", "setsid /bin/sleep 60 </dev/null >/dev/null 2>&1 & echo $! >> child-pids").run()?
    {terminal}
}}
test sibling {{ assert(true) }}
"#
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &["process"]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["instructions"] = serde_json::json!(100_000_000);
        });
        let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
        command
            .current_dir(&directory)
            .args(["test", "--test-format", "json", "--process-cgroup"])
            .arg(&root)
            .args(["--timeout", "250ms", "--repeat", "2", "--jobs", "2"]);
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let prefix = format!("tondo-worker-{}-", child.id());
        let output = child.wait_with_output().unwrap();
        // Observe exact fixture descendants before any assertion can unwind.
        // Retain and stop a surviving child so a failed reproduction is bounded.
        let pids = fs::read_to_string(directory.join("child-pids")).unwrap_or_default();
        let mut survivors = Vec::new();
        for pid in pids.lines() {
            let state = fs::read_to_string(format!("/proc/{pid}/stat"));
            if state
                .as_ref()
                .is_ok_and(|stat| !stat.rsplit_once(") ").unwrap().1.starts_with('Z'))
            {
                survivors.push(pid.to_owned());
                let _ = Command::new("kill").args(["-KILL", pid]).status();
            }
        }
        assert!(
            survivors.is_empty(),
            "descendants survived the public terminal: {survivors:?}"
        );
        assert_eq!(
            pids.lines().count(),
            2,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.code(),
            Some(if expected == "passed" { 0 } else { 1 }),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for test in report["tests"].as_array().unwrap() {
            let status = if test["id"].as_str().unwrap().ends_with("::child") {
                expected
            } else {
                "passed"
            };
            assert!(
                test["attempts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|attempt| attempt["status"] == status),
                "{test}"
            );
        }
        assert!(
            !fs::read_dir(&root).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&prefix)
            }),
            "an invocation process group survived publication"
        );
        // Other test threads may own groups under the same delegated parent;
        // each invocation's report must omit the physical path regardless.
        assert!(!String::from_utf8_lossy(&output.stdout).contains(root.to_str().unwrap()));
        fs::remove_dir_all(directory).unwrap();
    }
}

fn test_project_with_capabilities(source: &[u8], capabilities: &[&str]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    test_project_with_nonce(source, capabilities, nonce)
}

fn test_project_with_nonce(
    source: &[u8],
    capabilities: &[&str],
    nonce: u128,
) -> std::path::PathBuf {
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "tondo-test-cli-{}-{nonce}-{id}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::create_dir_all(directory.join("tests")).unwrap();
    fs::write(directory.join("src/main.to"), b"fn main() {}\n").unwrap();
    fs::write(directory.join("tests/smoke.to"), source).unwrap();
    fs::write(
        directory.join("tondo.toml"),
        format!(
            "[package]\nname = \"cli\"\n[target]\ncapabilities = {}\n",
            serde_json::to_string(&capabilities).unwrap()
        ),
    )
    .unwrap();
    let package_id = "workspace:cli@local";
    let production_source = b"fn main() {}\n";
    let production_hash = sha256(production_source);
    let source_hash = sha256(source);
    let production_manifest_text = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[\"console\",\"process\",\"clock\",\"environment\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    let mut production_manifest_value: serde_json::Value =
        serde_json::from_str(&production_manifest_text).unwrap();
    production_manifest_value["target"]["capabilities"] = serde_json::json!(capabilities);
    let production_manifest = serde_json::to_vec(&production_manifest_value).unwrap();
    let manifest_text = format!(
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[\"console\",\"process\",\"clock\",\"environment\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}},{{\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"tests\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
    );
    let mut manifest_value: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
    manifest_value["target"]["capabilities"] = serde_json::json!(capabilities);
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
    let production_project =
        ProjectPlan::parse(&production_manifest, &normalized_production_lock).unwrap();
    let test_plan = TestProjectPlan::for_discovered_project(Some(&production_project), &project, 1)
        .unwrap()
        .canonical_bytes()
        .unwrap();
    let mut test_plan_value: serde_json::Value = serde_json::from_slice(&test_plan).unwrap();
    remove_json_nulls(&mut test_plan_value);
    let test_plan_toml = toml::to_string(&toml::Value::try_from(test_plan_value).unwrap()).unwrap();
    fs::write(directory.join("tondo.test.toml"), test_plan_toml).unwrap();
    directory
}

fn write_toml_value(path: &std::path::Path, mut value: serde_json::Value) {
    remove_json_nulls(&mut value);
    fs::write(
        path,
        toml::to_string(&toml::Value::try_from(value).unwrap()).unwrap(),
    )
    .unwrap();
}

fn test_project_with_dependencies() -> std::path::PathBuf {
    use std::collections::BTreeSet;
    use tondo_compiler::artifact::{DeclaredBuildInputs, SourceSetId};
    use tondo_compiler::driver::{
        BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, Edition, HostProfile,
        Operation, ResourceLimits, SourceForm, execute,
    };
    use tondo_compiler::package::{PackageAlias, PackageGraph, PackageId, PackageNode};
    use tondo_compiler::source::{
        LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
    };

    let directory = test_project(
        b"import helper.main as helpers\ntest smoke { assert(helpers.number() == 42) }\n",
    );
    let mut records = Vec::new();
    let mut declarations = Vec::new();
    let mut sources = SourceDatabase::new();
    let mut nodes = Vec::new();
    let mut sets = BTreeSet::new();
    for (name, text, edges) in [
        ("foundation", "pub fn number(): Int { 41 }\n", Vec::new()),
        (
            "helper",
            "import foundation.main as base\npub fn number(): Int { base.number() + 1 }\n",
            vec![("foundation", "workspace:foundation@local")],
        ),
    ] {
        let id = PackageId::new(format!("workspace:{name}@local")).unwrap();
        let source_id = SourceId::new(format!("pkg:{}:{id}", id.as_str().len())).unwrap();
        let physical = format!("deps/{name}/main.to");
        let path = directory.join(&physical);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let source = sources
            .add(SourceInput::new(
                source_id.clone(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("src/main.to").unwrap(),
                SourceOrigin::Physical,
                text.as_bytes(),
            ))
            .unwrap();
        nodes.push(
            PackageNode::new(
                id.clone(),
                source_id,
                PackageAlias::new(name).unwrap(),
                Edition::V0_1,
                [ModulePath::new("main").unwrap()],
                edges.iter().map(|(alias, id)| {
                    (
                        PackageAlias::new(alias).unwrap(),
                        PackageId::new(*id).unwrap(),
                    )
                }),
            )
            .unwrap(),
        );
        sets.insert(SourceSetId::for_package(&id, "common").unwrap());
        let loose = PackageGraph::loose(&sources, source).unwrap();
        let mut graph_nodes = nodes.clone();
        graph_nodes.push(loose.package(loose.standard()).unwrap().clone());
        let packages =
            PackageGraph::new(id.clone(), loose.standard().clone(), graph_nodes).unwrap();
        // Compile real provider sources to obtain their public interfaces.
        let mut compilation_sources = SourceDatabase::new();
        for (_, input) in sources.iter() {
            compilation_sources
                .add(SourceInput::new(
                    input.source_id().clone(),
                    input.module().clone(),
                    input.path().clone(),
                    input.origin(),
                    input.bytes(),
                ))
                .unwrap();
        }
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            ["console", "clock", "environment"]
                .into_iter()
                .map(|value| CapabilityName::new(value).unwrap())
                .collect(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            compilation_sources,
            source,
        )
        .unwrap()
        .with_declared_build_inputs(DeclaredBuildInputs::new(BTreeSet::new(), sets.clone()));
        let output = execute(request).unwrap();
        assert_eq!(output.exit_code(), 0, "{:?}", output.diagnostics());
        let interface = output.interface().unwrap().encode().unwrap();
        let interface_path = format!("deps/{name}/api.ti");
        fs::write(directory.join(&interface_path), &interface).unwrap();
        let hash = sha256(&interface);
        let dependency_text = edges
            .iter()
            .map(|(alias, id)| format!("{{\"alias\":\"{alias}\",\"package\":\"{id}\"}}"))
            .collect::<Vec<_>>()
            .join(",");
        let source_text = format!(
            "{{\"source_set\":\"common\",\"physical_path\":\"{physical}\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{}\"}}",
            sha256(text.as_bytes())
        );
        let fingerprint = format!(
            "{{\"package_id\":\"{id}\",\"dependencies\":[{dependency_text}],\"sources\":[{source_text}],\"interface_hash\":\"{hash}\"}}"
        );
        records.push(serde_json::json!({
            "id": id.as_str(), "local_name": name, "edition": "0.1", "content_hash": sha256(fingerprint.as_bytes()),
            "dependencies": serde_json::from_str::<serde_json::Value>(&format!("[{dependency_text}]")).unwrap(),
            "sources": [serde_json::from_str::<serde_json::Value>(&source_text).unwrap()],
            "interface": {"path": interface_path, "sha256": hash}
        }));
        declarations.push(serde_json::json!({"alias": name, "package": id.as_str(), "interface_path": interface_path, "sha256": hash}));
    }
    let mut lock: serde_json::Value = toml::from_str::<toml::Value>(
        &fs::read_to_string(directory.join("tondo.lock.toml")).unwrap(),
    )
    .unwrap()
    .try_into()
    .unwrap();
    lock["test"] = serde_json::json!({"packages": records});
    write_toml_value(&directory.join("tondo.lock.toml"), lock);
    let mut plan: serde_json::Value = toml::from_str::<toml::Value>(
        &fs::read_to_string(directory.join("tondo.test.toml")).unwrap(),
    )
    .unwrap()
    .try_into()
    .unwrap();
    plan["dev_dependencies"] = declarations.into();
    write_toml_value(&directory.join("tondo.test.toml"), plan);
    directory
}

#[test]
fn public_test_dependencies_execute_transitive_code_and_keep_production_identical() {
    let directory = test_project_with_dependencies();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap()
    };
    let tested = run(&["test", "--test-format", "json", "--repeat", "2"]);
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
    let report = TestReport::parse(&tested.stdout).unwrap();
    assert_eq!(report.summary().passed, 1);
    let built = run(&[
        "check",
        "--emit-interface",
        "before.ti",
        "--emit-artifact",
        "before.ta",
    ]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    fs::remove_dir_all(directory.join("deps")).unwrap();
    let missing = run(&["test", "--list"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    let built = run(&[
        "check",
        "--emit-interface",
        "after.ti",
        "--emit-artifact",
        "after.ta",
    ]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(
        fs::read(directory.join("before.ti")).unwrap(),
        fs::read(directory.join("after.ti")).unwrap()
    );
    assert_eq!(
        fs::read(directory.join("before.ta")).unwrap(),
        fs::read(directory.join("after.ta")).unwrap()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_test_dependencies_use_locked_defaults_in_test_only_projects() {
    let directory = test_project_with_dependencies();
    fs::remove_file(directory.join("tondo.test.toml")).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    fs::remove_file(directory.join("src/main.to")).unwrap();
    let path = directory.join("tondo.lock.toml");
    let lock: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    write_toml_value(&path, serde_json::json!({"test": lock["test"]}));
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        TestReport::parse(&output.stdout).unwrap().summary().passed,
        1
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_test_dependencies_are_visible_to_unit_overlays_with_sealed_production() {
    let directory = test_project_with_dependencies();
    fs::write(
        directory.join("src/main_test.to"),
        b"import helper.main as helpers\ntest unitValue { assert(helpers.number() == 42) }\n",
    )
    .unwrap();
    let plan_path = directory.join("tondo.test.toml");
    let mut plan: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&plan_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    plan["roots"].as_array_mut().unwrap().push(
        serde_json::json!({"class":"unit-test", "physical_path":"src", "logical_path":"src"}),
    );
    plan["sources"].as_array_mut().unwrap().push(serde_json::json!({
        "class":"unit-test", "package":"workspace:cli@local", "physical_path":"src/main_test.to",
        "logical_path":"src/main_test.to", "module":"main", "input":"source:unit-test:src/main_test.to"
    }));
    write_toml_value(&plan_path, plan);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.summary().passed, 2);
    // Production is checked first. An overlay import cannot repair it.
    let invalid = b"import helper.main as helpers\nfn main() {}\n";
    fs::write(directory.join("src/main.to"), invalid).unwrap();
    let lock_path = directory.join("tondo.lock.toml");
    let mut lock: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&lock_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    let source_hash = sha256(invalid);
    lock["packages"][0]["sources"][0]["sha256"] = source_hash.clone().into();
    let fingerprint = format!(
        "{{\"package_id\":\"workspace:cli@local\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\",\"sha256\":\"{source_hash}\"}}],\"interface_hash\":null}}"
    );
    lock["packages"][0]["content_hash"] = sha256(fingerprint.as_bytes()).into();
    write_toml_value(&lock_path, lock.clone());
    lock.as_object_mut().unwrap().remove("test");
    let mut plan: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&plan_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    plan["project"]["lockfile_hash"] = sha256(&serde_json::to_vec(&lock).unwrap()).into();
    write_toml_value(&plan_path, plan);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "error[E1008]: `helper` is not the current package, `std`, or a dependency alias"
        ),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_test_dependencies_reject_drift_before_selection_or_publication() {
    let directory = test_project_with_dependencies();
    for path in ["deps/foundation/main.to", "deps/helper/api.ti"] {
        let path = directory.join(path);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"changed after locking\n").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args([
                "test",
                "--filter",
                "absent",
                "--allow-empty",
                "--test-format",
                "json",
                "--report",
                "junit=result.xml",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("has hash"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("expected"));
        assert!(output.stdout.is_empty());
        assert!(!directory.join("result.xml").exists());
        fs::write(path, original).unwrap();
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_test_dependencies_check_rehashed_interfaces_and_alias_collisions() {
    let directory = test_project_with_dependencies();
    let lock_path = directory.join("tondo.lock.toml");
    let plan_path = directory.join("tondo.test.toml");
    let original_lock: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&lock_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    let original_plan: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&plan_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    let interface_path = directory.join("deps/helper/api.ti");
    let original_interface = fs::read(&interface_path).unwrap();
    let interface = CompiledInterface::decode(&original_interface).unwrap();
    for (kind, expected) in [
        ("target", "target differs"),
        ("api", "API hash differs"),
        ("alias", "conflicts"),
    ] {
        let mut lock = original_lock.clone();
        let mut plan = original_plan.clone();
        if kind == "alias" {
            plan["dev_dependencies"][1]["alias"] = "cli".into();
        } else {
            let mut changed = serde_json::to_value(&interface).unwrap();
            if kind == "target" {
                changed["target"] = "other-target".into();
            } else {
                changed["api_hash"] = sha256(b"unobserved API").into();
            }
            let changed = serde_json::from_value::<CompiledInterface>(changed)
                .unwrap()
                .encode()
                .unwrap();
            fs::write(&interface_path, &changed).unwrap();
            let hash = sha256(&changed);
            lock["test"]["packages"][1]["interface"]["sha256"] = hash.clone().into();
            plan["dev_dependencies"][1]["sha256"] = hash.into();
            let record = &mut lock["test"]["packages"][1];
            let source = &record["sources"][0];
            let source_text = format!(
                "{{\"source_set\":{},\"physical_path\":{},\"logical_path\":{},\"module\":{},\"sha256\":{}}}",
                source["source_set"],
                source["physical_path"],
                source["logical_path"],
                source["module"],
                source["sha256"]
            );
            let fingerprint = format!(
                "{{\"package_id\":{},\"dependencies\":{},\"sources\":[{source_text}],\"interface_hash\":{}}}",
                record["id"], record["dependencies"], record["interface"]["sha256"]
            );
            record["content_hash"] = sha256(fingerprint.as_bytes()).into();
        }
        write_toml_value(&lock_path, lock);
        write_toml_value(&plan_path, plan);
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--list", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        fs::write(&interface_path, &original_interface).unwrap();
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_test_dependencies_reject_unclosed_lock_graphs() {
    let directory = test_project_with_dependencies();
    let lock_path = directory.join("tondo.lock.toml");
    let original: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&lock_path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    for (kind, expected) in [
        ("missing", "missing"),
        ("extra", "duplicate"),
        ("cycle", "cycle"),
        ("production", "production"),
        ("standard", "dependency"),
        ("metadata", "interface_sha256"),
        ("unknown", "unknown field"),
    ] {
        let mut lock = original.clone();
        let records = lock["test"]["packages"].as_array_mut().unwrap();
        match kind {
            "missing" => {
                records.pop();
            }
            "extra" => records.push(records[0].clone()),
            "cycle" => {
                records[0]["dependencies"] =
                    serde_json::json!([{"alias":"helper", "package":"workspace:helper@local"}])
            }
            "production" => {
                records[0]["dependencies"] =
                    serde_json::json!([{"alias":"cli", "package":"workspace:cli@local"}])
            }
            "standard" => {
                records[0]["dependencies"] = serde_json::json!([
                    {"alias":"stdlib", "package":tondo_compiler::project::BOOTSTRAP_STANDARD_PACKAGE}
                ])
            }
            "metadata" => records[0]["interface"]["sha256"] = sha256(b"another interface").into(),
            "unknown" => records[0]["ambient"] = true.into(),
            _ => unreachable!(),
        }
        write_toml_value(&lock_path, lock);
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--list", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    fs::remove_dir_all(directory).unwrap();
}

fn declare_runtime_inputs(directory: &std::path::Path, inputs: serde_json::Value) {
    let path = directory.join("tondo.toml");
    let mut config: serde_json::Value =
        toml::from_str::<toml::Value>(&fs::read_to_string(&path).unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    config["test"] = serde_json::json!({"inputs": inputs});
    write_toml_value(&path, config);
}

fn environment_input(secret: bool) -> serde_json::Value {
    if secret {
        serde_json::json!({"name":"host:credential", "source":"environment:TONDO_TEST_VALUE", "profile":"runtime", "visibility":"secret", "provider":"environment", "descriptor":"TONDO_TEST_FIXTURE_SECRET", "version":"fixture-v1", "capability":"environment"})
    } else {
        serde_json::json!({"name":"host:public", "source":"environment:TONDO_TEST_PUBLIC", "profile":"runtime", "visibility":"public", "sha256":sha256(b"public fixture"), "capability":"environment"})
    }
}

#[test]
fn public_runtime_inputs_reach_workers_and_report_only_declared_identity() {
    let directory = test_project(br#"import std.env
import std.testing
test inputs {
    let snapshot = env.snapshot()?
    match snapshot.arguments() {
        [] => ()
        [_, ..] => assert(false)
    }
    let publicName = env.Name.fromText("TONDO_TEST_PUBLIC")?
    let secretName = env.Name.fromText("TONDO_TEST_VALUE")?
    let ambientName = env.Name.fromText("TONDO_TEST_UNDECLARED")?
    match snapshot.get(publicName)? {
        some(value) => assert(value.asText() == some("public fixture"))
        none => assert(false)
    }
    match snapshot.get(secretName)? {
        some(value) => assert(value.asText() == some("fixture one") or value.asText() == some("fixture two"))
        none => assert(false)
    }
    match snapshot.get(ambientName)? {
        none => ()
        some(_) => assert(false)
    }
}
"#);
    declare_runtime_inputs(
        &directory,
        serde_json::json!([environment_input(false), environment_input(true)]),
    );
    let run = |value: &str| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("test")
            .arg("--project")
            .arg(&directory)
            .args(["--test-format", "json", "--repeat", "2"])
            .env("TONDO_TEST_PUBLIC", "public fixture")
            .env("TONDO_TEST_FIXTURE_SECRET", value)
            .env("TONDO_TEST_UNDECLARED", "must stay outside the program")
            .output()
            .unwrap()
    };
    let first = run("fixture one");
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = run("fixture two");
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let second_report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(first_report["inputs"], second_report["inputs"]);
    assert_eq!(first_report["inputs"]["secret_count"], 1);
    assert_eq!(
        first_report["inputs"]["reproducibility"],
        "secret-dependent-versioned"
    );
    for output in [&first, &second] {
        let text = String::from_utf8_lossy(&output.stdout);
        for excluded in [
            "fixture one",
            "fixture two",
            "TONDO_TEST_FIXTURE_SECRET",
            "host:credential",
            &sha256(b"fixture one")[7..],
            &sha256(b"fixture two")[7..],
        ] {
            assert!(!text.contains(excluded));
        }
    }
    let rejected = run("unaccepted fixture");
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stderr),
        String::from_utf8_lossy(&rejected.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_ne!(report["tests"], first_report["tests"]);
    let mut secret = environment_input(true);
    secret.as_object_mut().unwrap().remove("version");
    declare_runtime_inputs(
        &directory,
        serde_json::json!([environment_input(false), secret]),
    );
    let unversioned = run("fixture one");
    assert_eq!(unversioned.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&unversioned.stdout).unwrap();
    assert_eq!(
        report["inputs"]["reproducibility"],
        "secret-dependent-unversioned"
    );
    assert_ne!(
        report["inputs"]["secret_profile_sha256"],
        first_report["inputs"]["secret_profile_sha256"]
    );
    assert_eq!(
        report["inputs"]["public_sha256"],
        first_report["inputs"]["public_sha256"]
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runtime_input_failure_starts_no_entry_and_preserves_published_outputs() {
    let directory = test_project(b"test mustNotStart { assert(false) }\n");
    let build = |prefix: &str| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args([
                "check",
                "--emit-interface",
                &format!("{prefix}.ti"),
                "--emit-artifact",
                &format!("{prefix}.ta"),
            ])
            .env_remove("TONDO_TEST_FIXTURE_SECRET")
            .output()
            .unwrap()
    };
    assert!(build("before").status.success());
    declare_runtime_inputs(&directory, serde_json::json!([environment_input(true)]));
    let listing = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--list", "--test-format", "json", "--project"])
        .arg(&directory)
        .env_remove("TONDO_TEST_FIXTURE_SECRET")
        .output()
        .unwrap();
    assert_eq!(
        listing.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    for (name, bytes) in [
        ("results.json", b"prior json".as_slice()),
        ("results.xml", b"prior junit"),
    ] {
        fs::write(directory.join(name), bytes).unwrap();
    }
    let snapshots = fs::read(directory.join("tests/snapshots.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--test-format",
            "json",
            "--report",
            "json=results.json",
            "--report",
            "junit=results.xml",
            "--retry",
            "2",
        ])
        .env_remove("TONDO_TEST_FIXTURE_SECRET")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("provider unavailable"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("assert"));
    assert_eq!(
        fs::read(directory.join("results.json")).unwrap(),
        b"prior json"
    );
    assert_eq!(
        fs::read(directory.join("results.xml")).unwrap(),
        b"prior junit"
    );
    assert_eq!(
        fs::read(directory.join("tests/snapshots.json")).unwrap(),
        snapshots
    );
    assert!(build("after").status.success());
    for extension in ["ti", "ta"] {
        assert_eq!(
            fs::read(directory.join(format!("before.{extension}"))).unwrap(),
            fs::read(directory.join(format!("after.{extension}"))).unwrap()
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runtime_input_admission_rejects_drift_collisions_and_invalid_providers() {
    let directory = test_project(b"test mustNotStart { assert(false) }\n");
    for case in [
        "hash",
        "missing",
        "duplicate",
        "environment-alias",
        "provider",
        "capability",
        "name",
        "build",
        "value",
    ] {
        let mut input = environment_input(false);
        let mut declarations = Vec::new();
        match case {
            "hash" => input["sha256"] = sha256(b"other").into(),
            "missing" => {}
            "duplicate" => declarations.push(input.clone()),
            "environment-alias" => {
                let mut other = input.clone();
                other["name"] = "host:other".into();
                other["source"] = "environment:TONDO_TEST_PUBLIC".into();
                declarations.push(other);
            }
            "provider" => {
                input = environment_input(true);
                input["provider"] = "unregistered".into();
            }
            "capability" => input["capability"] = "network".into(),
            "name" => input["source"] = "environment:BAD=NAME".into(),
            "build" => {
                input = environment_input(true);
                input["profile"] = "build".into();
            }
            "value" => {
                input = environment_input(true);
                input["value"] = "forbidden value channel".into();
            }
            _ => unreachable!(),
        }
        declarations.push(input);
        declare_runtime_inputs(&directory, declarations.into());
        let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
        command.current_dir(&directory).args([
            "test",
            "--list",
            "--filter",
            "absent",
            "--allow-empty",
            "--test-format",
            "json",
        ]);
        if case == "missing" {
            command.env_remove("TONDO_TEST_PUBLIC");
        } else {
            command.env("TONDO_TEST_PUBLIC", "public fixture");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{case}");
    }
    declare_runtime_inputs(&directory, serde_json::json!([environment_input(true)]));
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["memory_bytes"] = 512.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .env("TONDO_TEST_FIXTURE_SECRET", "x".repeat(1024))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_reports_bind_captured_inputs_and_effective_resource_limits() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let report_for = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["test", "--project"])
            .arg(&directory)
            .args(["--test-format", "json"])
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        TestReport::parse(&output.stdout).unwrap()
    };
    let before = report_for(&[]);
    assert_ne!(before.metadata().inputs.public_sha256, "0".repeat(64));
    assert_ne!(
        before.metadata().limits.resource_profile_sha256,
        "0".repeat(64)
    );
    assert_eq!(
        before.canonical_bytes().unwrap(),
        report_for(&[]).canonical_bytes().unwrap()
    );
    fs::write(
        directory.join("tests/smoke.to"),
        "test smoke { assert(1 == 1) }\n",
    )
    .unwrap();
    let source_changed = report_for(&[]);
    assert_ne!(
        before.metadata().inputs.public_sha256,
        source_changed.metadata().inputs.public_sha256
    );
    assert_eq!(
        before.metadata().limits.resource_profile_sha256,
        source_changed.metadata().limits.resource_profile_sha256
    );
    let jobs_changed = report_for(&["--jobs", "2"]);
    assert_eq!(
        source_changed.metadata().inputs.public_sha256,
        jobs_changed.metadata().inputs.public_sha256
    );
    assert_ne!(
        source_changed.metadata().limits.resource_profile_sha256,
        jobs_changed.metadata().limits.resource_profile_sha256
    );
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["instructions"] = serde_json::json!(1000);
    });
    let plan_changed = report_for(&[]);
    assert_ne!(
        source_changed.metadata().inputs.public_sha256,
        plan_changed.metadata().inputs.public_sha256
    );
    assert_ne!(
        source_changed.metadata().limits.resource_profile_sha256,
        plan_changed.metadata().limits.resource_profile_sha256
    );
    let package = "workspace:cli@local";
    let store = SnapshotStore::from_entries(
        package,
        [tondo_compiler::test_snapshots::SnapshotEntry {
            node_id: "cli::integration::smoke::smoke".into(),
            name: "unused".into(),
            value: "captured".into(),
        }],
    )
    .unwrap();
    fs::write(
        directory.join("tests/snapshots.json"),
        store.canonical_bytes().unwrap(),
    )
    .unwrap();
    let snapshot_changed = report_for(&[]);
    assert_ne!(
        plan_changed.metadata().inputs.public_sha256,
        snapshot_changed.metadata().inputs.public_sha256
    );
    assert_eq!(
        plan_changed.metadata().limits.resource_profile_sha256,
        snapshot_changed.metadata().limits.resource_profile_sha256
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_source_classes_reject_production_nodes_imports_and_test_exports() {
    for (production, test_path, test_source, code) in [
        (
            "test misplaced {}\n",
            "tests/smoke.to",
            "test smoke {}\n",
            "E2001",
        ),
        (
            "import std.testing\nfn main() {}\n",
            "tests/smoke.to",
            "test smoke {}\n",
            "E2003",
        ),
        (
            "fn main() {}\n",
            "tests/smoke.to",
            "pub fn helper(): Int { 1 }\ntest smoke {}\n",
            "E2003",
        ),
        (
            "fn main() {}\n",
            "src/main_test.to",
            "pub fn helper(): Int { 1 }\ntest smoke {}\n",
            "E2003",
        ),
    ] {
        let directory = project_with_source_and_threads(production.as_bytes());
        fs::create_dir_all(directory.join("tests")).unwrap();
        fs::write(directory.join(test_path), test_source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["test", "--project"])
            .arg(&directory)
            .args(["--list", "--exact", "absent", "--allow-empty"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn test_only_projects_keep_unit_and_integration_sources_distinct() {
    let directory = project_with_source_and_threads(b"fn main() {}\n");
    fs::remove_file(directory.join("src/main.to")).unwrap();
    fs::create_dir_all(directory.join("tests/http")).unwrap();
    fs::write(
        directory.join("src/main_test.to"),
        "fn helper(): Int { 1 }\ntest unitCase { assert(helper() == 1) }\n",
    )
    .unwrap();
    fs::write(
        directory.join("tests/http/client.to"),
        "fn helper(): Int { 2 }\ntest integrationCase { assert(helper() == 2) }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--project"])
        .arg(&directory)
        .args(["--test-format", "json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    let ids = report
        .tests()
        .iter()
        .map(|test| test.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "executordemo::integration::http.client::integrationCase",
            "executordemo::unit::main::unitCase"
        ]
    );
    assert!(
        report
            .tests()
            .iter()
            .all(|test| test.status == AggregateStatus::Passed)
    );
}

#[test]
fn empty_shards_emit_reports_but_empty_selectors_still_fail() {
    let directory = test_project(b"test smoke { assert(true) }\n");
    let mut run_counts = Vec::new();
    let mut list_counts = Vec::new();
    for shard in ["1/2", "2/2"] {
        for list in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
            command.args(["test", "--project"]).arg(&directory).args([
                "--shard",
                shard,
                "--test-format",
                "json",
            ]);
            if list {
                command.arg("--list");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if list {
                list_counts.push(TestList::parse(&output.stdout).unwrap().tests().len());
            } else {
                run_counts.push(TestReport::parse(&output.stdout).unwrap().tests().len());
            }
        }
    }
    run_counts.sort();
    list_counts.sort();
    assert_eq!(run_counts, [0, 1]);
    assert_eq!(list_counts, [0, 1]);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--project"])
        .arg(&directory)
        .args([
            "--shard",
            "1/2",
            "--exact",
            "absent",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no tests matched"));
}

#[test]
fn integration_roots_have_distinct_packages_and_visible_path_ids() {
    let directory = project_with_source_and_threads(
        b"pub fn answer(): Int { 42 }\nfn hidden(): Int { 7 }\nfn main() {}\n",
    );
    fs::create_dir_all(directory.join("tests/http")).unwrap();
    fs::write(directory.join("tests/a.to"),
        "import executordemo.main\nfn helper(): Int { 1 }\ntest smoke { assert(helper() == 1)\nassert(main.answer() == 42) }\n").unwrap();
    fs::write(
        directory.join("tests/http/b.to"),
        "fn helper(): Int { 2 }\ntest smoke { assert(helper() == 2) }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PASS executordemo::integration::a::smoke"),
        "{stdout}"
    );
    assert!(
        stdout.contains("PASS executordemo::integration::http.b::smoke"),
        "{stdout}"
    );
    fs::write(
        directory.join("tests/a.to"),
        "import executordemo.main\ntest smoke { assert(main.hidden() == 7) }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("hidden"));
}

#[test]
fn integration_package_aliases_do_not_collide_with_a_user_package_name() {
    let directory = project_with_source_and_threads(b"pub fn answer(): Int { 42 }\nfn main() {}\n");
    let config_path = directory.join("tondo.toml");
    let mut config: toml::Value =
        toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["package"]["name"] = toml::Value::String("tondoIntegration0".into());
    fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
    fs::create_dir_all(directory.join("tests")).unwrap();
    fs::write(
        directory.join("tests/consumer.to"),
        "import tondoIntegration0.main\ntest smoke { assert(main.answer() == 42) }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("PASS tondoIntegration0::integration::consumer::smoke")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_companion_cannot_repair_invalid_production() {
    let directory =
        project_with_source_and_threads(b"fn compute(): Int { testHelper() }\nfn main() {}\n");
    fs::write(
        directory.join("src/main_test.to"),
        "fn testHelper(): Int { 42 }\ntest smoke { assert(compute() == 42) }\n",
    )
    .unwrap();
    for command in [
        vec!["check"],
        vec!["test"],
        vec!["test", "--list"],
        vec!["test", "--filter", "absent", "--allow-empty"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(&command)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("E1001"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_command_validates_the_full_target_before_selection() {
    for (source, options) in [
        ("test smoke { nonexistentFunction() }\n", vec!["--list"]),
        (
            "test good { assert(true) }\ntest bad { nonexistentFunction() }\n",
            vec!["--filter", "good"],
        ),
        ("test broken { let = }\n", vec!["--allow-empty"]),
        (
            "test good { assert(true) }\ntest bad { nonexistentFunction() }\n",
            vec!["--filter", "absent", "--allow-empty"],
        ),
    ] {
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .arg("test")
            .args(&options)
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{options:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.is_empty(),
            "invalid compilation must not list or execute tests"
        );
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn test_listing_compiles_without_entering_suites_or_consuming_runtime_limits() {
    let directory = test_project(b"suite outer { assert(false)\n test body { assert(false) }\n}\n");
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["instructions"] = serde_json::json!(1);
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list", "--test-format", "json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let list = TestList::parse(&output.stdout).unwrap();
    assert_eq!(list.tests().len(), 1);
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
        String::from_utf8_lossy(&output.stdout).contains("PASS cli::integration::smoke::smoke")
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

#[test]
fn test_projects_with_the_same_clock_sample_keep_independent_inputs() {
    let first_source = b"test first { assert(true) }\n";
    let second_source = b"test second { assert(false) }\n";
    let first = test_project_with_nonce(first_source, &[], u128::MAX);
    let second = test_project_with_nonce(second_source, &[], u128::MAX);
    let first_bytes = fs::read(first.join("tests/smoke.to")).unwrap();
    let second_bytes = fs::read(second.join("tests/smoke.to")).unwrap();
    for directory in std::collections::BTreeSet::from([&first, &second]) {
        fs::remove_dir_all(directory).unwrap();
    }
    assert_ne!(
        first, second,
        "clock resolution cannot identify a test project"
    );
    assert_eq!(first_bytes, first_source);
    assert_eq!(second_bytes, second_source);
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
fn test_plan_reconciles_every_discovered_source_identity_before_selection() {
    for field in [
        "physical_path",
        "logical_path",
        "module",
        "input",
        "package",
        "missing",
        "additional",
    ] {
        let directory = test_project(b"test smoke { assert(true) }\n");
        rewrite_test_plan(&directory, |plan| {
            let sources = plan["sources"].as_array_mut().unwrap();
            let index = sources
                .iter()
                .position(|source| source["class"] == "integration-test")
                .unwrap();
            match field {
                "physical_path" | "logical_path" => {
                    sources[index][field] = serde_json::json!("tests/other.to")
                }
                "module" | "input" => sources[index][field] = serde_json::json!("renamed"),
                "package" => sources[index][field] = serde_json::json!("workspace:other@local"),
                "missing" => {
                    sources.remove(index);
                }
                "additional" => {
                    let mut additional = sources[index].clone();
                    additional["physical_path"] = serde_json::json!("tests/other.to");
                    additional["logical_path"] = serde_json::json!("tests/other.to");
                    additional["module"] = serde_json::json!("other");
                    additional["input"] =
                        serde_json::json!("source:integration-test:tests/other.to");
                    sources.push(additional);
                }
                _ => unreachable!(),
            }
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--filter", "absent", "--allow-empty", "--list"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{field}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.is_empty(),
            "{field}: a drifted plan published a list"
        );
        fs::remove_dir_all(directory).unwrap();
    }
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
fn compiler_private_testing_calls_reject_copied_user_code_before_selection() {
    for call in [
        "runner.__runLeaf(\"injected\", () { assert(false) })",
        "runner.__runSuite(\"injected\", () { assert(false) })",
        "runner.__beginSuiteCleanup()",
    ] {
        for fragment in [
            format!("test blocked {{ {call} }}"),
            format!("suite blocked {{\n {call}\n test nested {{ assert(true) }}\n}}"),
            format!("fn unused() {{ {call} }}\ntest blocked {{ assert(true) }}"),
            format!("test blocked {{\n defer {{\n {call}\n }}\n}}"),
        ] {
            let source = format!(
                "import std.testing as runner\n{fragment}\ntest sibling {{ assert(true) }}\n"
            );
            let directory = test_project_with_capabilities(source.as_bytes(), &[]);
            for arguments in [vec!["test", "--list"], vec!["test", "--filter", "sibling"]] {
                let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                    .current_dir(&directory)
                    .args(arguments)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(1), "{source}");
                assert!(output.stdout.is_empty(), "{source}");
                let diagnostic = String::from_utf8_lossy(&output.stderr);
                assert!(diagnostic.contains("E1102"), "{source}\n{diagnostic}");
                assert!(
                    diagnostic.contains("compiler-private"),
                    "{source}\n{diagnostic}"
                );
                assert!(
                    diagnostic.contains("tests/smoke.to:"),
                    "{source}\n{diagnostic}"
                );
            }
            assert!(!directory.join("target/.tondo-test-root").exists());
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn temporary_testing_capability_is_checked_before_worker_dispatch() {
    for constructor in [
        "let temporary = testing.assertOk(testing.tempDirectory(\"capability\"))",
        "let make = testing.tempDirectory\n let temporary = testing.assertOk(make(\"capability\"))",
    ] {
        let source = format!(
            "import std.testing\ntest temporary {{\n {constructor}\n defer temporary.cleanup()\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("E1008"), "{diagnostic}");
        assert!(diagnostic.contains("filesystem"), "{diagnostic}");
        assert!(!directory.join("target/.tondo-test-root").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn temporary_testing_roots_are_fresh_for_repeated_workers_and_removed_before_publication() {
    let source = r#"import std.testing
import std.fs
import std.path
import std.bytes
test first {
    let temporary = testing.tempDirectory("first")?
    defer temporary.cleanup()
    let file = temporary.path().join("payload")?
    fs.writeAll(file, bytes.Bytes("first")?)?
    assert(String(fs.readAll(file)?)? == "first")
    var record = fs.open(path.Path.fromString("first-paths")?, fs.OpenMode.Append)?
    _ = record.write(temporary.path().toBytes())?
    _ = record.write(bytes.Bytes("\n")?)?
}
test second {
    let make = testing.tempDirectory
    let temporary = make("second")?
    defer temporary.cleanup()
    var record = fs.open(path.Path.fromString("second-paths")?, fs.OpenMode.Append)?
    _ = record.write(temporary.path().toBytes())?
    _ = record.write(bytes.Bytes("\n")?)?
}
"#;
    let directory = test_project_with_capabilities(source.as_bytes(), &["filesystem"]);
    fs::write(directory.join("first-paths"), b"").unwrap();
    fs::write(directory.join("second-paths"), b"").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--repeat",
            "2",
            "--jobs",
            "2",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert!(
        report
            .tests()
            .iter()
            .all(|test| test.status == AggregateStatus::Passed && test.attempts.len() == 2)
    );
    let mut roots = std::collections::BTreeSet::new();
    // Project discovery resolves aliases such as macOS /var -> /private/var.
    let expected_root = directory
        .canonicalize()
        .unwrap()
        .join("target/.tondo-test-root");
    for marker in ["first-paths", "second-paths"] {
        let paths = fs::read_to_string(directory.join(marker)).unwrap();
        assert_eq!(paths.lines().count(), 2);
        for path in paths.lines() {
            let path = std::path::Path::new(path);
            assert!(
                path.starts_with(&expected_root),
                "{path:?} is outside {expected_root:?}"
            );
            assert!(!path.exists());
            assert!(roots.insert(path.parent().unwrap().to_owned()));
            assert!(!String::from_utf8_lossy(&output.stdout).contains(path.to_str().unwrap()));
        }
    }
    assert_eq!(roots.len(), 4);
    assert_eq!(
        fs::read_dir(directory.join("target/.tondo-test-root"))
            .unwrap()
            .count(),
        0
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn temporary_testing_cleanup_failure_preserves_outputs_and_never_retries() {
    let source = r#"import std.testing
import std.path
import std.fs
import std.bytes
import std.time
test temporary {
    let temporary = testing.tempDirectory("cleanup")?
    defer temporary.cleanup()
    var attempts = fs.open(path.Path.fromString("started")?, fs.OpenMode.Append)?
    _ = attempts.write(bytes.Bytes("x")?)?
    fs.writeAll(path.Path.fromString("ready")?, temporary.path().toBytes())?
    let release = path.Path.fromString("release")?
    for {
        if String(fs.readAll(release)?)? == "ready" {
            break
        }
        time.sleep(time.Duration.fromNanoseconds(1000000))?
    }
}

"#;
    let directory = test_project_with_capabilities(source.as_bytes(), &["filesystem", "clock"]);
    fs::write(directory.join("release"), b"").unwrap();
    fs::write(directory.join("started"), b"").unwrap();
    fs::write(directory.join("results.json"), b"prior json").unwrap();
    fs::write(directory.join("results.xml"), b"prior junit").unwrap();
    let snapshots = fs::read(directory.join("tests/snapshots.json")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--retry",
            "1",
            "--test-format",
            "json",
            "--report",
            "json=results.json",
            "--report",
            "junit=results.xml",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut temporary = None;
    while std::time::Instant::now() < deadline {
        if let Ok(path) = fs::read_to_string(directory.join("ready"))
            && !path.is_empty()
        {
            temporary = Some(std::path::PathBuf::from(path));
            break;
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if let Some(path) = &temporary {
        std::os::unix::fs::symlink(&directory, path.join("link")).unwrap();
        fs::write(directory.join("release"), b"ready").unwrap();
    } else {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        temporary.is_some(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("temporary cleanup failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("symlink"), "{diagnostic}");
    assert_eq!(fs::read(directory.join("started")).unwrap(), b"x");
    assert_eq!(
        fs::read(directory.join("results.json")).unwrap(),
        b"prior json"
    );
    assert_eq!(
        fs::read(directory.join("results.xml")).unwrap(),
        b"prior junit"
    );
    assert_eq!(
        fs::read(directory.join("tests/snapshots.json")).unwrap(),
        snapshots
    );
    assert_eq!(
        fs::read_dir(directory.join("target/.tondo-test-root"))
            .unwrap()
            .count(),
        1
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn text_transform_memory_limits_preserve_siblings_and_unicode_semantics() {
    let source = format!(
        r#"test expansion {{
    let input = "{}"
    let replacement = "{}"
    let expanded = input.replace("a", replacement)
    assert(expanded.length() == 65536)
}}
test ordinary {{
    assert("a€a".replace("a", "🦀") == "🦀€🦀")
    assert("é".replace("", "x") == "xéx")
    assert("aaaa".replace("aa", "b") == "bb")
    assert(" \t€\n".trim() == "€")
    assert("AbÉ".toLowerAscii() == "abÉ")
    assert("aBé".toUpperAscii() == "ABé")
}}
"#,
        "a".repeat(1024),
        "€".repeat(64),
    );
    let directory = test_project_with_capabilities(source.as_bytes(), &[]);
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["memory_bytes"] = 32768.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        report
            .tests()
            .iter()
            .map(|test| test.status)
            .collect::<Vec<_>>(),
        [AggregateStatus::ResourceLimit, AggregateStatus::Passed]
    );
    assert!(report.tests().iter().all(|test| test.attempts.len() == 1));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn temporary_testing_file_and_atomic_writes_reject_quota_growth_without_partial_output() {
    let source = r#"import std.testing
import std.fs
import std.path
import std.bytes
test bounded {
    let temporary = testing.tempDirectory("bounded")?
    defer temporary.cleanup()
    let file = temporary.path().join("payload")?
    fs.rename(path.Path.fromString("fixture")?, file)?
    var append = fs.open(file, fs.OpenMode.Append)?
    match append.write(bytes.Bytes("x")?) {
        ok(_) => assert(false)
        err(_) => testing.log("file-rejected")
    }
    match fs.atomicWrite(file, bytes.Bytes("y")?) {
        ok(_) => assert(false)
        err(_) => testing.log("atomic-rejected")
    }
    var reader = fs.open(file, fs.OpenMode.Read)?
    let first = testing.assertSome(reader.read(1)?)
    assert(first.length() == 1)
    assert(first.get(0) == some(Byte(0u8)))
}
test sibling { assert(true) }
"#;
    let directory = test_project_with_capabilities(source.as_bytes(), &["filesystem"]);
    // A sparse fixture reaches the disk quota without allocating a 64 MiB
    // language value or depending on an external truncate command.
    fs::File::create(directory.join("fixture"))
        .unwrap()
        .set_len(tondo_compiler::test_temporaries::MAX_TEMP_BYTES)
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.tests().len(), 2);
    assert!(
        report
            .tests()
            .iter()
            .all(|test| test.status == AggregateStatus::Passed)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("file-rejected"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("atomic-rejected"));
    assert!(!directory.join("fixture").exists());
    assert_eq!(
        fs::read_dir(directory.join("target/.tondo-test-root"))
            .unwrap()
            .count(),
        0
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn testing_host_terminals_stop_the_leaf_and_preserve_sibling_evidence() {
    for operation in [
        "let a = 1\n let b = 2\n testing.assertEqual(ref a, ref b)",
        "let a = 1\n testing.assertNotEqual(ref a, ref a)",
        "testing.assertTextEqual(\"expected\", \"actual\")",
        "let absent: Int? = none\n _ = testing.assertSome(absent)",
        "testing.assertNone(some(42))",
        "let failure: Int ! String = err(\"bad\")\n _ = testing.assertOk(failure)",
        "let success: Int ! String = ok(42)\n _ = testing.assertErr(success)",
    ] {
        let source = format!(
            "import std.testing\nsuite shared {{\n test first {{ testing.log(\"first\") }}\n test middle {{\n defer testing.log(\"cleanup\")\n {operation}\n testing.log(\"unreachable\")\n }}\n test last {{ testing.log(\"last\") }}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{operation}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(output.status.code(), Some(1));
        for (name, status, logs) in [
            ("first", AggregateStatus::Passed, vec!["first"]),
            ("middle", AggregateStatus::FailedPanic, vec!["cleanup"]),
            ("last", AggregateStatus::Passed, vec!["last"]),
        ] {
            let leaf = report
                .tests()
                .iter()
                .find(|leaf| leaf.id.ends_with(&format!("::{name}")))
                .unwrap();
            assert_eq!(leaf.status, status, "{operation}: {name}");
            assert_eq!(leaf.attempts[0].logs, logs, "{operation}: {name}");
        }
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    }
}

#[test]
fn testing_assertion_unicode_truncation_preserves_the_worker_report() {
    let text = "€🦀é".repeat(400);
    for operation in [
        "let other = \"different\"\n testing.assertEqual(ref value, ref other)",
        "testing.assertNotEqual(ref value, ref value)",
        "testing.assertNone(some(value))",
        "let result: Int ! String = err(value)\n _ = testing.assertOk(result)",
        "let result: String ! String = ok(value)\n _ = testing.assertErr(result)",
    ] {
        let source = format!(
            "import std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n defer testing.log(\"cleanup\")\n let value = \"{text}\"\n {operation}\n testing.log(\"unreachable\")\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [
                AggregateStatus::Passed,
                AggregateStatus::FailedPanic,
                AggregateStatus::Passed
            ]
        );
        assert_eq!(report.tests()[1].attempts[0].logs, ["cleanup"]);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let failure = &wire["tests"][1]["attempts"][0]["failure"];
        assert_eq!(failure["code"], "P0007");
        let message = failure["message"].as_str().unwrap();
        assert!(message.contains("...<truncated>"), "{message}");
        assert!(message.len() < 2200, "{message}");
    }
}

#[test]
fn testing_assertion_display_failures_preserve_runner_control_and_cleanup() {
    for (body, expected, code) in [
        (
            "panic(\"display broke\")".to_owned(),
            AggregateStatus::FailedPanic,
            "P0008",
        ),
        (
            "testing.skip(\"display skipped\")".to_owned(),
            AggregateStatus::Skipped,
            "",
        ),
        (
            format!("\"{}\"", "x".repeat(20000)),
            AggregateStatus::ResourceLimit,
            "memory",
        ),
        (
            "for {}".to_owned(),
            AggregateStatus::ResourceLimit,
            "instructions",
        ),
    ] {
        let source = format!(
            "import std.testing\ntype Label = {{ value: Int }}\nimpl Display for Label {{\n fn display(self): String {{\n defer testing.log(\"display-cleanup\")\n {body}\n }}\n}}\nsuite outer {{\n test first {{}}\n test middle {{\n defer testing.log(\"test-cleanup\")\n let value = Label {{ value: 1 }}\n testing.assertNotEqual(ref value, ref value)\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &[]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 8192.into();
            plan["limits"]["instructions"] = 2000.into();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{code}, project={}: {error}: {}",
                directory.display(),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{code}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let middle = &report.tests()[1];
        assert_eq!(
            middle.attempts.len(),
            if expected == AggregateStatus::FailedPanic {
                2
            } else {
                1
            }
        );
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for (index, attempt) in middle.attempts.iter().enumerate() {
            if expected != AggregateStatus::ResourceLimit {
                assert_eq!(attempt.logs, ["display-cleanup", "test-cleanup"]);
            }
            if !code.is_empty() {
                assert_eq!(wire["tests"][1]["attempts"][index]["failure"]["code"], code);
            }
        }
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Skipped))
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn testing_assertion_display_timeout_preserves_cleanup_siblings_and_retry() {
    let source = b"import std.testing\ntype Label = { value: Int }\nimpl Display for Label {\n fn display(self): String {\n defer testing.log(\"display-cleanup\")\n for {}\n }\n}\nsuite outer {\n test first {}\n test middle {\n defer testing.log(\"test-cleanup\")\n let value = Label { value: 1 }\n testing.assertNotEqual(ref value, ref value)\n }\n test zlast {}\n}\n";
    let directory = test_project_with_capabilities(source, &[]);
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["instructions"] = 1_000_000_000_000_u64.into();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--timeout",
            "50ms",
            "--retry",
            "1",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "project={}: {error}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        report
            .tests()
            .iter()
            .map(|test| test.status)
            .collect::<Vec<_>>(),
        [
            AggregateStatus::Passed,
            AggregateStatus::Timeout,
            AggregateStatus::Passed
        ]
    );
    let middle = &report.tests()[1];
    assert_eq!(middle.attempts.len(), 2);
    for attempt in &middle.attempts {
        assert_eq!(
            attempt.status,
            tondo_compiler::test_result::AttemptStatus::Timeout
        );
        assert_eq!(attempt.logs, ["display-cleanup", "test-cleanup"]);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn testing_assertions_preserve_ieee_equality_for_borrowed_values() {
    let directory = test_project_with_capabilities(
        br#"import std.testing
test nan {
    let nan = 0.0 / 0.0
    let values = [nan]
    assert(values != values)
    testing.assertNotEqual(ref nan, ref nan)
    testing.assertNotEqual(ref values, ref values)
    let nested = [values]
    testing.assertNotEqual(ref nested, ref nested)
    let zeros = [-0.0]
    let positiveZeros = [0.0]
    testing.assertEqual(ref zeros, ref positiveZeros)
}
"#,
        &[],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(report.summary().passed, 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn testing_assertions_dispatch_display_only_on_failure_in_source_order() {
    let source = r#"import std.testing
import std.console
type Label = { text: String }
impl Display for Label {
    fn display(self): String {
        _ = console.println(self.text)
        "shown:{self.text}"
    }
}
fn compare[T: Equatable + Display](left: ref T, right: ref T) {
    testing.assertEqual(ref left, ref right)
}
test aPass {
    let left = Label { text: "quiet" }
    let right = Label { text: "different" }
    testing.assertEqual(ref left, ref left)
    testing.assertNotEqual(ref left, ref right)
    let success: Label ! String = ok(left)
    assert(testing.assertOk(success).text == "quiet")
    let failure: String ! Label = err(right)
    assert(testing.assertErr(failure).text == "different")
}
test bEqual {
    defer testing.log("cleanup")
    let left = Label { text: "left" }
    let right = Label { text: "right" }
    compare(ref left, ref right)
}
test cNotEqual {
    defer testing.log("cleanup")
    let value = Label { text: "equal" }
    let check = testing.assertNotEqual[Label]
    check(ref value, ref value)
}
test dOk {
    defer testing.log("cleanup")
    let value: Int ! Label = err(Label { text: "error" })
    let check = testing.assertOk[Int, Label]
    _ = check(value)
}
test eErr {
    defer testing.log("cleanup")
    let value: Label ! String = ok(Label { text: "value" })
    _ = testing.assertErr(value)
}
"#;
    let directory = test_project_with_capabilities(source.as_bytes(), &["console"]);
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
    assert!(report.tests()[0].attempts[0].stdout.is_empty());
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (index, stdout, message) in [
        (
            1,
            "left\nright\n",
            "assertion failed: expected shown:left, actual shown:right",
        ),
        (
            2,
            "equal\n",
            "assertion failed: values are equal (shown:equal)",
        ),
        (
            3,
            "error\n",
            "assertion failed: expected Ok, got Err(shown:error)",
        ),
        (
            4,
            "value\n",
            "assertion failed: expected Err, got Ok(shown:value)",
        ),
    ] {
        let test = &report.tests()[index];
        assert_eq!(test.status, AggregateStatus::FailedPanic);
        assert_eq!(test.attempts[0].stdout.as_bytes(), stdout.as_bytes());
        assert_eq!(test.attempts[0].logs, ["cleanup"]);
        assert_eq!(
            wire["tests"][index]["attempts"][0]["failure"]["code"],
            "P0007"
        );
        assert_eq!(
            wire["tests"][index]["attempts"][0]["failure"]["message"],
            format!("P0007: {message}")
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn writer_bytes_are_captured_per_attempt_and_projected_without_loss() {
    let source = r#"import std.console
import std.bytes
import std.io
import std.encoding
fn emit(data: Array[Byte]): !(console.ConsoleError | bytes.BytesError | io.IoError) {
    var output = console.stdout()?
    io.writeAll(var output, bytes.fromArray(data)?)?
}
suite outer {
    emit([Byte(226u8)])?
    defer {
        match emit([Byte(130u8), Byte(172u8)]) {
            ok(_) => ()
            err(_) => panic("suite output failed")
        }
    }
    test aText {
        console.print("prefix:")?
        var output = console.stdout()?
        _ = output.write(bytes.fromArray([Byte(226u8)])?)?
        io.writeAll(var output, bytes.fromArray([Byte(130u8), Byte(172u8)])?)?
        var errors = console.stderr()?
        _ = errors.write(bytes.Bytes("writer-error")?)?
    }
    test bBinary {
        emit([Byte(255u8), Byte(0u8)])?
        var errors = console.stderr()?
        io.writeAll(var errors, bytes.fromArray([Byte(226u8)])?)?
    }
    test cXml {
        emit([Byte(13u8), Byte(0u8)])?
    }
    test dEmpty {}
    test eCodec {
        let limits = encoding.EncodingLimits.defaults()
        var output = console.stdout()?
        encoding.Base64Options.standard(limits).encodeTo(bytes.Bytes("fo")?, var output)?
        var errors = console.stderr()?
        encoding.HexOptions.lower(limits).encodeTo(bytes.Bytes("fo")?, var errors)?
    }
}
"#;
    let directory = test_project_with_capabilities(source.as_bytes(), &["console"]);
    let mut previous = None;
    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args([
                "test",
                "--test-format",
                "json",
                "--report",
                "json=results.json",
                "--report",
                "junit=results.xml",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let report = TestReport::parse(&output.stdout).unwrap();
        assert_eq!(
            fs::read(directory.join("results.json")).unwrap(),
            output.stdout
        );
        if let Some(previous) = &previous {
            assert_eq!(&output.stdout, previous);
        }
        previous = Some(output.stdout.clone());
        assert_eq!(
            report.suites()[0].attempts[0].stdout.as_bytes(),
            "€".as_bytes()
        );
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for (index, stdout, stderr, encoding, data) in [
            (
                0,
                "prefix:€".as_bytes(),
                b"writer-error".as_slice(),
                "utf8",
                "prefix:€",
            ),
            (1, [0xff, 0].as_slice(), [0xe2].as_slice(), "base64", "/wA="),
            (2, [13, 0].as_slice(), b"".as_slice(), "utf8", "\r\0"),
            (3, b"".as_slice(), b"".as_slice(), "utf8", ""),
            (4, b"Zm8=".as_slice(), b"666f".as_slice(), "utf8", "Zm8="),
        ] {
            let attempt = &report.tests()[index].attempts[0];
            assert_eq!(attempt.stdout.as_bytes(), stdout);
            assert_eq!(attempt.stderr.as_bytes(), stderr);
            assert_eq!(
                wire["tests"][index]["attempts"][0]["stdout"],
                serde_json::json!({"encoding":encoding,"data":data})
            );
        }
        let xml = fs::read_to_string(directory.join("results.xml")).unwrap();
        assert!(xml.contains("<system-out>/wA=</system-out>"));
        assert!(xml.contains("<system-err>4g==</system-err>"));
        assert!(xml.contains("<system-out>DQA=</system-out>"));
        assert!(xml.contains("tondo.stdout.encoding\" value=\"base64\""));
    }
    let human = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--show-output"])
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(0));
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("[base64] /wA=")
    );
    assert!(
        String::from_utf8(human.stderr)
            .unwrap()
            .contains("[base64] 4g==")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn human_reports_show_failures_skips_suites_and_separate_streams() {
    let source = br#"import std.testing
import std.console
import std.bytes
import std.io
suite blocked {
    testing.tags(["causal": "setup"])
    testing.log("causal-setup-log")
    testing.skip("causal-setup-reason")
    test hidden { testing.failNow("must-not-run") }
}
suite outer {
    testing.log("passing-suite-log")
    test aFail {
        testing.tags(["kind": "failed"])
        testing.attach("trace", "text/plain", bytes.Bytes("artifact-private-body")?)
        testing.log("failure-log")
        console.print("failure-output")?
        var errors = console.stderr()?
        io.writeAll(var errors, bytes.fromArray([Byte(255u8)])?)?
        testing.failNow("failure-reason")
    }
    test bSkip {
        testing.tags(["kind": "skipped"])
        testing.log("skip-log")
        testing.skip("skip-reason")
    }
    test cPass {
        testing.tags(["hidden-tag": "value"])
        testing.log("passing-leaf-log")
        console.print("passing-output")?
    }
}
"#;
    let directory = test_project(source);
    fs::create_dir_all(directory.join(".github")).unwrap();
    fs::write(directory.join(".github/CODEOWNERS"), b"* @tondo\n").unwrap();
    for show_output in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
        command
            .current_dir(&directory)
            .args(["test", "--report", "json=human.json"]);
        command.args([
            "--artifacts",
            if show_output {
                "artifacts-verbose"
            } else {
                "artifacts-default"
            },
        ]);
        if show_output {
            command.arg("--show-output");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let report = TestReport::parse(&fs::read(directory.join("human.json")).unwrap()).unwrap();
        for node in report.suites().iter().chain(report.tests()) {
            assert!(stdout.contains(&node.id), "{stdout}");
        }
        for text in [
            "failure-reason",
            "failure-log",
            "failure-output",
            "skip reason: \"skip-reason\"",
            "skip-log",
            "owners: [\"@tondo\"]",
            "tags: {\"kind\":\"failed\"}",
            "artifact: {\"name\":\"trace\"",
        ] {
            assert!(stdout.contains(text), "missing {text}: {stdout}");
        }
        assert_eq!(stdout.matches("causal-setup-log").count(), 1);
        assert_eq!(stdout.matches("causal-setup-reason").count(), 1);
        assert!(
            stdout.contains("blocked by cli::integration::smoke::blocked attempt 1"),
            "{stdout}"
        );
        assert!(!stdout.contains("must-not-run"));
        assert!(!stdout.contains("artifact-private-body"));
        assert!(!stdout.contains("[base64] /w=="));
        assert!(
            stderr.contains("stderr cli::integration::smoke::outer::aFail attempt 1:"),
            "{stderr}"
        );
        assert!(stderr.contains("[base64] /w=="));
        assert!(!stderr.contains("failure-output"));
        for hidden in [
            "passing-suite-log",
            "passing-leaf-log",
            "passing-output",
            "hidden-tag",
        ] {
            assert_eq!(stdout.contains(hidden), show_output, "{stdout}");
        }
    }
    let listed = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list", "--order", "random"])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let listing = String::from_utf8(listed.stdout).unwrap();
    let seed = listing
        .lines()
        .next()
        .unwrap()
        .strip_prefix("order: random, seed: ")
        .unwrap();
    assert!(listing.contains("owners: [\"@tondo\"]"));
    assert!(!listing.contains("failure-output"));
    let replay = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--list", "--order", "random", "--seed", seed])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(replay.stdout, listing.as_bytes());
    let listed_json = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--list",
            "--order",
            "random",
            "--seed",
            seed,
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        listed_json.status.success(),
        "{}",
        String::from_utf8_lossy(&listed_json.stderr)
    );
    let list = TestList::parse(&listed_json.stdout).unwrap();
    assert_eq!(list.suites().len(), 2);
    assert!(list.suites().iter().all(|suite| suite.owners == ["@tondo"]));
    assert!(list.tests().iter().all(|test| test.parent.is_some()));
    let run_json = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--order",
            "random",
            "--seed",
            seed,
            "--test-format",
            "json",
            "--artifacts",
            "artifacts-random",
        ])
        .output()
        .unwrap();
    assert_eq!(
        run_json.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run_json.stderr)
    );
    let run = TestReport::parse(&run_json.stdout).unwrap();
    assert_eq!(list.execution_plan(), run.execution_plan());
    assert_eq!(
        list.suites()
            .iter()
            .map(|suite| (&suite.id, &suite.parent))
            .collect::<Vec<_>>(),
        run.suites()
            .iter()
            .map(|suite| (&suite.id, &suite.parent))
            .collect::<Vec<_>>()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn human_reports_keep_every_failed_attempt_without_show_output() {
    let directory = test_project(b"import std.testing\nimport std.console\ntest failure {\n testing.log(\"attempt-log\")\n console.print(\"attempt-output\")?\n testing.failNow(\"attempt-failure\")\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("attempt 1 (iteration 1, round 0)"));
    assert!(stdout.contains("attempt 2 (iteration 1, round 1)"));
    assert_eq!(stdout.matches("log 1: \"attempt-log\"").count(), 2);
    assert_eq!(stdout.matches("\nattempt-output\n").count(), 2);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn writer_output_quotas_count_raw_bytes_and_cannot_be_recovered_or_retried() {
    for (limit, operation, expected, retained) in [
        (
            4,
            "_ = writer.write(bytes.fromArray([Byte(255u8), Byte(0u8), Byte(1u8), Byte(2u8)])?)",
            AggregateStatus::Passed,
            vec![255, 0, 1, 2],
        ),
        (
            3,
            "_ = writer.write(bytes.fromArray([Byte(255u8), Byte(0u8), Byte(1u8), Byte(2u8)])?)",
            AggregateStatus::ResourceLimit,
            vec![],
        ),
        (
            3,
            "_ = io.writeAll(var writer, bytes.fromArray([Byte(255u8), Byte(0u8), Byte(1u8), Byte(2u8)])?)",
            AggregateStatus::ResourceLimit,
            vec![],
        ),
        (
            3,
            "_ = encoding.Base64Options.standard(encoding.EncodingLimits.defaults()).encodeTo(bytes.Bytes(\"fo\")?, var writer)",
            AggregateStatus::ResourceLimit,
            vec![],
        ),
        (
            3,
            "_ = encoding.HexOptions.lower(encoding.EncodingLimits.defaults()).encodeTo(bytes.Bytes(\"fo\")?, var writer)",
            AggregateStatus::ResourceLimit,
            vec![],
        ),
        (
            3,
            "console.print(\"ok\")?\n _ = writer.write(bytes.Bytes(\"no\")?)",
            AggregateStatus::ResourceLimit,
            b"ok".to_vec(),
        ),
    ] {
        let source = format!(
            "import std.console\nimport std.bytes\nimport std.io\nimport std.encoding\nsuite outer {{\n test aFirst {{}}\n test bWrite {{\n var writer = console.stdout()?\n {operation}\n }}\n test cLast {{}}\n}}\n"
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &["console"]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["output_bytes"] = limit.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{operation}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed]
        );
        let attempt = &report.tests()[1].attempts[0];
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(attempt.stdout.as_bytes(), retained);
        if expected == AggregateStatus::ResourceLimit {
            assert_eq!(
                attempt.failure.as_ref().unwrap().code.as_deref(),
                Some("output")
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn binary_capture_is_retained_separately_for_retries_and_repeats() {
    let source = b"import std.console\nimport std.bytes\nimport std.testing\ntest output {\n var writer = console.stdout()?\n _ = writer.write(bytes.fromArray([Byte(255u8)])?)?\n testing.failNow(\"retry\")\n}\n";
    let directory = test_project_with_capabilities(source, &["console"]);
    for campaign in [["--retry", "1"], ["--repeat", "2"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .args(campaign)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        let attempts = &report.tests()[0].attempts;
        assert_eq!(attempts.len(), 2);
        for attempt in attempts {
            assert_eq!(attempt.stdout.as_bytes(), [255]);
            assert!(attempt.stderr.is_empty());
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn testing_host_evidence_limits_are_atomic_and_are_not_retried() {
    for (budget, operation) in [
        ("output_bytes", "testing.log(\"123456789\")"),
        ("output_bytes", "testing.failNow(\"123456789\")"),
        ("output_bytes", "testing.skip(\"123456789\")"),
        ("output_bytes", "testing.assertTextEqual(\"a\", \"b\")"),
        (
            "output_bytes",
            "defer { testing.failNow(\"cleanup failure\")\n }\n testing.log(\"123456789\")",
        ),
        (
            "snapshot_bytes",
            "testing.snapshot(\"large\", \"123456789\")",
        ),
        (
            "artifact_bytes",
            "match bytes.Bytes(\"123456789\") {\n ok(payload) => testing.attach(\"large\", \"text/plain\", payload)\n err(_) => testing.failNow(\"could not create bytes\")\n }",
        ),
    ] {
        let source = format!(
            "import std.testing\nimport std.bytes\nsuite shared {{\n test first {{ testing.log(\"first\") }}\n test middle {{ {operation}\n testing.log(\"wrong\") }}\n test last {{ testing.log(\"last\") }}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| plan["limits"][budget] = 8.into());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(output.status.code(), Some(1));
        for leaf in report.tests() {
            assert_eq!(leaf.attempts.len(), 1, "{}", leaf.id);
            if leaf.id.ends_with("::middle") {
                assert_eq!(leaf.status, AggregateStatus::ResourceLimit);
                assert!(leaf.attempts[0].logs.is_empty());
                assert!(leaf.attempts[0].artifacts.is_empty());
                assert!(leaf.attempts[0].snapshots.is_empty());
            } else {
                assert_eq!(leaf.status, AggregateStatus::Passed);
                assert_eq!(leaf.attempts[0].logs.len(), 1);
            }
        }
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    }
}

#[test]
fn testing_console_output_belongs_to_its_node_and_uses_the_output_budget() {
    let directory = test_project(b"import std.testing\nimport std.console\nfn emitCleanup(text: String) {\n _ = console.print(text)\n}\nsuite shared {\n console.print(\"setup\")?\n defer emitCleanup(\"end\")\n test first { console.println(\"first\")? }\n test middle { console.print(\"1234567\")?\n console.println(\"8\")?\n testing.log(\"wrong\") }\n test last { console.print(\"last\")? }\n}\n");
    rewrite_test_plan(&directory, |plan| plan["limits"]["output_bytes"] = 8.into());
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    let report = TestReport::parse(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(output.status.code(), Some(1));
    for (name, status, stdout) in [
        ("first", AggregateStatus::Passed, "first\n"),
        ("middle", AggregateStatus::ResourceLimit, "1234567"),
        ("last", AggregateStatus::Passed, "last"),
    ] {
        let leaf = report
            .tests()
            .iter()
            .find(|leaf| leaf.id.ends_with(&format!("::{name}")))
            .unwrap();
        assert_eq!(leaf.status, status, "{name}");
        assert_eq!(
            leaf.attempts[0].stdout.as_bytes(),
            stdout.as_bytes(),
            "{name}"
        );
        assert!(leaf.attempts[0].logs.is_empty());
    }
    assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    assert_eq!(
        report.suites()[0].attempts[0].stdout.as_bytes(),
        b"setupend"
    );
}

#[test]
fn testing_cleanup_panics_cannot_be_hidden_by_host_terminals() {
    for cleanup in ["assert(false)", "testing.assertTextEqual(\"a\", \"b\")"] {
        let source = format!(
            "import std.testing\ntest leaf {{\n defer {cleanup}\n testing.skip(\"not available\")\n}}\nsuite setup {{\n defer {cleanup}\n testing.skip(\"not available\")\n test child {{ assert(true) }}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{cleanup}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(output.status.code(), Some(1), "{cleanup}");
        for leaf in report.tests() {
            assert_eq!(
                leaf.status,
                if leaf.id.ends_with("::child") {
                    AggregateStatus::BlockedSetup
                } else {
                    AggregateStatus::FailedPanic
                },
                "{cleanup}: {}",
                leaf.id
            );
            assert!(leaf.attempts[0].skip.is_none());
        }
        assert_eq!(report.suites()[0].status, AggregateStatus::FailedPanic);
        assert_eq!(
            report.suites()[0].attempts[0].phase,
            Some(tondo_compiler::test_result::AttemptPhase::Setup)
        );
    }
}

#[test]
fn suite_capture_admission_precedes_selection_and_worker_creation() {
    for source in [
        "suite shared {\n var value = 42\n test leaf { assert(value == 42) }\n}\n",
        "suite shared {\n var value = 42\n suite nested {\n test leaf { assert(value == 42) }\n }\n}\n",
        "import std.testing\nsuite shared {\n let value = 42\n test leaf { testing.assertEqual(ref value, ref value) }\n}\n",
        "import std.testing\nimport std.time\nfn timer(): time.Timer { testing.failNow(\"compile only\") }\nsuite shared {\n let value = timer()\n test leaf { value.cancel() }\n}\n",
    ] {
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--list", "--filter", "absent", "--allow-empty"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("E2005"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let directory = test_project(b"import std.testing\nsuite shared {\n let value = 42\n let text = \"Tondo\"\n suite nested {\n test leaf {\n let snapshot = value\n testing.assertEqual(ref snapshot, ref snapshot)\n assert(text == \"Tondo\")\n }\n }\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
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
fn phase_deadlines_do_not_sum_descendants_or_setup_and_teardown() {
    let directory = project_with_source_and_threads(b"fn main() {}\n");
    fs::create_dir_all(directory.join("tests")).unwrap();
    fs::write(
        directory.join("tests/phases.to"),
        r#"
import std.time
suite outer {
    _ = time.sleep(time.Duration.fromNanoseconds(350000000))
    defer { _ = time.sleep(time.Duration.fromNanoseconds(350000000))
    }
    suite inner {
        test first { _ = time.sleep(time.Duration.fromNanoseconds(350000000))
        }
        test second { _ = time.sleep(time.Duration.fromNanoseconds(350000000))
        }
    }
}
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--timeout", "500ms", "--test-format", "json"])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.tests().len(), 2);
    assert_eq!(report.suites().len(), 2);
    assert!(
        report
            .tests()
            .iter()
            .all(|test| test.status == AggregateStatus::Passed)
    );
    assert!(
        report
            .suites()
            .iter()
            .all(|suite| suite.status == AggregateStatus::Passed)
    );
}

#[test]
fn deferred_group_cancellation_waits_for_children_on_return_and_unwind() {
    for (terminal, expected) in [
        ("", AggregateStatus::Passed),
        ("testing.failNow(\"stop\")", AggregateStatus::FailedPanic),
    ] {
        let source = r#"import std.async
import std.testing
fn pending(waiter: async.Waiter[Int, String], started: async.Completer[Unit, String]): Int ! String {
    var owned = waiter
    var signal = started
    defer testing.log("child")
    _ = signal.complete(())
    owned.wait()
}
fn closeCompleter(completer: async.Completer[Int, String]) {
    var owned = completer
    _ = owned.cancel()
}
suite outer {
    test owned {
        defer testing.log("after")
        scope {
            var group = async.group[Int, String]()
            var (waiter, completer) = async.oneshot[Int, String]()
            var (ready, started) = async.oneshot[Unit, String]()
            let job = spawn pending(waiter, started)
            group.add(job)
            _ = ready.wait()?
            defer closeCompleter(completer)
            defer group.cancel()
            TERMINAL
        }
    }
    test zlast {}
}
"#.replace("TERMINAL", terminal);
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{terminal}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(report.tests()[0].status, expected);
        assert_eq!(report.tests()[0].attempts[0].logs, ["child", "after"]);
        assert_eq!(report.tests()[1].status, AggregateStatus::Passed);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn blocking_workers_keep_test_control_and_cleanup_in_their_envelope() {
    for (operation, expected) in [
        ("testing.log(\"work\")\n 1", AggregateStatus::Passed),
        (
            "testing.failNow(\"worker failure\")",
            AggregateStatus::FailedPanic,
        ),
        ("testing.skip(\"worker skip\")", AggregateStatus::Skipped),
    ] {
        let source = r#"import std.executor
import std.testing
fn work(): Int ! executor.ExecutorError {
    defer testing.log("worker-cleanup")
    OPERATION
}
suite outer {
    test owned {
        defer testing.log("parent-cleanup")
        let pool = executor.blockingPool(1, 1)?
        defer pool.shutdown()
        _ = pool.run(work)?
    }
    test zlast {}
}
"#
        .replace("OPERATION", operation);
        let directory = test_project_with_threads(source.as_bytes(), true);
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{operation}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        let owned = &report.tests()[0];
        assert_eq!(owned.status, expected, "{operation}: {:?}", owned.attempts);
        if expected == AggregateStatus::FailedPanic {
            assert!(
                owned.attempts[0]
                    .failure
                    .as_ref()
                    .unwrap()
                    .message
                    .contains("worker failure")
            );
        }
        if expected == AggregateStatus::Skipped {
            assert_eq!(
                owned.attempts[0].skip.as_ref().unwrap().reason,
                "worker skip"
            );
        }
        assert!(
            owned.attempts[0]
                .logs
                .ends_with(&["worker-cleanup".to_owned(), "parent-cleanup".to_owned()]),
            "{operation}: {:?}",
            owned.attempts
        );
        assert_eq!(report.tests()[1].status, AggregateStatus::Passed);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_instruction_budget_is_shared_with_blocking_workers() {
    for calls in [
        "assert(pool.run(work)? == 190)",
        "scope {\n let left = spawn pool.run(work)\n let right = spawn pool.run(work)\n assert((await left)? == 190)\n assert((await right)? == 190)\n }",
    ] {
        let source = r#"import std.executor
fn work(): Int ! executor.ExecutorError {
    var total = 0
    for value in 0..20 {
        total += value
    }
    total
}
suite outer {
    test first {
        let pool = executor.blockingPool(1, 1)?
        defer pool.shutdown()
        assert(pool.run(work)? == 190)
    }
    test many {
        let pool = executor.blockingPool(2, 2)?
        defer pool.shutdown()
        for index in 0..16 {
            RUN_WORK
        }
    }
    test zlast {}
}
"#
        .replace("RUN_WORK", calls);
        let directory = test_project_with_threads(source.as_bytes(), true);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["instructions"] = 1000.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [
                AggregateStatus::Passed,
                AggregateStatus::ResourceLimit,
                AggregateStatus::Passed
            ]
        );
        assert!(report.tests().iter().all(|test| test.attempts.len() == 1));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn minimum_phase_instruction_limit_still_publishes_a_terminal_report() {
    for (source, expected) in [
        ("test minimal {}\n", AggregateStatus::ResourceLimit),
        (
            "suite outer {\n test minimal {}\n}\n",
            AggregateStatus::BlockedSetup,
        ),
    ] {
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| plan["limits"]["instructions"] = 1.into());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        assert_eq!(report.tests()[0].status, expected);
        assert_eq!(report.tests()[0].attempts.len(), 1);
        if !report.suites().is_empty() {
            assert_eq!(report.suites()[0].status, AggregateStatus::ResourceLimit);
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_instruction_budgets_do_not_accumulate_across_sibling_leaves() {
    let mut source =
        String::from("fn work() {\n for value in 0..20 {\n _ = value + 1\n }\n}\nsuite outer {\n");
    for index in 0..128 {
        source.push_str(&format!("test leaf{index:02} {{ work() }}\n"));
    }
    source.push_str("}\n");
    let directory = test_project(source.as_bytes());
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["instructions"] = 1000.into()
    });
    let single = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--filter", "leaf00", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        single.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&single.stderr),
        String::from_utf8_lossy(&single.stdout)
    );
    let all = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        all.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&all.stderr),
        String::from_utf8_lossy(&all.stdout)
    );
    let report = TestReport::parse(&all.stdout).unwrap();
    assert_eq!(report.tests().len(), 128);
    assert!(
        report
            .tests()
            .iter()
            .all(|test| test.status == AggregateStatus::Passed)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn phase_instruction_exhaustion_reports_limits_and_preserves_other_nodes() {
    for (body, phase, expected) in [
        (
            "test first {}\n test middle { spin() }\n test zlast {}",
            None,
            vec![
                AggregateStatus::Passed,
                AggregateStatus::ResourceLimit,
                AggregateStatus::Passed,
            ],
        ),
        (
            "test first {}\n test middle { scope {\n let child = spawn childSpin()\n _ = await child\n }\n }\n test zlast {}",
            None,
            vec![
                AggregateStatus::Passed,
                AggregateStatus::ResourceLimit,
                AggregateStatus::Passed,
            ],
        ),
        (
            "test first {}\n test middle { defer { spin() }\n spin() }\n test zlast {}",
            None,
            vec![
                AggregateStatus::Passed,
                AggregateStatus::ResourceLimit,
                AggregateStatus::Passed,
            ],
        ),
        (
            "spin()\n test child {}",
            Some("setup"),
            vec![AggregateStatus::BlockedSetup],
        ),
        (
            "defer { spin() }\n test child {}",
            Some("teardown"),
            vec![AggregateStatus::Passed],
        ),
    ] {
        let directory = test_project(format!("fn spin() {{\n for {{}}\n}}\nfn childSpin() suspends {{ spin() }}\nsuite outer {{\n{body}\n}}\n").as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["instructions"] = 1000.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{phase:?}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{phase:?}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(report.tests().iter().all(|test| test.attempts.len() == 1));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        if let Some(phase) = phase {
            assert_eq!(report.suites()[0].status, AggregateStatus::ResourceLimit);
            assert_eq!(json["suites"][0]["attempts"][0]["phase"], phase);
            assert_eq!(
                json["suites"][0]["attempts"][0]["failure"]["code"],
                "instructions"
            );
        } else {
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            assert_eq!(
                json["tests"][1]["attempts"][0]["failure"]["code"],
                "instructions"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_heap_budgets_exclude_retained_ancestor_state() {
    for (parent_bytes, child_bytes) in [(3000, 0), (0, 3000), (3000, 3000)] {
        let source = format!(
            "suite outer {{\n let parent = \"{}\"\n defer {{ assert(parent.length() == {parent_bytes})\n }}\n test child {{\n let value = \"{}\"\n assert(value.length() == {child_bytes})\n }}\n test zlast {{}}\n}}\n",
            "p".repeat(parent_bytes),
            "c".repeat(child_bytes)
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 8192.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{parent_bytes}/{child_bytes}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        assert!(
            report
                .tests()
                .iter()
                .all(|test| test.status == AggregateStatus::Passed)
        );
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_frame_memory_is_admitted_before_local_storage_and_preserves_siblings() {
    let locals = (0..512)
        .map(|index| format!("let value{index} = {index}\n"))
        .collect::<String>();
    let source = format!(
        "fn largeFrame() {{\n{locals}}}\nsuite outer {{\n test first {{}}\n test middle {{ largeFrame() }}\n test zlast {{}}\n}}\n"
    );
    let directory = test_project(source.as_bytes());
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["memory_bytes"] = 8192.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(
        report
            .tests()
            .iter()
            .map(|test| test.status)
            .collect::<Vec<_>>(),
        [
            AggregateStatus::Passed,
            AggregateStatus::ResourceLimit,
            AggregateStatus::Passed
        ]
    );
    assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    assert_eq!(report.tests()[1].attempts.len(), 1);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["tests"][1]["attempts"][0]["failure"]["code"], "memory");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn phase_frame_collection_keeps_parent_roots_during_spawn() {
    let locals = (0..48)
        .map(|index| format!("let value{index} = {index}\n"))
        .collect::<String>();
    let source = format!(
        "import std.time\nfn garbage() {{\n let value = \"{}\"\n assert(value.length() == 2000)\n}}\nfn child() {{\n{locals}_ = time.sleep(time.Duration.fromNanoseconds(0))\n}}\ntest roots {{\n let retained = \"{}\"\n garbage()\n scope {{\n let work = spawn child()\n await work\n }}\n assert(retained.length() == 2000)\n}}\n",
        "g".repeat(2000),
        "r".repeat(2000)
    );
    for limit in [8192, 10000, 12000] {
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = limit.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "limit {limit}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_hosted_bytes_share_the_memory_budget_and_preserve_siblings() {
    for (copies, expected) in [
        (1, AggregateStatus::Passed),
        (12, AggregateStatus::ResourceLimit),
    ] {
        let source = format!(
            "import std.bytes\nsuite outer {{\n test first {{}}\n test middle {{\n let text = \"{}\"\n var held = [bytes.Bytes(text)?]\n for index in 1..{copies} {{\n held.push(bytes.Bytes(text)?)?\n }}\n assert(held.length() == {copies})\n for item in held {{ assert(item.length() == 2048) }}\n }}\n test zlast {{}}\n}}\n",
            "x".repeat(2048)
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 16384.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed]
        );
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(report.tests()[1].attempts.len(), 1);
        if expected == AggregateStatus::ResourceLimit {
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["tests"][1]["attempts"][0]["failure"]["code"], "memory");
            assert_eq!(output.status.code(), Some(1));
        } else {
            assert_eq!(output.status.code(), Some(0));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_executor_capacity_is_reserved_before_starting_workers() {
    for constructor in ["executor.pool", "executor.blockingPool", "actor"] {
        for (capacity, expected) in [
            (1, AggregateStatus::Passed),
            (2048, AggregateStatus::ResourceLimit),
        ] {
            let body = if constructor == "actor" {
                format!(
                    "let pool = executor.pool(1, 1)?\n defer pool.shutdown()\n let result = pool.actor(0, {capacity}, step)\n match result {{\n ok(actor) => {{\n actor.stop()?\n }}\n err(_) => ()\n }}"
                )
            } else {
                format!(
                    "let result = {constructor}(1, {capacity})\n match result {{\n ok(pool) => pool.shutdown()\n err(_) => ()\n }}"
                )
            };
            let source = format!(
                r#"import std.executor
fn step(state: mut Int, message: Int): Unit ! Never suspends {{
    state += message
}}
suite outer {{
    test first {{}}
    test middle {{
        {body}
    }}
    test zlast {{}}
}}
"#
            );
            let directory = test_project_with_threads(source.as_bytes(), true);
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 32768.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--retry", "1", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report
                    .tests()
                    .iter()
                    .map(|test| test.status)
                    .collect::<Vec<_>>(),
                [AggregateStatus::Passed, expected, AggregateStatus::Passed],
                "{constructor}, {capacity}"
            );
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            assert_eq!(report.tests()[1].attempts.len(), 1);
            assert_eq!(
                output.status.code(),
                Some(i32::from(expected != AggregateStatus::Passed))
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn phase_async_collect_memory_cannot_be_caught_as_a_collection_error() {
    for (count, spawned, expected) in [
        (2, false, AggregateStatus::Passed),
        (2048, false, AggregateStatus::ResourceLimit),
        (2048, true, AggregateStatus::ResourceLimit),
    ] {
        let call = if spawned {
            format!(
                "scope {{\n let pending = spawn cursor.collect(limit: {count})\n let result = await pending\n match result {{\n ok(values) => assert(values.length() == {count})\n err(_) => ()\n }}\n}}"
            )
        } else {
            format!(
                "let result = cursor.collect(limit: {count})\n match result {{\n ok(values) => assert(values.length() == {count})\n err(_) => ()\n }}"
            )
        };
        let source = format!(
            r#"type Countdown = {{ remaining: Int }}
impl AsyncIterator[Int] for Countdown {{
    fn next(mut self): Int? suspends {{
        if self.remaining == 0 {{
            return none
        }}
        self.remaining -= 1
        some(self.remaining)
    }}
}}
suite outer {{
    test first {{}}
    test middle {{
        var cursor = Countdown {{ remaining: {count} }}
        {call}
    }}
    test zlast {{}}
}}
"#
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{count}, spawned={spawned}"
        );
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_cleanup_memory_is_bounded_and_released_after_execution() {
    for (body, expected) in [
        ("defer noop()\n".repeat(256), AggregateStatus::ResourceLimit),
        (
            "for index in 0..512 {\n defer noop()\n}\n".to_owned(),
            AggregateStatus::Passed,
        ),
    ] {
        let directory = test_project(format!("fn noop() {{}}\nsuite outer {{\n test first {{}}\n test middle {{\n{body}\n}}\n test zlast {{}}\n}}\n").as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 16384.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed]
        );
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_scheduler_metadata_is_admitted_before_task_publication() {
    let profiles = [
        (1, 10_000_000, AggregateStatus::Passed),
        (512, 10_000_000, AggregateStatus::ResourceLimit),
    ]
    .into_iter()
    .chain((1..=48).map(|steps| (512, steps, AggregateStatus::ResourceLimit)));
    for (count, instructions, expected) in profiles {
        let middle = format!(
            r#"test middle {{
        for index in 0..{count} {{
            scope {{
                let job = spawn work()
                (await job)?
            }}
        }}
}}
"#
        );
        // Small instruction caps target the leaf directly: an equally small
        // suite setup would correctly block all descendants before this move.
        let has_suite = instructions == 10_000_000;
        let tests = if has_suite {
            format!("suite outer {{\n test first {{}}\n{middle}\n test zlast {{}}\n}}\n")
        } else {
            middle
        };
        let source = format!(
            "import std.time\nfn work(): Unit ! time.ClockError {{\n time.sleep(time.Duration.fromNanoseconds(0))?\n}}\n{tests}"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into();
            plan["limits"]["instructions"] = instructions.into();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            if has_suite {
                vec![AggregateStatus::Passed, expected, AggregateStatus::Passed]
            } else {
                vec![expected]
            },
            "count={count}, instructions={instructions}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        if has_suite {
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        }
        assert_eq!(report.tests()[usize::from(has_suite)].attempts.len(), 1);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_hosted_buffers_reclaim_dead_values_and_bound_builder_growth() {
    for (body, expected) in [
        ("for index in 0..128 {\n let value = bytes.Bytes(text)?\n assert(value.length() == 1024)\n }".to_owned(), AggregateStatus::Passed),
        ("var builder = bytes.builder()?\n for index in 0..32 {\n builder.append(bytes.Bytes(text)?)?\n }\n assert(builder.length() == 32768)".to_owned(), AggregateStatus::ResourceLimit),
        ("var builder = format.Builder.new()\n for index in 0..32 {\n builder.append(text)?\n }\n assert(builder.finish()?.length() == 32768)".to_owned(), AggregateStatus::ResourceLimit),
    ] {
        let source = format!("import std.bytes\nimport std.format\nsuite outer {{\n test first {{}}\n test middle {{\n let text = \"{}\"\n {body}\n }}\n test zlast {{}}\n}}\n", "x".repeat(1024));
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| plan["limits"]["memory_bytes"] = 16384.into());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo")).current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"]).output().unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| panic!("{error}: {}\n{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout)));
        assert_eq!(report.tests().iter().map(|test| test.status).collect::<Vec<_>>(), [AggregateStatus::Passed, expected, AggregateStatus::Passed], "{body}");
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(output.status.code(), Some(i32::from(expected != AggregateStatus::Passed)));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_hosted_buffers_survive_blocking_roots_and_reclaim_worker_garbage() {
    let source = r#"import std.bytes
import std.executor
fn work(): bytes.Bytes ! bytes.BytesError {
    let text = "TEXT"
    let retained = bytes.Bytes(text)?
    for index in 0..64 {
        let transient = bytes.Bytes(text)?
        assert(transient.length() == 1024)
        assert(retained.length() == 1024)
    }
    retained
}
test roots {
    let pool = executor.blockingPool(2, 2)?
    defer pool.shutdown()
    scope {
        let left = spawn pool.run(work)
        let right = spawn pool.run(work)
        let retained = (await left)?
        let second = (await right)?
        for index in 0..16 {
            let temporary = bytes.Bytes("TEXT")?
            assert(temporary.length() == 1024)
        }
        assert(retained.length() == 1024)
        assert(second.length() == 1024)
    }
}
"#
    .replace("TEXT", &"x".repeat(1024));
    let directory = test_project_with_threads(source.as_bytes(), true);
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["memory_bytes"] = 32768.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn phase_path_and_environment_name_payloads_share_memory_and_are_reclaimed() {
    for constructor in ["path.Path.fromString(text)?", "env.Name.fromText(text)?"] {
        for (copies, keep, expected) in [
            (1, true, AggregateStatus::Passed),
            (12, true, AggregateStatus::ResourceLimit),
            (64, false, AggregateStatus::Passed),
        ] {
            let body = if keep {
                format!(
                    "var held = [{constructor}]\n for index in 1..{copies} {{\n held.push({constructor})?\n }}\n assert(held.length() == {copies})"
                )
            } else {
                format!("for index in 0..{copies} {{\n _ = {constructor}\n }}")
            };
            let source = format!(
                "import std.path\nimport std.env\nsuite outer {{\n test first {{}}\n test middle {{\n let text = \"{}\"\n {body}\n }}\n test zlast {{}}\n}}\n",
                "x".repeat(2048)
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 16384.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--retry", "1", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report
                    .tests()
                    .iter()
                    .map(|test| test.status)
                    .collect::<Vec<_>>(),
                [AggregateStatus::Passed, expected, AggregateStatus::Passed],
                "{constructor}: copies={copies}, keep={keep}"
            );
            assert_eq!(report.tests()[1].attempts.len(), 1);
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn phase_concurrent_collection_payloads_share_memory_and_are_reclaimed() {
    for constructor in [
        "sync.Array[text]",
        "sync.Map[0: text]",
        "sync.Set[text]",
        "sync.Stack[text]",
        "sync.Queue[text]",
    ] {
        for (copies, keep, expected) in [
            (1, true, AggregateStatus::Passed),
            (12, true, AggregateStatus::ResourceLimit),
            (64, false, AggregateStatus::Passed),
        ] {
            let body = if keep {
                format!(
                    "var held = [{constructor}]\n for index in 1..{copies} {{\n held.push({constructor})?\n }}\n assert(held.length() == {copies})"
                )
            } else {
                format!("for index in 0..{copies} {{\n _ = {constructor}\n }}")
            };
            let source = format!(
                "import std.sync\nsuite outer {{\n test first {{}}\n test middle {{\n let text = \"{}\"\n {body}\n }}\n test zlast {{}}\n}}\n",
                "x".repeat(2048)
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 16384.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--retry", "1", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report
                    .tests()
                    .iter()
                    .map(|test| test.status)
                    .collect::<Vec<_>>(),
                [AggregateStatus::Passed, expected, AggregateStatus::Passed],
                "{constructor}: copies={copies}, keep={keep}"
            );
            assert_eq!(report.tests()[1].attempts.len(), 1);
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn host_mutation_waits_for_complete_vm_result_admission() {
    let source = format!(
        "import std.sync\nimport std.testing\nsuite outer {{\n\
         let shared = sync.Array[7]\n\
         defer {{\n match shared.get(0) {{\n\
          some(value) => {{\n if value == 9 {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
          none => testing.failNow(\"missing collection item\")\n }}\n }}\n\
         test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
          _ = local.set(0, 9)?\n }}\n}}\n",
        "p".repeat(6000)
    );
    let directory = test_project(source.as_bytes());
    let mut failures = Vec::new();
    for (memory, status, expected_log) in [
        (7232, AggregateStatus::ResourceLimit, "original"),
        (10240, AggregateStatus::Passed, "changed"),
    ] {
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = memory.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        let leaf = &report.tests()[0];
        let suite = &report.suites()[0];
        assert_eq!(leaf.status, status, "memory={memory}");
        assert_eq!(leaf.attempts.len(), 1);
        assert_eq!(suite.status, AggregateStatus::Passed, "memory={memory}");
        if suite.attempts[0].logs != [expected_log] {
            failures.push(format!(
                "memory={memory}: observed {:?}, expected {expected_log}",
                suite.attempts[0].logs
            ));
        }
    }
    fs::remove_dir_all(directory).unwrap();
    assert!(
        failures.is_empty(),
        "host mutation preceded complete result admission: {failures:?}"
    );
}

#[test]
fn collection_removal_waits_for_complete_vm_result_admission() {
    let mut failures = Vec::new();
    for (constructor, operation, rejected_memory) in [
        ("sync.Map[0: 7]", "local.remove(0)", 7088),
        ("sync.Stack[7]", "local.pop()", 7024),
        ("sync.Queue[7]", "local.dequeue()", 7024),
    ] {
        let source = format!(
            "import std.sync\nimport std.testing\nsuite outer {{\n\
             let shared = {constructor}\n\
             defer {{\n if shared.isEmpty() {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
             test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
             _ = {operation}\n }}\n}}\n",
            "p".repeat(6000)
        );
        let directory = test_project(source.as_bytes());
        for (memory, status, expected_log) in [
            (rejected_memory, AggregateStatus::ResourceLimit, "original"),
            (10240, AggregateStatus::Passed, "changed"),
        ] {
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = memory.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{constructor} {memory}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(report.tests()[0].status, status, "{constructor} {memory}");
            assert_eq!(report.tests()[0].attempts.len(), 1);
            assert_eq!(
                report.suites()[0].status,
                AggregateStatus::Passed,
                "{constructor} {memory}"
            );
            if report.suites()[0].attempts[0].logs != [expected_log] {
                failures.push(format!(
                    "{constructor} {memory}: observed {:?}, expected {expected_log}",
                    report.suites()[0].attempts[0].logs
                ));
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
    assert!(
        failures.is_empty(),
        "removed before VM import admission: {failures:?}"
    );
}

#[test]
fn collection_removal_imports_nested_payloads_and_empty_results_from_spawned_calls() {
    for (constructor, operation) in [
        ("sync.Map[0: item]", "shared.remove(0)"),
        ("sync.Stack[item]", "shared.pop()"),
        ("sync.Queue[item]", "shared.dequeue()"),
    ] {
        for spawned in [false, true] {
            let first = if spawned {
                format!("let pending = spawn {operation}\n let first = await pending")
            } else {
                format!("let first = {operation}")
            };
            let second = if spawned {
                format!("let pendingEmpty = spawn {operation}\n let second = await pendingEmpty")
            } else {
                format!("let second = {operation}")
            };
            let source = format!(
                "import std.sync\n\
                 type Envelope[T] = {{ values: Array[T] }}\n\
                 test nested {{\n scope {{\n\
                 let item: Envelope[String] = Envelope {{ values: [\"α\", \"🦀\"] }}\n\
                 let shared = {constructor}\n\
                 {first}\n\
                 match first {{\n some(value) => {{\n\
                   assert(value.values.get(0) == some(\"α\"))\n\
                   assert(value.values.get(1) == some(\"🦀\"))\n\
                 }}\n none => assert(false)\n }}\n\
                 assert(shared.isEmpty())\n\
                 {second}\n\
                 match second {{\n none => ()\n some(_) => assert(false)\n }}\n\
                 assert(shared.isEmpty())\n\
                 }}\n }}\n"
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 65536.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{constructor} spawned={spawned}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report.tests()[0].status,
                AggregateStatus::Passed,
                "{constructor} spawned={spawned}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert_eq!(report.tests()[0].attempts.len(), 1);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn map_insertion_waits_for_complete_vm_result_admission() {
    let mut failures = Vec::new();
    for (key, rejected_memory) in [(0, 7216), (1, 7184)] {
        let source = format!(
            "import std.sync\nimport std.testing\nsuite outer {{\n\
             let shared = sync.Map[0: 7]\n\
             defer {{\n match shared.get({key}) {{\n\
               some(value) => {{\n if value == 9 {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
               none => testing.log(\"original\")\n }}\n }}\n\
             test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
             _ = local.insert({key}, 9)?\n }}\n}}\n",
            "p".repeat(6000)
        );
        let directory = test_project(source.as_bytes());
        for (memory, status, expected_log) in [
            (rejected_memory, AggregateStatus::ResourceLimit, "original"),
            (10240, AggregateStatus::Passed, "changed"),
        ] {
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = memory.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "key={key} {memory}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(report.tests()[0].status, status, "key={key} {memory}");
            assert_eq!(report.tests()[0].attempts.len(), 1);
            assert_eq!(
                report.suites()[0].status,
                AggregateStatus::Passed,
                "key={key} {memory}"
            );
            if report.suites()[0].attempts[0].logs != [expected_log] {
                failures.push(format!(
                    "key={key} {memory}: observed {:?}, expected {expected_log}",
                    report.suites()[0].attempts[0].logs
                ));
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
    assert!(
        failures.is_empty(),
        "inserted before VM import admission: {failures:?}"
    );
}

#[test]
fn map_insertion_imports_optional_nested_previous_values_from_spawned_calls() {
    for spawned in [false, true] {
        let first = if spawned {
            "let pending = spawn shared.insert(1, item)\n let result = await pending\n let first = result?"
        } else {
            "let first = shared.insert(1, item)?"
        };
        let second = if spawned {
            "let pendingAgain = spawn shared.insert(0, replacement)\n let again = await pendingAgain\n let second = again?"
        } else {
            "let second = shared.insert(0, replacement)?"
        };
        let source = format!(
            "import std.sync\n\
             type Envelope[T] = {{ values: Array[T] }}\n\
             test nested {{\n scope {{\n\
             let item: Envelope[String] = Envelope {{ values: [\"α\", \"🦀\"] }}\n\
             let replacement: Envelope[String] = Envelope {{ values: [\"new\"] }}\n\
             let shared = sync.Map[0: item]\n\
             {first}\n\
             match first {{\n none => ()\n some(_) => assert(false)\n }}\n\
             {second}\n\
             match second {{\n some(value) => {{\n\
               assert(value.values.get(0) == some(\"α\"))\n\
               assert(value.values.get(1) == some(\"🦀\"))\n\
             }}\n none => assert(false)\n }}\n\
             match shared.get(0) {{\n some(value) => assert(value.values.get(0) == some(\"new\"))\n none => assert(false)\n }}\n\
             match shared.get(1) {{\n some(value) => assert(value.values.get(0) == some(\"α\"))\n none => assert(false)\n }}\n\
             }}\n }}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "spawned={spawned}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report.tests()[0].status,
            AggregateStatus::Passed,
            "spawned={spawned}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[0].attempts.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn collection_growth_waits_for_complete_vm_result_admission() {
    let mut failures = Vec::new();
    for (constructor, operation) in [
        ("sync.Set[7]", "local.insert(9)"),
        ("sync.Stack[7]", "local.push(9)"),
        ("sync.Queue[7]", "local.enqueue(9)"),
    ] {
        let source = format!(
            "import std.sync\nimport std.testing\nsuite outer {{\n\
             let shared = {constructor}\n\
             defer {{\n if shared.length() == 2 {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
             test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
             _ = {operation}?\n }}\n}}\n",
            "p".repeat(6000)
        );
        let directory = test_project(source.as_bytes());
        for (memory, status, expected_log) in [
            (7120, AggregateStatus::ResourceLimit, "original"),
            (10240, AggregateStatus::Passed, "changed"),
        ] {
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = memory.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{constructor} {memory}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(report.tests()[0].status, status, "{constructor} {memory}");
            assert_eq!(report.tests()[0].attempts.len(), 1);
            assert_eq!(
                report.suites()[0].status,
                AggregateStatus::Passed,
                "{constructor} {memory}"
            );
            if report.suites()[0].attempts[0].logs != [expected_log] {
                failures.push(format!(
                    "{constructor} {memory}: observed {:?}, expected {expected_log}",
                    report.suites()[0].attempts[0].logs
                ));
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
    assert!(
        failures.is_empty(),
        "grew before VM import admission: {failures:?}"
    );
}

#[test]
fn set_insertion_previews_duplicate_keys_in_direct_and_spawned_calls() {
    for spawned in [false, true] {
        let first = if spawned {
            "let pending = spawn shared.insert(9)\n let result = await pending\n let first = result?"
        } else {
            "let first = shared.insert(9)?"
        };
        let second = if spawned {
            "let pendingAgain = spawn shared.insert(9)\n let again = await pendingAgain\n let second = again?"
        } else {
            "let second = shared.insert(9)?"
        };
        let source = format!(
            "import std.sync\n test duplicate {{\n scope {{\n\
             let shared = sync.Set[7]\n\
             {first}\n assert(first)\n\
             {second}\n assert(not second)\n\
             assert(shared.length() == 2)\n assert(shared.contains(7))\n assert(shared.contains(9))\n\
             }}\n }}\n"
        );
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "spawned={spawned}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report.tests()[0].status,
            AggregateStatus::Passed,
            "spawned={spawned}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn public_test_headers_keep_tuple_conditions_and_nested_closures() {
    let directory = test_project(
        br#"type Config = { enabled: Bool }
fn enabled(config: Config): Bool { config.enabled }
fn accepts(predicate: fn(): Bool): Bool { predicate() }

test headerValues {
    let pair = (9, 10)
    if pair == (9, 10) { assert(true) } else { assert(false) }
    let left = 9
    let right = 10
    if pair == (left, right) { assert(true) } else { assert(false) }
    let ready = true
    if (ready) { assert(true) } else { assert(false) }
    if accepts(() { true }) { assert(true) } else { assert(false) }
    if enabled(Config { enabled: true }) { assert(true) } else { assert(false) }
    match (left, right) {
        (9, 10) => assert(true)
        _ => assert(false)
    }
    match (9, -10i32) {
        (9, /* prefix */ - /* sign */ 0xAi32) => assert(true)
        _ => assert(false)
    }
    match (1.25, -2.5f32) {
        (1.25, /* prefix */ - /* sign */ 2.5f32) => assert(true)
        _ => assert(false)
    }
    match ("left", " right ") {
        ("left", /* prefix */ " right ") => assert(true)
        _ => assert(false)
    }
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    let report = TestReport::parse(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(report.summary().passed, 1);
    for body in [
        "assert(!ready)",
        "assert(left || right)",
        ",",
        ")",
        "]",
        "=>",
    ] {
        fs::write(
            directory.join("tests/smoke.to"),
            format!("test invalid {{\n{body}\n}}\ntest valid {{ assert(true) }}\n"),
        )
        .unwrap();
        let rejected = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--list", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(1), "{body}");
        assert!(rejected.stdout.is_empty(), "{body}");
        let diagnostics = String::from_utf8_lossy(&rejected.stderr);
        assert!(diagnostics.contains("E0004"), "{body}: {diagnostics}");
        assert!(!diagnostics.contains("T0002"), "{body}: {diagnostics}");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn atomic_mutations_wait_for_complete_synchronous_vm_result_admission() {
    let mut failures = Vec::new();
    for (initial, replacement, operation, historical_memory) in [
        (
            "7",
            "9",
            "local.compareExchange(7, 9, sync.MemoryOrder.SeqCst, sync.MemoryOrder.SeqCst)",
            8254,
        ),
        (
            "(7, 8)",
            "(9, 10)",
            "local.swap((9, 10), sync.MemoryOrder.SeqCst)",
            8671,
        ),
    ] {
        let source = format!(
            "import std.sync\nimport std.testing\nsuite outer {{\n\
             let shared = sync.atomic({initial})\n\
             defer {{\n if shared.load(sync.MemoryOrder.SeqCst) == {replacement} {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
             test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
             _ = {operation}\n }}\n}}\n",
            "p".repeat(6000)
        );
        let directory = test_project(source.as_bytes());
        let mut run_at = |memory: u64| {
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = memory.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{operation} {memory}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            let status = report.tests()[0].status;
            let expected_log = match status {
                AggregateStatus::Passed => "changed",
                AggregateStatus::ResourceLimit => "original",
                _ => panic!("{operation} {memory}: unexpected {status:?}"),
            };
            assert_eq!(report.tests()[0].attempts.len(), 1);
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            if report.suites()[0].attempts[0].logs != [expected_log] {
                failures.push(format!(
                    "{operation} {memory}: observed {:?}, expected {expected_log}",
                    report.suites()[0].attempts[0].logs
                ));
            }
            status
        };
        // Keep the original regression quota, but locate an adjacent rejection
        // and success on this host: VM storage includes native Rust layouts.
        run_at(historical_memory);
        let mut rejected = 6144;
        let mut accepted = 10240;
        assert_eq!(run_at(rejected), AggregateStatus::ResourceLimit);
        assert_eq!(run_at(accepted), AggregateStatus::Passed);
        // This interval takes at most twelve probes, with fresh workers each
        // time. Every rejected probe must preserve the suite-owned value.
        while accepted - rejected > 1 {
            let middle = rejected + (accepted - rejected) / 2;
            match run_at(middle) {
                AggregateStatus::ResourceLimit => rejected = middle,
                AggregateStatus::Passed => accepted = middle,
                _ => unreachable!(),
            }
        }
        assert_eq!(run_at(rejected), AggregateStatus::ResourceLimit);
        assert_eq!(run_at(accepted), AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
    assert!(
        failures.is_empty(),
        "atomic storage changed before VM import admission: {failures:?}"
    );
}

#[test]
fn atomic_observed_values_survive_direct_indirect_and_deferred_calls() {
    for call in [
        "shared.compareExchange(initial, replacement, sync.MemoryOrder.SeqCst, sync.MemoryOrder.Acquire)",
        "exchange(shared, initial, replacement)",
        "action(shared, initial, replacement)",
    ] {
        let source = format!(
            "import std.sync\n\
             type Payload[T] = {{ first: T, pair: (T, T) }}\n\
             fn exchange(shared: sync.Atomic[Payload[Int]], expected: Payload[Int], desired: Payload[Int]): sync.CompareExchange[Payload[Int]] {{\n\
               shared.compareExchange(expected, desired, sync.MemoryOrder.SeqCst, sync.MemoryOrder.Acquire)\n }}\n\
             test values {{\n\
             let initial = Payload[Int] {{ first: 7, pair: (8, 9) }}\n\
             let replacement = Payload[Int] {{ first: 10, pair: (11, 12) }}\n\
             let shared = sync.atomic(initial)\n let action = exchange\n\
             match {call} {{\n sync.CompareExchange.Exchanged(previous) => assert(previous == initial)\n sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n\
             match {call} {{\n sync.CompareExchange.Mismatch(previous) => assert(previous == replacement)\n sync.CompareExchange.Exchanged(_) => assert(false)\n }}\n\
             assert(shared.load(sync.MemoryOrder.Relaxed) == replacement)\n\
             {{ defer {{\n let previous = shared.swap(initial, sync.MemoryOrder.AcqRel)\n assert(previous == replacement)\n }}\n }}\n\
             assert(shared.load(sync.MemoryOrder.SeqCst) == initial)\n\
             {{ defer {{\n match shared.compareExchange(initial, replacement, sync.MemoryOrder.Release, sync.MemoryOrder.Relaxed) {{\n\
                 sync.CompareExchange.Exchanged(previous) => assert(previous == initial)\n sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n }}\n }}\n\
             assert(shared.load(sync.MemoryOrder.SeqCst) == replacement)\n\
             }}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{call}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report.tests()[0].status,
            AggregateStatus::Passed,
            "{call}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn collection_compare_exchange_waits_for_complete_vm_result_admission() {
    let mut failures = Vec::new();
    for (constructor, operation, rejected_memory) in [
        ("sync.Array[7]", "local.compareExchange(0, 7, 9)", 7312),
        (
            "sync.Map[0: 7]",
            "local.compareExchange(0, some(7), some(9))",
            8240,
        ),
    ] {
        let source = format!(
            "import std.sync\nimport std.testing\nsuite outer {{\n\
             let shared = {constructor}\n\
             defer {{\n match shared.get(0) {{\n\
               some(value) => {{\n if value == 9 {{ testing.log(\"changed\") }} else {{ testing.log(\"original\") }}\n }}\n\
               none => testing.failNow(\"missing collection item\")\n }}\n }}\n\
             test child {{\n let local = shared\n let padding = \"{}\"\n assert(padding.length() == 6000)\n\
             _ = {operation}?\n }}\n}}\n",
            "p".repeat(6000)
        );
        let directory = test_project(source.as_bytes());
        for (memory, status, expected_log) in [
            (rejected_memory, AggregateStatus::ResourceLimit, "original"),
            (10240, AggregateStatus::Passed, "changed"),
        ] {
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = memory.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{constructor} {memory}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(report.tests()[0].status, status, "{constructor} {memory}");
            assert_eq!(report.tests()[0].attempts.len(), 1);
            assert_eq!(
                report.suites()[0].status,
                AggregateStatus::Passed,
                "{constructor} {memory}"
            );
            if report.suites()[0].attempts[0].logs != [expected_log] {
                failures.push(format!(
                    "{constructor} {memory}: observed {:?}, expected {expected_log}",
                    report.suites()[0].attempts[0].logs
                ));
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
    assert!(
        failures.is_empty(),
        "exchanged before VM import admission: {failures:?}"
    );
}

#[test]
fn collection_compare_exchange_imports_generic_exchanged_and_mismatch_payloads() {
    for map in [false, true] {
        for spawned in [false, true] {
            let constructor = if map {
                "sync.Map[0: item]"
            } else {
                "sync.Array[item]"
            };
            let arguments = if map {
                "0, some(item), some(replacement)"
            } else {
                "0, item, replacement"
            };
            let operation = format!("shared.compareExchange({arguments})");
            let call = |name: &str| {
                if spawned {
                    format!(
                        "let pending{name} = spawn {operation}\n let result{name} = await pending{name}\n let {name} = result{name}?"
                    )
                } else {
                    format!("let {name} = {operation}?")
                }
            };
            let check = |expected: &str| {
                if map {
                    format!(
                        "match previous {{\n some(value) => assert(value.values.get(0) == some(\"{expected}\"))\n none => assert(false)\n }}"
                    )
                } else {
                    format!("assert(previous.values.get(0) == some(\"{expected}\"))")
                }
            };
            let source = format!(
                "import std.sync\n type Envelope[T] = {{ values: Array[T] }}\n\
                 test exchanged {{\n scope {{\n\
                 let item: Envelope[String] = Envelope {{ values: [\"α\", \"🦀\"] }}\n\
                 let replacement: Envelope[String] = Envelope {{ values: [\"new\"] }}\n\
                 let shared = {constructor}\n\
                 {}\n match first {{\n sync.CompareExchange.Exchanged(previous) => {{ {} }}\n\
                   sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n\
                 {}\n match second {{\n sync.CompareExchange.Mismatch(previous) => {{ {} }}\n\
                   sync.CompareExchange.Exchanged(_) => assert(false)\n }}\n\
                 match shared.get(0) {{\n some(value) => assert(value.values.get(0) == some(\"new\"))\n none => assert(false)\n }}\n\
                 }}\n }}\n",
                call("first"),
                check("α"),
                call("second"),
                check("new")
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 65536.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "map={map} spawned={spawned}: {error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report.tests()[0].status,
                AggregateStatus::Passed,
                "map={map} spawned={spawned}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn map_compare_exchange_preserves_nested_optional_absence_and_removal() {
    for spawned in [false, true] {
        let call = |name: &str, arguments: &str| {
            if spawned {
                format!(
                    "let pending{name} = spawn shared.compareExchange({arguments})\n let result{name} = await pending{name}\n let {name} = result{name}?"
                )
            } else {
                format!("let {name} = shared.compareExchange({arguments})?")
            }
        };
        let source = format!(
            "import std.sync\n test optional {{\n scope {{\n\
             let shared: sync.Map[Int, Int?] = sync.Map[:]\n\
             {}\n match first {{\n sync.CompareExchange.Exchanged(previous) => assert(previous == none)\n sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n\
             assert(shared.get(0) == some(none))\n\
             {}\n match second {{\n sync.CompareExchange.Exchanged(previous) => assert(previous == some(none))\n sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n\
             {}\n match third {{\n sync.CompareExchange.Mismatch(previous) => assert(previous == some(some(7)))\n sync.CompareExchange.Exchanged(_) => assert(false)\n }}\n\
             {}\n match last {{\n sync.CompareExchange.Exchanged(previous) => assert(previous == some(some(7)))\n sync.CompareExchange.Mismatch(_) => assert(false)\n }}\n\
             assert(shared.get(0) == none)\n\
             }}\n }}\n",
            call("first", "0, none, some(none)"),
            call("second", "0, some(none), some(some(7))"),
            call("third", "0, some(some(8)), none"),
            call("last", "0, some(some(7)), none")
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "spawned={spawned}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report.tests()[0].status,
            AggregateStatus::Passed,
            "spawned={spawned}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_concurrent_collection_growth_remains_a_nonretryable_runner_limit() {
    for (initial, operation) in [
        ("let held = sync.Stack[text]", "held.push(text)"),
        ("let held = sync.Queue[text]", "held.enqueue(text)"),
        (
            "let held = sync.Map[0: text]",
            "held.insert(index + 1, text)",
        ),
        (
            "let held = sync.Set[text]",
            "held.insert(\"{index}{text}\")",
        ),
    ] {
        for (count, spawned, expected) in [
            (1, false, AggregateStatus::Passed),
            (1, true, AggregateStatus::Passed),
            (12, false, AggregateStatus::ResourceLimit),
            (12, true, AggregateStatus::ResourceLimit),
        ] {
            let step = if spawned {
                format!(
                    "scope {{\n let pending = spawn {operation}\n let result = await pending\n match result {{\n ok(_) => ()\n err(_) => ()\n }}\n}}"
                )
            } else {
                format!(
                    "let result = {operation}\n match result {{\n ok(_) => ()\n err(_) => ()\n }}"
                )
            };
            let source = format!(
                "import std.sync\nsuite outer {{\n test first {{}}\n test middle {{\n let text = \"{}\"\n {initial}\n for index in 0..{count} {{\n {step}\n }}\n }}\n test zlast {{}}\n}}\n",
                "x".repeat(2048)
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 16384.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--retry", "1", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{error}: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            });
            assert_eq!(
                report
                    .tests()
                    .iter()
                    .map(|test| test.status)
                    .collect::<Vec<_>>(),
                [AggregateStatus::Passed, expected, AggregateStatus::Passed],
                "{operation}: {count}, spawned={spawned}"
            );
            assert_eq!(report.tests()[1].attempts.len(), 1);
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn buffered_channel_results_preserve_generic_payloads_and_terminal_states() {
    for method in ["receive", "tryReceive"] {
        for spawned in [false, true] {
            let call = |binding: &str| {
                if spawned {
                    format!(
                        "let pending = spawn receiver.{method}()\n let {binding} = await pending\n"
                    )
                } else {
                    format!("let {binding} = receiver.{method}()\n")
                }
            };
            let item_pattern = if method == "receive" {
                "some(item)"
            } else {
                "channel.TryReceive.Item(item)"
            };
            let end_pattern = if method == "receive" {
                "none"
            } else {
                "channel.TryReceive.Closed"
            };
            let initial_empty = if method == "tryReceive" {
                "match receiver.tryReceive() {\n channel.TryReceive.Empty => ()\n _ => assert(false)\n }\n"
            } else {
                ""
            };
            let source = format!(
                "import std.channel\n type Envelope[T] = {{ values: Array[T] }}\n\
                 test buffered {{\n scope {{\n\
                 var (sender, receiver) = channel.bounded[Envelope[String]](2)?\n\
                 defer {{\n _ = receiver.close()\n }}\n {initial_empty}\
                 for text in [\"first\", \"é🦀\"] {{\n\
                   let item: Envelope[String] = Envelope {{ values: [text, \"tail\"] }}\n\
                   _ = sender.send(item)?\n }}\n\
                 for expected in [\"first\", \"é🦀\"] {{\n {}\
                   match result {{\n {item_pattern} => {{\n\
                     assert(item.values.get(0) == some(expected))\n\
                     assert(item.values.get(1) == some(\"tail\"))\n }}\n\
                     _ => assert(false)\n }}\n }}\n\
                 sender.close()\n {}\
                 match end {{\n {end_pattern} => ()\n _ => assert(false)\n }}\n\
                 }}\n }}\n",
                call("result"),
                call("end"),
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 65536.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            if method == "tryReceive" && spawned {
                // tryReceive is synchronous; spawning it must remain a static
                // error even though receive supports an outstanding Join.
                assert_eq!(output.status.code(), Some(1));
                assert!(output.stdout.is_empty());
                assert!(String::from_utf8_lossy(&output.stderr).contains("E1611"));
                fs::remove_dir_all(directory).unwrap();
                continue;
            }
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{method}/spawned={spawned}: {error}\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            assert_eq!(
                report.tests()[0].status,
                AggregateStatus::Passed,
                "{method}/spawned={spawned}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn rendezvous_results_preserve_generic_payloads_and_join_order() {
    for send_first in [false, true] {
        for await_send_first in [false, true] {
            let send = "let sent = spawn sender.send(item)\n";
            let receive = "let received = spawn receiver.receive()\n";
            let start = if send_first {
                format!("{send}{receive}")
            } else {
                format!("{receive}{send}")
            };
            let sent = "_ = await sent?\n";
            let received = "let result = await received\n";
            let finish = if await_send_first {
                format!("{sent}{received}")
            } else {
                format!("{received}{sent}")
            };
            let source = format!(
                "import std.channel\n type Envelope[T] = {{ values: Array[T] }}\n\
                 test rendezvous {{\n scope {{\n\
                 var (sender, receiver) = channel.bounded[Envelope[String]](0)?\n\
                 defer {{\n _ = receiver.close()\n }}\n\
                 for expected in [\"first\", \"é🦀\"] {{\n\
                   let item: Envelope[String] = Envelope {{ values: [expected, \"tail\"] }}\n\
                   {start}{finish}\
                   match result {{\n some(delivered) => {{\n\
                     assert(delivered.values.get(0) == some(expected))\n\
                     assert(delivered.values.get(1) == some(\"tail\"))\n }}\n\
                     none => assert(false)\n }}\n }}\n\
                 sender.close()\n match receiver.receive() {{\n\
                   none => ()\n some(_) => assert(false)\n }}\n }}\n }}\n"
            );
            let directory = test_project(source.as_bytes());
            rewrite_test_plan(&directory, |plan| {
                plan["limits"]["memory_bytes"] = 65536.into()
            });
            let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .current_dir(&directory)
                .args(["test", "--test-format", "json"])
                .output()
                .unwrap();
            let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "send_first={send_first}/await_send_first={await_send_first}: {error}\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn synchronous_channel_peers_preserve_generic_values_and_terminal_states() {
    for send in [false, true] {
        let start = if send {
            "let pending = spawn receiver.receive()\n"
        } else {
            "let pending = spawn sender.send(item)\n"
        };
        let operation = if send {
            "match sender.trySend(item) {\n ok(_) => ()\n err(_) => assert(false)\n }\n\
             let result = await pending\n"
        } else {
            "let result = receiver.tryReceive()\n"
        };
        let pattern = if send {
            "some(delivered)"
        } else {
            "channel.TryReceive.Item(delivered)"
        };
        let finish = if send {
            ""
        } else {
            "match await pending {\n ok(_) => ()\n err(_) => assert(false)\n }\n"
        };
        let source = format!(
            "import std.channel\n type Envelope[T] = {{ values: Array[T] }}\n\
             test peers {{\n scope {{\n\
             var (sender, receiver) = channel.bounded[Envelope[String]](0)?\n\
             defer {{\n _ = receiver.close()\n }}\n\
             match receiver.tryReceive() {{\n channel.TryReceive.Empty => ()\n _ => assert(false)\n }}\n\
             for expected in [\"first\", \"é🦀\"] {{\n\
               let item: Envelope[String] = Envelope {{ values: [expected, \"tail\"] }}\n\
               {start}{operation}\
               match result {{\n {pattern} => {{\n\
                 assert(delivered.values.get(0) == some(expected))\n\
                 assert(delivered.values.get(1) == some(\"tail\"))\n }}\n\
                 _ => assert(false)\n }}\n {finish} }}\n\
             sender.close()\n match receiver.tryReceive() {{\n\
               channel.TryReceive.Closed => ()\n _ => assert(false)\n }}\n }}\n }}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "send={send}: {error}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn console_public_errors_and_results_execute() {
    let source = source_file_with(
        br#"import std.console
import std.io

fn classify(failure: console.ConsoleError): Int {
    match failure {
        console.ConsoleError.Unavailable => 0
        console.ConsoleError.Closed => 1
        console.ConsoleError.Cancelled => 2
        console.ConsoleError.Io(io.IoError.Closed) => 3
        console.ConsoleError.Io(_) => 4
    }
}

fn main(): !console.ConsoleError {
    assert(classify(console.ConsoleError.Unavailable) == 0)
    assert(classify(console.ConsoleError.Closed) == 1)
    assert(classify(console.ConsoleError.Cancelled) == 2)
    assert(classify(console.ConsoleError.Io(io.IoError.Closed)) == 3)
    assert(classify(console.ConsoleError.Io(io.IoError.InvalidData)) == 4)
    var input: console.Input = console.stdin()?
    let line = console.readLine(var input)?
    assert(line == none)
    let output: console.Output = console.stdout()?
    let errors: console.Output = console.stderr()?
    console.print("first")?
    console.println("second")?
    console.flush()?
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"firstsecond\n");
    assert!(output.stderr.is_empty());
    fs::remove_file(source).unwrap();
}

#[test]
fn filesystem_errors_are_public_variants_and_preserve_host_categories() {
    let source = source_file_with(
        br#"import std.fs
import std.path

fn classify(error: fs.FsError): Int {
    match error {
        fs.FsError.NotFound => 0
        fs.FsError.PermissionDenied => 1
        fs.FsError.AlreadyExists => 2
        fs.FsError.InvalidPath => 3
        fs.FsError.NotDirectory => 4
        fs.FsError.IsDirectory => 5
        fs.FsError.Closed => 6
        fs.FsError.ResourceLimit => 7
        fs.FsError.Cancelled => 8
        fs.FsError.Io => 9
    }
}

fn classifyMode(mode: fs.OpenMode): Int {
    match mode {
        fs.OpenMode.Read => 0
        fs.OpenMode.Write => 1
        fs.OpenMode.ReadWrite => 2
        fs.OpenMode.Append => 3
        fs.OpenMode.Create => 4
        fs.OpenMode.CreateNew => 5
    }
}

fn main(): !path.PathError {
    assert(classifyMode(fs.OpenMode.Read) == 0)
    assert(classifyMode(fs.OpenMode.Write) == 1)
    assert(classifyMode(fs.OpenMode.ReadWrite) == 2)
    assert(classifyMode(fs.OpenMode.Append) == 3)
    assert(classifyMode(fs.OpenMode.Create) == 4)
    assert(classifyMode(fs.OpenMode.CreateNew) == 5)
    assert(classify(fs.FsError.NotFound) == 0)
    assert(classify(fs.FsError.PermissionDenied) == 1)
    assert(classify(fs.FsError.AlreadyExists) == 2)
    assert(classify(fs.FsError.InvalidPath) == 3)
    assert(classify(fs.FsError.NotDirectory) == 4)
    assert(classify(fs.FsError.IsDirectory) == 5)
    assert(classify(fs.FsError.Closed) == 6)
    assert(classify(fs.FsError.ResourceLimit) == 7)
    assert(classify(fs.FsError.Cancelled) == 8)
    assert(classify(fs.FsError.Io) == 9)
    let missing = path.Path.fromString("missing")?
    match fs.open(missing, fs.OpenMode.Read) {
        err(fs.FsError.NotFound) => {}
        _ => assert(false)
    }
    match fs.readAll(missing) {
        err(fs.FsError.NotFound) => {}
        _ => assert(false)
    }
    let existing = path.Path.fromString("file")?
    match fs.open(existing, fs.OpenMode.CreateNew) {
        err(fs.FsError.AlreadyExists) => {}
        _ => assert(false)
    }
    match fs.openDirectory(existing) {
        err(fs.FsError.NotDirectory) => {}
        _ => assert(false)
    }
    let directory = path.Path.fromString(".")?
    match fs.open(directory, fs.OpenMode.Read) {
        err(fs.FsError.IsDirectory) => {}
        _ => assert(false)
    }
}
"#,
    );
    let directory = source.with_extension("data");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("file"), b"preserved").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(directory.join("file")).unwrap(), b"preserved");
    fs::remove_dir_all(directory).unwrap();
    fs::remove_file(source).unwrap();
}

#[test]
fn filesystem_metadata_exposes_public_snapshot_values() {
    let source = source_file_with(
        br#"import std.fs
import std.path
import std.bytes

fn copy[T: Copy](value: T): T { value }
fn classify(kind: fs.FileKind): Int {
    match kind {
        fs.FileKind.File => 0
        fs.FileKind.Directory => 1
        fs.FileKind.Symlink => 2
        fs.FileKind.Other => 3
    }
}
fn main(): !(fs.FsError | path.PathError | bytes.BytesError) {
    let location = path.Path.fromString("file")?
    let info: fs.Metadata = fs.metadata(location)?
    let copied = copy(info)
    assert(classify(info.kind) == 0)
    assert(info.size == 3)
    assert(info.readOnly == false)
    let manual = fs.Metadata { kind: fs.FileKind.Other, size: 7, readOnly: true }
    let changed = manual with { size: 8 }
    assert(manual.size == 7)
    assert(changed.size == 8)
    assert(changed.readOnly)
    assert(classify(changed.kind) == 3)
    assert(classify(fs.FileKind.Symlink) == 2)
    let directory = fs.metadata(path.Path.fromString(".")?)?
    assert(classify(directory.kind) == 1)
    let readOnly = fs.metadata(path.Path.fromString("read-only")?)?
    assert(readOnly.readOnly)
    assert(readOnly.size == 4)
    fs.writeAll(location, bytes.Bytes("expanded")?)?
    let updated = fs.metadata(location)?
    assert(updated.size == 8)
    assert(info.size == 3)
    assert(copied.size == 3)
    fs.remove(location)?
    assert(info.kind == fs.FileKind.File)
    assert(copied.size == 3)
    match fs.metadata(location) {
        err(fs.FsError.NotFound) => {}
        _ => assert(false)
    }
}
"#,
    );
    let directory = source.with_extension("data");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("file"), b"abc").unwrap();
    let read_only = directory.join("read-only");
    fs::write(&read_only, b"data").unwrap();
    let original = fs::metadata(&read_only).unwrap().permissions();
    let mut permissions = original.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    fs::set_permissions(&read_only, original).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    for (body, code) in [
        ("_ = info.unknownField", "E1102"),
        ("info.size = 4", "E1411"),
        ("let size: Bool = info.size", "E1102"),
        (
            "_ = fs.Metadata { kind: fs.FileKind.File, size: 0 }",
            "E1102",
        ),
    ] {
        fs::write(
            &source,
            format!(
                "import std.fs\nfn main() {{\nlet info = fs.Metadata {{ kind: fs.FileKind.File, size: 0, readOnly: false }}\n{body}\n}}\n"
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["check", "--diagnostic-format", "json"])
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{body}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(code),
            "{body}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
    fs::remove_file(source).unwrap();
}

#[cfg(unix)]
#[test]
fn filesystem_metadata_observes_symlinks_and_other_entries_without_following_them() {
    let source = source_file_with(
        br#"import std.fs
import std.path
fn main(): !(fs.FsError | path.PathError) {
    let link = path.Path.fromString("link")?
    let before = fs.metadata(link)?
    assert(before.kind == fs.FileKind.Symlink)
    assert(before.size == 4)
    fs.remove(path.Path.fromString("file")?)?
    let dangling = fs.metadata(link)?
    assert(dangling.kind == fs.FileKind.Symlink)
    assert(dangling.size == before.size)
    let directoryLink = fs.metadata(path.Path.fromString("directory-link")?)?
    assert(directoryLink.kind == fs.FileKind.Symlink)
    let device = fs.metadata(path.Path.fromString("/dev/null")?)?
    assert(device.kind == fs.FileKind.Other)
}
"#,
    );
    let directory = source.with_extension("data");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("file"), b"abc").unwrap();
    std::os::unix::fs::symlink("file", directory.join("link")).unwrap();
    std::os::unix::fs::symlink(".", directory.join("directory-link")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    fs::remove_dir_all(directory).unwrap();
    fs::remove_file(source).unwrap();
}

#[test]
fn filesystem_handles_implement_static_io_protocols() {
    let source = source_file_with(
        br#"import std.fs
import std.path
import std.io
import std.bytes

fn next[R: io.Reader](reader: var R, max: Int): io.ReadResult ! io.IoError {
    reader.read(max)
}

fn emit[W: io.Writer](writer: var W, data: bytes.Bytes): Int ! io.IoError {
    let count = writer.write(data)?
    writer.flush()?
    count
}

fn main(): !(fs.FsError | path.PathError | io.IoError | bytes.BytesError) {
    let source = path.Path.fromString("input")?
    let destination = path.Path.fromString("output")?
    let data = bytes.Bytes("abcd")?
    var input = fs.open(source, fs.OpenMode.Read)?
    match next(var input, 0) {
        err(io.IoError.InvalidData) => {}
        _ => assert(false)
    }
    match next(var input, 2)? {
        io.ReadResult.Data(chunk) => assert(chunk.equal(bytes.Bytes("ab")?))
        io.ReadResult.Eof => assert(false)
    }
    let load = io.readAll[fs.File]
    assert(load(var input, io.limits(2, 1)?)?.equal(bytes.Bytes("cd")?))
    match next(var input, 1)? {
        io.ReadResult.Eof => {}
        _ => assert(false)
    }
    assert(io.readAll(var input, io.defaultLimits())?.length() == 0)
    match emit(var input, data) {
        err(io.IoError.Host) => {}
        _ => assert(false)
    }
    var output = fs.open(destination, fs.OpenMode.Create)?
    assert(emit(var output, data)? == 4)
    let save = io.writeAll[fs.File]
    save(var output, data)?
    match next(var output, 1) {
        err(io.IoError.Host) => {}
        _ => assert(false)
    }
    assert(fs.readAll(destination)?.equal(bytes.Bytes("abcdabcd")?))
    var limited = fs.open(source, fs.OpenMode.Read)?
    match io.readAll(var limited, io.limits(2, 2)?) {
        err(io.IoError.ResourceLimit) => {}
        _ => assert(false)
    }
}
"#,
    );
    let directory = source.with_extension("data");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("input"), b"abcd").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(directory.join("input")).unwrap(), b"abcd");
    assert_eq!(fs::read(directory.join("output")).unwrap(), b"abcdabcd");
    fs::remove_dir_all(directory).unwrap();
    fs::remove_file(source).unwrap();
}

#[test]
fn io_static_protocols_execute_user_implementations() {
    let source = source_file_with(
        br#"import std.io
import std.bytes

type Chunks = { done: Bool, payload: bytes.Bytes }
type Sink = { written: Int, flushed: Bool }

impl io.Reader for Chunks {
    fn read(var self, max: Int): io.ReadResult ! io.IoError suspends {
        if self.done {
            return io.ReadResult.Eof
        }
        self.done = true
        io.ReadResult.Data(self.payload)
    }
}

impl io.Writer for Sink {
    fn write(var self, data: bytes.Bytes): Int ! io.IoError suspends {
        self.written += data.length()
        data.length()
    }
    fn flush(var self): Unit ! io.IoError suspends {
        self.flushed = true
    }
}

fn count[R: io.Reader](reader: var R): Int ! io.IoError {
    match reader.read(4)? {
        io.ReadResult.Data(data) => data.length()
        io.ReadResult.Eof => 0
    }
}

fn emit[W: io.Writer](writer: var W, data: bytes.Bytes): Int ! io.IoError {
    let accepted = writer.write(data)?
    writer.flush()?
    accepted
}

fn main(): !(io.IoError | bytes.BytesError) {
    let data = bytes.Bytes("ok")?
    var reader = Chunks { done: false, payload: data }
    assert(count(var reader)? == 2)
    assert(count(var reader)? == 0)
    var writer = Sink { written: 0, flushed: false }
    assert(emit(var writer, data)? == 2)
    assert(writer.written == 2)
    assert(writer.flushed)
    reader.done = false
    let all = io.readAll(var reader, io.limits(2, 2)?)?
    assert(all.equal(data))
    writer.flushed = false
    io.writeAll(var writer, data)?
    assert(writer.written == 4)
    assert(writer.flushed)
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    fs::remove_file(source).unwrap();
}

#[test]
fn io_static_helpers_preserve_progress_errors_and_flush() {
    let source = source_file_with(
        br#"import std.io
import std.bytes

type Chunks = { mode: Int, calls: Int, payload: bytes.Bytes }
impl io.Reader for Chunks {
    fn read(var self, max: Int): io.ReadResult ! io.IoError suspends {
        self.calls += 1
        if self.mode == 0 {
            return io.ReadResult.Eof
        }
        if self.mode == 1 {
            let empty = match bytes.empty() {
                ok(value) => value
                err(_) => return err(io.IoError.ResourceLimit)
            }
            return io.ReadResult.Data(empty)
        }
        if self.mode == 3 {
            return err(io.IoError.Cancelled)
        }
        if self.mode == 4 and self.calls > 1 {
            return err(io.IoError.Host)
        }
        if self.mode == 5 and self.calls > 2 {
            return io.ReadResult.Eof
        }
        io.ReadResult.Data(self.payload)
    }
}

type Sink = { mode: Int, calls: Int, accepted: Int, flushed: Int }
impl io.Writer for Sink {
    fn write(var self, data: bytes.Bytes): Int ! io.IoError suspends {
        self.calls += 1
        if self.mode == 0 {
            return 0
        }
        if self.mode == 1 {
            return data.length() + 1
        }
        if self.mode == 2 {
            return err(io.IoError.Cancelled)
        }
        self.accepted += 1
        1
    }
    fn flush(var self): Unit ! io.IoError suspends {
        self.flushed += 1
        if self.mode == 3 {
            return err(io.IoError.Host)
        }
    }
}

fn main(): !(io.IoError | bytes.BytesError) {
    let data = bytes.Bytes("ab")?
    for (mode, maxBytes, maxRead, expected, calls) in [
        (1, 4, 2, io.IoError.InvalidData, 1),
        (2, 4, 1, io.IoError.InvalidData, 1),
        (2, 3, 2, io.IoError.ResourceLimit, 2),
        (3, 4, 2, io.IoError.Cancelled, 1),
        (4, 4, 2, io.IoError.Host, 2),
    ] {
        var reader = Chunks { mode, calls: 0, payload: data }
        match io.readAll(var reader, io.limits(maxBytes, maxRead)?) {
            err(error) => assert(error == expected)
            ok(_) => assert(false)
        }
        assert(reader.calls == calls)
    }
    for (mode, length, calls) in [(0, 0, 1), (5, 4, 3)] {
        var reader = Chunks { mode, calls: 0, payload: data }
        let contents = io.readAll(var reader, io.limits(4, 2)?)?
        assert(contents.length() == length)
        assert(reader.calls == calls)
    }
    for (mode, expected, calls, flushed) in [
        (0, io.IoError.InvalidData, 1, 0),
        (1, io.IoError.InvalidData, 1, 0),
        (2, io.IoError.Cancelled, 1, 0),
        (3, io.IoError.Host, 2, 1),
    ] {
        var writer = Sink { mode, calls: 0, accepted: 0, flushed: 0 }
        match io.writeAll(var writer, data) {
            err(error) => assert(error == expected)
            ok(_) => assert(false)
        }
        assert(writer.calls == calls)
        assert(writer.flushed == flushed)
    }
    for payload in [bytes.empty()?, data] {
        var writer = Sink { mode: 4, calls: 0, accepted: 0, flushed: 0 }
        io.writeAll(var writer, payload)?
        assert(writer.calls == payload.length())
        assert(writer.accepted == payload.length())
        assert(writer.flushed == 1)
    }
    for (maximum, request) in [(0, 1), (1, 0), (-1, 1), (1, -1)] {
        match io.limits(maximum, request) {
            err(io.IoError.ResourceLimit) => {}
            _ => assert(false)
        }
    }
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    fs::remove_file(source).unwrap();
}

#[test]
fn io_static_protocols_reject_value_traits_and_private_limit_construction() {
    for (body, expected) in [
        ("fn consume(value: io.Reader) {}\nfn main() {}\n", "E1110"),
        ("fn consume(value: io.Writer) {}\nfn main() {}\n", "E1110"),
        (
            "fn main() {\n _ = io.IoLimits { maxBytes: 0, maxRead: 0 }\n}\n",
            "private",
        ),
    ] {
        let source = source_file_with(format!("import std.io\n{body}").as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["check", "--diagnostic-format", "json"])
            .arg(&source)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{body}");
        let diagnostics = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostics.contains(expected), "{body}: {diagnostics}");
        assert!(!diagnostics.contains("E0004"), "{diagnostics}");
        fs::remove_file(source).unwrap();
    }
}

#[test]
fn channel_endpoint_results_preserve_generic_drain_and_fork_lifecycle() {
    for constructor in [
        "channel.bounded[Envelope[String]](2)",
        "channel.unbounded[Envelope[String]]()",
    ] {
        let source = format!(
            "import std.channel\n type Envelope[T] = {{ values: Array[T] }}\n\
             test endpoints {{\n\
             var (sender, receiver) = {constructor}?\n\
             var sending = sender.fork()?\n var receiving = receiver.fork()?\n\
             sender.close()\n\
             for text in [\"first\", \"é🦀\"] {{\n\
               let item: Envelope[String] = Envelope {{ values: [text, \"tail\"] }}\n\
               _ = sending.send(item)?\n }}\n\
             assert(receiver.close().length() == 0)\n\
             let remaining = receiving.close()\n assert(remaining.length() == 2)\n\
             for (index, expected) in [(0, \"first\"), (1, \"é🦀\")] {{\n\
               match remaining.get(index) {{\n some(delivered) => {{\n\
                 assert(delivered.values.get(0) == some(expected))\n\
                 assert(delivered.values.get(1) == some(\"tail\"))\n }}\n\
                 none => assert(false)\n }}\n }}\n\
             let rejected: Envelope[String] = Envelope {{ values: [\"closed\"] }}\n\
             match sending.trySend(rejected) {{\n\
               err(channel.TrySendError.Closed(returned)) => {{\n\
                 assert(returned.values.get(0) == some(\"closed\"))\n }}\n\
               _ => assert(false)\n }}\n sending.close()\n }}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{constructor}: {error}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn blocking_channel_results_preserve_generic_previews_forks_and_drain() {
    for constructor in [
        "channel.bounded[Envelope[String]](2)",
        "channel.unbounded[Envelope[String]]()",
    ] {
        let source = format!(
            "import std.channel\n import std.executor\n\
             type Envelope[T] = {{ values: Array[T] }}\n\
             fn work(): Array[Envelope[String]] ! (channel.ChannelError | channel.TrySendError[Envelope[String]]) {{\n\
               var (sender, receiver) = {constructor}?\n\
               var sending = sender.fork()?\n var receiving = receiver.fork()?\n\
               sender.close()\n\
               let first: Envelope[String] = Envelope {{ values: [\"first\", \"tail\"] }}\n\
               _ = sending.trySend(first)?\n\
               match receiver.tryReceive() {{\n\
                 channel.TryReceive.Item(delivered) => assert(delivered.values.get(0) == some(\"first\"))\n\
                 _ => assert(false)\n }}\n\
               let second: Envelope[String] = Envelope {{ values: [\"é🦀\", \"tail\"] }}\n\
               _ = sending.trySend(second)?\n\
               assert(receiver.close().length() == 0)\n\
               let remaining = receiving.close()\n sending.close()\n remaining\n }}\n\
             test worker {{\n\
               let pool = executor.blockingPool(1, 1)?\n defer pool.shutdown()\n\
               let result = pool.run(work)?\n assert(result.length() == 1)\n\
               match result.get(0) {{\n some(delivered) => {{\n\
                 assert(delivered.values.get(0) == some(\"é🦀\"))\n\
                 assert(delivered.values.get(1) == some(\"tail\"))\n }}\n\
                 none => assert(false)\n }}\n }}\n"
        );
        let directory = test_project_with_threads(source.as_bytes(), true);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 65536.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{constructor}: {error}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_channels_reserve_capacity_and_bound_retained_payloads() {
    let payload = "x".repeat(2048);
    let mut cases = Vec::new();
    for (capacity, expected) in [
        (2, AggregateStatus::Passed),
        (1024, AggregateStatus::ResourceLimit),
    ] {
        cases.push((
            format!(
                r#"match channel.bounded[String]({capacity}) {{
    ok((sender, receiver)) => {{
        sender.close()
        _ = receiver.close()
    }}
    err(_) => ()
}}
"#
            ),
            expected,
        ));
    }
    for constructor in ["channel.bounded[String](20)", "channel.unbounded[String]()"] {
        for (count, drain, expected) in [
            (1, false, AggregateStatus::Passed),
            (20, false, AggregateStatus::ResourceLimit),
            (20, true, AggregateStatus::Passed),
        ] {
            let receive = if drain { "_ = receiver.receive()" } else { "" };
            cases.push((
                format!(
                    r#"let text = "{payload}"
var (sender, receiver) = {constructor}?
defer sender.close()
defer {{
    _ = receiver.close()
}}
for index in 0..{count} {{
    let result = sender.send(text)
    match result {{
        ok(_) => ()
        err(_) => ()
    }}
    {receive}
}}
"#
                ),
                expected,
            ));
        }
    }
    for (body, expected) in cases {
        let source = format!(
            "import std.channel\nsuite outer {{\n let (parentSender, parentReceiver) = channel.bounded[String](1)?\n defer parentSender.close()\n defer {{\n assert(parentReceiver.receive() == some(\"parent\"))\n _ = parentReceiver.close()\n }}\n parentSender.send(\"parent\")?\n test first {{}}\n test middle {{\n {body}\n }}\n test zlast {{\n let (sender, receiver) = channel.bounded[String](1)?\n sender.send(\"survived\")?\n assert(receiver.receive() == some(\"survived\"))\n sender.close()\n _ = receiver.close()\n }}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "channel case {body}: {error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_text_diff_workspace_and_retained_results_share_memory() {
    let mut cases = Vec::new();
    for (lines, expected) in [
        (4, AggregateStatus::Passed),
        (128, AggregateStatus::ResourceLimit),
    ] {
        let left = serde_json::to_string(&"old\n".repeat(lines)).unwrap();
        let right = serde_json::to_string(&"new\n".repeat(lines)).unwrap();
        cases.push((
            format!(
                "let diff = testing.diffText({left}, {right})\n assert(diff.render().length() > 0)"
            ),
            expected,
        ));
    }
    // Ordinary nominal copies include the records and their hunk strings.
    // Keep the former two-large-diff input as an exhaustion case and exercise
    // both a single large record and multiple smaller retained records below
    // the same unchanged 32768-byte limit.
    for (count, retained, payload_bytes, expected) in [
        (1, true, 2048, AggregateStatus::Passed),
        (2, true, 512, AggregateStatus::Passed),
        (2, true, 2048, AggregateStatus::ResourceLimit),
        (16, true, 2048, AggregateStatus::ResourceLimit),
        (32, false, 2048, AggregateStatus::Passed),
    ] {
        let left = "x".repeat(payload_bytes);
        let right = "y".repeat(payload_bytes);
        let body = if retained {
            format!(
                r#"var held = [testing.diffText(left, right)]
for index in 1..{count} {{
    held.push(testing.diffText(left, right))?
}}
assert(held.length() == {count})
for diff in held {{
    assert(diff.render().length() > 0)
}}
"#
            )
        } else {
            format!(
                r#"for index in 0..{count} {{
    let diff = testing.diffText(left, right)
    assert(diff.render().length() > 0)
}}
"#
            )
        };
        cases.push((
            format!("let left = \"{left}\"\nlet right = \"{right}\"\n{body}"),
            expected,
        ));
    }
    let left = serde_json::to_string(&"old\n".repeat(128)).unwrap();
    let right = serde_json::to_string(&"new\n".repeat(128)).unwrap();
    cases.push((
        format!("testing.assertTextEqual({left}, {right})"),
        AggregateStatus::ResourceLimit,
    ));
    for (body, expected) in cases {
        let source = format!(
            "import std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n {body}\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_blocking_returns_admit_copies_and_reclaim_consumed_payloads() {
    for (bytes, count, expected) in [
        (1024, 16, AggregateStatus::Passed),
        (12000, 1, AggregateStatus::Passed),
        (12000, 8, AggregateStatus::Passed),
        (20000, 1, AggregateStatus::ResourceLimit),
    ] {
        let source = format!(
            "import std.executor\nfn work(): String ! executor.ExecutorError {{ \"{}\" }}\nsuite outer {{\n test first {{}}\n test middle {{\n let pool = executor.blockingPool(1, 1)?\n defer pool.shutdown()\n for index in 0..{count} {{\n let value = pool.run(work)?\n assert(value.length() == {bytes})\n }}\n }}\n test zlast {{}}\n}}\n",
            "x".repeat(bytes)
        );
        let directory = test_project_with_threads(source.as_bytes(), true);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "payload={bytes}, count={count}, project={}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_scoped_values_survive_defers_and_release_before_loop_transfers() {
    for body in [
        "for index in 0..8 {\n let value = pool.run(work)?\n defer { assert(value.length() == 12000)\n }\n calls += 1\n if index < 7 { continue\n }\n break\n }",
        "for index in 0..4 {\n {\n let value = pool.run(work)?\n defer { assert(value.length() == 12000)\n }\n calls += 1\n }\n let value = pool.run(work)?\n assert(value.length() == 12000)\n calls += 1\n }",
    ] {
        let source = format!(
            "import std.executor\nfn work(): String ! executor.ExecutorError {{ \"{}\" }}\nsuite outer {{\n test first {{}}\n test middle {{\n let pool = executor.blockingPool(1, 1)?\n defer pool.shutdown()\n var calls = 0\n {body}\n assert(calls == 8)\n }}\n test zlast {{}}\n}}\n",
            "x".repeat(12000)
        );
        let directory = test_project_with_threads(source.as_bytes(), true);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed; 3],
            "body={body}, project={}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert!(output.status.success());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_slices_admit_outputs_and_reclaim_iteration_buffers() {
    for (body, expected) in [
        (
            format!(
                "let value = \"{}\"\n for index in 0..16 {{\n let suffix = value[-4:]\n let reverse = value[-1:-5:-1]\n assert(suffix == \"aé🙂b\")\n assert(reverse == \"b🙂éa\")\n assert(value.byteLength() == 20000)\n }}",
                "aé🙂b".repeat(2500)
            ),
            AggregateStatus::Passed,
        ),
        (
            format!(
                "let value = \"{}\"\n for index in 0..16 {{\n let reverse = value[::-1]\n assert(reverse.byteLength() == 1024)\n }}",
                "aé🙂b".repeat(128)
            ),
            AggregateStatus::Passed,
        ),
        (
            format!(
                "let value = \"{}\"\n let reverse = value[::-1]\n assert(reverse.byteLength() == 20000)",
                "aé🙂b".repeat(2500)
            ),
            AggregateStatus::ResourceLimit,
        ),
        (
            "var values = [0, 1, 2, 3, 4, 5, 6, 7]\n for index in 0..16 {\n let reverse = values[::-2]\n assert(reverse == [7, 5, 3, 1])\n values[::2] = [0, 2, 4, 6]\n }\n assert(values == [0, 1, 2, 3, 4, 5, 6, 7])".into(),
            AggregateStatus::Passed,
        ),
    ] {
        let source = format!(
            "suite outer {{\n test first {{}}\n test middle {{\n {body}\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &[]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report.tests().iter().map(|test| test.status).collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "project={}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(output.status.code(), Some(i32::from(expected != AggregateStatus::Passed)));
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_place_validation_admits_map_key_snapshots_and_releases_them() {
    for (bytes, expected) in [
        (1024, AggregateStatus::Passed),
        (12000, AggregateStatus::ResourceLimit),
    ] {
        let source = format!(
            "suite outer {{\n test first {{}}\n test middle {{\n let left = \"{}\"\n let right = \"{}\"\n var values = [left: 1, right: 2]\n for index in 0..8 {{\n (values[left], values[right]) = (11, 4)\n }}\n assert(values[left] == some(11))\n assert(values[right] == some(4))\n }}\n test zlast {{}}\n}}\n",
            "x".repeat(bytes),
            "y".repeat(bytes)
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &[]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "payload={bytes}, project={}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_host_arguments_admit_all_detached_copies_before_dispatch() {
    for (bytes, deferred, expected) in [
        (1024, false, AggregateStatus::Passed),
        (1024, true, AggregateStatus::Passed),
        (12000, false, AggregateStatus::ResourceLimit),
        (12000, true, AggregateStatus::ResourceLimit),
    ] {
        let body = if deferred {
            "defer { testing.assertTextEqual(value, value)\n }"
        } else {
            "testing.assertTextEqual(value, value)"
        };
        let source = format!(
            "import std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n let value = \"{}\"\n {body}\n }}\n test zlast {{}}\n}}\n",
            "x".repeat(bytes)
        );
        let directory = test_project_with_capabilities(source.as_bytes(), &[]);
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "payload={bytes}, deferred={deferred}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_shrinking_admits_complete_candidates_before_publication() {
    for (value, count, expected) in [
        (
            format!("\"{}\"", "€".repeat(16)),
            17,
            AggregateStatus::Passed,
        ),
        (
            format!("\"{}\"", "€".repeat(256)),
            257,
            AggregateStatus::ResourceLimit,
        ),
        (
            format!("[{}]", ["8"; 4].join(", ")),
            21,
            AggregateStatus::Passed,
        ),
        (
            format!("[{}]", ["8"; 128].join(", ")),
            641,
            AggregateStatus::ResourceLimit,
        ),
    ] {
        let source = format!(
            "import std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n let value = {value}\n match testing.shrink(ref value) {{\n ok(candidates) => {{\n var count = 0\n for ref candidate in candidates {{\n count += 1\n }}\n assert(count == {count})\n }}\n err(_) => {{}}\n }}\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_generation_admits_output_and_reclaims_discarded_descriptors() {
    let mut cases = Vec::new();
    for operation in ["nextBytes", "nextText"] {
        for (maximum, expected) in [
            (64, AggregateStatus::Passed),
            (1048576, AggregateStatus::ResourceLimit),
        ] {
            cases.push((
                format!(
                    r#"var generator = testing.Generator.new(7)
match generator.{operation}({maximum}) {{
    ok(value) => assert(value.length() <= {maximum})
    err(_) => ()
}}
"#
                ),
                expected,
            ));
        }
    }
    cases.push((
        r#"for index in 0..1024 {
    var generator = testing.Generator.new(7)
    _ = generator.id()
    _ = generator.nextInt(-100, 100)?
    assert(generator.drawCount() > 0)
}
"#
        .into(),
        AggregateStatus::Passed,
    ));
    cases.push((
        r#"for index in 0..1024 {
    let tolerance = testing.FloatTolerance.from(0.0, 0.0)?
    testing.assertFloatNear(1.0, 1.0, ref tolerance)
}
"#
        .into(),
        AggregateStatus::Passed,
    ));
    for (body, expected) in cases {
        let source = format!(
            "import std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n {body}\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["memory_bytes"] = 32768.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [AggregateStatus::Passed, expected, AggregateStatus::Passed],
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        assert_eq!(
            output.status.code(),
            Some(i32::from(expected != AggregateStatus::Passed))
        );
        if expected == AggregateStatus::ResourceLimit {
            let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["tests"][1]["attempts"][0]["failure"]["code"], "memory");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_heap_exhaustion_is_reported_without_losing_sibling_evidence() {
    let directory = test_project(b"fn grow() {\n var text = \"0123456789\"\n for step in 0..20 {\n text = \"{text}{text}\"\n }\n}\nsuite outer {\n test first {}\n test middle { grow() }\n test zlast {}\n}\n");
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["memory_bytes"] = 4096.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        report
            .tests()
            .iter()
            .map(|test| test.status)
            .collect::<Vec<_>>(),
        [
            AggregateStatus::Passed,
            AggregateStatus::ResourceLimit,
            AggregateStatus::Passed
        ]
    );
    assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    assert_eq!(report.tests()[1].attempts.len(), 1);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["tests"][1]["attempts"][0]["failure"]["code"], "memory");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn testing_descriptor_output_and_attachment_counts_are_enforced_in_the_worker() {
    for (operation, output_limit, artifact_count) in [
        (
            "testing.attach(\"a\", \"text/plain\", bytes.Bytes(\"\")?)",
            81,
            0,
        ),
        ("testing.snapshot(\"a\", \"\")", 71, 0),
        (
            "for index in 0..257 {\n testing.attach(\"{index}\", \"text/plain\", bytes.Bytes(\"\")?)\n }",
            100_000,
            256,
        ),
    ] {
        let source = format!(
            "import std.bytes\nimport std.testing\nsuite outer {{\n test first {{}}\n test middle {{\n {operation}\n }}\n test zlast {{}}\n}}\n"
        );
        let directory = test_project(source.as_bytes());
        rewrite_test_plan(&directory, |plan| {
            plan["limits"]["output_bytes"] = output_limit.into()
        });
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--retry", "1", "--test-format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{operation}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{operation}: {error}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            [
                AggregateStatus::Passed,
                AggregateStatus::ResourceLimit,
                AggregateStatus::Passed
            ]
        );
        let attempt = &report.tests()[1].attempts[0];
        assert_eq!(report.tests()[1].attempts.len(), 1);
        assert_eq!(attempt.artifacts.len(), artifact_count);
        assert!(attempt.snapshots.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn phase_virtual_timer_limits_restore_the_clock_and_allow_released_slots() {
    let directory = test_project(
        br#"import std.testing
import std.time
suite outer {
 test first {
  testing.withVirtualTime((clock) {
   let first = time.Timer.after(time.Duration.fromNanoseconds(100))?
   defer first.cancel()
   let second = time.Timer.after(time.Duration.fromNanoseconds(100))?
   defer second.cancel()
   clock.settle()
  })?
 }
 test zlast {
  for index in 0..2 {
   testing.withVirtualTime((clock) {
    let first = time.Timer.after(time.Duration.fromNanoseconds(100))?
    first.cancel()
    let second = time.Timer.after(time.Duration.fromNanoseconds(100))?
    second.cancel()
    clock.settle()
   })?
  }
 }
}
"#,
    );
    rewrite_test_plan(&directory, |plan| {
        plan["limits"]["virtual_timers"] = 1.into()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        report
            .tests()
            .iter()
            .map(|test| test.status)
            .collect::<Vec<_>>(),
        [AggregateStatus::ResourceLimit, AggregateStatus::Passed]
    );
    assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["tests"][0]["attempts"][0]["failure"]["code"],
        "virtual-timers"
    );
    assert_eq!(
        json["tests"][1]["attempts"][0]["virtual_time"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn phase_output_allowances_are_independent_for_suite_setup_and_teardown() {
    let directory = test_project(b"import std.testing\nsuite outer {\n testing.log(\"abc\")\n defer { testing.log(\"def\") }\n test child {}\n}\n");
    rewrite_test_plan(&directory, |plan| plan["limits"]["output_bytes"] = 3.into());
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    assert_eq!(
        report.suites()[0].attempts[0]
            .logs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["abc", "def"]
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn phase_timeouts_preserve_siblings_setup_blocking_and_teardown_results() {
    for (source, phase, tests, expected) in [
        (
            "suite outer { test first {}\n test middle { _ = time.sleep(time.Duration.fromNanoseconds(300000000))\n }\n test zlast {}\n}\n",
            None,
            3,
            vec![
                AggregateStatus::Passed,
                AggregateStatus::Timeout,
                AggregateStatus::Passed,
            ],
        ),
        (
            "suite outer { _ = time.sleep(time.Duration.fromNanoseconds(300000000))\n test child {}\n}\n",
            Some("setup"),
            1,
            vec![AggregateStatus::BlockedSetup],
        ),
        (
            "suite outer { defer { _ = time.sleep(time.Duration.fromNanoseconds(300000000))\n }\n test child {}\n}\n",
            Some("teardown"),
            1,
            vec![AggregateStatus::Passed],
        ),
    ] {
        let directory = project_with_source_and_threads(b"fn main() {}\n");
        fs::create_dir_all(directory.join("tests")).unwrap();
        fs::write(
            directory.join("tests/phases.to"),
            format!("import std.time\n{source}"),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--timeout", "100ms", "--test-format", "json"])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{phase:?}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap();
        assert_eq!(report.tests().len(), tests);
        assert_eq!(
            report
                .tests()
                .iter()
                .map(|test| test.status)
                .collect::<Vec<_>>(),
            expected
        );
        if let Some(phase) = phase {
            assert_eq!(report.suites()[0].status, AggregateStatus::Timeout);
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["suites"][0]["attempts"][0]["phase"], phase);
        } else {
            assert_eq!(report.suites()[0].status, AggregateStatus::Passed);
        }
    }
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
fn phase_timeout_retry_keeps_complete_attempts_and_fresh_participations() {
    let directory = test_project(b"suite outer {\n test slow { for {}\n }\n test sibling {}\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--timeout",
            "20ms",
            "--retry",
            "1",
            "--test-format",
            "json",
        ])
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout).unwrap();
    let slow = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::slow"))
        .unwrap();
    assert_eq!(slow.status, AggregateStatus::Timeout);
    assert_eq!(slow.attempts.len(), 2);
    assert!(
        slow.attempts
            .iter()
            .all(|attempt| attempt.status == tondo_compiler::test_result::AttemptStatus::Timeout)
    );
    let sibling = report
        .tests()
        .iter()
        .find(|test| test.id.ends_with("::sibling"))
        .unwrap();
    assert_eq!(sibling.status, AggregateStatus::Passed);
    assert_eq!(sibling.attempts.len(), 1);
    assert_eq!(report.suites()[0].attempts.len(), 2);
}

#[test]
fn phase_retry_units_cover_suite_failures_and_independent_leaf_failures() {
    for (source, expected_suites, expected_kind) in [
        (
            "suite outer {\n test first { for {}\n }\n test second { for {}\n }\n}\n",
            3,
            "test",
        ),
        ("suite outer { for {}\n test child {}\n}\n", 2, "suite"),
        (
            "import std.time\nsuite outer { defer { _ = time.sleep(time.Duration.fromNanoseconds(100000000))\n }\n test child {}\n}\n",
            2,
            "suite",
        ),
    ] {
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args([
                "test",
                "--timeout",
                "20ms",
                "--retry",
                "1",
                "--test-format",
                "json",
            ])
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let report = TestReport::parse(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(report.suites()[0].attempts.len(), expected_suites);
        assert!(report.tests().iter().all(|test| test.attempts.len() == 2));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            value["retry"]["rounds"][0]["units"]
                .as_array()
                .unwrap()
                .iter()
                .all(|unit| unit["kind"] == expected_kind)
        );
    }
}

#[cfg(unix)]
#[test]
fn external_interrupt_drains_cleanup_and_preserves_complete_outputs() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    enum Delivery {
        Coordinator,
        Worker,
        Both,
    }
    use Delivery::{Both, Coordinator, Worker};

    for (body, ready_in_cleanup, cleanup_ns, second_request, expected_exit, delivery) in [
        ("for {}", false, 100000000_u64, false, 4, Coordinator),
        ("displayValue()", false, 100000000, false, 4, Coordinator),
        ("displayValue()", false, 100000000, false, 4, Worker),
        (
            "_ = time.sleep(time.Duration.fromNanoseconds(60000000000))",
            false,
            100000000,
            false,
            4,
            Coordinator,
        ),
        ("", true, 100000000, false, 4, Coordinator),
        ("", true, 60000000000, false, 3, Coordinator),
        ("", true, 60000000000, true, 3, Coordinator),
        ("for {}", false, 100000000, false, 4, Worker),
        ("", true, 60000000000, false, 3, Worker),
        // Forced-stop cases keep cleanup pending beyond the five-second test
        // deadline; launching the signal sender can itself take over 500 ms
        // on a loaded portable host. Successful cleanup retains its short wait.
        ("", true, 60000000000, true, 3, Worker),
        ("", true, 500000000, false, 4, Both),
        ("", true, 60000000000, true, 3, Both),
        (
            "scope {\n let pending = spawn childWork()\n _ = await pending\n}",
            false,
            100000000,
            false,
            4,
            Coordinator,
        ),
        (
            "_ = process.command(\"/bin/sh\", \"-c\", \"read unused && exit 77; printf %s $$ > process-pid; printf %s \\\"$PPID\\\" > ready; exec /bin/sleep 60\").run()",
            false,
            100000000,
            false,
            4,
            Coordinator,
        ),
        (
            "_ = process.command(\"/bin/sh\", \"-c\", \"setsid /bin/sleep 60 </dev/null >/dev/null 2>&1 & echo $! > descendant-pid; printf %s $$ > process-pid; echo ready > ready; exec /bin/sleep 60\").run()",
            false,
            100000000,
            false,
            4,
            Coordinator,
        ),
        (
            "_ = process.command(\"/bin/sh\", \"-c\", \"setsid /bin/sleep 60 </dev/null >/dev/null 2>&1 & echo $! > descendant-pid; printf %s $$ > process-pid; echo ready > ready; exec /bin/sleep 60\").run()",
            false,
            60000000000,
            false,
            3,
            Coordinator,
        ),
    ] {
        let process_case = body.contains("process.command");
        let cleanup_ready = if ready_in_cleanup {
            "mark(\"ready\")"
        } else {
            ""
        };
        let body_ready = if ready_in_cleanup
            || body.starts_with("scope")
            || body.contains("process.command")
            || body == "displayValue()"
        {
            ""
        } else {
            "mark(\"ready\")"
        };
        let process_import = if process_case {
            "import std.process\n"
        } else {
            ""
        };
        let display_helper = if body == "displayValue()" {
            "type Label = { value: Int }\nimpl Display for Label {\n fn display(self): String {\n defer testing.log(\"display-cleanup\")\n let ready = testing.assertOk(testing.tempDirectory(\"display-ready\"))\n defer ready.cleanup()\n for {}\n }\n}\nfn displayValue() {\n let value = Label { value: 1 }\n testing.assertNotEqual(ref value, ref value)\n}\n"
        } else {
            ""
        };
        let source = format!(
            r#"{process_import}import std.fs
import std.path
import std.bytes
import std.time
import std.testing
fn mark(name: String) {{
    let destination = match path.Path.fromString(name) {{
        ok(value) => value
        err(_) => panic("test marker path failed")
    }}
    let content = match bytes.Bytes("ready") {{
        ok(value) => value
        err(_) => panic("test marker bytes failed")
    }}
    match fs.writeAll(destination, content) {{
        ok(_) => ()
        err(_) => panic("test marker write failed")
    }}
}}
fn cleanup() {{
    {cleanup_ready}
    _ = time.sleep(time.Duration.fromNanoseconds({cleanup_ns}))
    mark("cleaned")
}}
{display_helper}
fn childWork() suspends {{
    defer mark("child-cleaned")
    mark("ready")
    for {{}}
}}
test active {{
    let temporary = testing.tempDirectory("interrupted")?
    defer temporary.cleanup()
    defer cleanup()
    testing.snapshot("value", "updated value")
    {body_ready}
    {body}
}}
test later {{ mark("later") }}
"#
        );
        let mut capabilities = vec!["console", "clock", "environment", "filesystem"];
        if process_case {
            capabilities.push("process");
        }
        let directory = test_project_with_capabilities(source.as_bytes(), &capabilities);
        rewrite_test_plan(&directory, |plan| {
            plan["snapshot_stores"] = serde_json::json!([{"name":"default", "path":"tests/snapshots.json", "update":true, "max_bytes":1048576}]);
        });
        let json = directory.join("results.json");
        let junit = directory.join("results.xml");
        fs::write(&json, b"prior json").unwrap();
        fs::write(&junit, b"prior junit").unwrap();
        let snapshots = fs::read(directory.join("tests/snapshots.json")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
        command.arg("test");
        #[cfg(target_os = "linux")]
        if process_case {
            command
                .arg("--process-cgroup")
                .arg(process_isolation_root());
        }
        let mut child = command
            .current_dir(&directory)
            .args([
                "--update-snapshots",
                "--report",
                "json=results.json",
                "--report",
                "junit=results.xml",
                "--artifacts",
                "artifacts",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if process_case && !cfg!(target_os = "linux") {
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code(), Some(3));
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("process isolation unavailable")
            );
            assert!(output.stdout.is_empty());
            assert!(!directory.join("ready").exists());
            assert!(!directory.join("target/.tondo-test-root").exists());
            assert_eq!(fs::read(&json).unwrap(), b"prior json");
            assert_eq!(fs::read(&junit).unwrap(), b"prior junit");
            assert_eq!(
                fs::read(directory.join("tests/snapshots.json")).unwrap(),
                snapshots
            );
            fs::remove_dir_all(directory).unwrap();
            continue;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut worker_pid = String::new();
        while Instant::now() < deadline {
            // Display is synchronous. Its temporary helper publishes readiness
            // without introducing suspendible filesystem calls into the trait.
            let ready = if body == "displayValue()" {
                fs::read_dir(directory.join("target/.tondo-test-root"))
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .filter_map(|worker| fs::read_dir(worker.path()).ok())
                    .flatten()
                    .filter_map(Result::ok)
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.starts_with("display-ready-"))
                    })
            } else {
                directory.join("ready").is_file()
            };
            if ready {
                // Observe the coordinator's direct worker from the OS. The
                // fixture needs no process capability merely to signal ready.
                let listing = Command::new("ps")
                    .args(["-axo", "pid=,ppid="])
                    .output()
                    .unwrap();
                assert!(listing.status.success());
                let listing = String::from_utf8(listing.stdout).unwrap();
                let children = listing
                    .lines()
                    .filter_map(|line| {
                        let mut fields = line.split_whitespace();
                        let pid = fields.next()?.parse::<u32>().ok()?;
                        let parent = fields.next()?.parse::<u32>().ok()?;
                        (parent == child.id()).then_some(pid)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(children.len(), 1, "coordinator must own one active worker");
                worker_pid = children[0].to_string();
                break;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let ready = !worker_pid.is_empty();
        let mut delivery_failures = Vec::new();
        if ready {
            let signal_pids = match delivery {
                Worker => vec![worker_pid.clone()],
                Coordinator => vec![child.id().to_string()],
                Both => vec![child.id().to_string(), worker_pid.clone()],
            };
            for request in 0..=usize::from(second_request) {
                if request > 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
                // Queue the same delivery at both endpoints before either handles it.
                // Otherwise the coordinator can reap the worker after its second
                // request before the test's next kill process sends that request.
                // Single-endpoint cases retain ordinary asynchronous delivery.
                let signals: &[&str] = if matches!(delivery, Both) {
                    &["-STOP", "-INT", "-CONT"]
                } else {
                    &["-INT"]
                };
                for signal in signals {
                    let sent = Command::new("kill")
                        .arg(signal)
                        .args(&signal_pids)
                        .output()
                        .unwrap();
                    if !sent.status.success() {
                        delivery_failures.push(format!(
                            "{delivery:?}, request={request}, signal={signal}, pids={signal_pids:?}: {}",
                            String::from_utf8_lossy(&sent.stderr)
                        ));
                    }
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        #[cfg(target_os = "linux")]
        if let Ok(pid) = fs::read_to_string(directory.join("descendant-pid")) {
            let pid = pid.trim();
            assert!(pid.parse::<u32>().is_ok_and(|pid| pid > 0));
            let stat = fs::read_to_string(format!("/proc/{pid}/stat"));
            let alive = stat
                .as_ref()
                .is_ok_and(|stat| !stat.rsplit_once(") ").unwrap().1.starts_with('Z'));
            if alive {
                let _ = Command::new("kill").args(["-KILL", pid]).status();
            }
            assert!(!alive, "descendant {pid} survived interruption");
        }
        assert!(
            delivery_failures.is_empty(),
            "signal delivery failed: {delivery_failures:?}; worker output: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            ready,
            "worker did not reach body in {directory:?}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{body}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            directory.join("cleaned").exists(),
            expected_exit == 4,
            "cleanup mismatch: delivery={delivery:?}, second_request={second_request}, cleanup_ns={cleanup_ns}, body={body}"
        );
        assert!(!worker_pid.is_empty());
        assert!(
            !Command::new("kill")
                .args(["-0", &worker_pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
            "worker {worker_pid} survived interruption"
        );
        assert!(!directory.join("later").exists());
        if body.starts_with("scope") {
            assert!(directory.join("child-cleaned").exists());
        }
        if body.contains("process.command") {
            let process_pid = fs::read_to_string(directory.join("process-pid")).unwrap();
            assert!(process_pid.parse::<u32>().is_ok_and(|pid| pid > 0));
            assert!(
                !Command::new("kill")
                    .args(["-0", &process_pid])
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "host process {process_pid} survived cancellation"
            );
        }
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interrupted"));
        assert_eq!(fs::read(&json).unwrap(), b"prior json");
        assert_eq!(fs::read(&junit).unwrap(), b"prior junit");
        assert_eq!(
            fs::read(directory.join("tests/snapshots.json")).unwrap(),
            snapshots
        );
        assert!(!directory.join("artifacts/manifests").exists());
        assert_eq!(
            fs::read_dir(directory.join("target/.tondo-test-root"))
                .unwrap()
                .count(),
            0,
            "worker temporary root survived interruption in {directory:?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }
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
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["__test-worker", "--entry", "missing"])
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
            .contains("closed worker input hash is required")
    );
}

#[test]
fn public_test_body_checks_reject_invalid_results_terminal_errors_and_suite_returns() {
    for (source, expected) in [
        ("test invalid { 42 }\n", "E1102"),
        ("test invalid {\n return 42\n}\n", "E1102"),
        (
            "suite invalid {\n if true {\n return\n }\n test hidden {}\n}\n",
            "E1205",
        ),
        (
            "fn work(): Int suspends { 1 }\ntest invalid {\n scope {\n fail spawn work()\n }\n}\n",
            "E1105",
        ),
    ] {
        let directory = test_project(source.as_bytes());
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(["test", "--list", "--allow-empty", "--filter", "missing"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn public_test_diagnostics_point_to_the_original_source_before_selection() {
    let source = "// café\nsuite outer {\n test invalid {\n  let answer: Int = \"wrong\"\n }\n}\n";
    let directory = test_project(source.as_bytes());
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args([
            "test",
            "--filter",
            "absent",
            "--allow-empty",
            "--diagnostic-format=json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let diagnostics: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let diagnostic = diagnostics
        .iter()
        .find(|item| item["code"] == "E1102")
        .unwrap_or_else(|| panic!("{stderr}"));
    let start = source.find("\"wrong\"").unwrap() as u64;
    assert_eq!(diagnostic["file"], "tests/smoke.to");
    assert_eq!(diagnostic["range"]["start"]["byte"], start);
    assert_eq!(diagnostic["range"]["end"]["byte"], start + 7);
    assert_eq!(diagnostic["range"]["start"]["line"], 3);
    assert_eq!(diagnostic["range"]["start"]["column"], 20);
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn public_tests_infer_recoverable_errors_and_preserve_cleanup_and_retry_evidence() {
    let directory = test_project(
        br#"
import std.testing
enum Failure { Raised(String) }
enum Other { Raised }
fn broken(): Unit ! Failure {
    fail Failure.Raised("private error payload")
}
test propagated {
    defer testing.log("leaf cleanup")
    broken()?
}
test direct {
    fail Other.Raised
}
test union {
    if false {
        fail Other.Raised
    }
    broken()?
}
test returned {
    return
}
suite unavailable {
    defer testing.log("setup cleanup")
    broken()?
    test blocked { assert(false) }
}
suite available {
    let nested = () {
        return
    }
    nested()
    test healthy { assert(true) }
}
suite enclosing {
    test childError {
        fail Other.Raised
    }
    test continuation { assert(true) }
}
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--retry", "1", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report = TestReport::parse(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let original_source = fs::read_to_string(directory.join("tests/smoke.to")).unwrap();
    for (name, error_name) in [
        ("propagated", "Failure"),
        ("direct", "Other"),
        ("union", "Failure"),
        ("childError", "Other"),
    ] {
        let test = report
            .tests()
            .iter()
            .find(|test| test.name == name)
            .unwrap();
        assert_eq!(test.attempts.len(), 2, "{name}");
        for attempt in &test.attempts {
            assert_eq!(
                attempt.status,
                tondo_compiler::test_result::AttemptStatus::FailedError
            );
            let failure = attempt.failure.as_ref().unwrap();
            assert_eq!(failure.kind, "error");
            assert_eq!(failure.code, None);
            assert!(
                failure.error_type.as_deref().unwrap().ends_with(error_name),
                "{failure:?}"
            );
            assert!(failure.stack.is_empty());
            let source = failure
                .source
                .as_ref()
                .expect("recoverable error retains its source terminal");
            assert_eq!(source.file, "tests/smoke.to");
            let terminal = &original_source[source.start as usize..source.end as usize];
            assert_eq!(
                terminal.trim(),
                if name == "direct" || name == "childError" {
                    "fail Other.Raised"
                } else {
                    "broken()?"
                }
            );
        }
    }
    let blocked = report
        .tests()
        .iter()
        .find(|test| test.name == "blocked")
        .unwrap();
    assert_eq!(blocked.attempts.len(), 2);
    assert!(blocked.attempts.iter().all(|attempt| attempt.status
        == tondo_compiler::test_result::AttemptStatus::BlockedSetup
        && attempt.failure.is_none()
        && attempt.blocked_by.is_some()));
    let suite = report
        .suites()
        .iter()
        .find(|suite| suite.name == "unavailable")
        .unwrap();
    assert!(suite.attempts.iter().all(|attempt| attempt.status
        == tondo_compiler::test_result::AttemptStatus::FailedError
        && attempt.phase == Some(tondo_compiler::test_result::AttemptPhase::Setup)));
    for name in ["returned", "healthy"] {
        let test = report
            .tests()
            .iter()
            .find(|test| test.name == name)
            .unwrap();
        assert_eq!(test.attempts.len(), 1);
        assert_eq!(
            test.attempts[0].status,
            tondo_compiler::test_result::AttemptStatus::Passed
        );
    }
    let serialized = wire.to_string();
    assert_eq!(serialized.matches("leaf cleanup").count(), 2);
    assert_eq!(serialized.matches("setup cleanup").count(), 2);
    assert!(!serialized.contains("private error payload"));
    assert!(!serialized.contains(&directory.display().to_string()));
    let repeated = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(&directory)
        .args(["test", "--repeat", "2", "--test-format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        repeated.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated = TestReport::parse(&repeated.stdout).unwrap();
    for test in repeated.tests() {
        assert_eq!(test.attempts.len(), 2);
        let initial = report
            .tests()
            .iter()
            .find(|node| node.id == test.id)
            .unwrap();
        for attempt in &test.attempts {
            assert_eq!(attempt.status, initial.attempts[0].status);
            assert_eq!(attempt.failure, initial.attempts[0].failure);
        }
    }
    fs::remove_dir_all(directory).unwrap();
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
        String::from_utf8_lossy(&output.stdout).contains("PANIC cli::integration::smoke::smoke")
    );
    assert!(String::from_utf8_lossy(&json_bytes).contains("tondo-test-report-0.1/8"));
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
        b"import std.console\nfn main(): !console.ConsoleError {\n    console.print(\"Hello\")?\n    console.print(\", Tondo!\")?\n}\n",
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

fn ordinary_meta_project() -> std::path::PathBuf {
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "tondo-meta-cli-{}-{}-{id}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for path in [
        "src/model",
        "tools/builder/src",
        "tools/helper/src",
        "inputs",
    ] {
        fs::create_dir_all(directory.join(path)).unwrap();
    }
    fs::write(
        directory.join("tondo.toml"),
        r#"[package]
name = "app"
[target]
capabilities = []
[meta.dependencies]
builder = { package = "workspace:builder@1", path = "tools/builder" }
[[meta.inputs]]
name = "schema"
path = "inputs/schema.txt"
[[meta.generators]]
id = "build-values"
provider = { package = "builder", entry = "main.generate" }
inputs = ["schema"]
model_roots = [{ package = "app", module = "model" }]
outputs = [{ logical_path = "generated/values.to", module = "generated" }]
limits = { steps = 100000, memory_bytes = 1048576, output_bytes = 8192 }
[[meta.derive_providers]]
trait = { package = "app", module = "model", name = "ValueOf" }
provider = { package = "builder", entry = "main.expand" }
limits = { steps = 100000, memory_bytes = 1048576, output_bytes = 8192 }
"#,
    )
    .unwrap();
    fs::write(directory.join("src/main.to"),b"import app.model\nimport app.generated\nfn main() {\n    assert(model.answer() == generated.number())\n}\n").unwrap();
    fs::write(directory.join("src/model/user.to"),b"pub trait ValueOf {\n    fn value(self): Int\n}\npub type User = { priv secret: Int }\nderive ValueOf for User\nfn extract[T: ValueOf + Discard](item: T): Int { item.value() }\npub fn answer(): Int { extract(User { secret: 42 }) }\n").unwrap();
    fs::write(directory.join("inputs/schema.txt"), b"*").unwrap();
    fs::write(
        directory.join("tools/builder/tondo.toml"),
        r#"[package]
name = "builder"
[dependencies]
helper = { package = "workspace:helper@1", path = "../helper" }
"#,
    )
    .unwrap();
    fs::write(
        directory.join("tools/helper/src/main.to"),
        b"pub fn answer(): Int { 42 }\n",
    )
    .unwrap();
    fs::write(directory.join("tools/builder/src/main.to"),br#"import std.meta
import helper.main as helper
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let input = request.input("schema")?
    assert(input.bytes.length() == 1)
    assert(input.bytes[0] == Byte(42u8))
    let declarations = request.snapshot().declarations
    assert(declarations.length() == 3)
    assert(declarations[0].identity == "User")
    assert(declarations[1].identity == "ValueOf")
    assert(declarations[2].identity == "answer")
    let value = helper.answer()
    ok(meta.GenerateResponse { outputs: [request.outputs()[0].path: "pub fn number(): Int {{\n    {value}\n}}\n"], diagnostics: [] })
}
pub fn expand(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    ok(meta.DeriveResponse { source: "impl {request.traitIdentity()} for {request.target()} {{\nfn value(self): Int {{ self.secret }}\n}}\n", diagnostics: [], mappings: [] })
}
"#).unwrap();
    directory
}

#[test]
fn meta_toml_lock_and_run_execute_generators_derives_and_transitive_sources() {
    let directory = ordinary_meta_project();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap()
    };
    let missing = invoke(&["run"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("tondo lock"));
    let lock = invoke(&["lock"]);
    assert!(
        lock.status.success(),
        "{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let bytes = fs::read(directory.join("tondo.lock.toml")).unwrap();
    let value: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(value["meta_packages"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["generators"][0]["provider_hash"]
            .as_str()
            .unwrap()
            .len(),
        71
    );
    let output = invoke(&[
        "run",
        "--emit-interface",
        "app.ti",
        "--emit-artifact",
        "app.ta",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact = BuildArtifact::decode(&fs::read(directory.join("app.ta")).unwrap()).unwrap();
    assert_eq!(
        artifact
            .generation()
            .iter()
            .filter(|record| record.kind == "generator")
            .count(),
        1
    );
    let lock = invoke(&["lock"]);
    assert!(
        lock.status.success(),
        "{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    assert_eq!(fs::read(directory.join("tondo.lock.toml")).unwrap(), bytes);
    assert!(!directory.join("generated/values.to").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn meta_toml_pins_inputs_sources_and_preserves_lock_on_failed_resolution() {
    let directory = ordinary_meta_project();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap()
    };
    let output = invoke(&["lock"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = fs::read(directory.join("tondo.lock.toml")).unwrap();
    for path in ["inputs/schema.txt", "tools/helper/src/main.to"] {
        let before = fs::read(directory.join(path)).unwrap();
        fs::write(directory.join(path), b"changed").unwrap();
        let output = invoke(&["run", "--emit-artifact", "unexpected.ta"]);
        assert!(!output.status.success(), "accepted changed {path}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("hash"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!directory.join("unexpected.ta").exists());
        assert_eq!(
            fs::read(directory.join("tondo.lock.toml")).unwrap(),
            original
        );
        fs::write(directory.join(path), before).unwrap();
    }
    fs::write(
        directory.join("tools/builder/src/main.to"),
        b"pub fn generate(request: Int): Int { request }\n",
    )
    .unwrap();
    let output = invoke(&["lock"]);
    assert!(!output.status.success());
    assert_eq!(
        fs::read(directory.join("tondo.lock.toml")).unwrap(),
        original
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn meta_toml_test_consumers_reuse_sealed_generated_production() {
    let directory = ordinary_meta_project();
    fs::create_dir(directory.join("tests")).unwrap();
    fs::write(directory.join("tests/generated.to"),b"import app.generated\nimport app.model\ntest generated {\n    assert(generated.number() == model.answer())\n}\n").unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap()
    };
    let output = invoke(&["lock"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = fs::read(directory.join("tondo.lock.toml")).unwrap();
    let output = invoke(&["test"]);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(directory.join("tondo.lock.toml")).unwrap(),
        original
    );
    fs::remove_dir_all(directory).unwrap();
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
