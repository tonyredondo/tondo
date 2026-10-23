//! The `tondo` command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tondo::driver;

const USAGE: &str = "\
New to Tondo, or a model writing it? Start with `tondo guide`.

usage:
  tondo run [<project>|<file.to>] [--link <library>]... [-- <argument>...]
                                                       compile and run a program
  tondo build [<project>|<file.to>] [--output <path>] [--link <library>]...
                                                       compile a native executable
  tondo check [<project>|<file.to>] [--json]           report diagnostics only (as JSON)
  tondo expand [<project>|<file.to>]                   show what each `derive` expanded to
  tondo fmt [--check | --compact] <file.to>...         format files canonically, or print them compactly
  tondo api [<module>|<file.to>] [--private] [--list] [--project <dir>]
                                                       show a module's declarations and documentation
  tondo guide [<topic>] [--list] [--skill]             learn Tondo: the language, the library, the commands
  tondo update [<project>] [<dependency>...]           fetch dependencies again and rewrite tondo.lock
  tondo test [<project>] [--filter <text> | --glob <pattern> | --exact <id>] [--shard <i/n>]
            [--order canonical|random] [--schedules <n>] [--seed <hex>] [--list] [--jobs <n>] [--timeout <seconds>]
            [--retry <n>] [--allow-flaky] [--repeat <n>] [--codeowners auto|none|<path>]
            [--show-output] [--allow-empty] [--deny-skips] [--test-format human|json]
            [--report json=<path> | junit=<path>]... [--diagnostic-format human|json]
            [--artifacts <dir>] [--update-snapshots]
                                                       run the tests of a project
  tondo lsp                                            serve an editor: diagnostics, hover, definitions,
                                                       references, rename, completion, highlighting
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
    // Output into a closed pipe ends `tondo` quietly, as `tondo api | head`
    // expects, instead of failing to print.
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn signal(signal: i32, handler: usize) -> usize;
        }
        const SIGPIPE: i32 = 13;
        const SIG_DFL: usize = 0;
        unsafe { signal(SIGPIPE, SIG_DFL) };
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(command) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match command {
        "run" => run_command(&args[1..]),
        "build" => build_command(&args[1..]),
        "check" => check_command(&args[1..]),
        "expand" => expand_command(&args[1..]),
        "fmt" => fmt_command(&args[1..]),
        "api" => api_command(&args[1..]),
        "guide" => guide_command(&args[1..]),
        "update" => update_command(&args[1..]),
        "bind" => bind_command(&args[1..]),
        "test" => test_command(&args[1..]),
        "lsp" => tondo::lsp::serve().map(|()| ExitCode::SUCCESS),
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

/// Shows the `impl` each `derive` of the program expanded to (27.3).
fn expand_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (path, rest) = source_argument(args);
    if !rest.is_empty() {
        return Err(format!("unexpected arguments\n{USAGE}"));
    }
    let analysis = driver::analyze(&path)?;
    for expansion in &analysis.expansions {
        println!("// {} ({})\n{}", expansion.request, expansion.location, expansion.source.trim_end());
        println!();
    }
    if analysis.diags.has_errors() {
        eprint!("{}", analysis.diags.render(&analysis.sources));
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// Builds the tests of a project, runs those selected and reports them
/// (testing 10).
fn test_command(args: &[OsString]) -> Result<ExitCode, String> {
    use tondo::runner::{self, Codeowners, Glob, Order, Selector};
    let (path, rest) = match args.first() {
        Some(first) if !first.to_string_lossy().starts_with('-') => (PathBuf::from(first), &args[1..]),
        _ => source_argument(args),
    };
    let mut options = runner::Options {
        selector: Selector::All,
        shard: None,
        order: Order::Canonical,
        list: false,
        jobs: 1,
        timeout: std::time::Duration::from_secs(60),
        retry: 0,
        repeat: 1,
        schedules: None,
        codeowners: Codeowners::Auto,
        show_output: false,
        allow_empty: false,
        deny_skips: false,
        allow_flaky: false,
        json: false,
        reports: Vec::new(),
        artifacts: None,
        update_snapshots: false,
    };
    let mut diagnostics_json = false;
    let usage = |message: String| format!("{message}\n{USAGE}");
    // A count written canonically: `0` or digits without a leading zero.
    let count = |text: &str, flag: &str| {
        text.parse::<u64>()
            .ok()
            .filter(|_| text == "0" || (!text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit())))
            .ok_or_else(|| usage(format!("{flag} takes a number written without sign or leading zeros")))
    };
    let (mut seed, mut random, mut seen) = (None, false, Vec::new());
    let mut schedules = None;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let flag = arg.to_string_lossy().into_owned();
        if seen.contains(&flag) && flag != "--report" {
            return Err(usage(format!("{flag} appears twice")));
        }
        seen.push(flag.clone());
        let mut value = || {
            rest.next().map(|v| v.to_string_lossy().into_owned()).ok_or_else(|| usage(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "--filter" | "--glob" | "--exact" if !matches!(options.selector, Selector::All) => {
                return Err(usage("give only one of --filter, --glob and --exact".into()));
            }
            "--filter" => options.selector = Selector::Filter(value()?),
            "--glob" => options.selector = Selector::Glob(Glob::parse(&value()?).map_err(usage)?),
            "--exact" => options.selector = Selector::Exact(value()?),
            "--shard" => {
                let text = value()?;
                let parts = text.split_once('/').map(|(i, n)| (count(i, "--shard"), count(n, "--shard")));
                match parts {
                    Some((Ok(index), Ok(total))) if index >= 1 && index <= total => {
                        options.shard = Some((index, total))
                    }
                    _ => return Err(usage(format!("--shard takes index/count with 1 <= index <= count, not {text}"))),
                }
            }
            "--order" => match value()?.as_str() {
                "canonical" => random = false,
                "random" => random = true,
                other => return Err(usage(format!("--order is canonical or random, not {other}"))),
            },
            "--seed" => {
                let text = value()?;
                let valid = (1..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit());
                seed = Some(
                    valid
                        .then(|| u64::from_str_radix(&text, 16).ok())
                        .flatten()
                        .ok_or_else(|| usage(format!("--seed takes one to sixteen hexadecimal digits, not {text}")))?,
                );
            }
            "--list" => options.list = true,
            "--jobs" => {
                options.jobs = count(&value()?, "--jobs")
                    .ok()
                    .filter(|&n| n > 0)
                    .map(|n| n as usize)
                    .ok_or_else(|| usage("--jobs takes a positive number".into()))?
            }
            "--timeout" => {
                let seconds: f64 = value()?
                    .parse()
                    .ok()
                    .filter(|&s: &f64| s > 0.0 && s.is_finite())
                    .ok_or_else(|| usage("--timeout takes a positive number of seconds".into()))?;
                options.timeout = std::time::Duration::from_secs_f64(seconds);
            }
            "--retry" => options.retry = count(&value()?, "--retry")? as u32,
            "--repeat" => {
                options.repeat = count(&value()?, "--repeat")?
                    .try_into()
                    .ok()
                    .filter(|&n: &u32| n > 0)
                    .ok_or_else(|| usage("--repeat takes a positive number".into()))?
            }
            "--schedules" => {
                schedules = Some(
                    count(&value()?, "--schedules")?
                        .try_into()
                        .ok()
                        .filter(|&n: &u32| n > 0)
                        .ok_or_else(|| usage("--schedules takes a positive number".into()))?,
                )
            }
            "--codeowners" => {
                options.codeowners = match value()?.as_str() {
                    "auto" => Codeowners::Auto,
                    "none" => Codeowners::None,
                    "" => return Err(usage("--codeowners takes auto, none or a path".into())),
                    path => Codeowners::Path(path.to_string()),
                }
            }
            "--show-output" => options.show_output = true,
            "--allow-empty" => options.allow_empty = true,
            "--deny-skips" => options.deny_skips = true,
            "--allow-flaky" => options.allow_flaky = true,
            "--update-snapshots" => options.update_snapshots = true,
            "--artifacts" => options.artifacts = Some(PathBuf::from(value()?)),
            "--test-format" | "--diagnostic-format" => {
                let json = match value()?.as_str() {
                    "human" => false,
                    "json" => true,
                    other => return Err(usage(format!("{flag} is human or json, not {other}"))),
                };
                if flag == "--test-format" {
                    options.json = json;
                } else {
                    diagnostics_json = json;
                }
            }
            "--report" => {
                let text = value()?;
                let report = match text.split_once('=') {
                    Some(("json", path)) if !path.is_empty() => (false, PathBuf::from(path)),
                    Some(("junit", path)) if !path.is_empty() => (true, PathBuf::from(path)),
                    _ => return Err(usage(format!("--report takes json=<path> or junit=<path>, not {text}"))),
                };
                if options.reports.iter().any(|(_, p)| *p == report.1) {
                    return Err(usage(format!("two reports go to {}", report.1.display())));
                }
                options.reports.push(report);
            }
            other => return Err(usage(format!("unknown option {other}"))),
        }
    }
    let given = |flag: &str| seen.iter().any(|s| s == flag);
    if seed.is_some() && !random && schedules.is_none() {
        return Err(usage("--seed goes with --order random or --schedules".into()));
    }
    let campaign = ["--repeat", "--retry", "--allow-flaky", "--list", "--update-snapshots"];
    if schedules.is_some()
        && let Some(flag) = campaign.iter().find(|f| given(f))
    {
        return Err(usage(format!("--schedules runs everything once per order; it does not go with {flag}")));
    }
    if given("--repeat") && (given("--retry") || given("--allow-flaky")) {
        return Err(usage("--repeat runs everything again; it does not go with --retry or --allow-flaky".into()));
    }
    let listing = ["--show-output", "--deny-skips", "--retry", "--repeat", "--allow-flaky"];
    if options.list
        && let Some(flag) = listing.iter().find(|f| given(f))
    {
        return Err(usage(format!("--list runs nothing, so it does not go with {flag}")));
    }
    if options.update_snapshots {
        // Snapshots are written in one known order (9.7).
        let incompatible = ["--shard", "--retry", "--repeat", "--allow-flaky", "--list"];
        if let Some(flag) = incompatible.iter().find(|f| given(f)) {
            return Err(usage(format!("--update-snapshots does not go with {flag}")));
        }
        if options.jobs != 1 || random {
            return Err(usage("--update-snapshots runs with --jobs 1 and the canonical order".into()));
        }
    }
    if options.list && given("--artifacts") {
        return Err(usage("--list attaches nothing, so it does not go with --artifacts".into()));
    }
    if options.list && !options.reports.is_empty() {
        return Err(usage("--list writes no report; use --test-format json".into()));
    }
    // A seed chosen now is shown, so the orders can be repeated.
    let chosen = seed.is_none();
    let seed = seed.unwrap_or_else(|| {
        let entropy = format!("{:?}{}", std::time::SystemTime::now(), std::process::id());
        let digest = tondo::hash::sha256(entropy.as_bytes());
        u64::from_be_bytes(digest[..8].try_into().unwrap())
    });
    if random {
        options.order = Order::Random(seed, chosen);
    }
    options.schedules = schedules.map(|count| (count, seed, chosen));
    // The build is reused while nothing it reads changes.
    let dir = runner::build_dir(&path)?;
    let (executable, tree_file) = (dir.join("tests"), dir.join("tree"));
    let tree = match runner::Tree::load(&tree_file).filter(|_| executable.is_file()) {
        Some(tree) => tree,
        None => {
            let analysis = driver::analyze_tests(&path)?;
            if diagnostics_json {
                eprint!("{}", analysis.diags.render_json(&analysis.sources, &analysis.source_id));
            } else if !analysis.diags.list.is_empty() {
                eprint!("{}", analysis.diags.render(&analysis.sources));
            }
            let Some(program) = &analysis.program else {
                // A report says it did not compile (testing 15.6).
                return Ok(match runner::uncompiled(&path, &options) {
                    Err(runner::Failure::Output(message)) => {
                        eprintln!("tondo: {message}");
                        ExitCode::from(TOOL_FAILURE)
                    }
                    _ => ExitCode::FAILURE,
                });
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
    match runner::test(&path, &executable, &tree, &options) {
        Ok(failed) => Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }),
        Err(runner::Failure::Input(message) | runner::Failure::Empty(message)) => {
            eprintln!("tondo: {message}");
            Ok(ExitCode::FAILURE)
        }
        Err(runner::Failure::Output(message)) => {
            eprintln!("tondo: {message}");
            Ok(ExitCode::from(TOOL_FAILURE))
        }
    }
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
    let compact = args.iter().any(|a| a == "--compact");
    let files: Vec<&OsString> = args.iter().filter(|a| *a != "--check" && *a != "--compact").collect();
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
        if compact {
            match tondo::compact::compact(sources.file(index), &mut diags) {
                Some(text) => print!("{text}"),
                None => {
                    eprint!("{}", diags.render(&sources));
                    status = ExitCode::FAILURE;
                }
            }
            continue;
        }
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

/// `tondo guide`: the overview, a topic, the topics' names, or the
/// overview as an agent skill.
fn guide_command(args: &[OsString]) -> Result<ExitCode, String> {
    let args: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
    match args.as_slice() {
        [] => print!("{}", tondo::guide::TOPICS[0].1),
        ["--list"] => {
            for (name, _) in tondo::guide::TOPICS {
                println!("{name}");
            }
        }
        ["--skill"] => print!("{}", tondo::guide::skill()),
        [name] => match tondo::guide::topic(name) {
            Some(text) => print!("{text}"),
            None => return Err(format!("there is no topic `{name}`; `tondo guide --list` shows them")),
        },
        _ => return Err(format!("expected one topic\n{USAGE}")),
    }
    Ok(ExitCode::SUCCESS)
}

/// `tondo api`: the declarations of a standard module, a module of the
/// project or its dependencies, or a file; the whole package without one.
fn api_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (mut private, mut list, mut project, mut target) = (false, false, PathBuf::from("."), None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--private") => private = true,
            Some("--list") => list = true,
            Some("--project") => {
                project = rest.next().map(PathBuf::from).ok_or(format!("--project names a directory\n{USAGE}"))?
            }
            Some(name) if !name.starts_with('-') && target.is_none() => target = Some(name.to_string()),
            _ => return Err(format!("unexpected argument `{}`\n{USAGE}", arg.to_string_lossy())),
        }
    }
    // The modules of the project and of its dependencies, by name.
    let modules = || -> Result<Vec<(String, Vec<PathBuf>, bool)>, String> {
        let packages = tondo::deps::resolve(&project, None)?;
        let mut modules: Vec<(String, Vec<PathBuf>, bool)> = Vec::new();
        for (index, package) in packages.iter().enumerate() {
            let (files, _) = tondo::project::sources(&package.dir, &package.name)?;
            for file in files {
                let name = file.module.join(".");
                match modules.iter_mut().find(|(n, _, _)| *n == name) {
                    Some((_, paths, _)) => paths.push(file.path),
                    None => modules.push((name, vec![file.path], index == 0)),
                }
            }
        }
        Ok(modules)
    };
    let render = |paths: &[PathBuf]| -> Result<String, String> {
        let mut out = String::new();
        for path in paths {
            let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            out.push_str(&tondo::api::render(&text, private));
        }
        Ok(out)
    };
    if list {
        for name in tondo::api::standard_modules() {
            println!("{name}");
        }
        if let Ok(modules) = modules() {
            for (name, _, _) in modules {
                println!("{name}");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    match target {
        Some(name) if name == "prelude" || name.starts_with("std.") => match tondo::api::standard(&name) {
            Some(text) => print!("{text}"),
            None => return Err(format!("there is no standard module `{name}`; `tondo api --list` shows them")),
        },
        Some(file) if file.ends_with(".to") => print!("{}", render(&[PathBuf::from(file)])?),
        Some(name) => {
            let modules = modules()?;
            let Some((_, paths, _)) = modules.iter().find(|(n, _, _)| *n == name) else {
                return Err(format!("the project has no module `{name}`; `tondo api --list` shows them"));
            };
            print!("{}", render(paths)?);
        }
        None => {
            let mut first = true;
            for (name, paths, own) in modules()? {
                let declarations = render(&paths)?;
                if own && !declarations.is_empty() {
                    print!("{}// module {name}\n\n{declarations}", if first { "" } else { "\n" });
                    first = false;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
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
