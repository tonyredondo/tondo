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
    // Several words, quoted together or not, find the same.
    let (apart, _) = json(&dir, &["api", "--search", "decode", "base64", "--json"]);
    let (together, _) = json(&dir, &["api", "--search", "decode base64", "--json"]);
    assert_eq!(apart["matches"], together["matches"]);
    assert!(!apart["matches"].as_array().unwrap().is_empty(), "{apart}");
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

/// `check` of a project checks its tests too, once its code is right, as an
/// editor shows them: a test that does not compile is not left for `test`.
#[test]
fn checks_the_tests_of_a_project() {
    let dir = scratch("checked-tests");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "pub fn twice(n: Int): Int {\n    n * 2\n}\n\nfn main() {}\n").unwrap();
    std::fs::write(dir.join("src/main_test.to"), "test doubles {\n    assert(twice(\"2\") == 4)\n}\n").unwrap();
    std::fs::write(dir.join("tests/api.to"), "import app\n\ntest api {\n    assert(app.twice(2) == true)\n}\n")
        .unwrap();
    let (_, err, code) = tondo(&dir, &["check"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("src/main_test.to:2:"), "{err}");
    assert!(err.contains("tests/api.to:4:"), "{err}");
    // An error of the code comes alone: the tests are checked once it is right.
    std::fs::write(dir.join("src/main.to"), "pub fn twice(n: Int): Int {\n    n * \"2\"\n}\n\nfn main() {}\n").unwrap();
    let (_, err, code) = tondo(&dir, &["check"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("src/main.to:2:") && !err.contains("_test.to"), "{err}");
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
    // This one has no repair: only accepting it.
    assert_eq!(list[2]["fixes"].as_array().unwrap().len(), 1, "{found}");
    assert!(list[2]["fixes"][0]["title"].as_str().unwrap().starts_with("accept"), "{found}");
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
    // On every address: macOS lets a server listen on all beside one on a single one.
    let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let program = format!(
        "import std.http\n\nfn main(): !http.HttpError {{\n    var app = http.App()\n    app.listen({port})?\n}}\n"
    );
    std::fs::write(dir.join("server.to"), program).unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "server.to"])
        .current_dir(&dir)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A server that did listen would never end.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while server.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = server.kill();
    let ran = server.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("cannot listen"), "{said}");
}

/// A program with nothing left to do lets out what it wrote, even to a pipe,
/// when its last worker runs out of work: tasks keep several busy first.
#[test]
fn output_goes_out_when_a_program_has_nothing_to_do() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("idle-output");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.time\n\nfn busy(n: Int): Int {\n    var total = 0\n    \
        for i in 0..1000000 {\n        total += i % (n + 1)\n    }\n    total\n}\n\nfn main() {\n    \
        var group = Group[Int]()\n    for n in 0..8 {\n        group.add(spawn busy(n))\n    }\n    \
        _ = group.wait()\n    console.println(\"waiting\")\n    time.sleep(time.seconds(30))\n}\n";
    std::fs::write(dir.join("idle.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo).args(["build", "idle.to", "--output", "idle"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut child = Command::new(dir.join("idle")).stdout(std::process::Stdio::piped()).spawn().unwrap();
    // While it sleeps: it lets everything out as it ends, after 30 seconds.
    let stdout = child.stdout.take().unwrap();
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
        let _ = said.send(line);
    });
    let line = heard.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(line, "waiting\n");
}

/// Exploring orders, the thread that stands in for a worker that a function
/// C called back holds sees the task go on although nothing wakes it: here
/// a process it waits for ends. The case of it otherwise runs as any other.
#[test]
fn a_thread_stands_in_while_a_callback_waits_for_the_host() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return;
    }
    let dir = scratch("stand-in");
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/run/ffi_callback_waits_for_a_process.to");
    let (_, stderr, status) = tondo(&dir, &["build", case.to_str().unwrap(), "--output", "callback"]);
    assert_eq!(status, 0, "{stderr}");
    let mut child = Command::new(dir.join("callback"))
        .env("TONDO_RT_SCHEDULE", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // One that missed it would hang.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the program hung: {:?}", child.wait_with_output().unwrap());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "the process ended: true\n");
    assert!(output.status.success());
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

/// `tondo dap` takes what a program needs to be built, and only that.
#[test]
fn the_debug_adapter_says_what_it_takes() {
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo")).args(["dap", "--strip"]).output().unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(!ran.status.success() && said.contains("dap takes --link <library> and --static"), "{said}");
}

fn project_at(name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (file, text) in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn said_by(dir: &Path, args: &[&str]) -> String {
    let ran = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr))
}

