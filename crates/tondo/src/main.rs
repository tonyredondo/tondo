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
  tondo test [<project>] [--filter <text> | --exact <id>] [--list] [--jobs <n>] [--timeout <seconds>]
            [--show-output] [--allow-empty] [--deny-skips]
                                                       run the tests of a project
  tondo bind [<header.h>...] [--package <name>] [--library <library>]... [--project <dir>] [--output <file.to>] [-I <dir>] [-D <name[=value]>]
                                                       write a module of extern declarations for a C header
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
        "bind" => bind_command(&args[1..]),
        "test" => test_command(&args[1..]),
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

/// Builds the tests of a project, runs those selected and reports them
/// (testing 10).
fn test_command(args: &[OsString]) -> Result<ExitCode, String> {
    use tondo::runner::{self, Status};
    let (path, rest) = match args.first() {
        Some(first) if !first.to_string_lossy().starts_with('-') => (PathBuf::from(first), &args[1..]),
        _ => source_argument(args),
    };
    let mut options = runner::Options {
        filter: None,
        exact: None,
        list: false,
        jobs: 1,
        timeout: std::time::Duration::from_secs(60),
        show_output: false,
        allow_empty: false,
        deny_skips: false,
    };
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let mut value = |what: &str| {
            rest.next()
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{what} needs a value\n{USAGE}"))
        };
        match arg.to_string_lossy().as_ref() {
            "--filter" if options.filter.is_none() && options.exact.is_none() => {
                options.filter = Some(value("--filter")?)
            }
            "--exact" if options.filter.is_none() && options.exact.is_none() => options.exact = Some(value("--exact")?),
            "--filter" | "--exact" => return Err(format!("give one of --filter and --exact, once\n{USAGE}")),
            "--list" => options.list = true,
            "--jobs" => {
                options.jobs = value("--jobs")?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| format!("--jobs takes a positive number\n{USAGE}"))?
            }
            "--timeout" => {
                let seconds: f64 = value("--timeout")?
                    .parse()
                    .ok()
                    .filter(|&s: &f64| s > 0.0 && s.is_finite())
                    .ok_or_else(|| format!("--timeout takes a positive number of seconds\n{USAGE}"))?;
                options.timeout = std::time::Duration::from_secs_f64(seconds);
            }
            "--show-output" => options.show_output = true,
            "--allow-empty" => options.allow_empty = true,
            "--deny-skips" => options.deny_skips = true,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    // The build is reused while nothing it reads changes.
    let dir = runner::build_dir(&path)?;
    let (executable, tree_file) = (dir.join("tests"), dir.join("tree"));
    let tree = match runner::Tree::load(&tree_file).filter(|_| executable.is_file()) {
        Some(tree) => tree,
        None => {
            let analysis = driver::analyze_tests(&path)?;
            if !analysis.diags.list.is_empty() {
                eprint!("{}", analysis.diags.render(&analysis.sources));
            }
            let Some(program) = &analysis.program else {
                return Ok(ExitCode::FAILURE);
            };
            let locations = program.tests.iter().map(|n| analysis.sources.location(n.span)).collect();
            let tree = runner::Tree { nodes: program.tests.clone(), locations };
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            // Linked aside and renamed, so another run of the same tests keeps its own.
            let partial = executable.with_extension(format!("partial-{}", std::process::id()));
            driver::link(&driver::object(&analysis)?, &partial, &analysis.libraries)?;
            std::fs::rename(&partial, &executable)
                .map_err(|e| format!("cannot write {}: {e}", executable.display()))?;
            tree.save(&tree_file)?;
            tree
        }
    };
    let selected = runner::select(&tree, &options)?;
    if selected.is_empty() {
        let message = match (&options.filter, &options.exact) {
            (Some(text), _) => format!("no test matches --filter {text:?}"),
            (_, Some(id)) => format!("no suite or test is {id:?}; `tondo test --list` lists them"),
            _ => "the project has no tests: write `test name { ... }` in a `_test.to` file or under `tests/`".into(),
        };
        if options.allow_empty {
            println!("{message}");
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!("tondo: {message}");
        return Ok(ExitCode::FAILURE);
    }
    if options.list {
        for node in runner::listed(&tree, &selected) {
            let suffix = if tree.nodes[node as usize].suite { " (suite)" } else { "" };
            println!("{}{suffix}", tree.nodes[node as usize].id);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let started = std::time::Instant::now();
    // Each test's line in canonical order, as soon as the ones before it
    // are known.
    let mut known: std::collections::HashMap<u32, runner::Outcome> = std::collections::HashMap::new();
    let mut printed = 0;
    let line = |outcome: &runner::Outcome| {
        let id = &tree.nodes[outcome.node as usize].id;
        let status = match &outcome.status {
            Status::Passed => "ok".to_string(),
            Status::Skipped(reason) => format!("skipped: {}", reason.lines().next().unwrap_or_default()),
            Status::Blocked(suite, true) => format!("skipped: its suite {} skipped", tree.nodes[*suite as usize].id),
            Status::Blocked(suite, false) => format!("BLOCKED: its suite {} failed", tree.nodes[*suite as usize].id),
            Status::Timeout => "TIMEOUT".into(),
            Status::Crashed(_) => "CRASHED".into(),
            Status::Error(_) | Status::Panic(_) => "FAILED".into(),
        };
        println!("test {id} ... {status}");
    };
    let outcomes = runner::run(&executable, &path, &tree, &selected, &options, |outcome| {
        if !tree.nodes[outcome.node as usize].suite {
            known.insert(outcome.node, outcome.clone());
        }
        while let Some(outcome) = selected.get(printed).and_then(|t| known.get(t)) {
            line(outcome);
            printed += 1;
        }
    });
    let outcomes = runner::summary(&tree, &outcomes);
    // Failures, with what each wrote; skips with their reasons.
    let mut failures = Vec::new();
    for outcome in &outcomes {
        let node = &tree.nodes[outcome.node as usize];
        let what = if node.suite {
            match outcome.status {
                Status::Passed => format!("suite {}", node.id),
                _ if outcome.teardown => format!("suite {} (teardown)", node.id),
                _ => format!("suite {} (setup)", node.id),
            }
        } else {
            node.id.clone()
        };
        let detail = match &outcome.status {
            Status::Error(error) if error.contains("\n --> ") => format!("error: {error}"),
            Status::Error(error) => format!("error: {error}\n --> {}", tree.locations[outcome.node as usize]),
            Status::Panic(text) => text.clone(),
            Status::Timeout => format!("timed out after {}s", options.timeout.as_secs_f64()),
            Status::Crashed(how) => how.clone(),
            Status::Skipped(reason) if node.suite => format!("skipped: {reason}"),
            Status::Passed | Status::Skipped(_) | Status::Blocked(..) if !options.show_output => continue,
            _ => String::new(),
        };
        let mut text = format!("--- {what}\n");
        if !detail.is_empty() {
            text.push_str(&detail);
            text.push('\n');
        }
        for (label, bytes) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
            if !bytes.is_empty() {
                text.push_str(&format!("{label}:\n{}", String::from_utf8_lossy(bytes)));
                if !bytes.ends_with(b"\n") {
                    text.push('\n');
                }
            }
        }
        if !outcome.logs.is_empty() {
            text.push_str(&format!("logs:\n{}\n", outcome.logs.join("\n")));
        }
        if !outcome.tags.is_empty() {
            let tags: Vec<String> = outcome.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
            text.push_str(&format!("tags: {}\n", tags.join(", ")));
        }
        failures.push(text);
    }
    if !failures.is_empty() {
        println!("\n{}", failures.join("\n"));
    }
    let tests: Vec<&runner::Outcome> = outcomes.iter().filter(|o| !tree.nodes[o.node as usize].suite).collect();
    let count = |f: fn(&Status) -> bool| tests.iter().filter(|o| f(&o.status)).count();
    let failed = runner::failed(&outcomes, options.deny_skips);
    println!(
        "{}. {} passed; {} failed; {} skipped; {} blocked; {:.2}s",
        if failed { "FAILED" } else { "ok" },
        count(|s| *s == Status::Passed),
        count(|s| matches!(s, Status::Error(_) | Status::Panic(_) | Status::Timeout | Status::Crashed(_))),
        count(|s| matches!(s, Status::Skipped(_) | Status::Blocked(_, true))),
        count(|s| matches!(s, Status::Blocked(_, false))),
        started.elapsed().as_secs_f64()
    );
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
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

/// Writes a module of `extern fn`, `extern type`, aliases and constants
/// for C headers, and adds the library to `[native] libraries`.
fn bind_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (mut headers, mut libraries, mut arguments) = (Vec::new(), Vec::new(), Vec::new());
    let (mut project, mut output, mut package) = (PathBuf::from("."), None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        let mut value = |what: &str| args.next().cloned().ok_or_else(|| format!("{what} needs a value\n{USAGE}"));
        match text.as_ref() {
            "--library" | "--link" => libraries.push(value("--library")?.to_string_lossy().into_owned()),
            "--project" => project = PathBuf::from(value("--project")?),
            "--package" => package = Some(value("--package")?.to_string_lossy().into_owned()),
            "--output" | "-o" => output = Some(PathBuf::from(value("--output")?)),
            "-I" | "-D" => arguments.push(format!("{text}{}", value(&text)?.to_string_lossy())),
            flag if flag.starts_with("-I") || flag.starts_with("-D") => arguments.push(flag.to_string()),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}\n{USAGE}")),
            _ => headers.push(PathBuf::from(arg)),
        }
    }
    let command = std::iter::once("tondo bind".to_string())
        .chain(std::env::args().skip(2).map(|a| if a.contains(' ') { format!("{a:?}") } else { a }))
        .collect::<Vec<_>>()
        .join(" ");
    // Without headers: those of a package, or a library's debug information.
    let mut library_file = None;
    let mut name = headers.first().cloned();
    if headers.is_empty() {
        if let Some(package) = &package {
            let found = tondo::bind::find_package(package)?;
            headers = found.headers;
            arguments.extend(found.arguments);
            libraries.extend(found.libraries);
            library_file = found.library_file;
            name = Some(PathBuf::from(format!("{package}.h")));
        } else if let Some(library) = libraries.first().map(PathBuf::from).filter(|l| l.is_file()) {
            let stem = library.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let stem = stem.strip_prefix("lib").unwrap_or(&stem).split('.').next().unwrap_or_default().to_string();
            name = Some(PathBuf::from(format!("{stem}.h")));
            library_file = Some(library);
        } else {
            return Err(format!("expected a C header, a --package, or a --library file to read\n{USAGE}"));
        }
    }
    let module = match &library_file {
        Some(library) if headers.is_empty() => tondo::bind::generate_from_library(library, &command)?,
        _ => tondo::bind::generate(&headers, &arguments, &command)?,
    };
    let mut sources = tondo::source::SourceMap::default();
    let index = sources.add(Path::new("bound.to"), module.clone());
    let mut diags = tondo::diag::Diagnostics::default();
    // The canonical format, or the text as generated if the formatter rejects it.
    let module = tondo::fmt::format(sources.file(index), &mut diags).unwrap_or(module);
    let (name, path) = match output {
        Some(path) => (path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), path),
        None => tondo::bind::module_path(&project, name.as_deref().unwrap_or(Path::new("native.h"))),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, module).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    eprintln!("wrote {}: import it as a module of the project, `import <package>.{name}`", path.display());
    // Only a project's manifest takes the libraries.
    let is_project = ["tondo.toml", "src/main.to", "main.to"].iter().any(|f| project.join(f).is_file());
    for library in &libraries {
        if !is_project {
            eprintln!("link {library:?}: add it to [native] libraries in the project's tondo.toml");
        } else if tondo::bind::add_library(&project, library)? {
            eprintln!("added {library:?} to [native] libraries in tondo.toml");
        }
    }
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
            libraries.push(tondo::deps::library_entry(&library.to_string_lossy(), &cwd));
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
