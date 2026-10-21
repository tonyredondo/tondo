//! Runs every program under `tests/cases` and checks the expectations
//! written in its comments:
//!
//! - `// expect-stdout: <line>` — one line of standard output, in order;
//! - `// expect-exit: <status>` — process exit status (default 0);
//! - `// expect-panic: <code>` — a language panic such as `P0005`;
//! - `// expect-error: <code> [text]` — compilation fails with this
//!   diagnostic, whose message contains `text` when given;
//! - `// expect-warning: <code> [text]` — compilation reports this warning.
//!
//! Executables run with `TONDO_RT_CHECK_LEAKS=1`, so every case also checks
//! that reference counting frees each string exactly once.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
struct Expect {
    stdout: Vec<String>,
    exit: Option<i32>,
    panic: Option<String>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn expectations(source: &str) -> Expect {
    let mut expect = Expect::default();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("// expect-") else { continue };
        let (key, value) = rest.split_once(':').unwrap_or((rest, ""));
        let value = value.strip_prefix(' ').unwrap_or(value).to_string();
        match key {
            "stdout" => expect.stdout.push(value),
            "exit" => expect.exit = Some(value.trim().parse().expect("numeric exit status")),
            "panic" => expect.panic = Some(value.trim().into()),
            "error" => expect.errors.push(value.trim().into()),
            "warning" => expect.warnings.push(value.trim().into()),
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
    let source = std::fs::read_to_string(path).unwrap();
    let expect = expectations(&source);
    let compiler = env!("CARGO_BIN_EXE_tondo");
    let binary = scratch.join(path.file_stem().unwrap());
    let build = Command::new(compiler)
        .arg("build")
        .arg(path)
        .arg("--output")
        .arg(&binary)
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

    let run = Command::new(&binary).env("TONDO_RT_CHECK_LEAKS", "1").output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let expected_stdout: String = expect.stdout.iter().map(|l| format!("{l}\n")).collect();
    if stdout != expected_stdout {
        return Err(format!("stdout differs\n--- expected\n{expected_stdout}--- actual\n{stdout}--- stderr\n{stderr}"));
    }
    let expected_exit = expect.exit.unwrap_or(if expect.panic.is_some() { 101 } else { 0 });
    if run.status.code() != Some(expected_exit) {
        return Err(format!("exit status {:?}, expected {expected_exit}\n{stderr}", run.status.code()));
    }
    if let Some(code) = &expect.panic
        && !stderr.contains(&format!("panic[{code}]"))
    {
        return Err(format!("expected panic {code}, stderr:\n{stderr}"));
    }
    Ok(())
}

#[test]
fn cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases");
    let mut paths = Vec::new();
    collect(&root, &mut paths);
    paths.sort();
    assert!(!paths.is_empty(), "no cases found under {}", root.display());

    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cases");
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
