//! The command line for people and for tools (toolchain 7 and 7.12): the
//! help of each command, completions for the shells, exit statuses, and
//! the JSON of each command.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// What a command printed as JSON, and its exit status.
fn json(dir: &Path, args: &[&str]) -> (Value, i32) {
    let (stdout, stderr, status) = tondo(dir, args);
    let value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}\n{stdout}\n{stderr}"));
    (value, status)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_command_has_its_help() {
    let dir = scratch("cli-help");
    let (_, usage, status) = tondo(&dir, &[]);
    assert_eq!(status, 2);
    assert!(usage.contains("usage:\n  tondo run ") && usage.contains("  tondo graph "), "{usage}");
    for name in tondo::cli::names() {
        let (help, stderr, status) = tondo(&dir, &[name, "--help"]);
        assert_eq!(status, 0, "{name} --help: {stderr}");
        assert!(help.starts_with(&format!("usage:\n  tondo {name}")), "{name}: {help}");
        assert_eq!(tondo(&dir, &["help", name]).0, help);
    }
    // A mistake shows the command's own help, with status 2.
    let (_, stderr, status) = tondo(&dir, &["fmt", "--nothing", "main.to"]);
    assert_eq!(status, 2);
    assert!(stderr.starts_with("tondo: fmt does not take --nothing\nusage:\n  tondo fmt "), "{stderr}");
    let (_, stderr, status) = tondo(&dir, &["help", "nothing"]);
    assert_eq!(status, 2, "{stderr}");
}

#[test]
fn completes_in_each_shell() {
    let dir = scratch("cli-completions");
    for shell in ["bash", "zsh", "fish"] {
        let (script, stderr, status) = tondo(&dir, &["completions", shell]);
        assert_eq!(status, 0, "{stderr}");
        for name in tondo::cli::names() {
            assert!(script.contains(name), "{shell} lacks {name}");
        }
        assert!(script.contains("test-format") && script.contains("mermaid"), "{shell}: {script}");
    }
    let (bash, _, _) = tondo(&dir, &["completions", "bash"]);
    assert!(bash.contains("complete -o default -F _tondo tondo"), "{bash}");
    let (_, _, status) = tondo(&dir, &["completions", "powershell"]);
    assert_eq!(status, 2);
    assert!(tondo::cli::flags("graph").contains(&"--reverse".to_string()));
}

#[test]
fn says_errors_as_json() {
    let dir = scratch("cli-errors");
    let (error, status) = json(&dir, &["fmt", "--json", "--nothing", "main.to"]);
    assert_eq!(status, 2);
    assert_eq!(error["format"], "tondo-error-0.1/1");
    assert_eq!((&error["ok"], &error["usage"], &error["status"]), (&json!(false), &json!(true), &json!(2)));
    let (error, status) = json(&dir, &["api", "std.nothing", "--json"]);
    assert_eq!(status, 3);
    assert_eq!(
        error,
        json!({
            "format": "tondo-error-0.1/1", "ok": false, "status": 3, "usage": false,
            "message": "there is no standard module `std.nothing`; `tondo api --list` shows them",
        })
    );
}

#[test]
fn commands_give_json() {
    let dir = scratch("cli-json");
    let (version, status) = json(&dir, &["version", "--json"]);
    assert_eq!(status, 0);
    assert_eq!((&version["format"], &version["backend"]), (&json!("tondo-version-0.1/1"), &json!("cranelift")));
    assert!(version["version"].as_str().unwrap().starts_with("0.1"), "{version}");

    let (topics, _) = json(&dir, &["guide", "--list", "--json"]);
    assert!(topics["topics"].as_array().unwrap().iter().any(|t| t == "pitfalls"), "{topics}");
    let (topic, _) = json(&dir, &["guide", "testing", "--json"]);
    assert!(topic["text"].as_str().unwrap().starts_with("# Testing"), "{topic}");

    std::fs::write(dir.join("main.to"), "import std.console\nfn main() {\n  console.println(\"hi\")\n}\n").unwrap();
    std::fs::write(dir.join("wrong.to"), "fn main() {\n    missing()\n}\n").unwrap();
    // Not formatted: says so, and changes nothing with --check.
    let (checked, status) = json(&dir, &["fmt", "--check", "--json", "main.to"]);
    assert_eq!(status, 1);
    assert_eq!(checked["files"], json!([{ "file": "main.to", "changed": true }]));
    let (formatted, status) = json(&dir, &["fmt", "--json", "main.to"]);
    assert_eq!((status, &formatted["ok"]), (0, &json!(true)));
    let (again, _) = json(&dir, &["fmt", "--json", "main.to"]);
    assert_eq!(again["files"], json!([{ "file": "main.to", "changed": false }]));

    let (built, status) = json(&dir, &["build", "main.to", "--output", "hello", "--json"]);
    assert_eq!((status, &built["format"], &built["output"]), (0, &json!("tondo-build-0.1/1"), &json!("hello")));
    assert_eq!(built["diagnostics"], json!([]));
    let (failed, status) = json(&dir, &["build", "wrong.to", "--json"]);
    assert_eq!((status, &failed["ok"], &failed["output"]), (1, &json!(false), &Value::Null));
    assert_eq!(failed["diagnostics"][0]["code"], "E1001");
    let (checked, status) = json(&dir, &["check", "main.to", "--json"]);
    assert_eq!((status, &checked["format"], &checked["diagnostics"]), (0, &json!("tondo-check-0.1/1"), &json!([])));

    // Diagnostics as JSON beside what the command does.
    let (_, stderr, status) = tondo(&dir, &["run", "wrong.to", "--diagnostic-format", "json"]);
    assert_eq!(status, 1);
    let diagnostics: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(diagnostics[0]["code"], "E1001");

    let (programs, status) = json(&dir, &["ps", "--json"]);
    assert_eq!((status, &programs["format"]), (0, &json!("tondo-ps-0.1/1")));
    assert!(programs["programs"].is_array());
}
