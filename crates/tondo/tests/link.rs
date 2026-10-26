//! Tondo's own linker (H12): executables it links run, and `--strip`
//! leaves out their symbol table.

#![cfg(target_os = "linux")]

use object::{Object, ObjectSymbol};
use std::path::Path;
use std::process::Command;

fn build(dir: &Path, name: &str, strip: bool) -> Vec<u8> {
    let output = dir.join(name);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
    command.args(["build", "main.to", "--output"]).arg(&output).current_dir(dir);
    if strip {
        command.arg("--strip");
    }
    let status = command.status().unwrap();
    assert!(status.success());
    let run = Command::new(&output).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "linked\n");
    std::fs::read(&output).unwrap()
}

#[test]
fn links_and_strips() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.to"), "import std.console\n\nconsole.println(\"linked\")\n").unwrap();
    let full = build(&dir, "full", false);
    let stripped = build(&dir, "stripped", true);
    let file = object::File::parse(full.as_slice()).unwrap();
    assert!(file.symbols().any(|s| s.name() == Ok("tondo_main")));
    let file = object::File::parse(stripped.as_slice()).unwrap();
    assert_eq!(file.symbols().count(), 0);
    assert!(stripped.len() < full.len());
}

/// What the compiler and the linker make does not depend on how their work
/// spreads over the cores, which differs from one build to the next.
#[test]
fn builds_the_same_bytes_each_time() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link_same");
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.json\nimport std.yaml\nimport std.regex\n\npub type Item = { name: String, tags: Array[String] }\n\nlet item = Item { name: \"x\", tags: [\"a\"] }\nlet text = json.encode(item)\nlet _ = yaml.encode(item)\nlet _ = regex.Regex.compile(\"[a-z]+\")\nif text.length() > 0 {\n    console.println(\"linked\")\n}\n";
    std::fs::write(dir.join("main.to"), program).unwrap();
    let first = build(&dir, "first", false);
    let second = build(&dir, "second", false);
    assert!(first == second, "two builds of the same program differ");
}
