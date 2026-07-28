use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use tondo_compiler::artifact::{BuildArtifact, CompiledInterface, sha256};
use tondo_compiler::project::{
    BOOTSTRAP_STANDARD_PACKAGE, LOCKFILE_FORMAT, MANIFEST_FORMAT, bootstrap_standard_hash,
};

fn source_file_with(bytes: &[u8]) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tondo-cli-{}-{nonce}.to", std::process::id()));
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
        "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"tondo-capabilities/1\",\"capabilities\":[\"console\",\"process\"],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"src/main.to\",\"form\":\"module\"}},\"standard\":\"{BOOTSTRAP_STANDARD_PACKAGE}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"src/main.to\",\"logical_path\":\"src/main.to\",\"module\":\"main\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
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
    assert_eq!(artifact.interface_hash(), interface.content_hash().unwrap());
    assert!(artifact.reproducible());

    fs::remove_dir_all(directory).unwrap();
}