/// The codes no program of the cases can show: a file's bytes, a project.
#[test]
fn codes_of_files_and_projects() {
    let dir = project_at("code-bytes", &[("bad.to", b"fn main() {}\n\xff\n")]);
    assert!(said_by(&dir, &["check", "bad.to"]).contains("E0001"));
    let manifest: &[u8] = b"[package]\nname = \"app\"\n";
    let dir = project_at(
        "code-script",
        &[
            ("tondo.toml", manifest),
            ("src/main.to", b"import app.other\n\nother.go()\n"),
            ("src/other/o.to", b"import app\n\npub fn go() {}\n"),
        ],
    );
    assert!(said_by(&dir, &["check"]).contains("E1801"));
    let dir = project_at(
        "code-systems",
        &[
            ("tondo.toml", manifest),
            ("src/main.to", b"fn main() {\n    _ = at()\n}\n"),
            ("src/at_linux.to", b"fn at(): Int {\n    1\n}\n"),
            ("src/at_macos.to", b"fn at(): String {\n    \"a\"\n}\n"),
        ],
    );
    assert!(said_by(&dir, &["check"]).contains("E1808"));
}

/// The panics of the test library, which only a test run gives.
#[test]
fn codes_of_a_test_run() {
    let manifest: &[u8] = b"[package]\nname = \"app\"\n";
    let tests: &[u8] = b"import std.testing\nimport std.time\n\n\
        test backwards {\n    testing.withVirtualTime((clock) {\n        clock.advance(time.seconds(-1))\n    })?\n}\n\n\
        test leaves {\n    defer {\n        testing.skip(\"late\")\n    }\n}\n\n\
        test tagged {\n    testing.tags([\"a\": \"1\"])\n    testing.tags([\"a\": \"2\"])\n}\n\n\
        test snapped {\n    testing.snapshot(\"same\", 1)\n    testing.snapshot(\"same\", 1)\n}\n";
    let dir = project_at(
        "code-tests",
        &[("tondo.toml", manifest), ("src/main.to", b"fn main() {}\n"), ("src/main_test.to", tests)],
    );
    let said = said_by(&dir, &["test", "--update-snapshots"]);
    assert!(["P2001", "P2002", "P2005", "P2008"].iter().all(|code| said.contains(code)), "{said}");
}

/// Locks taken in opposite orders warn in any test, and fail with `--lock-order`.
#[test]
fn a_test_says_of_locks_in_opposite_orders() {
    let manifest: &[u8] = b"[package]\nname = \"app\"\n";
    let tests: &[u8] = b"import std.sync\n\ntest crossed {\n    let first = sync.Mutex(0)\n    \
        let second = sync.Mutex(0)\n    first.update((a) {\n        second.update((b) { b += 1 })\n    })\n    \
        second.update((b) {\n        first.update((a) { a += 1 })\n    })\n}\n";
    let dir = project_at(
        "lock-orders",
        &[("tondo.toml", manifest), ("src/main.to", b"fn main() {}\n"), ("src/main_test.to", tests)],
    );
    let said = said_by(&dir, &["test"]);
    assert!(said.contains("crossed ... ok") && said.contains("orders that can deadlock"), "{said}");
    let said = said_by(&dir, &["test", "--lock-order"]);
    assert!(said.contains("crossed ... FAILED") && said.contains("P0019"), "{said}");
}

