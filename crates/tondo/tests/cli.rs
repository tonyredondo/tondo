//! The command line for people and for tools (toolchain 7 and 7.12): the
//! help of each command, completions for the shells, exit statuses, and
//! the JSON of each command.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// What a command printed as JSON, and its exit status.
fn json(dir: &Path, args: &[&str]) -> (Value, i32) {
    let (stdout, stderr, status) = tondo(dir, args);
    let value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}\n{stdout}\n{stderr}"));
    (value, status)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_command_has_its_help() {
    let dir = scratch("cli-help");
    let (_, usage, status) = tondo(&dir, &[]);
    assert_eq!(status, 2);
    assert!(usage.contains("usage:\n  tondo run ") && usage.contains("  tondo graph "), "{usage}");
    for name in tondo::cli::names() {
        let (help, stderr, status) = tondo(&dir, &[name, "--help"]);
        assert_eq!(status, 0, "{name} --help: {stderr}");
        assert!(help.starts_with(&format!("usage:\n  tondo {name}")), "{name}: {help}");
        assert_eq!(tondo(&dir, &["help", name]).0, help);
    }
    // A mistake shows the command's own help, with status 2.
    let (_, stderr, status) = tondo(&dir, &["fmt", "--nothing", "main.to"]);
    assert_eq!(status, 2);
    assert!(stderr.starts_with("tondo: fmt does not take --nothing\nusage:\n  tondo fmt "), "{stderr}");
    let (_, stderr, status) = tondo(&dir, &["help", "nothing"]);
    assert_eq!(status, 2, "{stderr}");
}

#[test]
fn completes_in_each_shell() {
    let dir = scratch("cli-completions");
    for shell in ["bash", "zsh", "fish"] {
        let (script, stderr, status) = tondo(&dir, &["completions", shell]);
        assert_eq!(status, 0, "{stderr}");
        for name in tondo::cli::names() {
            assert!(script.contains(name), "{shell} lacks {name}");
        }
        assert!(script.contains("test-format") && script.contains("mermaid"), "{shell}: {script}");
    }
    let (bash, _, _) = tondo(&dir, &["completions", "bash"]);
    assert!(bash.contains("complete -o default -F _tondo tondo"), "{bash}");
    let (_, _, status) = tondo(&dir, &["completions", "powershell"]);
    assert_eq!(status, 2);
    assert!(tondo::cli::flags("graph").contains(&"--reverse".to_string()));
}

#[test]
fn says_errors_as_json() {
    let dir = scratch("cli-errors");
    let (error, status) = json(&dir, &["fmt", "--json", "--nothing", "main.to"]);
    assert_eq!(status, 2);
    assert_eq!(error["format"], "tondo-error-0.1/1");
    assert_eq!((&error["ok"], &error["usage"], &error["status"]), (&json!(false), &json!(true), &json!(2)));
    let (error, status) = json(&dir, &["api", "std.nothing", "--json"]);
    assert_eq!(status, 3);
    assert_eq!(
        error,
        json!({
            "format": "tondo-error-0.1/1", "ok": false, "status": 3, "usage": false,
            "message": "there is no module or declaration `std.nothing`; `tondo api --list` shows the modules, and `tondo api --search <words>` finds declarations",
        })
    );
}

