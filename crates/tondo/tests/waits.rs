//! The tests wait for what says that something happened: a line the
//! program prints, an answer, or, for what only asking can see,
//! `common::until`. A test sleeps only where time is what is under test,
//! as that nothing happens meanwhile, and the comment above the sleep
//! starts by saying so.

use std::path::Path;

/// What a comment above a sleep starts with.
const UNDER_TEST: &str = "// Time is what is under test";

#[test]
fn the_tests_sleep_only_where_time_is_under_test() {
    // Spelled in two, so that this line is not one of them.
    let sleep = concat!("thread::", "sleep(");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
        for (at, line) in lines.iter().enumerate() {
            if !line.contains(sleep) || line.starts_with("//") {
                continue;
            }
            // The first line of the comment right above it.
            let said = lines[..at].iter().rev().take_while(|line| line.starts_with("//")).last();
            if !said.is_some_and(|said| said.starts_with(UNDER_TEST)) {
                found.push(format!("{}:{}", path.file_name().unwrap().to_string_lossy(), at + 1));
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