/// What a program asks of itself beyond compiling.
#[test]
fn check_says_what_it_is_asked() {
    let source: &[u8] = b"/// Says it.\npub fn told() {}\n\npub fn untold() {}\n\n/// Says it too.\npub fn long[\n    T,\n](value: T): T {\n    value\n}\n\nfn hidden() {}\n\nfn main() {\n    \
        var values = [1, 2]\n    for round in 0..3 {\n        let before = values\n        \
        _ = before.length() + round\n        values.push(round)\n    }\n    \
        var other = [1]\n    for round in 0..3 {\n        let kept = other\n        other.push(round)\n        \
        _ = kept.length()\n    }\n    hidden()\n    told()\n    untold()\n}\n";
    let dir = project_at("check-lints", &[("lints.to", source)]);
    let plain = said_by(&dir, &["check", "lints.to"]);
    assert!(!plain.contains("W1010") && !plain.contains("W1009"), "{plain}");
    let said = said_by(&dir, &["check", "lints.to", "--docs"]);
    assert!(said.contains("W1010") && said.contains("`untold` is public"), "{said}");
    assert!(!said.contains("`told` is public") && !said.contains("`long`") && !said.contains("hidden"), "{said}");
    let said = said_by(&dir, &["check", "lints.to", "--performance"]);
    assert!(said.contains("`before` shares `values`"), "{said}");
    assert!(!said.contains("`kept` shares"), "what it reads after the write keeps it: {said}");
}

/// What a comment silences stays in the data, and a profile that was not asked
/// for gives nothing to silence.
#[test]
fn check_lists_what_it_silences() {
    let source: &[u8] = b"// tondo: allow W1004: the name C gives it\ntype CUShort = UInt16\n\n\
        // tondo: allow W1010: a library of one\npub fn told() {}\n\nfn main() {\n    told()\n}\n";
    let dir = project_at("check-allow", &[("allow.to", source)]);
    let said = said_by(&dir, &["check", "allow.to"]);
    assert!(!said.contains("W1004") && !said.contains("W1019"), "{said}");
    let said = said_by(&dir, &["check", "allow.to", "--docs"]);
    assert!(!said.contains("W1010") && !said.contains("W1019"), "{said}");
    let said = said_by(&dir, &["check", "allow.to", "--json"]);
    let found: serde_json::Value = serde_json::from_str(&said).unwrap();
    assert_eq!(found["silenced"][0]["code"], "W1004", "{said}");
    // Any other warning offers the comment, to finish: as it is, it is an error.
    let dir = project_at("check-offer", &[("offer.to", b"type CUInt = Int\n\nfn main() {}\n")]);
    let said = said_by(&dir, &["check", "offer.to", "--json"]);
    let found: serde_json::Value = serde_json::from_str(&said).unwrap();
    let fixes = found["diagnostics"][0]["fixes"].as_array().unwrap();
    let offered = fixes.iter().find(|fix| fix["title"] == "accept `W1004`, saying why").expect(&said);
    assert_eq!(offered["applicability"], "requires-decision", "{said}");
    assert_eq!(offered["edits"][0]["replacement"], "// tondo: allow W1004: \n", "{said}");
}

