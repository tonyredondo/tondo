//! The `tondo` command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tondo::driver;

const USAGE: &str = "\
usage:
  tondo run <file.to> [-- <argument>...]   compile and run a program
  tondo build <file.to> [--output <path>]  compile a native executable
  tondo check <file.to>                    report diagnostics only
  tondo <file.to> [<argument>...]          run a script (for #! lines)
  tondo version";

/// Exit status for usage and toolchain failures (distinct from program errors).
const TOOL_FAILURE: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(command) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match command {
        "run" => run_command(&args[1..]),
        "build" => build_command(&args[1..]),
        "check" => check_command(&args[1..]),
        "version" | "--version" | "-V" => {
            println!("tondo {} (native, cranelift)", driver::VERSION);
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        file if file.ends_with(".to") => run(Path::new(file), &args[1..]),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    };
    result.unwrap_or_else(|message| {
        eprintln!("tondo: {message}");
        ExitCode::from(TOOL_FAILURE)
    })
}

fn source_argument(args: &[OsString]) -> Result<PathBuf, String> {
    match args.first() {
        Some(path) if path != "--" => Ok(PathBuf::from(path)),
        _ => Err(format!("expected a source file\n{USAGE}")),
    }
}

/// Prints diagnostics; returns the analysis only when it has no errors.
fn analyze(path: &Path) -> Result<Option<driver::Analysis>, String> {
    let analysis = driver::analyze(path)?;
    if !analysis.diags.list.is_empty() {
        eprint!("{}", analysis.diags.render(&analysis.sources));
    }
    Ok(analysis.program.is_some().then_some(analysis))
}

fn check_command(args: &[OsString]) -> Result<ExitCode, String> {
    let path = source_argument(args)?;
    Ok(if analyze(&path)?.is_some() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn build_command(args: &[OsString]) -> Result<ExitCode, String> {
    let path = source_argument(args)?;
    let output = match &args[1..] {
        [] => PathBuf::from(path.file_stem().ok_or("invalid source path")?),
        [flag, output] if flag == "--output" || flag == "-o" => PathBuf::from(output),
        _ => return Err(format!("unexpected arguments\n{USAGE}")),
    };
    let Some(analysis) = analyze(&path)? else {
        return Ok(ExitCode::FAILURE);
    };
    driver::link(&driver::object(&analysis)?, &output)?;
    Ok(ExitCode::SUCCESS)
}

fn run_command(args: &[OsString]) -> Result<ExitCode, String> {
    let path = source_argument(args)?;
    let program_args = match &args[1..] {
        [] => &[][..],
        [separator, rest @ ..] if separator == "--" => rest,
        _ => return Err(format!("program arguments go after `--`\n{USAGE}")),
    };
    run(&path, program_args)
}

/// Compiles (or reuses a cached build of) `path` and executes it.
fn run(path: &Path, program_args: &[OsString]) -> Result<ExitCode, String> {
    let source = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let identity = driver::compiler_identity();
    let key = driver::fingerprint(&[identity.as_bytes(), path.as_os_str().as_encoded_bytes(), &source]);
    let dir = driver::cache_dir().join("run").join(key);
    let executable = dir.join(path.file_stem().ok_or("invalid source path")?);
    if !executable.exists() {
        let Some(analysis) = analyze(path)? else {
            return Ok(ExitCode::FAILURE);
        };
        let object = driver::object(&analysis)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let partial = dir.join(format!(".partial-{}", std::process::id()));
        driver::link(&object, &partial)?;
        std::fs::rename(&partial, &executable).map_err(|e| e.to_string())?;
    }
    exec(&executable, program_args)
}

#[cfg(unix)]
fn exec(executable: &Path, args: &[OsString]) -> Result<ExitCode, String> {
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(executable).args(args).exec();
    Err(format!("cannot execute {}: {error}", executable.display()))
}

#[cfg(not(unix))]
fn exec(executable: &Path, args: &[OsString]) -> Result<ExitCode, String> {
    let status = std::process::Command::new(executable).args(args).status().map_err(|e| e.to_string())?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}
