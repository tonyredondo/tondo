//! `tondo api` (toolchain 7.2) and `tondo fmt --compact` (7).

use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn shows_declarations_without_bodies() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("api-project");
    let _ = std::fs::remove_dir_all(&dir);
    write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n\n[dependencies]\nshapes = { path = \"libs/shapes\" }\n");
    write(dir.join("src/main.to"), "import app.models\n\nfn main() {}\n");
    write(
        dir.join("src/models/models.to"),
        r#"import std.io as stream

/// A person.
pub type User = {
    /// What they are called.
    name: String
    priv secret: String
}

pub fn User.new(name: String): User {
    User { name, secret: "" }
}

/// Writes the user.
pub fn save(user: User, out: stream.Writer): Unit ! stream.IoError {
    out.writeLine(user.name)?
}

fn helper(): Int {
    1
}

/// Things that have a size.
pub trait Sized {
    /// How big.
    fn size(self): Int
}

impl Sized for User {
    fn size(self): Int {
        self.name.length()
    }
}

test helps {
    assert helper() == 1
}
"#,
    );
    write(dir.join("libs/shapes/tondo.toml"), "[package]\nname = \"shapes\"\n");
    write(dir.join("libs/shapes/src/shapes.to"), "/// A circle.\npub type Circle = { radius: Float }\n");

    let (stdout, stderr, status) = tondo(&dir, &["api", "app.models"]);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(
        stdout,
        "\
/// A person.
pub type User = {
    /// What they are called.
    name: String
    // private fields
}

pub fn User.new(name: String): User

/// Writes the user.
pub fn save(user: User, out: io.Writer): Unit ! io.IoError

/// Things that have a size.
pub trait Sized {
    /// How big.
    fn size(self): Int
}

impl Sized for User
"
    );
    let (stdout, _, _) = tondo(&dir, &["api", "--private", "app.models"]);
    assert!(stdout.contains("priv secret: String") && stdout.contains("fn helper(): Int"), "{stdout}");
    assert!(!stdout.contains("helps"), "{stdout}");
    let (stdout, _, _) = tondo(&dir, &["api"]);
    assert!(stdout.starts_with("// module app.models\n\n/// A person."), "{stdout}");
    let (stdout, _, _) = tondo(&dir, &["api", "shapes"]);
    assert_eq!(stdout, "/// A circle.\npub type Circle = {\n    radius: Float\n}\n");
    let (stdout, _, _) = tondo(&dir, &["api", "--list"]);
    assert!(
        stdout.contains("std.http\n") && stdout.contains("prelude\n") && stdout.contains("app.models\n"),
        "{stdout}"
    );
    let (stdout, _, _) = tondo(&dir, &["api", "std.fs"]);
    assert!(stdout.contains("pub fn readText(path: String): String ! FsError"), "{stdout}");
    let (stdout, _, _) = tondo(&dir, &["api", "std.csv"]);
    assert!(stdout.contains("pub fn rows(input: io.Reader, separator: Char = ','): Rows"), "{stdout}");
    // As data: each declaration's kind, name, text and documentation.
    let json = |args: &[&str]| -> serde_json::Value {
        let (stdout, stderr, status) = tondo(&dir, args);
        assert_eq!(status, 0, "{stderr}");
        serde_json::from_str(&stdout).unwrap()
    };
    let models = json(&["api", "app.models", "--json"]);
    assert_eq!((&models["format"], &models["module"]), (&"tondo-api-0.1/1".into(), &"app.models".into()));
    let declarations = models["declarations"].as_array().unwrap();
    let named: Vec<(&str, &str)> =
        declarations.iter().map(|d| (d["kind"].as_str().unwrap(), d["name"].as_str().unwrap())).collect();
    assert_eq!(
        named,
        [("type", "User"), ("fn", "User.new"), ("fn", "save"), ("trait", "Sized"), ("impl", "Sized for User")]
    );
    assert_eq!(declarations[0]["doc"], "A person.");
    assert_eq!(declarations[2]["declaration"], "pub fn save(user: User, out: io.Writer): Unit ! io.IoError");
    assert_eq!(declarations[3]["declaration"], "pub trait Sized {\n    /// How big.\n    fn size(self): Int\n}");
    let whole = json(&["api", "--json"]);
    assert_eq!(whole["modules"][0]["module"], "app.models");
    let list = json(&["api", "--list", "--json"]);
    let list: Vec<&str> = list["modules"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
    assert!(list.contains(&"std.http") && list.contains(&"app.models"), "{list:?}");
    let prelude = json(&["api", "prelude", "--json"]);
    let names: Vec<&str> =
        prelude["declarations"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Option.map") && names.contains(&"Iterator[T] for ArrayIterator[T]"), "{names:?}");
    let (_, stderr, status) = tondo(&dir, &["api", "std.nothing"]);
    assert_eq!(
        (status, stderr.trim()),
        (
            3,
            "tondo: there is no module or declaration `std.nothing`; `tondo api --list` shows the modules, and `tondo api --search <words>` finds declarations"
        )
    );
}

#[test]
fn prints_files_compactly() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("api-compact");
    let _ = std::fs::remove_dir_all(&dir);
    write(
        dir.join("main.to"),
        "import std.console\n\n// Says hello.\nfn main() {\n    let total = 1 +\n        2\n    console.println(\"{total}\")\n}\n",
    );
    let (stdout, stderr, status) = tondo(&dir, &["fmt", "--compact", "main.to"]);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(
        stdout,
        "import std.console\n// Says hello.\nfn main(){let total=1+2\nconsole.println(\"{total}\")\n}\n"
    );
    write(dir.join("again.to"), &stdout);
    let (_, _, status) = tondo(&dir, &["fmt", "again.to"]);
    assert_eq!(status, 0);
    let formatted = std::fs::read_to_string(dir.join("again.to")).unwrap();
    assert_eq!(
        formatted,
        "import std.console\n\n// Says hello.\nfn main() {\n    let total = 1 + 2\n    console.println(\"{total}\")\n}\n"
    );
}