/// A long call of C holds its worker, and nothing else: a task it made ready
/// and a timer go on, on one core too, where no other worker is waiting.
#[test]
fn a_long_call_of_c_holds_up_no_other_task() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("long-call");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.ffi\nimport std.time\n\n\
        extern fn usleep(micros: ffi.CUInt): ffi.CInt\n\n\
        fn wakes(input: Receiver[Int], start: time.Instant) {\n    _ = input.receive()\n    \
        console.println(\"{start.elapsed().milliseconds()}\")\n}\n\n\
        fn naps(start: time.Instant) {\n    time.sleep(time.milliseconds(10))\n    \
        console.println(\"{start.elapsed().milliseconds()}\")\n}\n\n\
        fn main() {\n    for _ in 0..6 {\n        let (out, input) = channel[Int](1)\n        \
        let start = time.now()\n        let woken = spawn wakes(input, start)\n        \
        let napped = spawn naps(start)\n        time.sleep(time.milliseconds(2))\n        \
        out.send(1)\n        unsafe {\n            _ = usleep(700000u32)\n        }\n        \
        woken.wait()\n        napped.wait()\n    }\n}\n";
    std::fs::write(dir.join("calls.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo).args(["build", "calls.to", "--output", "calls"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let alone = cfg!(target_os = "linux")
        .then(|| Command::new("taskset").args(["-c", "0", "./calls"]).current_dir(&dir).output());
    for ran in
        [alone.and_then(Result::ok), Some(Command::new(dir.join("calls")).output().unwrap())].into_iter().flatten()
    {
        let said = String::from_utf8_lossy(&ran.stdout);
        assert_eq!(said.lines().count(), 12, "{said}");
        for line in said.lines() {
            assert!(line.parse::<u64>().unwrap() < 500, "a task waited for the call: {said}");
        }
    }
}

/// `TONDO_DUMP_MIR` writes the MIR of the functions it names after each
/// pass, or after the one it asks for, with what the passes decided and
/// why; nothing without it.
#[test]
fn the_mir_of_a_function_and_what_the_passes_decided() {
    let source: &[u8] = b"import std.console\n\nfn measured(count: Int): Int {\n    var total = 0\n    \
        for i in 0..count {\n        total += \"{i * 1000}\".length()\n    }\n    total\n}\n\n\
        fn fib(n: Int): Int {\n    if n < 2 {\n        return n\n    }\n    fib(n - 1) + fib(n - 2)\n}\n\n\
        fn main() {\n    for round in 0..3 {\n        console.println(\"{measured(round)} {fib(round)}\")\n    }\n}\n";
    let dir = project_at("dump-mir", &[("main.to", source)]);
    let dumped = |asked: &str| {
        let ran = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["build", "main.to", "--output", "main"])
            .env("TONDO_DUMP_MIR", asked)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        String::from_utf8_lossy(&ran.stderr).into_owned()
    };
    let all = dumped("measured");
    for stage in ["lower", "inline", "prove", "stack"] {
        assert!(all.contains(&format!("== measured after {stage}\n")), "{stage}: {all}");
    }
    let stack = dumped("measured@stack");
    assert!(!stack.contains("after prove") && stack.contains("== measured after stack"), "{stack}");
    assert!(stack.contains("String (in the frame)") && stack.contains("length of l"), "{stack}");
    assert!(stack.contains("note prove: loop at b"), "{stack}");
    assert!(stack.contains("@main.to:6:"), "{stack}");
    let calls = dumped("fib@inline");
    assert!(calls.contains("note inline: the call of `fib` stays: it is this one"), "{calls}");
    let (_, quiet, _) = tondo(&dir, &["build", "main.to", "--output", "main"]);
    assert!(!quiet.contains("== measured"), "{quiet}");
}

/// `TONDO_LAYOUT_SEED` lays a build's functions out in another order, the
/// same for the same seed: the program does what it did, its panics name
/// the same frames, and only where its code lies changes.
#[test]
fn a_layout_seed_moves_the_code_and_nothing_else() {
    let source: &[u8] = b"fn twice(n: Int): Int {\n    n * 2\n}\n\nfn deep(n: Int): Int {\n    if n == 0 {\n        \
        panic(\"bottom\")\n    }\n    deep(n - 1) + 1\n}\n\nfn main() {\n    let total = twice(21)\n    \
        deep(total % 3)\n}\n";
    let dir = project_at("layout-seed", &[("main.to", source)]);
    let built = |seed: Option<&str>, output: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
        command.args(["build", "main.to", "--output", output]).current_dir(&dir);
        match seed {
            Some(seed) => command.env("TONDO_LAYOUT_SEED", seed),
            None => command.env_remove("TONDO_LAYOUT_SEED"),
        };
        let ran = command.output().unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        let run = Command::new(dir.join(output)).env("TONDO_DUMP", "off").output().unwrap();
        let said = String::from_utf8_lossy(&run.stderr)
            .lines()
            .filter(|l| !l.contains("dump of"))
            .collect::<Vec<_>>()
            .join("\n");
        (std::fs::read(dir.join(output)).unwrap(), said)
    };
    let (plain, said) = built(None, "plain");
    let (one, said_one) = built(Some("1"), "one");
    // The same name, as macOS signs an executable with its name.
    let (again, _) = built(Some("1"), "one");
    let (two, said_two) = built(Some("2"), "two");
    assert!(said.contains("deep") && said.contains("bottom"), "{said}");
    assert_eq!((&said_one, &said_two), (&said, &said));
    assert_eq!(one, again);
    assert!(one != plain && two != one);
}

/// `tondo asm` writes the machine code a release build makes of a
/// function, under the lines of the program it is of, with its IR before
/// and after Cranelift optimizes it with `--clif`, as data with `--json`,
/// and with `--against` what changed since a git revision.
#[test]
fn the_machine_code_of_a_function() {
    let source = "fn fib(n: Int): Int {\n    if n < 2 {\n        return n\n    }\n    fib(n - 1) + fib(n - 2)\n}\n\n\
        fn main() {\n    _ = fib(20)\n}\n";
    let dir = project_at("machine-code", &[("main.to", source.as_bytes())]);
    let (said, stderr, status) = tondo(&dir, &["asm", "main.to", "fib"]);
    assert_eq!(status, 0, "{stderr}");
    assert!(said.starts_with("fib ("), "{said}");
    assert!(said.contains("\nmain.to:2  if n < 2 {\n"), "{said}");
    assert!(said.lines().any(|line| line.trim_end().ends_with("ret")), "{said}");
    let (said, _, _) = tondo(&dir, &["asm", "main.to", "fib", "--clif"]);
    assert!(said.contains("IR before Cranelift optimizes it:") && said.contains("block0"), "{said}");
    let (found, status) = json(&dir, &["asm", "main.to", "fib", "--json"]);
    assert_eq!(status, 0);
    let fib = &found["functions"][0];
    assert_eq!(fib["function"], "fib");
    assert!(fib["instructions"].as_array().unwrap().len() > 5, "{found}");
    assert!(fib["lines"].as_array().unwrap().iter().any(|l| l["source"] == "fib(n - 1) + fib(n - 2)"), "{found}");
    let (_, stderr, status) = tondo(&dir, &["asm", "main.to", "nobody"]);
    assert_eq!(status, 3, "{stderr}");
    assert!(stderr.contains("no code of a function named `nobody`"), "{stderr}");
    // Since a revision: the same code, then the calls the other way round.
    let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
    git(&["init", "-q"]);
    git(&["add", "main.to"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "one"]);
    let (said, _, _) = tondo(&dir, &["asm", "main.to", "fib", "--against", "HEAD"]);
    assert!(said.starts_with("a release build makes the same code as HEAD ("), "{said}");
    std::fs::write(dir.join("main.to"), source.replace("fib(n - 1) + fib(n - 2)", "fib(n - 2) * 3 + fib(n - 1)"))
        .unwrap();
    let (said, _, _) = tondo(&dir, &["asm", "main.to", "fib", "--against", "HEAD"]);
    assert!(said.starts_with("fib, against HEAD ("), "{said}");
    assert!(said.lines().any(|l| l.starts_with("  +")) && said.lines().any(|l| l.starts_with("  -")), "{said}");
    assert!(said.ends_with("1 function changed, 0 the same\n"), "{said}");
    // Every function of the program's own code, and the code another tondo
    // makes of it: itself, the same; one that writes `nop` for `ret`, not.
    let (said, _, _) = tondo(&dir, &["asm", "main.to", "--all"]);
    assert!(said.contains("fib (") && said.contains("\nmain ("), "{said}");
    let this = env!("CARGO_BIN_EXE_tondo");
    let (said, _, _) = tondo(&dir, &["asm", "main.to", "--all", "--compiler", this]);
    assert!(
        said.starts_with("a release build makes the same code as the tondo at ") && said.contains(" in 2 functions"),
        "{said}"
    );
    let other = dir.join("other-tondo");
    std::fs::write(&other, format!("#!/bin/sh\n'{this}' \"$@\" | sed 's/\"ret\"/\"nop\"/'\n")).unwrap();
    Command::new("chmod").arg("+x").arg(&other).status().unwrap();
    let (found, status) = json(&dir, &["asm", "main.to", "--all", "--compiler", other.to_str().unwrap(), "--json"]);
    assert_eq!(status, 0, "{found}");
    let changes = found["changes"].as_array().unwrap();
    assert!(!changes.is_empty() && changes.iter().all(|c| c["diff"].to_string().contains("\"+ret\"")), "{found}");
    let (_, stderr, status) = tondo(&dir, &["asm", "main.to", "fib", "--against", "HEAD", "--compiler", this]);
    assert_eq!(status, 2, "{stderr}");
    assert!(stderr.contains("choose one"), "{stderr}");
}

/// `tondo optimizations --against` says what changed since a git revision:
/// what only it has, and what only the code has, each place known by its
/// code rather than its line, so that code moved down a line is no change.
#[test]
fn what_a_release_build_makes_differently_since_a_revision() {
    let fast: &[u8] =
        b"fn quarter(value: Int): Int {\n    if value < 0 {\n        return 0\n    }\n    value / 4\n}\n\n\
        fn main() {\n    let values = [1, 2, 3]\n    for i in 0..3 {\n        _ = quarter(values[i])\n    }\n}\n";
    let dir = project_at("optimizations-against", &[("main.to", fast)]);
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(&dir).status().unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "main.to"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "fast"]);
    // Moved down a line, the same code makes the same.
    std::fs::write(dir.join("main.to"), [b"\n", fast].concat()).unwrap();
    let (said, stderr, status) = tondo(&dir, &["optimizations", "main.to", "--against", "HEAD"]);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(said, "a release build makes of the code what it made of HEAD\n");
    // A function that calls itself is a call where it was inlined.
    let slow = String::from_utf8_lossy(fast).replace("return 0", "return quarter(0 - value)");
    std::fs::write(dir.join("main.to"), &slow).unwrap();
    let (said, _, _) = tondo(&dir, &["optimizations", "main.to", "--against", "HEAD", "--kind", "call"]);
    assert!(said.starts_with("only now:\n  main.to:3:16: `quarter(0 - value)` stays a call of `quarter`"), "{said}");
    let (found, status) = json(&dir, &["optimizations", "main.to", "--against", "HEAD", "--json"]);
    assert_eq!(status, 0);
    assert_eq!(found["against"]["ref"], "HEAD");
    assert!(found["new"].as_array().unwrap().iter().any(|o| o["kind"] == "call"), "{found}");
    // And the other way, what the revision had.
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qam", "slow"]);
    std::fs::write(dir.join("main.to"), fast).unwrap();
    let (said, _, _) = tondo(&dir, &["optimizations", "main.to", "--against", "HEAD", "--kind", "call"]);
    assert!(said.starts_with("only in HEAD ("), "{said}");
    assert!(said.contains("\n  main.to:3:16: `quarter(0 - value)` stays a call of `quarter`"), "{said}");
    let (_, stderr, status) = tondo(&dir, &["optimizations", "main.to", "--against", "nowhere"]);
    assert_eq!(status, 3, "{stderr}");
    assert!(stderr.contains("git knows no revision nowhere"), "{stderr}");
}