#[test]
fn commands_give_json() {
    let dir = scratch("cli-json");
    let (version, status) = json(&dir, &["version", "--json"]);
    assert_eq!(status, 0);
    assert_eq!((&version["format"], &version["backend"]), (&json!("tondo-version-0.1/1"), &json!("cranelift")));
    assert!(version["version"].as_str().unwrap().starts_with("0.1"), "{version}");
    assert_eq!(version["features"], json!(tondo::cli::FEATURES));
    assert!(tondo::cli::FEATURES.contains(&"test-exact-many"));

    let (topics, _) = json(&dir, &["guide", "--list", "--json"]);
    assert!(topics["topics"].as_array().unwrap().iter().any(|t| t == "pitfalls"), "{topics}");
    let (topic, _) = json(&dir, &["guide", "testing", "--json"]);
    assert!(topic["text"].as_str().unwrap().starts_with("# Testing"), "{topic}");

    std::fs::write(dir.join("main.to"), "import std.console\nfn main() {\n  console.println(\"hi\")\n}\n").unwrap();
    std::fs::write(dir.join("wrong.to"), "fn main() {\n    missing()\n}\n").unwrap();
    // Not formatted: says so, and changes nothing with --check.
    let (checked, status) = json(&dir, &["fmt", "--check", "--json", "main.to"]);
    assert_eq!(status, 1);
    assert_eq!(checked["files"], json!([{ "file": "main.to", "changed": true }]));
    let (formatted, status) = json(&dir, &["fmt", "--json", "main.to"]);
    assert_eq!((status, &formatted["ok"]), (0, &json!(true)));
    let (again, _) = json(&dir, &["fmt", "--json", "main.to"]);
    assert_eq!(again["files"], json!([{ "file": "main.to", "changed": false }]));

    let (built, status) = json(&dir, &["build", "main.to", "--output", "hello", "--json"]);
    assert_eq!((status, &built["format"], &built["output"]), (0, &json!("tondo-build-0.1/1"), &json!("hello")));
    assert_eq!(built["diagnostics"], json!([]));
    let (failed, status) = json(&dir, &["build", "wrong.to", "--json"]);
    assert_eq!((status, &failed["ok"], &failed["output"]), (1, &json!(false), &Value::Null));
    assert_eq!(failed["diagnostics"][0]["code"], "E1001");
    let (checked, status) = json(&dir, &["check", "main.to", "--json"]);
    assert_eq!((status, &checked["format"], &checked["diagnostics"]), (0, &json!("tondo-check-0.1/1"), &json!([])));

    // Diagnostics as JSON beside what the command does.
    let (_, stderr, status) = tondo(&dir, &["run", "wrong.to", "--diagnostic-format", "json"]);
    assert_eq!(status, 1);
    let diagnostics: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(diagnostics[0]["code"], "E1001");

    let (programs, status) = json(&dir, &["ps", "--json"]);
    assert_eq!((status, &programs["format"]), (0, &json!("tondo-ps-0.1/1")));
    assert!(programs["programs"].is_array());
}

