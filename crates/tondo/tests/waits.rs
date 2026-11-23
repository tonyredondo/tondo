//! The tests wait for what says that something happened: a line the
//! program prints, an answer, or, for what only asking can see,
//! `common::until`. A test sleeps only where time is what is under test,
//! as that nothing happens meanwhile, and the comment above the sleep
//! starts by saying so. The tests of the compiler's and the runtime's code,
//! what `#[cfg(test)]` marks in their sources, are held to it too.

use std::path::{Path, PathBuf};

/// What a comment above a sleep starts with.
const UNDER_TEST: &str = "// Time is what is under test";

/// The files of a directory and those below it.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => files(&path, out),
            false if path.extension().is_some_and(|extension| extension == "rs") => out.push(path),
            false => {}
        }
    }
}

/// The lines of the items `#[cfg(test)]` marks, to the brace that closes
/// each.
fn tested(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if lines[at] == "#[cfg(test)]" {
            let (mut depth, mut opened) = (0i64, false);
            at += 1;
            while at < lines.len() {
                depth += lines[at].matches('{').count() as i64 - lines[at].matches('}').count() as i64;
                opened |= lines[at].contains('{');
                out.push(at);
                if opened && depth <= 0 {
                    break;
                }
                at += 1;
            }
        }
        at += 1;
    }
    out
}

#[test]
fn the_tests_sleep_only_where_time_is_under_test() {
    // Spelled in two, so that this line is not one of them.
    let sleep = concat!("thread::", "sleep(");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked: Vec<(PathBuf, bool)> = Vec::new();
    for entry in std::fs::read_dir(root.join("tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            checked.push((path, true));
        }
    }
    let mut sources = Vec::new();
    files(&root.join("src"), &mut sources);
    files(&root.join("../tondo-runtime/src"), &mut sources);
    checked.extend(sources.into_iter().map(|path| (path, false)));
    let mut found = Vec::new();
    for (path, whole) in checked {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
        let read = match whole {
            true => (0..lines.len()).collect(),
            false => tested(&lines),
        };
        for at in read {
            let line = lines[at];
            if !line.contains(sleep) || line.starts_with("//") {
                continue;
            }
            // The first line of the comment right above it.
            let said = lines[..at].iter().rev().take_while(|line| line.starts_with("//")).last();
            if !said.is_some_and(|said| said.starts_with(UNDER_TEST)) {
                let shown = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                found.push(format!("{shown}:{}", at + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a test sleeps at {found:?}: wait for what says it happened, a line the program prints, an answer, or \
         `common::until` for what only asking can see; where time is what is under test, the comment above the \
         sleep starts with {UNDER_TEST:?}"
    );
}

/// Every program a test starts, `tondo`, a program it built or a tool of
/// the system, starts through `common::command`, which keeps its files in
/// the tests' own cache: a crash's dump in the user's would prune the
/// newest of theirs, which the runtime keeps.
#[test]
fn the_tests_start_programs_only_through_common() {
    // Spelled in two, so that this line is not one of them.
    let started = concat!("Command::", "new(");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        for (number, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
            if line.contains(started) {
                found.push(format!("{}:{}", path.display(), number + 1));
            }
        }
    }
    assert!(found.is_empty(), "start these through `common::command`: {found:#?}");
}