/// `tondo optimizations` says what a release build makes of the program's
/// own code, at each place, in the functions whose code it is in: the
/// calls inlined and those kept, with why, the texts made in the frame or
/// on the heap, with what keeps them, the checks dropped and those a loop
/// keeps; one function alone with `--function`, as data with `--json`, and
/// the diagnostics of a program that does not check.
#[test]
fn what_a_release_build_makes_of_the_code() {
    let source: &[u8] = b"import std.console\n\nfn sum(values: Array[Int]): Int {\n    var total = 0\n    \
        for i in 0..values.length() {\n        total += values[i]\n    }\n    total\n}\n\n\
        fn fib(n: Int): Int {\n    if n < 2 {\n        return n\n    }\n    fib(n - 1) + fib(n - 2)\n}\n\n\
        fn quarter(value: Int): Int {\n    if value < 0 {\n        return 0\n    }\n    value / 4\n}\n\n\
        fn main() {\n    let values = [1, 2, 3, 4]\n    for round in 0..3 {\n        \
        console.println(\"{sum(values)} {fib(round)} {\"{round * 1000}\".length()} {quarter(values[round])}\")\n        \
        console.println(\"{3 * values[round]}\")\n    }\n}\n";
    let dir = project_at("optimizations", &[("main.to", source)]);
    let (said, stderr, status) = tondo(&dir, &["optimizations", "main.to"]);
    assert_eq!(status, 0, "{stderr}");
    for line in [
        "main.to:6:9: `total += values[i]` keeps its overflow check, in a loop (in main, sum)",
        "main.to:6:18: `values[i]` stays within its array: no bounds check (in main, sum)",
        "main.to:15:5: `fib(n - 1)` stays a call of `fib`: the function calls itself (in fib)",
        "main.to:15:5: `fib(n - 1)` stays a call of `fib`: inlining stops after 3 rounds (in main)",
        "main.to:22:5: `value / 4` is a shift or a mask: its dividend is not negative (in main, quarter)",
        "main.to:28:27: `sum(values)` is inlined: the body of `sum` is copied here (in main)",
        "main.to:28:54: `\"{round * 1000}\"` is made in the frame, without the heap (in main)",
        "main.to:29:27: `3 * values[round]` is checked by the range of the other factor, not by the product (in main)",
    ] {
        assert!(said.lines().any(|said| said == line), "{line}\n{said}");
    }
    assert!(said.contains("is made on the heap: it is passed to a call that may keep it (in main)"), "{said}");
    // The copies of a place: a recursion inlined into `main` then kept as
    // a call is told as a call there, and the standard library is not told.
    assert!(!said.contains("`fib(n - 1)` is inlined"), "{said}");
    assert!(!said.contains("<std>") && !said.contains("String."), "{said}");
    let (fib, _, _) = tondo(&dir, &["optimizations", "main.to", "--function", "fib"]);
    assert!(fib.lines().count() >= 3 && fib.lines().all(|line| line.ends_with("(in fib)")), "{fib}");
    let (found, status) = json(&dir, &["optimizations", "main.to", "--json"]);
    assert_eq!(status, 0);
    assert_eq!(found["format"], "tondo-optimizations-0.1/1");
    let all = found["optimizations"].as_array().unwrap();
    let bounds = all.iter().find(|o| o["source"] == "values[i]").unwrap();
    assert_eq!(bounds["kind"], "unchecked");
    assert_eq!(
        (bounds["file"].as_str(), bounds["line"].as_u64(), bounds["column"].as_u64()),
        (Some("main.to"), Some(6), Some(18))
    );
    assert_eq!(bounds["functions"], json!(["main", "sum"]));
    let kinds: std::collections::BTreeSet<&str> = all.iter().map(|o| o["kind"].as_str().unwrap()).collect();
    for kind in ["inlined", "call", "frame", "heap", "unchecked", "checked", "arithmetic"] {
        assert!(kinds.contains(kind), "{kind}: {found}");
    }
    let wrong = project_at("optimizations-wrong", &[("main.to", b"fn main() {\n    let x: Int = \"no\"\n}\n")]);
    let (said, status) = json(&wrong, &["optimizations", "main.to", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(said["ok"], false);
    assert!(said["diagnostics"].to_string().contains("E"), "{said}");
}
