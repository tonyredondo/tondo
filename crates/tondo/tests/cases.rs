//! Runs every program under `tests/cases` and checks the expectations
//! written in its comments:
//!
//! - `// expect-stdout: <line>` — one line of standard output, in order;
//! - `// expect-stderr: <line>` — a line the standard error contains;
//! - `// expect-exit: <status>` — process exit status (default 0);
//! - `// expect-panic: <code>` — a language panic such as `P0005`;
//! - `// expect-crash` — a signal ends the process, as a fault does;
//! - `// expect-error: <code> [text]` — compilation fails with this
//!   diagnostic, whose message contains `text` when given; every error is
//!   one of these, since one that follows from another is a cascade;
//! - `// expect-warning: <code> [text]` — compilation reports this warning;
//!   a case that fails to compile reports no other;
//! - `// expect-platform: <os or arch>...` — the case runs only there, as
//!   `linux` or `aarch64`;
//! - `// expect-env: <NAME>=<value>` — the executable runs with it set;
//! - `// expect-no-replay: <why>` — with `TONDO_CASES_REPLAY`, it is not
//!   recorded, for the reason given.
//! - `// expect-no-schedule: <why>` — with `TONDO_CASES_SCHEDULE`, its tasks
//!   run as ever, for the reason given.
//! - `// expect-optimization: <line>:<column> <kind> [text]` — `tondo
//!   optimizations` tells, at that place of the case, a remark of that
//!   kind whose text contains `text` when given: what a release build
//!   makes of the code, which a change to the passes must keep.
//!
//! Executables run with `TONDO_RT_CHECK_LEAKS=1`, so every case also checks
//! that reference counting frees each string exactly once. With
//! `TONDO_CASES_STATIC` set, cases build static executables (`--static`);
//! with `TONDO_CASES_DEBUG`, debug builds (`--debug`); with
//! `TONDO_CASES_LIVE`, live builds (`--live`). With `TONDO_CASES_REPLAY`,
//! debug builds whose runs are recorded, each checked as ever and then
//! replayed, which must write the same and end the same. With
//! `TONDO_CASES_SCHEDULE`, a seed, the tasks of each case run in the order it
//! gives (testing 8.10): what a case says of its output holds in any order.
//! With `TONDO_CASES_REPEAT`, a number, each case that starts tasks runs that
//! many times, each checked. With `TONDO_CASES_COVERAGE`, a directory, each
//! build counts the lines it runs, the standard library's too, and each case
//! leaves in a directory of its own what its counters count and their counts
//! (docs/refactor.md, 0.1). With `TONDO_CASES_SHARD=k/n`, only the k-th of
//! n shares of the cases runs, so that jobs of CI split a mode between them.
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
    no_replay: bool,
    no_schedule: bool,
    optimizations: Vec<String>,
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
            "no-replay" => expect.no_replay = true,
            "no-schedule" => expect.no_schedule = true,
            "optimization" => expect.optimizations.push(value.trim().into()),
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
    let mut build = Command::new(compiler);
    build
        .current_dir(path.parent().unwrap())
        .arg("build")
        .arg(path.file_name().unwrap())
        .arg("--output")
        .arg(&binary)
        .args(std::env::var_os("TONDO_CASES_STATIC").map(|_| "--static"))
        .args(std::env::var_os("TONDO_CASES_DEBUG").or(std::env::var_os("TONDO_CASES_REPLAY")).map(|_| "--debug"))
        .args(std::env::var_os("TONDO_CASES_LIVE").map(|_| "--live"))
        .env("XDG_CACHE_HOME", scratch.join("cache"));
    let coverage = std::env::var_os("TONDO_CASES_COVERAGE")
        .map(|dir| Path::new(&dir).join(scratch.file_name().unwrap()).join(path.file_stem().unwrap()));
    if let Some(dir) = &coverage {
        std::fs::create_dir_all(dir.join("counts")).unwrap();
        build.env("TONDO_COVERAGE_STD", dir.join("counted"));
    }
    // Long enough for a debug build of `tondo` under an emulator, where a
    // case that uses much of the standard library takes minutes: a build
    // that hangs names its case.
    let build = run_with_timeout(&mut build, 1200).map_err(|e| format!("its build: {e}"))?;
    let diagnostics = String::from_utf8_lossy(&build.stderr);
    let matches = |line: &str, severity: &str, expected: &str| {
        let (code, text) = expected.split_once(' ').unwrap_or((expected, ""));
        line.strip_prefix(&format!("{severity}[{code}]: ")).is_some_and(|m| m.contains(text))
    };
    let expected = expect.errors.iter().map(|e| ("error", e)).chain(expect.warnings.iter().map(|w| ("warning", w)));
    for (severity, expected) in expected {
        if !diagnostics.lines().any(|line| matches(line, severity, expected)) {
            let (code, text) = expected.split_once(' ').unwrap_or((expected, ""));
            return Err(format!("missing {severity}[{code}]: {text} in diagnostics:\n{diagnostics}"));
        }
    }
    let unexpected: Vec<&str> = diagnostics
        .lines()
        .filter(|line| line.starts_with("error["))
        .filter(|line| !expect.errors.iter().any(|expected| matches(line, "error", expected)))
        .collect();
    if !unexpected.is_empty() {
        return Err(format!("errors the case does not expect:\n{}\n\n{diagnostics}", unexpected.join("\n")));
    }
    if !expect.errors.is_empty() {
        let unexpected: Vec<&str> = diagnostics
            .lines()
            .filter(|line| line.starts_with("warning["))
            .filter(|line| !expect.warnings.iter().any(|expected| matches(line, "warning", expected)))
            .collect();
        if !unexpected.is_empty() {
            return Err(format!("warnings the case does not expect:\n{}\n\n{diagnostics}", unexpected.join("\n")));
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
    // What a release build makes of it, once, in the modes that build one.
    let release = ["TONDO_CASES_DEBUG", "TONDO_CASES_REPLAY", "TONDO_CASES_LIVE"];
    if !expect.optimizations.is_empty() && release.iter().all(|mode| std::env::var_os(mode).is_none()) {
        optimized(path, scratch, &expect.optimizations)?;
    }

    // A crash writes no core file, which takes the system a while.
    let program = || {
        let mut command = Command::new(&binary);
        if expect.crash {
            command = Command::new("sh");
            command.args(["-c", "ulimit -c 0; exec \"$0\""]).arg(&binary);
        }
        // A case that panics on purpose leaves no dump in the user's cache.
        command.env("TONDO_DUMP", "off");
        command.envs(expect.env.iter().map(|(name, value)| (name, value)));
        command.env("TONDO_RT_CHECK_LEAKS", "1").current_dir(scratch);
        if let Some(dir) = &coverage {
            command.env("TONDO_TEST_COVERAGE", dir.join("counts"));
        }
        if let Some(seed) = std::env::var_os("TONDO_CASES_SCHEDULE").filter(|_| !expect.no_schedule) {
            command.env("TONDO_RT_SCHEDULE", seed).env("TONDO_RACE", "1");
        }
        // It serves the tools, so that one that hangs can say why.
        if std::env::var_os("TONDO_CASES_REPLAY").is_none() {
            command.env("TONDO_DIAGNOSTICS", "1");
        }
        command
    };
    // With `TONDO_CASES_REPEAT`, a case that starts tasks runs that many
    // times, as memory that orders weakly, as arm64's, brings out what one run
    // rarely shows (docs/refactor.md, 0.9).
    let tasks = std::fs::read_to_string(path).is_ok_and(|text| text.contains("spawn"));
    let repeat = std::env::var("TONDO_CASES_REPEAT").ok().and_then(|n| n.parse().ok()).filter(|_| tasks).unwrap_or(1);
    for round in 1..=repeat {
        let once = || -> Result<(), String> {
            let mut command = program();
            // A seed of its own each time, which a failure says.
            let recording = std::env::var_os("TONDO_CASES_REPLAY").filter(|_| !expect.no_replay).map(|_| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                (binary.with_extension("calls"), format!("{:x}", now.as_nanos() as u64))
            });
            if let Some((calls, seed)) = &recording {
                command.env("TONDO_RT_RECORD", calls).env("TONDO_RT_SCHEDULE", seed);
            }
            let run = run_with_timeout(&mut command, 60)?;
            let stdout = String::from_utf8_lossy(&run.stdout);
            let stderr = String::from_utf8_lossy(&run.stderr);
            let expected_stdout: String = expect.stdout.iter().map(|l| format!("{l}\n")).collect();
            if stdout != expected_stdout {
                return Err(format!(
                    "stdout differs\n--- expected\n{expected_stdout}--- actual\n{stdout}--- stderr\n{stderr}"
                ));
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
            if let Some((calls, seed)) = &recording {
                let mut replay = program();
                let again = run_with_timeout(replay.env("TONDO_RT_REPLAY", calls).env("TONDO_RT_SCHEDULE", seed), 60)
                    .map_err(|e| format!("its replay, of seed {seed}: {e}"))?;
                if again.stdout != run.stdout || again.status != run.status {
                    let (out, err) = (String::from_utf8_lossy(&again.stdout), String::from_utf8_lossy(&again.stderr));
                    let status = (run.status, again.status);
                    return Err(format!(
                        "its replay, of seed {seed}, differs: {status:?}\n--- recorded\n{stdout}--- replayed\n{out}--- stderr\n{err}"
                    ));
                }
            }
            Ok(())
        };
        once().map_err(|e| if repeat > 1 { format!("run {round} of {repeat}: {e}") } else { e })?;
    }
    Ok(())
}

/// Whether `tondo optimizations` tells each remark a case expects.
fn optimized(path: &Path, scratch: &Path, expected: &[String]) -> Result<(), String> {
    let told = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .current_dir(path.parent().unwrap())
        .args([std::ffi::OsStr::new("optimizations"), path.file_name().unwrap(), std::ffi::OsStr::new("--json")])
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .output()
        .map_err(|e| format!("tondo optimizations: {e}"))?;
    let told: serde_json::Value = serde_json::from_slice(&told.stdout)
        .map_err(|e| format!("tondo optimizations: {e}\n{}", String::from_utf8_lossy(&told.stderr)))?;
    let found = told["optimizations"].as_array().cloned().unwrap_or_default();
    for expected in expected {
        let (at, rest) = expected.split_once(' ').unwrap_or((expected, ""));
        let (kind, text) = rest.split_once(' ').unwrap_or((rest, ""));
        let place = at
            .split_once(':')
            .and_then(|(line, column)| Some((line.parse::<u64>().ok()?, column.parse::<u64>().ok()?)));
        let (line, column) =
            place.ok_or(format!("`expect-optimization: {expected}` is not `<line>:<column> <kind> [text]`"))?;
        let here = |o: &serde_json::Value| {
            o["line"] == line
                && o["column"] == column
                && o["kind"] == kind
                && o["text"].as_str().is_some_and(|t| t.contains(text))
        };
        if !found.iter().any(here) {
            let said: Vec<String> = found
                .iter()
                .map(|o| {
                    format!(
                        "{}:{} {} {}",
                        o["line"],
                        o["column"],
                        o["kind"].as_str().unwrap_or(""),
                        o["text"].as_str().unwrap_or("")
                    )
                })
                .collect();
            return Err(format!("no optimization `{expected}`; a release build made of it:\n{}", said.join("\n")));
        }
    }
    Ok(())
}

/// What a program that does not end says of itself, by the tools every user
/// has: a dump, why it stands still and where its tasks are. CI keeps the dump.
fn why_it_hangs(pid: u32) -> String {
    let dir = std::env::var_os("TONDO_CI_DUMPS").map_or(std::env::temp_dir(), PathBuf::from);
    let _ = std::fs::create_dir_all(&dir);
    let dump = dir.join(format!("hung-{pid}.dump.json"));
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let dumped = Command::new(tondo).args(["dump", &pid.to_string(), "--output"]).arg(&dump).output();
    if !dumped.is_ok_and(|output| output.status.success()) {
        return String::new();
    }
    let read = |view: &str| {
        let shown = Command::new(tondo).args(["dump", "read"]).arg(&dump).arg(view).output();
        shown.map_or(String::new(), |output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    format!("\n--- why\n{}\n--- tasks\n{}\n--- dump: {}", read("why"), read("tasks"), dump.display())
}

/// A program that does not end says why in the failure of its case.
#[test]
fn a_case_that_hangs_says_why() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hangs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = "import std.time\n\nfn main() {\n    time.sleep(time.seconds(120))\n}\n";
    std::fs::write(dir.join("hangs.to"), source).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(compiler).args(["build", "hangs.to", "--output", "hangs"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut program = Command::new(dir.join("hangs"));
    program.env("TONDO_DIAGNOSTICS", "1").env("TONDO_CI_DUMPS", &dir);
    let failure = run_with_timeout(&mut program, 2).err().unwrap_or_default();
    assert!(failure.contains("timed out") && failure.contains("hangs.to:4 in main"), "{failure}");
}

/// Runs a program, failing the case when it has not finished in time.
fn run_with_timeout(command: &mut Command, seconds: u64) -> Result<std::process::Output, String> {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let why = why_it_hangs(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {seconds}s{why}"));
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
    // Empty, as a workflow gives a value its job does not have, it shares
    // nothing out.
    if let Some(shard) = std::env::var("TONDO_CASES_SHARD").ok().filter(|shard| !shard.is_empty()) {
        let parsed = shard.split_once('/').and_then(|(k, n)| Some((k.parse().ok()?, n.parse().ok()?)));
        let Some((k, n)) = parsed.filter(|&(k, n)| (1..=n).contains(&k)) else {
            panic!("TONDO_CASES_SHARD is k/n, with k from 1 to n, not {shard}")
        };
        paths = share(paths, k, n);
    }
    // How many ran, even when they pass, to compare one way of running them
    // with another.
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{name}: {} to run", paths.len());

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

/// The k-th of n shares of `paths`: those whose index modulo n is k - 1.
fn share(paths: Vec<PathBuf>, k: usize, n: usize) -> Vec<PathBuf> {
    paths.into_iter().enumerate().filter(|(index, _)| index % n == k - 1).map(|(_, path)| path).collect()
}

#[test]
fn shares_together_are_every_case() {
    let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("{i}.to"))).collect();
    for n in 1..=4 {
        let mut together: Vec<PathBuf> = (1..=n).flat_map(|k| share(paths.clone(), k, n)).collect();
        together.sort();
        let mut all = paths.clone();
        all.sort();
        assert_eq!(together, all, "{n} shares");
    }
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
