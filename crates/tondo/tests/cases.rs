//! Runs every program under `tests/cases` and checks the expectations
//! written in its comments:
//!
//! - `// expect-stdout: <line>` — one line of standard output, in order;
//! - `// expect-stderr: <line>` — a line the standard error contains;
//! - `// expect-exit: <status>` — process exit status (default 0);
//! - `// expect-panic: <code>` — a language panic such as `P0005`;
//! - `// expect-crash` — a signal ends the process, as a fault does;
//! - `// expect-error: <code> [text]` — compilation fails with this
//!   diagnostic, whose message contains `text` when given;
//! - `// expect-warning: <code> [text]` — compilation reports this warning;
//! - `// expect-platform: <os or arch>...` — the case runs only there, as
//!   `linux` or `aarch64`;
//! - `// expect-env: <NAME>=<value>` — the executable runs with it set.
//!
//! Executables run with `TONDO_RT_CHECK_LEAKS=1`, so every case also checks
//! that reference counting frees each string exactly once. With
//! `TONDO_CASES_STATIC` set, cases build static executables (`--static`);
//! with `TONDO_CASES_DEBUG`, debug builds (`--debug`); with
//! `TONDO_CASES_LIVE`, live builds (`--live`).
//!
//! Each directory under `tests/projects` is a project built as a whole,
//! with the expectations written in any of its files.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
struct Expect {
    stdout: Vec<String>,
    stderr: Vec<String>,
    exit: Option<i32>,
    panic: Option<String>,
    crash: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    platforms: Vec<String>,
    env: Vec<(String, String)>,
}

fn expectations(source: &str) -> Expect {
    let mut expect = Expect::default();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("// expect-") else { continue };
        let (key, value) = rest.split_once(':').unwrap_or((rest, ""));
        let value = value.strip_prefix(' ').unwrap_or(value).to_string();
        match key {
            "stdout" => expect.stdout.push(value),
            "stderr" => expect.stderr.push(value),
            "exit" => expect.exit = Some(value.trim().parse().expect("numeric exit status")),
            "panic" => expect.panic = Some(value.trim().into()),
            "crash" => expect.crash = true,
            "error" => expect.errors.push(value.trim().into()),
            "warning" => expect.warnings.push(value.trim().into()),
            "platform" => expect.platforms.extend(value.split_whitespace().map(String::from)),
            "env" => {
                let (name, value) = value.split_once('=').expect("`NAME=value`");
                expect.env.push((name.trim().into(), value.trim().into()));
            }
            other => panic!("unknown expectation `{other}`"),
        }
    }
    expect
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "to") {
            out.push(path);
        }
    }
}

fn run_case(path: &Path, scratch: &Path) -> Result<(), String> {
    let mut sources = Vec::new();
    if path.is_dir() {
        collect(path, &mut sources);
    } else {
        sources.push(path.to_path_buf());
    }
    let source: String = sources.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
    let expect = expectations(&source);
    let here = [std::env::consts::OS, std::env::consts::ARCH];
    if !expect.platforms.is_empty() && !expect.platforms.iter().any(|p| here.contains(&p.as_str())) {
        return Ok(());
    }
    let compiler = env!("CARGO_BIN_EXE_tondo");
    let binary = scratch.join(path.file_stem().unwrap());
    // From its directory, so that locations read `case.to:4:5`.
    let build = Command::new(compiler)
        .current_dir(path.parent().unwrap())
        .arg("build")
        .arg(path.file_name().unwrap())
        .arg("--output")
        .arg(&binary)
        .args(std::env::var_os("TONDO_CASES_STATIC").map(|_| "--static"))
        .args(std::env::var_os("TONDO_CASES_DEBUG").map(|_| "--debug"))
        .args(std::env::var_os("TONDO_CASES_LIVE").map(|_| "--live"))
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .output()
        .unwrap();
    let diagnostics = String::from_utf8_lossy(&build.stderr);
    let expected = expect.errors.iter().map(|e| ("error", e)).chain(expect.warnings.iter().map(|w| ("warning", w)));
    for (severity, expected) in expected {
        let (code, text) = expected.split_once(' ').unwrap_or((expected, ""));
        let header = format!("{severity}[{code}]: ");
        let found = diagnostics.lines().any(|line| line.strip_prefix(&header).is_some_and(|m| m.contains(text)));
        if !found {
            return Err(format!("missing {header}{text} in diagnostics:\n{diagnostics}"));
        }
    }
    if !expect.errors.is_empty() {
        return if build.status.success() { Err("compiled, but errors were expected".into()) } else { Ok(()) };
    }
    if !build.status.success() {
        return Err(format!("compilation failed:\n{diagnostics}"));
    }
    if expect.warnings.is_empty() && !diagnostics.is_empty() {
        return Err(format!("unexpected diagnostics:\n{diagnostics}"));
    }

    // A crash writes no core file, which takes the system a while.
    let mut command = Command::new(&binary);
    if expect.crash {
        command = Command::new("sh");
        command.args(["-c", "ulimit -c 0; exec \"$0\""]).arg(&binary);
    }
    command.envs(expect.env.iter().map(|(name, value)| (name, value)));
    let run = run_with_timeout(command.env("TONDO_RT_CHECK_LEAKS", "1").current_dir(scratch))?;
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let expected_stdout: String = expect.stdout.iter().map(|l| format!("{l}\n")).collect();
    if stdout != expected_stdout {
        return Err(format!("stdout differs\n--- expected\n{expected_stdout}--- actual\n{stdout}--- stderr\n{stderr}"));
    }
    let expected_exit = expect.exit.unwrap_or(if expect.panic.is_some() { 101 } else { 0 });
    if expect.crash {
        use std::os::unix::process::ExitStatusExt;
        if run.status.signal().is_none() {
            return Err(format!("exit status {:?}, expected a signal\n{stderr}", run.status.code()));
        }
    } else if run.status.code() != Some(expected_exit) {
        return Err(format!("exit status {:?}, expected {expected_exit}\n{stderr}", run.status.code()));
    }
    if let Some(code) = &expect.panic
        && !stderr.contains(&format!("panic[{code}]"))
    {
        return Err(format!("expected panic {code}, stderr:\n{stderr}"));
    }
    if let Some(missing) = expect.stderr.iter().find(|line| !stderr.lines().any(|l| l == line.as_str())) {
        return Err(format!("missing `{missing}` in stderr:\n{stderr}"));
    }
    Ok(())
}

