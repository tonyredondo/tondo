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
        (3, "tondo: there is no standard module `std.nothing`; `tondo api --list` shows them")
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
