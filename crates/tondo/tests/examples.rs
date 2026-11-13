//! The projects and scripts of `examples/`: each compiles without a warning,
//! is in the canonical format, and passes its own tests.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(args)
        .current_dir(dir)
        .env("TONDO_DUMP", "off")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    (said, output.status.success())
}

/// The Tondo files under a directory, but those a build wrote.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|extension| extension == "to") {
            out.push(path);
        }
    }
}

/// What is wrong with one example: a project, the packages of a directory
/// or its scripts.
fn problems(example: &Path) -> Vec<String> {
    if !example.join("tondo.toml").exists() {
        // A directory of packages, as a program and its plugins: each is a
        // project of its own.
        let mut packages: Vec<PathBuf> = std::fs::read_dir(example)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.join("tondo.toml").exists())
            .collect();
        packages.sort();
        if !packages.is_empty() {
            return packages.iter().flat_map(|package| problems(package)).collect();
        }
    }
    let name = example.file_name().unwrap().to_string_lossy().into_owned();
    let mut found = Vec::new();
    let mut files = Vec::new();
    sources(example, &mut files);
    for file in &files {
        let path = file.to_string_lossy().into_owned();
        let (said, ok) = tondo(example, &["fmt", "--check", &path]);
        if !ok {
            found.push(format!("{path} is not in the canonical format: `tondo fmt` writes it\n{said}"));
        }
    }
    if !example.join("tondo.toml").exists() {
        // A directory of scripts: each is a program of its own.
        for file in &files {
            let path = file.to_string_lossy().into_owned();
            let (said, ok) = tondo(example, &["check", &path]);
            if !ok || said.contains("warning[") {
                found.push(format!("`tondo check {path}`:\n{said}"));
            }
        }
        return found;
    }
    // A project that declares functions for C, or gives a plugin, is a
    // library.
    let library = files.iter().any(|file| {
        let text = std::fs::read_to_string(file).unwrap();
        text.contains("pub extern fn") || text.contains("@plugin.Export")
    });
    let flag: &[&str] = if library { &["--library"] } else { &[] };
    let (said, ok) = tondo(example, &[&["check"], flag].concat());
    if !ok || said.contains("warning[") {
        found.push(format!("`tondo check` of examples/{name}:\n{said}"));
        return found;
    }
    let (said, ok) = tondo(example, &[&["test"], flag].concat());
    if !ok {
        found.push(format!("`tondo test` of examples/{name}:\n{said}"));
    }
    found
}

#[test]
fn the_examples_are_checked_formatted_and_pass_their_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").canonicalize().unwrap();
    let mut examples: Vec<PathBuf> =
        std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().path()).filter(|path| path.is_dir()).collect();
    examples.sort();
    assert!(!examples.is_empty(), "there are no examples in {}", root.display());
    let found: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = examples.iter().map(|example| scope.spawn(move || problems(example))).collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect()
    });
    assert!(found.is_empty(), "{}", found.join("\n"));
}
