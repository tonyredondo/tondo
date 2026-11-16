//! The matrix of the ways a value can leave the scope whose region made it,
//! `tests/cases/run/regions/matrix.txt`: each statement and operation of the
//! MIR and each native that can be given a value a region makes has a row
//! that names the cases that run it, or `-` and why it can take no such
//! value, so that a new one fails here until its cases are written.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The variants of an enum of `source`, by name, as `Kind::Variant`.
fn variants(source: &str, name: &str) -> BTreeSet<String> {
    let start = source.find(&format!("pub enum {name} {{")).unwrap_or_else(|| panic!("no enum {name}"));
    source[start..]
        .lines()
        .skip(1)
        .take_while(|line| *line != "}")
        .filter(|line| line.starts_with("    ") && line[4..].starts_with(|c: char| c.is_ascii_uppercase()))
        .map(|line| {
            let name_end = line[4..].find(|c: char| !c.is_ascii_alphanumeric()).map_or(line.len(), |end| end + 4);
            format!("{name}::{}", &line[4..name_end])
        })
        .collect()
}

/// The symbols of a list of strings of `source`, `const NAME: &[&str] = &[...]`.
fn listed(source: &str, name: &str) -> BTreeSet<String> {
    let start = source.find(&format!("const {name}: &[&str] = &[")).unwrap_or_else(|| panic!("no list {name}"));
    let rest = &source[start..];
    let end = rest.find("];").unwrap();
    rest[..end].split('"').skip(1).step_by(2).map(String::from).collect()
}

#[test]
fn the_matrix_names_every_way_out() {
    let mir = read("crates/tondo/src/mir.rs");
    let regions = read("crates/tondo/src/regions.rs");
    let mut ways = variants(&mir, "Statement");
    ways.extend(variants(&mir, "Rvalue"));
    let mut natives = listed(&regions, "READING");
    natives.extend(listed(&regions, "KEEPING"));
    ways.extend(natives.iter().map(|symbol| format!("native {symbol}")));
    assert!(ways.len() > 100, "the lists were not read: {ways:?}");

    let matrix = read("tests/cases/run/regions/matrix.txt");
    let mut rows = BTreeSet::new();
    let mut named = BTreeSet::new();
    let mut problems = Vec::new();
    for line in matrix.lines().filter(|line| !line.trim().is_empty() && !line.starts_with('#')) {
        let (way, rest) = match line.strip_prefix("native ") {
            Some(rest) => {
                let (symbol, rest) = rest.trim_start().split_once(char::is_whitespace).unwrap_or((rest, ""));
                (format!("native {symbol}"), rest)
            }
            None => line.split_once(char::is_whitespace).map_or((line.to_string(), ""), |(w, r)| (w.to_string(), r)),
        };
        let rest = rest.trim();
        if !rows.insert(way.clone()) {
            problems.push(format!("{way} has two rows"));
        }
        if !ways.contains(&way) {
            problems.push(format!("{way} is no statement, operation or listed native of the compiler"));
        }
        match rest.strip_prefix('-') {
            Some(why) if why.trim().is_empty() => problems.push(format!("{way}: `-` says why it can take no value")),
            Some(_) => {}
            None if rest.is_empty() => problems.push(format!("{way} names no case")),
            None => {
                for case in rest.split_whitespace() {
                    if !root().join("tests").join(case).exists() {
                        problems.push(format!("{way}: tests/{case} does not exist"));
                    }
                    named.insert(case.to_string());
                }
            }
        }
    }
    for way in ways.difference(&rows) {
        problems.push(format!("{way} has no row: write the cases that run it, or `-` and why it takes no value"));
    }
    let dir = root().join("tests/cases/run/regions");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "to") {
            let case = format!("cases/run/regions/{}", path.file_name().unwrap().to_string_lossy());
            if !named.contains(&case) {
                problems.push(format!("tests/{case} is named by no row"));
            }
        }
    }
    assert!(problems.is_empty(), "tests/cases/run/regions/matrix.txt:\n{}", problems.join("\n"));
}
