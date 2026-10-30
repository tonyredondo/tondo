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
            "message": "there is no standard module `std.nothing`; `tondo api --list` shows them",
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