/// Runs a program, failing the case when it has not finished in a minute.
fn run_with_timeout(command: &mut Command) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.read_to_end(&mut bytes);
        bytes
    });
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = err.read_to_end(&mut bytes);
        bytes
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out after 60s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    Ok(std::process::Output { status, stdout: stdout.join().unwrap(), stderr: stderr.join().unwrap() })
}

#[test]
fn cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases");
    let mut paths = Vec::new();
    collect(&root, &mut paths);
    run_all(paths, "cases");
}

#[test]
fn projects() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/projects");
    let paths = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    run_all(paths, "projects");
}

fn run_all(mut paths: Vec<PathBuf>, name: &str) {
    paths.sort();
    assert!(!paths.is_empty(), "no {name} found");

    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunks: Vec<&[PathBuf]> = paths.chunks(paths.len().div_ceil(workers)).collect();
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let scratch = &scratch;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|path| run_case(path, scratch).err().map(|e| format!("{}: {e}", path.display())))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{} of {} cases failed:\n\n{}", failures.len(), paths.len(), failures.join("\n\n"));
}

#[test]
fn json_diagnostics() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("json");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unknown.to");
    std::fs::write(&path, "fn main() {\n    missing()\n}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).arg("check").arg(&path).arg("--json").output().unwrap();
    assert!(!output.status.success());
    let mut json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!((&json["format"], &json["ok"]), (&"tondo-check-0.1/1".into(), &false.into()), "{json}");
    let diagnostic = &mut json["diagnostics"][0];
    assert!(diagnostic["id"].as_str().unwrap().starts_with("diag:"), "{diagnostic}");
    diagnostic["id"] = "diag:…".into();
    let expected = serde_json::json!({
        "id": "diag:…", "severity": "error", "code": "E1001", "message": "unknown name `missing`",
        "source_id": "root:unknown.to", "module": "unknown", "file": "unknown.to",
        "range": { "start": { "byte": 16, "line": 1, "column": 4 }, "end": { "byte": 23, "line": 1, "column": 11 } },
        "expected": null, "actual": null, "related": [], "fixes": [],
    });
    assert_eq!(json["diagnostics"], serde_json::json!([expected]));
}

/// Dependencies from a git repository (by tag) and an archive, reached
/// through a path dependency; `tondo.lock` pins them until `tondo update`.
#[test]
fn dependencies() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dependencies");
    let _ = std::fs::remove_dir_all(&root);
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "tag.gpgSign=false",
            ])
            .args(args)
            .current_dir(root.join("greet"))
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    };
    write("greet/src/greet.to", "pub fn hello(name: String): String {\n    \"hello {name}\"\n}\n");
    write("greet/tondo.toml", "[package]\nname = \"greet\"\n");
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "v1"]);
    git(&["tag", "-a", "v1", "-m", "v1"]);
    write("greet/src/greet.to", "pub fn hello(name: String): String {\n    \"HELLO {name}\"\n}\n");
    git(&["commit", "-qam", "v2"]);
    write("math-1.0/tondo.toml", "[package]\nname = \"math\"\n");
    write("math-1.0/src/math.to", "pub fn twice(value: Int): Int {\n    value * 2\n}\n");
    let tar = Command::new("tar").args(["-czf", "math-1.0.tar.gz", "math-1.0"]).current_dir(&root).status().unwrap();
    assert!(tar.success());
    let url = format!("file://{}/math-1.0.tar.gz", root.display());
    write("app/libs/units/src/units.to", "import math\n\npub fn meters(km: Int): Int {\n    math.twice(km) * 500\n}\n");
    write(
        "app/libs/units/tondo.toml",
        &format!("[package]\nname = \"units\"\n\n[dependencies]\nmath = {{ url = \"{url}\" }}\n"),
    );
    let repository = format!("file://{}/greet", root.display());
    write(
        "app/tondo.toml",
        &format!(
            "[dependencies]\ngreet = {{ git = \"{repository}\", tag = \"v1\" }}\nunits = {{ path = \"libs/units\" }}\n"
        ),
    );
    write(
        "app/src/main.to",
        "import std.console\nimport greet\nimport units\n\nconsole.println(\"{greet.hello(\"ada\")} {units.meters(3)}\")\n",
    );
    let tondo = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(args)
            .arg(root.join("app"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "tondo {args:?}: {stderr}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(tondo(&["run"]), "hello ada 3000\n");
    let lock = std::fs::read_to_string(root.join("app/tondo.lock")).unwrap();
    assert!(lock.contains("commit = ") && lock.contains("sha256 = "), "{lock}");
    // Moving the tag changes nothing until the lock is updated.
    git(&["tag", "-f", "-a", "v1", "-m", "moved"]);
    assert_eq!(tondo(&["run"]), "hello ada 3000\n");
    tondo(&["update"]);
    assert_eq!(tondo(&["run"]), "HELLO ada 3000\n");
}