/// The text of a comment line: what follows `//` or `///` and a space.
fn comment(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("///").or(trimmed.strip_prefix("//")).map(|l| l.strip_prefix(' ').unwrap_or(l))
}

/// The examples the standard library's comments carry, a `tondo` block in
/// a module's header or a declaration's `///`: each, with its file and
/// the line it starts at.
fn standard_examples() -> Vec<(String, usize, String)> {
    let files = tondo::stdlib::sources().iter().copied().chain(tondo::api::intrinsic_files());
    let mut found = Vec::new();
    for (name, text) in files {
        let lines: Vec<&str> = text.lines().collect();
        let mut at = 0;
        while at < lines.len() {
            if comment(lines[at]).is_some_and(|l| l.trim() == "```tondo") {
                let start = at + 1;
                let mut code = String::new();
                at += 1;
                while at < lines.len() && comment(lines[at]).is_some_and(|l| l.trim() != "```") {
                    code.push_str(comment(lines[at]).unwrap_or_default());
                    code.push('\n');
                    at += 1;
                }
                found.push((name.to_string(), start, code));
            }
            at += 1;
        }
    }
    found
}

/// Every public type of the standard library shows how it is used: an
/// example in its `///`, which `standard_examples_compile` compiles.
#[test]
fn every_standard_type_has_an_example() {
    let mut missing = Vec::new();
    for module in tondo::api::standard_modules() {
        let Some(text) = tondo::api::standard(&module) else { continue };
        for declaration in tondo::api::declarations(&text) {
            let kind = declaration["kind"].as_str().unwrap_or_default();
            let doc = declaration["doc"].as_str().unwrap_or_default();
            if ["type", "enum", "trait", "alias", "context"].contains(&kind) && !doc.contains("```tondo") {
                missing.push(format!("{module}.{}", declaration["name"].as_str().unwrap_or_default()));
            }
        }
    }
    assert!(missing.is_empty(), "these types have no example in their `///`: {missing:?}");
}

