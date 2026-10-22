//! `derive` with providers (27.2 to 27.7 of the language): a package names
//! the provider of its trait, and a program that depends on it derives the
//! trait for its types.

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

/// A program with `main`, and the package `show` it depends on: its traits
/// and their providers.
fn project(name: &str, main: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n\n[dependencies]\nshow = { path = \"libs/show\" }\n");
    write(dir.join("src/main.to"), main);
    let show = dir.join("libs/show");
    write(
        show.join("tondo.toml"),
        "[package]\nname = \"show\"\n\n[derive]\nShow = \"derive/show.to\"\nLoop = \"derive/loop.to\"\nFiles = \"derive/files.to\"\nExtra = \"derive/extra.to\"\n",
    );
    write(
        show.join("src/show.to"),
        "pub trait Show {\n    fn show(self): String\n}\n\npub trait Plain {\n    fn plain(self): String\n}\n\npub trait Loop {\n    fn spin(self): Int\n}\n\npub trait Files {\n    fn read(self): String\n}\n\npub trait Extra {\n    fn extra(self): Int\n}\n",
    );
    write(
        show.join("derive/show.to"),
        r#"import std.meta

/// Writes `impl Show` for a record: its name and fields.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    if target.kind() != meta.DeclarationKind.Record {
        out.error("`Show` derives only for records, and `{target.name()}` is not one")
        return out.finishDerive()?
    }
    var parts: Array[String] = []
    for field in target.fields() {
        parts.push(meta.code(r"$name: {self.$name}", ["name": field.name]))
    }
    var binders: Array[String] = []
    for generic in target.generics() {
        binders.push("{generic.name}: Display")
    }
    let header = if binders.length() == 0 { "" } else { "[{binders.join(", ")}]" }
    out.add(meta.code(r"""
        impl$binders $trait for $type {
            fn show(self): String {
                "$name {{ $fields }}"
            }
        }
        """, [
        "binders": header,
        "trait": request.traitName(),
        "type": target.typeName(),
        "name": target.name(),
        "fields": parts.join(", "),
    ]))?
    out.finishDerive()
}
"#,
    );
    write(
        show.join("derive/loop.to"),
        "import std.meta\n\npub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {\n    var n = 0\n    for {\n        n += 1\n    }\n}\n",
    );
    write(
        show.join("derive/files.to"),
        "import std.meta\nimport std.fs\n\npub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {\n    var out = request.sourceBuilder()\n    out.add(fs.readText(\"secret\")?)?\n    out.finishDerive()\n}\n",
    );
    write(
        show.join("derive/extra.to"),
        "import std.meta\n\npub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {\n    var out = request.sourceBuilder()\n    out.add(\"impl {request.traitName()} for {request.target().typeName()} {{\\n    fn extra(self): Int {{ 1 }}\\n}}\\n\\nfn helper() {{}}\\n\")?\n    out.finishDerive()\n}\n",
    );
    dir
}

#[test]
fn derives_with_a_provider() {
    let main = "\
import std.console
import show

pub type Point = { x: Int, y: Int }
derive show.Show for Point

pub type Page[T] = { number: Int, item: T }
derive[T] show.Show for Page[T]

fn main() {
    console.println(show.Show.show(Point { x: 1, y: 2 }))
    console.println(show.Show.show(Page { number: 3, item: \"intro\" }))
}
";
    let dir = project("derive-works", main);
    let (stdout, stderr, status) = tondo(&dir, &["run"]);
    assert_eq!((stdout.as_str(), status), ("Point { x: 1, y: 2 }\nPage { number: 3, item: intro }\n", 0), "{stderr}");
    let (stdout, _, status) = tondo(&dir, &["expand"]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with(
            "// derive show.Show for Point (src/main.to:5:1)\nimpl show.Show for Point {\n    fn show(self): String {\n        \"Point {{ x: {self.x}, y: {self.y} }}\"\n    }\n}\n"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("impl[T: Display] show.Show for Page[T] {"), "{stdout}");
}

#[test]
fn reports_what_a_derive_cannot_do() {
    let main = "\
import show

pub type Point = { x: Int, y: Int }
derive show.Plain + show.Loop + show.Files + show.Extra for Point

pub enum Shape {
    Circle(Float)
}
derive show.Show for Shape

derive show.Show + show.Show for Point
derive show.Missing for Point
derive show.Show for Other
derive[T] show.Show for Point

fn main() {}
";
    let dir = project("derive-errors", main);
    let (_, stderr, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 1);
    for expected in [
        "error[E2102]: `show.Plain` has no provider: the package that declares it names one in its tondo.toml, as `[derive]` with `Plain = \"derive/plain.to\"`",
        "error[E2107]: the provider of `show.Loop` spent its budget of 100000000 steps for `Point`",
        "error[E2108]: the provider of `show.Files` uses what a provider cannot: it does not compile:\nerror[E2108]: `std.fs` is not available to a provider or generator",
        "error[E2105]: the provider of `show.Extra` gave something other than one `impl show.Extra for Point`",
        "error[E2104]: `show.Show`: `Show` derives only for records, and `Shape` is not one",
        "error[E2103]: `show.Show` is already derived for `Point` in this module",
        "error[E2103]: the module `show` declares no trait `Missing`",
        "error[E2101]: `derive` names `Other`, which is not a record, enum or newtype of this module",
        "error[E2101]: `derive` names `Point`, which must be written with exactly the binders of `derive`",
    ] {
        assert!(stderr.contains(expected), "missing {expected} in {stderr}");
    }
}
