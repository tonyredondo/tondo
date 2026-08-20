use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_tondo-reliability")
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("reliability crate must belong to the workspace")
        .to_owned()
}

fn run(arguments: &[&str]) -> Output {
    Command::new(binary())
        .args(arguments)
        .output()
        .expect("reliability CLI must start")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("CLI output must be UTF-8")
}

struct TemporaryWorkspace(PathBuf);

impl TemporaryWorkspace {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must follow the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tondo-reliability-cli-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("testing")).unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(path.join("Cargo.lock"), "version = 4\n").unwrap();
        Self(path)
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn synthetic_coverage() -> serde_json::Value {
    let paths = [
        "crates/tondo-compiler/src/syntax/cst.rs",
        "crates/tondo-compiler/src/syntax/format/representative.rs",
        "crates/tondo-compiler/src/syntax/lexer.rs",
        "crates/tondo-compiler/src/syntax/parser.rs",
        "crates/tondo-compiler/src/hir/availability.rs",
        "crates/tondo-compiler/src/hir/capabilities.rs",
        "crates/tondo-compiler/src/hir/check.rs",
        "crates/tondo-compiler/src/hir/regions.rs",
        "crates/tondo-compiler/src/hir/terminal.rs",
        "crates/tondo-compiler/src/hir/traits.rs",
        "crates/tondo-compiler/src/resolve/representative.rs",
        "crates/tondo-compiler/src/types.rs",
        "crates/tondo-compiler/src/hir/verify.rs",
        "crates/tondo-compiler/src/mir/verify.rs",
        "crates/tondo-vm/src/bytecode/verify.rs",
        "crates/tondo-vm/src/runtime/heap.rs",
        "crates/tondo-vm/src/runtime/value.rs",
        "crates/tondo-compiler/src/bytecode/lower.rs",
        "crates/tondo-vm/src/runtime/execute.rs",
        "crates/tondo-compiler/src/artifact.rs",
        "crates/tondo-compiler/src/project.rs",
        "crates/tondo-conformance/src/representative.rs",
        "crates/tondo-reference-adapter/src/representative.rs",
        "crates/tondo-reliability/src/representative.rs",
    ];
    let files = paths
        .into_iter()
        .map(|filename| {
            serde_json::json!({
                "filename": format!("/workspace/{filename}"),
                "summary": {
                    "lines": {"count": 10, "covered": 9},
                    "functions": {"count": 5, "covered": 4},
                    "regions": {"count": 20, "covered": 17}
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "data": [{
            "totals": {
                "lines": {"count": 100, "covered": 90},
                "functions": {"count": 50, "covered": 40},
                "regions": {"count": 200, "covered": 170}
            },
            "files": files
        }]
    })
}

#[test]
fn repository_evidence_commands_are_readable_and_current_through_the_cli() {
    let root = workspace_root();
    let root = root.to_str().expect("workspace path must be UTF-8");
    for arguments in [
        vec!["quality", "check", "--root", root],
        vec!["inventory", "check", "--root", root],
        vec!["matrix", "check", "--root", root],
        vec!["tracker", "lint", "--root", root],
        vec!["check", "--root", root],
    ] {
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "`{arguments:?}` failed: {}",
            text(&output.stderr)
        );
        assert!(!text(&output.stdout).trim().is_empty());
    }

    let tracker_json = run(&["tracker", "lint", "--json", "--root", root]);
    assert!(
        tracker_json.status.success(),
        "{}",
        text(&tracker_json.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&tracker_json.stdout).unwrap();
    assert_eq!(report["format"], "tondo-tracker-graph/1");

    let ratchet = run(&["ratchet", "check", "--root", root]);
    assert!(!ratchet.status.success());
    assert!(text(&ratchet.stderr).contains("coverage report is required"));

    let workspace = TemporaryWorkspace::new();
    let before = workspace.0.join("before.json");
    let test_log = workspace.0.join("tests.log");
    let evidence = workspace.0.join("layer-evidence.json");
    let provenance = run(&["quality", "provenance", "--root", root]);
    assert!(provenance.status.success(), "{}", text(&provenance.stderr));
    fs::write(&before, provenance.stdout).unwrap();
    let lineage = DraftLineage::load(Path::new(root), DRAFT_LINEAGE_PATH).unwrap();
    let names = lineage
        .case_layers()
        .iter()
        .flat_map(|layer| layer.cases.iter())
        .flat_map(|case| case.evidence.iter())
        .map(|id| id.rsplit(':').next().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    fs::write(
        &test_log,
        names
            .iter()
            .map(|name| format!("test evidence::{name} ... ok\n"))
            .collect::<String>(),
    )
    .unwrap();
    let attestation = run(&[
        "layer-evidence",
        "attest",
        "--test-log",
        test_log.to_str().unwrap(),
        "--before",
        before.to_str().unwrap(),
        "--output",
        evidence.to_str().unwrap(),
        "--root",
        root,
    ]);
    assert!(
        attestation.status.success(),
        "{}",
        text(&attestation.stderr)
    );
    assert!(
        text(&attestation.stdout).contains(&format!("{} observations", names.len())),
        "{}",
        text(&attestation.stdout)
    );
}

#[test]
fn quality_capture_and_verify_are_reproducible_in_an_isolated_workspace() {
    let workspace = TemporaryWorkspace::new();
    let root = workspace.0.to_str().unwrap();
    let coverage = workspace.0.join("coverage.json");
    let mutants = workspace.0.join("mutants.json");
    fs::write(
        &coverage,
        serde_json::to_vec(&synthetic_coverage()).unwrap(),
    )
    .unwrap();
    fs::write(
        &mutants,
        serde_json::to_vec(&serde_json::json!({
            "outcomes": [
                {"outcome": "CaughtMutant", "mutant": {"name": "caught"}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let coverage = coverage.to_str().unwrap();
    let mutants = mutants.to_str().unwrap();

    let capture = run(&[
        "quality",
        "capture",
        "--coverage",
        coverage,
        "--mutants",
        mutants,
        "--revision",
        "test-revision",
        "--root",
        root,
    ]);
    assert!(capture.status.success(), "{}", text(&capture.stderr));
    assert!(text(&capture.stdout).contains("quality baseline updated"));
    let baseline = fs::read_to_string(workspace.0.join("testing/quality-baseline.json")).unwrap();
    assert!(baseline.ends_with('\n'));
    assert!(baseline.contains("\"revision\": \"test-revision\""));

    let before_coverage = workspace.0.join("coverage.before.json");
    let after_coverage = workspace.0.join("coverage.after.json");
    let before_mutation = workspace.0.join("mutation.before.json");
    let after_mutation = workspace.0.join("mutation.after.json");
    for path in [
        &before_coverage,
        &after_coverage,
        &before_mutation,
        &after_mutation,
    ] {
        let output = run(&["quality", "provenance", "--root", root]);
        assert!(output.status.success(), "{}", text(&output.stderr));
        fs::write(path, output.stdout).unwrap();
    }
    let coverage_binding = workspace.0.join("coverage.binding.json");
    let mutation_binding = workspace.0.join("mutation.binding.json");
    for (kind, report, before, after, output) in [
        (
            "coverage",
            coverage,
            &before_coverage,
            &after_coverage,
            &coverage_binding,
        ),
        (
            "mutation",
            mutants,
            &before_mutation,
            &after_mutation,
            &mutation_binding,
        ),
    ] {
        let output_path = run(&[
            "quality",
            "bind",
            "--kind",
            kind,
            "--report",
            report,
            "--before",
            before.to_str().unwrap(),
            "--after",
            after.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--root",
            root,
        ]);
        assert!(
            output_path.status.success(),
            "{}",
            text(&output_path.stderr)
        );
    }

    for arguments in [
        vec![
            "quality",
            "verify",
            "--coverage",
            coverage,
            "--coverage-binding",
            coverage_binding.to_str().unwrap(),
            "--root",
            root,
        ],
        vec![
            "quality",
            "verify",
            "--coverage",
            coverage,
            "--coverage-binding",
            coverage_binding.to_str().unwrap(),
            "--mutants",
            mutants,
            "--mutants-binding",
            mutation_binding.to_str().unwrap(),
            "--root",
            root,
        ],
        vec!["quality", "check", "--root", root],
    ] {
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "`{arguments:?}` failed: {}",
            text(&output.stderr)
        );
    }

    let second_capture = run(&[
        "quality",
        "capture",
        "--coverage",
        coverage,
        "--mutants",
        mutants,
        "--revision",
        "test-revision",
        "--root",
        root,
    ]);
    assert!(second_capture.status.success());
    assert!(text(&second_capture.stdout).contains("quality baseline unchanged"));
}

#[test]
fn invalid_commands_and_quality_option_leaks_have_stable_failures() {
    let root = workspace_root();
    let root = root.to_str().unwrap();
    for arguments in [
        vec![],
        vec!["unknown"],
        vec!["--unknown"],
        vec!["inventory", "check", "--coverage", "report.json"],
        vec!["quality", "check", "--coverage", "report.json"],
        vec!["quality", "capture", "--root", root],
        vec!["quality", "verify", "--root", root],
        vec![
            "quality",
            "verify",
            "--coverage",
            "missing.json",
            "--revision",
            "forbidden",
            "--root",
            root,
        ],
    ] {
        let output = run(&arguments);
        assert!(
            !output.status.success(),
            "`{arguments:?}` unexpectedly passed"
        );
        assert!(text(&output.stderr).starts_with("tondo-reliability:"));
    }
}