/// Every example of the standard library's documentation compiles, and
/// each module that a program imports has one at its head, which `tondo
/// api` and an editor's hover show.
#[test]
fn standard_examples_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("std-examples");
    let _ = std::fs::remove_dir_all(&dir);
    let examples = standard_examples();
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = examples
            .chunks(8)
            .enumerate()
            .map(|(chunk, examples)| {
                let dir = dir.clone();
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for (index, (file, line, code)) in examples.iter().enumerate() {
                        let case = dir.join(format!("{chunk}-{index}"));
                        // An example of tests is a test file of a project.
                        let (_, stderr, status) = match code.lines().any(|l| l.starts_with("test ")) {
                            true => {
                                write(case.join("tondo.toml"), "[package]\nname = \"example\"\n");
                                write(case.join("src/main.to"), "fn main() {}\n");
                                write(case.join("src/main_test.to"), code);
                                tondo(&case, &["test", "--list"])
                            }
                            false => {
                                write(case.join("main.to"), code);
                                tondo(&case, &["check", "main.to"])
                            }
                        };
                        if status != 0 {
                            failures.push(format!("std/{file}.to:{line}:\n{code}\n{stderr}"));
                        }
                    }
                    failures
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let missing: Vec<String> = tondo::api::standard_modules()
        .into_iter()
        .filter(|module| module != "prelude" && tondo::api::example(module).is_none())
        .collect();
    assert!(missing.is_empty(), "these modules have no example at their head: {missing:?}");
}

/// The warnings of the standard library are dropped, so nobody would see them
/// come back: it has none, but those it accepts and says why.
#[test]
fn the_standard_library_has_no_warnings() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("std-warnings");
    std::fs::create_dir_all(&dir).unwrap();
    let (modules, stderr, status) = tondo(&dir, &["api", "--list"]);
    assert_eq!(status, 0, "{stderr}");
    let names = modules.lines().filter_map(|line| line.split_whitespace().next());
    let imports: String = names
        .map(|name| name.trim_start_matches("std."))
        .filter(|name| !matches!(*name, "prelude" | "testing"))
        .map(|name| format!("import std.{name}\n"))
        .collect();
    std::fs::write(dir.join("all.to"), format!("{imports}\nfn main() {{}}\n")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "all.to", "--json"])
        .env("TONDO_STD_WARNINGS", "1")
        .current_dir(&dir)
        .output()
        .unwrap();
    let found: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let diagnostics = found["diagnostics"].as_array().unwrap();
    assert!(found["ok"] == true && diagnostics.len() > 10, "{found}");
    let own: Vec<_> = diagnostics.iter().filter(|d| d["file"].as_str().unwrap().starts_with("<std>")).collect();
    assert!(own.is_empty(), "{own:#?}");
}

/// The public API of the standard library, every declaration of every
/// module, as recorded in `tests/snapshots/std-api.txt`: a change to it, as
/// the refactoring's moves could make by mistake, shows here
/// (docs/refactor.md, phase 0.5). `TONDO_UPDATE_API=1` records it again, on
/// purpose.
#[test]
fn the_standard_api_is_as_recorded() {
    let dir = std::env::temp_dir();
    let (listed, _, _) = tondo(&dir, &["api", "--list", "--json"]);
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let mut text = String::new();
    for module in listed["modules"].as_array().unwrap().iter().filter_map(|m| m.as_str()) {
        if module != "prelude" && !module.starts_with("std.") {
            continue;
        }
        let (shown, stderr, status) = tondo(&dir, &["api", module, "--json"]);
        assert_eq!(status, 0, "{module}: {stderr}");
        let shown: serde_json::Value = serde_json::from_str(&shown).unwrap();
        let mut declarations: Vec<&str> =
            shown["declarations"].as_array().unwrap().iter().filter_map(|d| d["declaration"].as_str()).collect();
        declarations.sort();
        text.push_str(&format!("== {module}\n"));
        for declaration in declarations {
            text.push_str(declaration);
            text.push('\n');
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/std-api.txt");
    if std::env::var_os("TONDO_UPDATE_API").is_some() {
        write(path, &text);
        return;
    }
    let recorded = std::fs::read_to_string(&path).unwrap_or_default();
    if recorded != text {
        let before: std::collections::BTreeSet<&str> = recorded.lines().collect();
        let after: std::collections::BTreeSet<&str> = text.lines().collect();
        let gone: Vec<&&str> = before.difference(&after).take(20).collect();
        let new: Vec<&&str> = after.difference(&before).take(20).collect();
        panic!(
            "the standard library's API changed; if on purpose, run with TONDO_UPDATE_API=1\ngone: {gone:#?}\nnew: {new:#?}"
        );
    }
}
