//! Generators (27.5 of the language): a package declares them in
//! `[generators.<name>]` of its `tondo.toml`, and the modules they write join
//! it before it is checked.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// A package `app` with `files`, each a path and its text.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

#[test]
fn generates_modules_from_inputs_and_declarations() {
    let dir = project(
        "generate-works",
        &[
            (
                "tondo.toml",
                "[package]\nname = \"app\"\n\n[generators.colors]\nprogram = \"gen/colors.to\"\ninputs = [\"colors.txt\"]\noutputs = [\"palette\"]\nroots = [\"models.User\"]\n",
            ),
            ("colors.txt", "red #ff0000\ngreen #00ff00\n"),
            (
                "src/models/models.to",
                "import app.places\n\npub type User = { name: String, home: places.Address, priv secret: String }\n",
            ),
            (
                "src/places/places.to",
                "pub type Address = { city: String, kind: Kind }\n\npub enum Kind {\n    House\n    Flat(Int)\n}\n",
            ),
            (
                "src/main.to",
                "import std.console\nimport app.palette\n\nfn main() {\n    console.println(\"{palette.Red} {palette.Green}\")\n    console.println(palette.Hash)\n    console.println(\"{palette.Declarations}\")\n    console.println(palette.Stable)\n}\n",
            ),
            (
                "gen/colors.to",
                r#"import std.meta
import std.random
import std.uuid

/// A constant per line of the input, its hash, and what the roots declare.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let input = request.input("colors.txt")?
    var lines: Array[String] = []
    for line in input.text().unwrapOr("").lines() {
        let parts = line.split(" ")
        let name = "{parts[0].slice(0, 1).unwrapOr("").toUpper()}{parts[0].slice(1, parts[0].length()).unwrapOr("")}"
        lines.push(meta.code(r"pub const $name: String = $hex", ["name": name, "hex": "\"{parts[1]}\""]))
    }
    lines.push("pub const Hash: String = \"{input.sha256}\"")
    // What is the same on every run works in a generator.
    var seeded = random.Random.seeded(7)
    lines.push("pub const Stable: String = \"{uuid.Uuid.v5(uuid.Uuid.namespaceDns(), "app")} {seeded.int(1..=6)}\"")
    var declared: Array[String] = []
    for declaration in request.declarations() {
        var parts: Array[String] = []
        for field in declaration.fields() {
            parts.push(field.name)
        }
        for variant in declaration.variants() {
            parts.push(variant.name)
        }
        declared.push("\"{declaration.module()}.{declaration.name()}: {parts.join(" ")}\"")
    }
    lines.push("pub const Declarations: Array[String] = [{declared.join(", ")}]")
    var out = request.sourceBuilder()
    out.write("palette", lines.join("\n"))?
    out.finish()
}
"#,
            ),
        ],
    );
    let (stdout, stderr, status) = tondo(&dir, &["run"]);
    assert_eq!(
        (stdout.as_str(), status),
        (
            "#ff0000 #00ff00\n9edb2f9d5c03d433f24a539eadd8797d05907cbf3d42489b6979e3f93b613eff\n[app.models.User: name home, app.places.Address: city kind, app.places.Kind: House Flat]\n91ad9bcb-d637-579d-818a-e391a5631aa1 4\n",
            0
        ),
        "{stderr}"
    );
    let (stdout, _, status) = tondo(&dir, &["expand"]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with("// generator colors wrote palette (`[generators.colors]` of the package `app`)\npub const Red: String = \"#ff0000\"\n"),
        "{stdout}"
    );
}