/// `new`, `init`, `add` and `remove` make and change a project; `fix`
/// applies the compiler's safe repairs; `explain` says what a code means;
/// `clean` empties the cache.
#[test]
fn manages_projects() {
    let dir = scratch("cli-manage");
    let (stdout, stderr, status) = tondo(&dir, &["new", "hello"]);
    assert_eq!(status, 0, "{stderr}");
    assert!(stdout.contains("made the project `hello`"), "{stdout}");
    assert_eq!(tondo(&dir, &["run", "hello"]).0, "Hello, world!\n");
    let (tested, _, status) = tondo(&dir, &["test", "hello"]);
    assert_eq!(status, 0, "{tested}");
    let (_, stderr, status) = tondo(&dir, &["new", "hello"]);
    assert_eq!(status, 3);
    assert!(stderr.contains("is not empty"), "{stderr}");
    let (_, stderr, status) = tondo(&dir, &["new", "my-app"]);
    assert_eq!(status, 3);
    assert!(stderr.contains("a package's name is an identifier"), "{stderr}");
    std::fs::create_dir_all(dir.join("shapes")).unwrap();
    let (made, status) = json(&dir.join("shapes"), &["init", "--json"]);
    assert_eq!((status, &made["name"]), (0, &json!("shapes")));

    // A dependency by path, imported and then taken out.
    let (_, stderr, status) = tondo(&dir, &["add", "figures", "--path", "../shapes", "--project", "hello"]);
    assert_eq!(status, 0, "{stderr}");
    let manifest = std::fs::read_to_string(dir.join("hello/tondo.toml")).unwrap();
    assert_eq!(
        manifest,
        "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[dependencies]\nfigures = { path = \"../shapes\" }\n"
    );
    let (_, stderr, status) = tondo(&dir, &["add", "figures", "--path", "../shapes", "--project", "hello"]);
    assert_eq!(status, 3);
    assert!(stderr.contains("depends on `figures` already"), "{stderr}");
    let (_, stderr, status) = tondo(&dir, &["add", "lost", "--path", "../nowhere", "--project", "hello"]);
    assert_eq!(status, 3, "{stderr}");
    assert_eq!(std::fs::read_to_string(dir.join("hello/tondo.toml")).unwrap(), manifest);
    let (_, _, status) = tondo(&dir, &["add", "x", "--tag", "v1", "--path", "y", "--project", "hello"]);
    assert_eq!(status, 2);
    let (_, stderr, status) = tondo(&dir, &["remove", "figures", "--project", "hello"]);
    assert_eq!(status, 0, "{stderr}");
    assert!(!std::fs::read_to_string(dir.join("hello/tondo.toml")).unwrap().contains("figures"));

    // The safe repairs, applied; then the program compiles.
    std::fs::write(dir.join("fixable.to"), "fn main() {\n    let value = Some(1)\n    let _ = value\n}\n").unwrap();
    let (fixed, status) = json(&dir, &["fix", "fixable.to", "--json"]);
    assert_eq!(
        (status, &fixed["format"], fixed["applied"].as_array().unwrap().len()),
        (0, &json!("tondo-fix-0.1/1"), 1)
    );
    assert!(std::fs::read_to_string(dir.join("fixable.to")).unwrap().contains("some(1)"));
    // Imports too: those the file lacks, in order, and not those it does
    // not use.
    let program = "import std.console\nimport std.process\n\nfn main() {\n    console.println(json.encode(time.seconds(1).seconds()))\n}\n";
    std::fs::write(dir.join("imports.to"), program).unwrap();
    let (_, status) = json(&dir, &["fix", "imports.to", "--json"]);
    assert_eq!(status, 0);
    let fixed = std::fs::read_to_string(dir.join("imports.to")).unwrap();
    assert!(fixed.starts_with("import std.console\nimport std.json\nimport std.time\n\nfn main"), "{fixed}");

    // What a code means, with the guide's examples of it.
    let (explained, _, status) = tondo(&dir, &["explain", "E1003"]);
    assert_eq!(status, 0);
    assert!(explained.starts_with("E1003 shadowing (error)\n\nA declaration would hide a binding"), "{explained}");
    assert!(explained.contains("From `tondo guide pitfalls`"), "{explained}");
    let (all, _) = json(&dir, &["explain", "--json"]);
    assert_eq!(all["codes"].as_array().unwrap().len(), tondo::explain::CODES.len());
    let (_, stderr, status) = tondo(&dir, &["explain", "E9999"]);
    assert_eq!(status, 3, "{stderr}");

    // The cache emptied: here, a cache of its own.
    let cache = scratch("cli-cache");
    std::fs::create_dir_all(cache.join("tondo/run/entry")).unwrap();
    std::fs::write(cache.join("tondo/run/entry/program"), [0u8; 1000]).unwrap();
    std::fs::create_dir_all(cache.join("tondo/deps/kept")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["clean", "--json"])
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .unwrap();
    let cleaned: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cleaned["freed"], 1000);
    assert!(!cache.join("tondo/run").exists() && cache.join("tondo/deps/kept").exists());
}

/// `tondo api` shows one declaration and what belongs to it, and finds
/// declarations by their names and documentation.
#[test]
fn the_api_of_a_declaration_and_a_search() {
    let dir = scratch("cli-api");
    let (client, _, status) = tondo(&dir, &["api", "std.http.Client"]);
    assert_eq!(status, 0);
    assert!(client.contains("pub type Client") && client.contains("pub fn Client.get("), "{client}");
    assert!(!client.contains("pub type App"), "{client}");
    let (strings, _, _) = tondo(&dir, &["api", "String"]);
    assert!(strings.contains("pub fn String.split("), "{strings}");
    let (declared, _) = json(&dir, &["api", "std.json.encode", "--json"]);
    assert_eq!(declared["declaration"], "std.json.encode");
    assert_eq!(declared["declarations"][0]["name"], "encode");
    let (found, _) = json(&dir, &["api", "--search", "base64", "--json"]);
    let names: Vec<String> = found["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| format!("{}.{}", m["module"].as_str().unwrap(), m["name"].as_str().unwrap()))
        .collect();
    assert!(
        names.starts_with(&["std.encoding.Base64Alphabet".into(), "std.encoding.base64Decode".into()]),
        "{names:?}"
    );
    let (module, _) = json(&dir, &["api", "std.csv", "--json"]);
    assert!(module["summary"].as_str().unwrap().starts_with("comma-separated values"), "{module}");
    let (_, stderr, status) = tondo(&dir, &["api", "std.nothing.Here"]);
    assert_eq!(status, 3);
    assert!(stderr.contains("`tondo api --search <words>`"), "{stderr}");
}

/// Editing a program and running it again keeps its last two builds in
/// the cache, not one per version.
#[test]
fn the_cache_keeps_a_programs_last_builds() {
    let dir = scratch("cli-builds");
    let cache = dir.join("cache");
    let run = |text: &str| {
        std::fs::write(dir.join("main.to"), format!("import std.console\n\nconsole.println(\"{text}\")\n")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["run", "main.to"])
            .current_dir(&dir)
            .env("XDG_CACHE_HOME", &cache)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), format!("{text}\n"));
    };
    for text in ["one", "two", "three", "four"] {
        run(text);
    }
    let builds = std::fs::read_dir(cache.join("tondo/run")).unwrap().flatten();
    assert_eq!(builds.filter(|entry| entry.path().is_dir()).count(), 2);
}

