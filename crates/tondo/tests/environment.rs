//! What the runtime reads of the environment (docs/refactor.md, phase 3):
//! every variable in `Config`, in config.rs, read once as a program starts,
//! and the table of the toolchain's specification (7.17) lists the same.
//! A read anywhere else is one of those known here, with why.

use std::collections::BTreeSet;
use std::path::Path;

/// The reads of the environment outside config.rs, by file, with why.
const KNOWN: &[(&str, &str, &str)] = &[
    ("zones.rs", "\"TZ\"", "the system's zone, read once as the program first asks"),
    ("diagnostics/dumps.rs", "\"XDG_CACHE_HOME\"", "the user's cache, where a crash's dump goes"),
    ("diagnostics/dumps.rs", "\"HOME\"", "the user's cache, where a crash's dump goes"),
    ("testing.rs", "(name)", "the environment a test changed, given back before the next"),
];

fn runtime() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tondo-runtime/src")
}

fn sources(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

/// The names of the variables config.rs reads, which its fields' comments
/// name as they start.
fn configured() -> BTreeSet<String> {
    let text = std::fs::read_to_string(runtime().join("config.rs")).unwrap();
    let documented: BTreeSet<String> = text
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("/// `TONDO_"))
        .map(|rest| format!("TONDO_{}", &rest[..rest.find('`').unwrap()]))
        .collect();
    let read: BTreeSet<String> =
        text.split("(\"TONDO_").skip(1).map(|rest| format!("TONDO_{}", &rest[..rest.find('"').unwrap()])).collect();
    assert_eq!(documented, read, "config.rs reads the variables its fields' comments name");
    read
}

#[test]
fn the_runtime_reads_the_environment_in_one_place() {
    let dir = runtime();
    let mut files = Vec::new();
    sources(&dir, &mut files);
    let mut found = Vec::new();
    for path in files {
        let file = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        if file == "config.rs" {
            continue;
        }
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            for call in ["env::var(", "env::var_os(", "getenv("] {
                if let Some(at) = line.find(call) {
                    let read = &line[at + call.len() - 1..];
                    let argument = &read[..read.find(')').map_or(read.len(), |end| end + 1)];
                    let argument = argument.trim_start_matches('(').trim_end_matches(')');
                    let known =
                        KNOWN.iter().any(|(f, a, _)| *f == file && (*a == argument || *a == format!("({argument})")));
                    if !known {
                        found.push(format!("{file}: {}", line.trim()));
                    }
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "the runtime reads the environment outside config.rs: {found:#?}\na variable of Tondo's is a field of \
         `Config`, read once as the program starts, with a row in the table of the toolchain's specification (7.17); \
         one that must be read where it is used is added to KNOWN, in this test, with why"
    );
}

#[test]
fn the_specification_lists_every_variable_the_runtime_reads() {
    let spec =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/TONDO_TOOLCHAIN_SPEC.md"))
            .unwrap();
    let section = &spec[spec.find("### 7.17").unwrap()..];
    let section = &section[..section.find("\n## ").unwrap()];
    let listed: BTreeSet<String> = section
        .lines()
        .filter_map(|line| line.strip_prefix("| `TONDO_"))
        .map(|rest| format!("TONDO_{}", &rest[..rest.find('`').unwrap()]))
        .collect();
    let configured = configured();
    assert!(!configured.is_empty());
    let missing: Vec<_> = configured.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(&configured).collect();
    assert!(missing.is_empty(), "the table of 7.17 does not list {missing:?}, which config.rs reads");
    assert!(extra.is_empty(), "the table of 7.17 lists {extra:?}, which config.rs does not read");
}