#[test]
fn reports_what_a_generator_cannot_do() {
    let generator = |body: &str| {
        format!(
            "import std.meta\n\npub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {{\n{body}\n}}\n"
        )
    };
    let write = |output: &str, source: &str| {
        generator(&format!(
            "    var out = request.sourceBuilder()\n    out.write(\"{output}\", \"{source}\")?\n    out.finish()"
        ))
    };
    let manifest = "\
[package]
name = \"app\"

[generators.partial]
program = \"gen/partial.to\"
outputs = [\"one\", \"two\"]

[generators.spin]
program = \"gen/spin.to\"
outputs = [\"spin\"]
limits = { steps = \"1000\" }

[generators.files]
program = \"gen/files.to\"
outputs = [\"files\"]

[generators.dice]
program = \"gen/dice.to\"
outputs = [\"dice\"]

[generators.fetch]
program = \"gen/fetch.to\"
outputs = [\"fetch\"]

[generators.socket]
program = \"gen/socket.to\"
outputs = [\"socket\"]

[generators.broken]
program = \"gen/broken.to\"
outputs = [\"broken\"]

[generators.derives]
program = \"gen/derives.to\"
outputs = [\"derives\"]

[generators.hand]
program = \"gen/hand.to\"
outputs = [\"models\"]

[generators.roots]
program = \"gen/hand.to\"
outputs = [\"other\"]
roots = [\"models.Missing\", \"models.User\", \"one.Thing\"]

[generators.reads]
program = \"gen/hand.to\"
inputs = [\"missing.txt\"]
outputs = [\"reads\"]
";
    let files = [
        ("tondo.toml", manifest.to_string()),
        ("src/models/models.to", "import app.one\n\npub type User = { thing: one.Thing }\n".to_string()),
        ("src/main.to", "import app.one\n\nfn main() {}\n".to_string()),
        ("gen/partial.to", write("one", "pub const A: Int = 1")),
        ("gen/spin.to", generator("    var n = 0\n    for {\n        n += 1\n    }")),
        ("gen/files.to", format!("import std.fs\n{}", write("files", "pub const A: Int = 1"))),
        ("gen/dice.to", generator("    var dice = random.Random.new()\n    var out = request.sourceBuilder()\n    out.write(\"dice\", \"pub const A: Int = {dice.int(1..=6)}\")?\n    out.finish()").replace("import std.meta", "import std.meta\nimport std.random")),
        ("gen/fetch.to", format!("import std.http\n{}", write("fetch", "pub const A: Int = 1"))),
        ("gen/socket.to", format!("import std.net\n{}", write("socket", "pub const A: Int = 1"))),
        ("gen/broken.to", write("broken", "fn (")),
        ("gen/derives.to", write("derives", "derive x.Y for Z")),
        ("gen/hand.to", write("models", "pub const A: Int = 1")),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let dir = project("generate-errors", &files);
    let (_, stderr, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 1);
    for expected in [
        "error[E2106]: the generator `[generators.partial]` of the package `app`: the output `two` is not written",
        "error[E2107]: the generator `[generators.spin]` of the package `app`: it spent its budget of 1000 steps; `limits = { steps = \"...\" }` raises it",
        "error[E2108]: the generator `[generators.files]` of the package `app`: it uses what a generator cannot: it does not compile:\nerror[E2108]: `std.fs` is not available to a provider or generator",
        "error[E2108]: the generator `[generators.dice]` of the package `app`: it uses what a generator cannot: it does not compile:\nerror[E2108]: `Random.new`, which gives another value each time, is not available to a provider or generator",
        "error[E2108]: `std.http` is not available to a provider or generator",
        "error[E2108]: `std.net` is not available to a provider or generator",
        "error[E2105]: the generator `[generators.broken]` of the package `app`: its output `broken` is not valid Tondo source; `tondo expand` shows it",
        "error[E2105]: the generator `[generators.derives]` of the package `app`: its output `derives` has a `derive`",
        "error[E2106]: the generator `[generators.hand]` of the package `app`: its output `models` would replace a module written by hand",
        "error[E2106]: the generator `[generators.roots]` of the package `app`: its root `models.Missing` is not a public record, enum or newtype",
        "error[E2109]: the generator `[generators.roots]` of the package `app`: `app.models.User` names `one.Thing`, which a generator of this build writes",
        "error[E2109]: the generator `[generators.roots]` of the package `app`: its root `one.Thing` is in `app.one`, which a generator of this build writes",
        "error[E2106]: the generator `[generators.reads]` of the package `app`: cannot read its input `missing.txt`",
        // A generator with a problem adds none of its modules.
        "error[E1008]: the package `app` has no module `app.one`",
    ] {
        assert!(stderr.contains(expected), "missing {expected} in {stderr}");
    }
}
