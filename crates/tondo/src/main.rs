//! The `tondo` command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tondo::driver;

const USAGE: &str = "\
usage:
  tondo run [<project>|<file.to>] [--link <library>]... [-- <argument>...]
                                                       compile and run a program
  tondo build [<project>|<file.to>] [--output <path>] [--link <library>]...
                                                       compile a native executable
  tondo check [<project>|<file.to>] [--json]           report diagnostics only (as JSON)
  tondo fmt [--check] <file.to>...                     format files canonically
  tondo update [<project>] [<dependency>...]           fetch dependencies again and rewrite tondo.lock
  tondo <file.to> [<argument>...]                      run a script (for #! lines)
  tondo version

A project is a directory with src/main.to, or with main.to or tondo.toml,
given as <project> or with --project <dir>; the current directory by default.
--link links a native library by name (-l) or file, besides a project's
[native] libraries.";

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
        "fmt" => fmt_command(&args[1..]),
        "update" => update_command(&args[1..]),
        "version" | "--version" | "-V" => {
            println!("tondo {} (native, cranelift)", driver::VERSION);
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        file if file.ends_with(".to") => run(Path::new(file), &args[1..], &[]),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    };
    result.unwrap_or_else(|message| {
        eprintln!("tondo: {message}");
        // Usage errors end with the usage text.
        ExitCode::from(if message.ends_with(USAGE) { 2 } else { TOOL_FAILURE })
    })
}

/// The program to compile, and the arguments after it.
fn source_argument(args: &[OsString]) -> (PathBuf, &[OsString]) {
    match args {
        [flag, dir, rest @ ..] if flag == "--project" => (PathBuf::from(dir), rest),
        [path, rest @ ..] if path != "--" && !path.to_string_lossy().starts_with('-') => (PathBuf::from(path), rest),
        _ => (PathBuf::from("."), args),
    }
}

/// The name of the executable built from `path`: the file's stem, or the
/// project directory's name.
fn program_name(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_dir() { std::fs::canonicalize(path).map_err(|e| e.to_string())? } else { path.to_path_buf() };
    let name = if path.is_dir() { path.file_name() } else { path.file_stem() };
    name.map(PathBuf::from).ok_or_else(|| format!("cannot name a program after {}", path.display()))
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
    let (path, rest) = source_argument(args);
    let json = match rest {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err(format!("unexpected arguments\n{USAGE}")),
    };
    if json {
        let analysis = driver::analyze(&path)?;
        print!("{}", analysis.diags.render_json(&analysis.sources, &analysis.source_id));
        return Ok(if analysis.program.is_some() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
    }
    Ok(if analyze(&path)?.is_some() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Resolves the named dependencies, or all of them, again instead of
/// using `tondo.lock`.
fn update_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (path, rest) = match args.first() {
        Some(first) if Path::new(first).is_dir() => (PathBuf::from(first), &args[1..]),
        _ => source_argument(args),
    };
    let names: Vec<String> = rest.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    tondo::deps::resolve(&path, Some(&names))?;
    Ok(ExitCode::SUCCESS)
}

/// Rewrites files in the canonical format (21), or with `--check` reports
/// those that are not.
fn fmt_command(args: &[OsString]) -> Result<ExitCode, String> {
    let check = args.iter().any(|a| a == "--check");
    let files: Vec<&OsString> = args.iter().filter(|a| *a != "--check").collect();
    if files.is_empty() {
        return Err(format!("expected a source file\n{USAGE}"));
    }
    let mut status = ExitCode::SUCCESS;
    for file in files {
        let path = Path::new(file);
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut sources = tondo::source::SourceMap::default();
        let index = sources.add(path, text.clone());
        let mut diags = tondo::diag::Diagnostics::default();
        let Some(formatted) = tondo::fmt::format(sources.file(index), &mut diags) else {
            eprint!("{}", diags.render(&sources));
            status = ExitCode::FAILURE;
            continue;
        };
        if formatted == text {
            continue;
        }
        if check {
            eprintln!("{} is not formatted", path.display());
            status = ExitCode::FAILURE;
        } else {
            std::fs::write(path, formatted).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }
    Ok(status)
}

/// The `--link <library>` options before `--`, and the other arguments.
fn link_options(args: &[OsString]) -> Result<(Vec<String>, Vec<OsString>), String> {
    let (mut libraries, mut rest) = (Vec::new(), Vec::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            rest.push(arg.clone());
            rest.extend(args.by_ref().cloned());
        } else if arg == "--link" {
            let library = args.next().ok_or_else(|| format!("--link needs a library\n{USAGE}"))?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            libraries.push(tondo::deps::link_argument(&library.to_string_lossy(), &cwd));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((libraries, rest))
}

fn build_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (path, rest) = source_argument(args);
    let (libraries, rest) = link_options(rest)?;
    let output = match rest.as_slice() {
        [] => program_name(&path)?,
        [flag, output] if flag == "--output" || flag == "-o" => PathBuf::from(output),
        _ => return Err(format!("unexpected arguments\n{USAGE}")),
    };
    let Some(analysis) = analyze(&path)? else {
        return Ok(ExitCode::FAILURE);
    };
    let libraries = [analysis.libraries.clone(), libraries].concat();
    driver::link(&driver::object(&analysis)?, &output, &libraries)?;
    Ok(ExitCode::SUCCESS)
}

fn run_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (path, rest) = source_argument(args);
    let (libraries, rest) = link_options(rest)?;
    let program_args = match rest.as_slice() {
        [] => &[][..],
        [separator, rest @ ..] if separator == "--" => rest,
        _ => return Err(format!("program arguments go after `--`\n{USAGE}")),
    };
    run(&path, program_args, &libraries)
}

/// Compiles (or reuses a cached build of) `path` and executes it.
fn run(path: &Path, program_args: &[OsString], libraries: &[String]) -> Result<ExitCode, String> {
    let identity = driver::compiler_identity();
    let mut parts = vec![identity.into_bytes(), libraries.join("\n").into_bytes()];
    for input in driver::inputs(path)? {
        parts.push(input.as_os_str().as_encoded_bytes().to_vec());
        parts.push(std::fs::read(&input).map_err(|e| format!("cannot read {}: {e}", input.display()))?);
    }
    let key = driver::fingerprint(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let dir = driver::cache_dir().join("run").join(key);
    let executable = dir.join(program_name(path)?);
    if !executable.exists() {
        let Some(analysis) = analyze(path)? else {
            return Ok(ExitCode::FAILURE);
        };
        let object = driver::object(&analysis)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let partial = dir.join(format!(".partial-{}", std::process::id()));
        let libraries = [analysis.libraries.clone(), libraries.to_vec()].concat();
        driver::link(&object, &partial, &libraries)?;
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
