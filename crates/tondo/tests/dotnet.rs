//! A profiler of .NET written in Tondo, in `tests/dotnet`: `tondo bind`
//! binds .NET's corprof.h, `tondo build --library` builds the profiler, and
//! a console program of .NET runs under it. On Linux, where `dotnet` is
//! installed with its SDK; elsewhere the test says it was skipped.

#![cfg(all(target_os = "linux", target_env = "gnu"))]

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

/// The profiler's identity, as `tests/dotnet/profiler` gives it.
const PROFILER: &str = "{7a4f2c1e-9b3d-4e6a-8c5f-1d2e3f4a5b6c}";

/// The standard output of `command`, which must end well.
fn run(command: &mut Command, what: &str) -> String {
    let child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let output = common::ended(child, what);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{what} failed:\n{stdout}{}", String::from_utf8_lossy(&output.stderr));
    stdout
}

/// `dotnet`, without its banner, its telemetry or the servers that a build
/// leaves running.
fn dotnet() -> Command {
    let mut command = common::command("dotnet");
    command.env("DOTNET_NOLOGO", "1").env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
    command
}

#[test]
fn a_profiler_of_dotnet_in_tondo() {
    if !dotnet().arg("--version").output().is_ok_and(|output| output.status.success()) {
        eprintln!("skipped: no `dotnet` with its SDK on PATH");
        return;
    }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/dotnet");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dotnet");
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("profiler");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::copy(fixtures.join("profiler/tondo.toml"), project.join("tondo.toml")).unwrap();
    std::fs::copy(fixtures.join("profiler/src/profiler.to"), project.join("src/profiler.to")).unwrap();

    let compiler = env!("CARGO_BIN_EXE_tondo");
    let include = fixtures.join("include");
    run(
        common::command(compiler)
            .arg("bind")
            .arg(include.join("corprof.h"))
            .arg("-I")
            .arg(&include)
            .arg("--project")
            .arg(&project),
        "tondo bind",
    );
    let built = dir.join("built");
    run(
        common::command(compiler).args(["build", "--library"]).arg(&project).arg("--output").arg(&built),
        "tondo build",
    );

    let app = dir.join("app");
    run(dotnet().args(["new", "console", "--no-restore", "--output"]).arg(&app), "dotnet new");
    run(
        dotnet().arg("build").arg(&app).args(["--disable-build-servers", "--output"]).arg(app.join("build")),
        "dotnet build",
    );

    let ran = run(
        dotnet()
            .arg(app.join("build/app.dll"))
            .env("CORECLR_ENABLE_PROFILING", "1")
            .env("CORECLR_PROFILER", PROFILER)
            .env("CORECLR_PROFILER_PATH", built.join("libprofiler.so")),
        "the program of .NET under the profiler",
    );
    let lines: Vec<&str> = ran.lines().collect();
    let module = |name: &str| lines.iter().any(|line| line.starts_with("module /") && line.ends_with(name));
    assert_eq!(lines.first(), Some(&"initialize"), "{ran}");
    assert!(module("/System.Private.CoreLib.dll"), "{ran}");
    assert!(module("/app/build/app.dll"), "{ran}");
    assert!(lines.contains(&"Hello, World!"), "{ran}");
    assert_eq!(lines.last(), Some(&"shutdown"), "{ran}");
}