/// Every code a diagnostic of the compiler gives has its explanation.
#[test]
fn every_code_is_explained() {
    let source = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
        .unwrap()
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            match path.is_dir() {
                true => std::fs::read_dir(path).unwrap().flatten().map(|e| e.path()).collect::<Vec<_>>(),
                false => vec![path],
            }
        })
        .filter(|path| path.extension().is_some_and(|e| e == "rs"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<String>();
    let mut missing = Vec::new();
    let used = source.match_indices("error(\"E").map(|(i, _)| &source[i + 7..i + 12]);
    let warned = source.match_indices("warning(\"W").map(|(i, _)| &source[i + 9..i + 14]);
    for code in used.chain(warned) {
        if tondo::explain::explain(code).is_none() && !missing.contains(&code.to_string()) {
            missing.push(code.to_string());
        }
    }
    assert!(missing.is_empty(), "codes without an explanation: {missing:?}");
}

/// A project with files of each system (language 6.8): this machine's are
/// built, run and tested; `check` checks the other's too and names it; the
/// other's files are not counted by coverage; `systems` limits a package.
#[test]
fn builds_the_files_of_its_system() {
    let (here, other) = if cfg!(target_os = "macos") { ("macos", "linux") } else { ("linux", "macos") };
    let shown = |system: &str| if system == "macos" { "macOS" } else { "Linux" };
    let dir = scratch("systems");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "import std.console\n\nfn main() {\n    console.println(describe())\n}\n")
        .unwrap();
    let describe = |system: &str| format!("fn describe(): String {{\n    \"{}\"\n}}\n", shown(system));
    std::fs::write(dir.join(format!("src/describe_{here}.to")), describe(here)).unwrap();
    std::fs::write(
        dir.join(format!("src/describe_{here}_test.to")),
        format!("test describes{} {{\n    assert(describe() == \"{}\")\n}}\n", shown(here), shown(here)),
    )
    .unwrap();
    std::fs::write(dir.join(format!("src/describe_{other}_test.to")), "test elsewhere {\n    assert(false)\n}\n")
        .unwrap();
    // Missing for the other system: where it is, and what to write.
    let (_, err, code) = tondo(&dir, &["check"]);
    assert_eq!(code, 1, "{err}");
    let hint = format!(
        "on {}: unknown name `describe`; `describe` is in `src/describe_{here}.to`, for {}: write it for {} too, in a file `*_{other}.to` or a common one, as `fn describe(): String`",
        shown(other),
        shown(here),
        shown(other)
    );
    assert!(err.contains(&hint), "{err}");
    // Another signature on the other system.
    std::fs::write(dir.join(format!("src/describe_{other}.to")), "fn describe(): Int {\n    1\n}\n").unwrap();
    let (_, err, code) = tondo(&dir, &["check"]);
    assert_eq!(code, 1, "{err}");
    let mismatch = format!(
        "`describe` is `fn describe(): String` here, for {}, and `fn describe(): Int` in `src/describe_{other}.to`, for {}",
        shown(here),
        shown(other)
    );
    assert!(err.contains(&mismatch), "{err}");
    // Both right: it checks, and runs and tests this machine's files.
    std::fs::write(dir.join(format!("src/describe_{other}.to")), describe(other)).unwrap();
    let (out, err, code) = tondo(&dir, &["check"]);
    assert_eq!((out.as_str(), code), ("", 0), "{err}");
    let (out, err, code) = tondo(&dir, &["run"]);
    assert_eq!((out.as_str(), code), (format!("{}\n", shown(here)).as_str(), 0), "{err}");
    let (out, err, code) = tondo(&dir, &["test", "--coverage"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 passed"), "{out}");
    let note = format!("Not built on {}, so not counted: `src/describe_{other}.to`.", shown(here));
    assert!(out.contains(&note), "{out}");
    // A package of the other system only.
    std::fs::write(dir.join("tondo.toml"), format!("[package]\nname = \"app\"\nsystems = [\"{other}\"]\n")).unwrap();
    let (_, err, code) = tondo(&dir, &["run"]);
    assert_ne!(code, 0);
    let refused = format!(
        "the package `app` runs on {}, as `systems` in its tondo.toml says: it is not built for {}",
        shown(other),
        shown(here)
    );
    assert!(err.contains(&refused), "{err}");
}

/// A write made from an earlier read has a repair: one `update`, unless what
/// was read goes on being used.
#[test]
fn a_lost_update_has_a_repair() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lost-update-fix");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.sync\n\nfn main() {\n    let cell = sync.Mutex(0)\n    \
        let seen = cell.get()\n    cell.update((n) { n = seen + 1 })\n    cell.set(cell.get() * 2)\n    \
        let kept = cell.get()\n    cell.set(kept + 1)\n    console.println(\"{kept}\")\n}\n";
    std::fs::write(dir.join("fix.to"), program).unwrap();
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "fix.to", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let found: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap();
    let list = found["diagnostics"].as_array().unwrap();
    let edits = |at: usize| list[at]["fixes"][0]["edits"].as_array().cloned().unwrap_or_default();
    assert_eq!(list.len(), 3, "{found}");
    let replaced: Vec<&str> =
        list[0]["fixes"][0]["edits"].as_array().unwrap().iter().map(|e| e["replacement"].as_str().unwrap()).collect();
    assert_eq!(replaced, ["n", ""], "{found}");
    assert_eq!(edits(1)[0]["replacement"], "cell.update((value) { value = value * 2 })", "{found}");
    assert!(edits(2).is_empty(), "{found}");
}

/// A lock in a `defer` warns, and a counter has its repair.
#[test]
fn an_unused_string_has_a_repair() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("unused-string");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.cli\n\nfn main() {\n    cli.markup(\"[red]no[/]\")\n}\n";
    std::fs::write(dir.join("fix.to"), program).unwrap();
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "fix.to", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let found: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap();
    let fix = &found["diagnostics"][0]["fixes"][0];
    assert_eq!(fix["title"], "write it with `console.println`", "{found}");
    let edits = fix["edits"].as_array().cloned().unwrap_or_default();
    let written: Vec<&str> = edits.iter().map(|edit| edit["replacement"].as_str().unwrap()).collect();
    assert_eq!(written, ["console.println(", ")", "import std.console\n"], "{found}");
}

#[test]
fn a_lock_in_a_defer_has_a_repair() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lock-in-defer");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.sync\n\nfn main() {\n    let count = sync.Mutex(0)\n    count.set(1)\n    defer {\n        \
        count.update((n) { n += 2 })\n    }\n    console.println(\"{count.get()}\")\n}\n";
    std::fs::write(dir.join("fix.to"), program).unwrap();
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["check", "fix.to", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let found: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap();
    let edits = found["diagnostics"][0]["fixes"][0]["edits"].as_array().cloned().unwrap_or_default();
    let written: Vec<&str> = edits.iter().map(|edit| edit["replacement"].as_str().unwrap()).collect();
    assert_eq!(written, ["sync.Atomic", ".store", ".add(2)", ".load()"], "{found}");
}

/// A module in a directory `internal` is for its package: the others cannot
/// import it, nor see it among what the package offers.
#[test]
fn an_internal_module_stays_in_its_package() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("internal-modules");
    let _ = std::fs::remove_dir_all(&dir);
    let units = dir.join("units");
    let app = dir.join("app");
    std::fs::create_dir_all(units.join("src/internal/convert")).unwrap();
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(units.join("tondo.toml"), "[package]\nname = \"units\"\n").unwrap();
    std::fs::write(units.join("src/internal/convert/c.to"), "pub fn twice(n: Int): Int {\n    n * 2\n}\n").unwrap();
    let four = "import units.internal.convert\n\npub fn four(): Int {\n    convert.twice(2)\n}\n";
    std::fs::write(units.join("src/units.to"), four).unwrap();
    std::fs::write(
        app.join("tondo.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nunits = { path = \"../units\" }\n",
    )
    .unwrap();
    let tondo =
        |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(&app).output().unwrap();
    let outside = "import units.internal.convert\n\nfn main() {\n    _ = convert.twice(1)\n}\n";
    std::fs::write(app.join("src/main.to"), outside).unwrap();
    let said = tondo(&["check"]);
    let error = String::from_utf8_lossy(&said.stderr);
    assert!(error.contains("`units.internal.convert` is internal to the package `units`"), "{error}");
    let through = "import std.console\nimport units\n\nfn main() {\n    console.println(\"{units.four()}\")\n}\n";
    std::fs::write(app.join("src/main.to"), through).unwrap();
    let ran = tondo(&["run"]);
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "4\n", "{}", String::from_utf8_lossy(&ran.stderr));
    let listed = String::from_utf8_lossy(&tondo(&["api", "--list", "--project", "."]).stdout).to_string();
    assert!(listed.contains("units") && !listed.contains("internal"), "{listed}");
}

/// A test past its time says where its tasks were.
#[test]
fn a_test_that_times_out_says_where_it_was() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-timeout");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    let code = "import std.time\n\nfn slow() {\n    time.sleep(time.seconds(60))\n}\n\nfn main() {}\n";
    std::fs::write(dir.join("src/main.to"), code).unwrap();
    std::fs::write(dir.join("src/main_test.to"), "test hangs {\n    slow()\n}\n").unwrap();
    let ran =
        Command::new(env!("CARGO_BIN_EXE_tondo")).args(["test", "--timeout", "2"]).current_dir(&dir).output().unwrap();
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(said.contains("timed out after 2s") && said.contains("main.to:4 in slow"), "{said}");
}

/// A port in use is not a failure to connect.
#[test]
fn a_server_says_why_it_cannot_listen() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("taken-port");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let program = format!(
        "import std.http\n\nfn main(): !http.HttpError {{\n    var app = http.App.new()\n    app.listen({port})?\n}}\n"
    );
    std::fs::write(dir.join("server.to"), program).unwrap();
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo")).args(["run", "server.to"]).current_dir(&dir).output().unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("cannot listen"), "{said}");
}

/// A program with nothing left to do lets out what it wrote, even to a pipe.
#[test]
fn output_goes_out_when_a_program_has_nothing_to_do() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("idle-output");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.time\n\nfn main() {\n    console.println(\"waiting\")\n    \
        time.sleep(time.seconds(30))\n}\n";
    std::fs::write(dir.join("idle.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo).args(["build", "idle.to", "--output", "idle"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut child = Command::new(dir.join("idle")).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    let read = std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line);
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!((read.unwrap(), line.as_str()), (8, "waiting\n"));
}

/// Programs broken at random, as an edit leaves them, get diagnostics. The
/// Nightly's, for the time it takes.
#[test]
fn the_compiler_outlives_broken_programs() {
    if std::env::var_os("TONDO_SWEEP").is_none() {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("broken-programs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/run");
    let mut files: Vec<_> = std::fs::read_dir(cases).unwrap().flatten().map(|entry| entry.path()).collect();
    files.retain(|path| path.extension().is_some_and(|extension| extension == "to"));
    files.sort();
    let mut seed = 7u64;
    let mut next = move |limit: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize % limit.max(1)
    };
    for round in 0..3000 {
        let text = std::fs::read_to_string(&files[next(files.len())]).unwrap();
        let mut words: Vec<&str> = text.split_inclusive(|c: char| !c.is_alphanumeric()).collect();
        for _ in 0..1 + next(3) {
            let (at, other) = (next(words.len()), next(words.len()));
            match next(3) {
                0 => drop(words.remove(at.min(words.len() - 1))),
                1 => words[at] = words[other],
                _ => words.insert(at, words[other]),
            }
        }
        let file = dir.join("broken.to");
        std::fs::write(&file, words.concat()).unwrap();
        let ran =
            Command::new(env!("CARGO_BIN_EXE_tondo")).args(["check", "broken.to"]).current_dir(&dir).output().unwrap();
        let said = String::from_utf8_lossy(&ran.stderr);
        assert!(!said.contains("is a bug of tondo"), "round {round}: {said}\n{}", words.concat());
    }
}
