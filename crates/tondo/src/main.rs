//! The `tondo` command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tondo::cli::print_json;
use tondo::driver;
use tondo::target::Target;

/// What a usage error ends with: `main` shows the usage of its command
/// there (`tondo::cli`).
const USAGE: &str = "\u{0}usage";

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
        eprintln!("{}", tondo::cli::usage());
        return ExitCode::from(2);
    };
    // `--help` asks for a command's usage, before the program's arguments.
    let asks_help = args[1..].iter().take_while(|a| *a != "--").any(|a| a == "--help" || a == "-h");
    if asks_help && let Some(help) = tondo::cli::help(command) {
        print!("{help}");
        return ExitCode::SUCCESS;
    }
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
        "graph" => graph_command(&args[1..]),
        "test" => test_command(&args[1..]),
        "doc-test" => doc_test_command(&args[1..]),
        "lsp" => tondo::lsp::serve().map(|()| ExitCode::SUCCESS),
        "mcp" => tondo::mcp::serve().map(|()| ExitCode::SUCCESS),
        "reload" => reload_command(&args[1..]),
        "setup" => match args.get(1).and_then(|a| a.to_str()) {
            None if args.len() == 1 => tondo::setup::setup(false).map(|()| ExitCode::SUCCESS),
            Some("--json") if args.len() == 2 => tondo::setup::setup(true).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("setup takes only --json\n{USAGE}")),
        },
        "ps" => ps_command(&args[1..]),
        "counters" => counters_command(&args[1..]),
        "trace" => trace_command(&args[1..]),
        "dump" => dump_command(&args[1..]),
        "debug" => debug_command(&args[1..]),
        "target" => target_command(&args[1..]),
        "dap" => {
            let program = |path: &Path, live| {
                cached_executable(path, &BuildOptions { debug: true, live, ..BuildOptions::default() })
            };
            let tests = |path: &Path| {
                let build =
                    TestBuild { library: false, debug: true, statically: false, target: None, diagnostics_json: false };
                let built = build_tests(path, &build)?;
                Ok(built.map(|(executable, tree)| (executable, tree.nodes.into_iter().map(|n| n.id).collect())))
            };
            tondo::dap::serve(tondo::dap::Builds { program: &program, tests: &tests }).map(|()| ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            if args.iter().skip(1).any(|a| a == "--json") {
                let target = Target::machine().map(Target::name);
                let fields =
                    serde_json::json!({ "version": driver::VERSION, "backend": "cranelift", "target": target });
                print_json("version", true, fields);
            } else {
                println!("tondo {} (native, cranelift)", driver::VERSION);
            }
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => match args.get(1).and_then(|a| a.to_str()) {
            None => {
                println!("{}", tondo::cli::usage());
                Ok(ExitCode::SUCCESS)
            }
            Some(name) => match tondo::cli::help(name) {
                Some(help) => {
                    print!("{help}");
                    Ok(ExitCode::SUCCESS)
                }
                None => Err(format!("no command `{name}`\n{USAGE}")),
            },
        },
        "completions" => match args.get(1).and_then(|a| a.to_str()).and_then(tondo::cli::completions) {
            Some(script) if args.len() == 2 => {
                print!("{script}");
                Ok(ExitCode::SUCCESS)
            }
            _ => Err(format!("completions are for bash, zsh or fish\n{USAGE}")),
        },
        file if file.ends_with(".to") => run(Path::new(file), &args[1..], &BuildOptions::default()),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    };
    // With `--json`, what goes wrong is JSON too, on stdout.
    let json = args[1..].iter().take_while(|a| *a != "--").any(|a| a == "--json")
        || args.windows(2).any(|w| w[0] == "--test-format" && w[1] != "human");
    if json && let Err(message) = &result {
        let (message, usage) = match message.strip_suffix(USAGE) {
            Some(message) => (message.trim_end(), true),
            None => (message.as_str(), false),
        };
        let status = if usage { 2 } else { TOOL_FAILURE };
        let fields = serde_json::json!({ "message": message, "usage": usage, "status": status });
        print_json("error", false, fields);
        return ExitCode::from(status);
    }
    result.unwrap_or_else(|message| match message.strip_suffix(USAGE) {
        // A usage error shows its command's usage, or every command's.
        Some(message) => {
            eprintln!("tondo: {}", message.trim_end());
            match tondo::cli::help(command) {
                Some(help) => eprint!("{help}"),
                None => eprintln!("{}", tondo::cli::usage()),
            }
            ExitCode::from(2)
        }
        None => {
            eprintln!("tondo: {message}");
            ExitCode::from(TOOL_FAILURE)
        }
    })
}

/// Takes `flag` out of the arguments: whether it was there.
fn take_flag(args: &mut Vec<OsString>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// How the program's diagnostics are shown on stderr: as text, or as one
/// JSON array (`--diagnostic-format`, 22.3).
#[derive(Clone, Copy, Default, PartialEq)]
enum Shown {
    #[default]
    Human,
    Json,
}

fn show(analysis: &driver::Analysis, shown: Shown) {
    if analysis.diags.list.is_empty() {
        return;
    }
    match shown {
        Shown::Human => eprint!("{}", analysis.diags.render(&analysis.sources)),
        Shown::Json => eprintln!("{}", analysis.diags.json(&analysis.sources, &analysis.source_id)),
    }
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

/// Shows diagnostics; returns the analysis only when it has no errors.
fn analyze(path: &Path, library: bool, shown: Shown) -> Result<Option<driver::Analysis>, String> {
    let analysis = if library { driver::analyze_library(path)? } else { driver::analyze(path)? };
    show(&analysis, shown);
    Ok(analysis.program.is_some().then_some(analysis))
}

fn check_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (json, library) = (args.iter().any(|a| a == "--json"), args.iter().any(|a| a == "--library"));
    let args: Vec<OsString> = args.iter().filter(|a| *a != "--json" && *a != "--library").cloned().collect();
    let (path, rest) = source_argument(&args);
    if !rest.is_empty() {
        return Err(format!("unexpected arguments\n{USAGE}"));
    }
    let analysis = if library { driver::analyze_library(&path)? } else { driver::check(&path)? };
    let ok = analysis.program.is_some();
    if json {
        let diagnostics = analysis.diags.json(&analysis.sources, &analysis.source_id);
        print_json("check", ok, serde_json::json!({ "diagnostics": diagnostics }));
    } else {
        show(&analysis, Shown::Human);
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Shows the `impl` each `derive` of the program expanded to (27.3).
fn expand_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let (path, rest) = source_argument(&args);
    if !rest.is_empty() {
        return Err(format!("unexpected arguments\n{USAGE}"));
    }
    let analysis = driver::check(&path)?;
    let ok = !analysis.diags.has_errors();
    if json {
        let expansions: Vec<serde_json::Value> = analysis
            .expansions
            .iter()
            .map(|e| serde_json::json!({ "derive": e.request, "location": e.location, "source": e.source }))
            .collect();
        let diagnostics = analysis.diags.json(&analysis.sources, &analysis.source_id);
        print_json("expand", ok, serde_json::json!({ "expansions": expansions, "diagnostics": diagnostics }));
        return Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE });
    }
    for expansion in &analysis.expansions {
        println!("// {} ({})\n{}", expansion.request, expansion.location, expansion.source.trim_end());
        println!();
    }
    if !ok {
        show(&analysis, Shown::Human);
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
        events: false,
        reports: Vec::new(),
        artifacts: None,
        update_snapshots: false,
        launcher: None,
    };
    let mut diagnostics_json = false;
    let (mut statically, mut debug, mut library) = (false, false, false);
    let mut target = None;
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
            "--static" => statically = true,
            "--debug" => debug = true,
            "--library" => library = true,
            "--target" => target = Some(Target::parse(&value()?)?),
            "--deny-skips" => options.deny_skips = true,
            "--allow-flaky" => options.allow_flaky = true,
            // The report as JSON, as `--test-format json` gives it.
            "--json" => options.json = true,
            "--update-snapshots" => options.update_snapshots = true,
            "--artifacts" => options.artifacts = Some(PathBuf::from(value()?)),
            "--test-format" => match value()?.as_str() {
                "human" => (options.json, options.events) = (false, false),
                "json" => (options.json, options.events) = (true, false),
                "events" => (options.json, options.events) = (true, true),
                other => return Err(usage(format!("--test-format is human, json or events, not {other}"))),
            },
            "--diagnostic-format" => match value()?.as_str() {
                "human" => diagnostics_json = false,
                "json" => diagnostics_json = true,
                other => return Err(usage(format!("--diagnostic-format is human or json, not {other}"))),
            },
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
    driver::ready(target)?;
    options.launcher = target.map(Target::launcher).transpose()?.flatten();
    let built = build_tests(&path, &TestBuild { library, debug, statically, target, diagnostics_json })?;
    let Some((executable, tree)) = built else {
        // A report says it did not compile (testing 15.6).
        return Ok(match runner::uncompiled(&path, &options) {
            Err(runner::Failure::Output(message)) => {
                eprintln!("tondo: {message}");
                ExitCode::from(TOOL_FAILURE)
            }
            _ => ExitCode::FAILURE,
        });
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
        Err(runner::Failure::Interrupted(forced)) => Ok(ExitCode::from(if forced { TOOL_FAILURE } else { 4 })),
    }
}

/// How a project's tests are built.
struct TestBuild {
    library: bool,
    debug: bool,
    statically: bool,
    target: Option<Target>,
    diagnostics_json: bool,
}

/// The executable of a project's tests and their tree, built unless what
/// it reads is unchanged; none, after its diagnostics, when it does not
/// compile.
fn build_tests(path: &Path, build: &TestBuild) -> Result<Option<(PathBuf, tondo::runner::Tree)>, String> {
    let dir = tondo::runner::build_dir(path, build.statically, build.debug, build.target)?;
    let (executable, tree_file) = (dir.join("tests"), dir.join("tree"));
    if let Some(tree) = tondo::runner::Tree::load(&tree_file).filter(|_| executable.is_file()) {
        return Ok(Some((executable, tree)));
    }
    let analysis = driver::analyze_tests(path, build.library)?;
    if build.diagnostics_json {
        eprint!("{}", analysis.diags.render_json(&analysis.sources, &analysis.source_id));
    } else if !analysis.diags.list.is_empty() {
        eprint!("{}", analysis.diags.render(&analysis.sources));
    }
    let Some(program) = &analysis.program else { return Ok(None) };
    let locations = program.tests.iter().map(|n| analysis.sources.location(n.span)).collect();
    let tree = tondo::runner::Tree { nodes: program.tests.clone(), locations };
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    // Linked aside and renamed, so another run of the same tests keeps its own.
    let partial = executable.with_extension(format!("partial-{}", std::process::id()));
    driver::link(
        &driver::object(&analysis, build.debug, build.target)?,
        &partial,
        &analysis.libraries,
        driver::Linking { strip: false, statically: build.statically, target: build.target },
    )?;
    std::fs::rename(&partial, &executable).map_err(|e| format!("cannot write {}: {e}", executable.display()))?;
    tree.save(&tree_file)?;
    Ok(Some((executable, tree)))
}

/// Checks the examples of a Markdown document (21.6): their records as
/// JSON on stdout, and why each failing one fails on stderr.
fn doc_test_command(args: &[OsString]) -> Result<ExitCode, String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let usage = |message: String| format!("{message}\n{USAGE}");
    let markdown = match args.as_slice() {
        [flag, edition, markdown] if flag == "--edition" && edition == "0.1" => markdown,
        [flag, edition, _] if flag == "--edition" => {
            return Err(usage(format!("doc-test checks edition 0.1, not {edition}")));
        }
        [markdown] => markdown,
        _ => return Err(usage("doc-test takes one Markdown file".into())),
    };
    let bytes = std::fs::read(markdown).map_err(|e| format!("cannot read {markdown}: {e}"))?;
    // The file as given, in NFC with `/` between directories.
    use unicode_normalization::UnicodeNormalization;
    let file: String = markdown.replace(std::path::MAIN_SEPARATOR, "/").nfc().collect();
    match tondo::doctest::check(&file, &bytes) {
        Ok(report) => {
            println!("{}", report.records);
            for failure in &report.failures {
                eprintln!("{failure}");
            }
            Ok(if report.failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        Err(error) => {
            eprintln!("{file}:{}: error[{}]: {}", error.line, error.code, error.message);
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Lists the running Tondo programs.
fn ps_command(args: &[OsString]) -> Result<ExitCode, String> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err(format!("ps takes only --json\n{USAGE}")),
    };
    tondo::observe::ps(json).map(|()| ExitCode::SUCCESS)
}

/// Shows the counters of a running program.
fn counters_command(args: &[OsString]) -> Result<ExitCode, String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let usage = |message: &str| format!("{message}\n{USAGE}");
    let (mut name, mut interval, mut count, mut json) = (None, std::time::Duration::from_secs(1), None, false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--interval" => {
                let seconds = rest.next().and_then(|s| s.parse::<f64>().ok()).filter(|s| *s >= 0.1 && *s <= 3600.0);
                let seconds =
                    seconds.ok_or_else(|| usage("--interval takes seconds, from 0.1 to 3600, as --interval 2"))?;
                interval = std::time::Duration::from_secs_f64(seconds);
            }
            "--count" => {
                let n = rest.next().and_then(|n| n.parse::<u64>().ok()).filter(|n| *n > 0);
                count =
                    Some(n.ok_or_else(|| usage("--count takes how many times to show the counters, as --count 1"))?);
            }
            other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
            other => return Err(usage(&format!("counters does not take `{other}`"))),
        }
    }
    let name = name
        .ok_or_else(|| usage("counters takes a running program: its process ID, or its name; `tondo ps` lists them"))?;
    tondo::observe::counters(&name, interval, count, json).map(|()| ExitCode::SUCCESS)
}

/// Debugs a program it builds or starts, or a running debug build.
fn debug_command(args: &[OsString]) -> Result<ExitCode, String> {
    let usage = |message: &str| format!("{message}\n{USAGE}");
    let (mut json, mut target, mut program_args, mut live) = (false, None, Vec::new(), false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str().unwrap_or_default() {
            "--json" => json = true,
            "--live" => live = true,
            "--" => program_args = rest.by_ref().cloned().collect(),
            other if !other.starts_with('-') && target.is_none() => target = Some(arg.clone()),
            other => return Err(usage(&format!("debug does not take `{other}`"))),
        }
    }
    let target = target.ok_or_else(|| {
        usage("debug takes a program: a .to file, a project, an executable, or a running one's process ID or name")
    })?;
    let path = PathBuf::from(&target);
    let debugged = if path.extension().is_some_and(|e| e == "to")
        || path.join("tondo.toml").is_file()
        || path.join("src/main.to").is_file()
    {
        let options = BuildOptions { debug: true, live, ..BuildOptions::default() };
        match cached_executable(&path, &options)? {
            Some(executable) => tondo::debug::Debugged::Started(executable, program_args),
            None => return Ok(ExitCode::FAILURE),
        }
    } else if path.is_file() {
        tondo::debug::Debugged::Started(path, program_args)
    } else {
        tondo::debug::Debugged::Running(target.to_string_lossy().into_owned())
    };
    let status = tondo::debug::debug(debugged, json)?;
    Ok(ExitCode::from(status.clamp(0, 255) as u8))
}

/// Patches a running live build with its program as its sources say now
/// (toolchain 7.10); it goes on with its old code when they do not compile.
fn reload_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let [name] = &args[..] else {
        return Err(format!("reload takes a running live program's process ID or name\n{USAGE}"));
    };
    let reloaded = tondo::live::reload_named(&name.to_string_lossy());
    if json {
        let (ok, fields) = match reloaded {
            Ok(reloaded) => (true, reloaded.json()),
            Err(tondo::live::Refused::Failed(message)) => return Err(message),
            Err(refused) => {
                let restart = matches!(refused, tondo::live::Refused::Restart(_));
                (false, serde_json::json!({ "message": refused.message(), "restart": restart }))
            }
        };
        print_json("reload", ok, fields);
        return Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE });
    }
    match reloaded {
        Ok(reloaded) => {
            println!("{}", reloaded.summary());
            Ok(ExitCode::SUCCESS)
        }
        Err(tondo::live::Refused::Diagnostics(text)) => {
            eprint!("{text}");
            eprintln!("tondo: not reloaded: the program goes on as it was");
            Ok(ExitCode::FAILURE)
        }
        Err(tondo::live::Refused::Restart(message)) => {
            eprintln!("tondo: not reloaded: {message}");
            Ok(ExitCode::FAILURE)
        }
        Err(tondo::live::Refused::Failed(message)) => Err(message),
    }
}

/// Dumps a running program, or reads a dump.
fn dump_command(args: &[OsString]) -> Result<ExitCode, String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let usage = |message: &str| format!("{message}\n{USAGE}");
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
    match rest.as_slice() {
        ["read", file, view @ ..] => {
            let view = match view {
                [] => tondo::observe::View::Summary,
                ["tasks"] => tondo::observe::View::Tasks,
                ["heap"] => tondo::observe::View::Heap,
                ["objects"] => tondo::observe::View::Objects,
                ["object", address] => tondo::observe::View::Object(address.to_string()),
                ["task", id] => {
                    tondo::observe::View::Task(id.parse().map_err(|_| usage("task takes a task's number"))?)
                }
                _ => return Err(usage("dump read shows tasks, task <id>, heap, objects or object <address>")),
            };
            tondo::observe::read(Path::new(file), view, json).map(|()| ExitCode::SUCCESS)
        }
        [name] if !name.starts_with('-') => tondo::observe::dump(name, None, json).map(|()| ExitCode::SUCCESS),
        [name, "--output", path] => {
            tondo::observe::dump(name, Some(PathBuf::from(path)), json).map(|()| ExitCode::SUCCESS)
        }
        _ => Err(usage("dump takes a running program, its process ID or its name, or `read <file>`")),
    }
}

/// Traces a running program, or one it starts.
fn trace_command(args: &[OsString]) -> Result<ExitCode, String> {
    let usage = |message: &str| format!("{message}\n{USAGE}");
    let (mut name, mut duration, mut output, mut program, mut json) = (None, None, None, None, false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str().unwrap_or_default() {
            "--" => {
                program = Some(rest.by_ref().cloned().collect::<Vec<_>>());
                break;
            }
            "--json" => json = true,
            "--duration" => {
                let seconds = rest.next().and_then(|s| s.to_str()?.parse::<f64>().ok()).filter(|s| *s > 0.0);
                let seconds = seconds.ok_or_else(|| usage("--duration takes seconds, as --duration 10"))?;
                duration = Some(std::time::Duration::from_secs_f64(seconds));
            }
            "--output" => output = Some(PathBuf::from(rest.next().ok_or_else(|| usage("--output takes a path"))?)),
            other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
            other => return Err(usage(&format!("trace does not take `{other}`"))),
        }
    }
    let target = match (name, program) {
        (Some(name), None) => tondo::observe::Traced::Running(name),
        (None, Some(program)) if !program.is_empty() => {
            // A Tondo program is built first, as `tondo run` builds it.
            let first = Path::new(&program[0]);
            let source = first.extension().is_some_and(|e| e == "to") || first.join("tondo.toml").is_file();
            let executable = match source {
                true => match cached_executable(first, &BuildOptions::default())? {
                    Some(executable) => executable,
                    None => return Ok(ExitCode::FAILURE),
                },
                false => first.to_path_buf(),
            };
            tondo::observe::Traced::Started(executable, program[1..].to_vec())
        }
        _ => {
            return Err(usage(
                "trace takes a running program, its process ID or its name, or a program to start after `--`",
            ));
        }
    };
    tondo::observe::trace(target, duration, output, json).map(|()| ExitCode::SUCCESS)
}

/// Resolves the named dependencies, or all of them, again instead of
/// using `tondo.lock`.
fn update_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    if let Some(flag) = args.iter().find(|a| a.to_string_lossy().starts_with('-')) {
        return Err(format!("update does not take {}\n{USAGE}", flag.display()));
    }
    let (path, rest) = match args.first() {
        Some(first) if Path::new(first).is_dir() => (PathBuf::from(first), &args[1..]),
        _ => source_argument(&args),
    };
    let names: Vec<String> = rest.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let packages = tondo::deps::resolve(&path, Some(&names))?;
    if json {
        let packages: Vec<serde_json::Value> = packages[1..]
            .iter()
            .map(|p| serde_json::json!({ "name": p.name, "dir": p.dir, "tests_only": p.tests_only }))
            .collect();
        print_json("update", true, serde_json::json!({ "lock": path.join("tondo.lock"), "packages": packages }));
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes a module of `extern fn`, `extern type`, aliases and constants
/// for C headers, and adds the library to `[native] libraries`.
fn bind_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (mut headers, mut libraries, mut arguments) = (Vec::new(), Vec::new(), Vec::new());
    let (mut project, mut output, mut package, mut json) = (PathBuf::from("."), None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        let mut value = |what: &str| args.next().cloned().ok_or_else(|| format!("{what} needs a value\n{USAGE}"));
        match text.as_ref() {
            "--library" | "--link" => libraries.push(value("--library")?.to_string_lossy().into_owned()),
            "--project" => project = PathBuf::from(value("--project")?),
            "--package" => package = Some(value("--package")?.to_string_lossy().into_owned()),
            "--output" | "-o" => output = Some(PathBuf::from(value("--output")?)),
            "--json" => json = true,
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
    if !json {
        eprintln!("wrote {}: import it as a module of the project, `import <package>.{name}`", path.display());
    }
    // Only a project's manifest takes the libraries.
    let is_project = ["tondo.toml", "src/main.to", "main.to"].iter().any(|f| project.join(f).is_file());
    let mut added = Vec::new();
    for library in &libraries {
        if !is_project {
            if !json {
                eprintln!("link {library:?}: add it to [native] libraries in the project's tondo.toml");
            }
        } else if tondo::bind::add_library(&project, library)? {
            if !json {
                eprintln!("added {library:?} to [native] libraries in tondo.toml");
            }
            added.push(library);
        }
    }
    if json {
        print_json(
            "bind",
            true,
            serde_json::json!({ "file": path, "module": name, "libraries": libraries, "added": added }),
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Shows what a program is made of (7.11): its packages, its modules with
/// their files, and what each imports.
fn graph_command(args: &[OsString]) -> Result<ExitCode, String> {
    use tondo::graph::View;
    let (mut path, mut view, mut module, mut reverse, mut output) = (None, View::Modules, None, false, "text");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--packages") => view = View::Packages,
            Some("--files") => view = View::Files,
            Some("--reverse") => reverse = true,
            Some("--module") => {
                module = Some(args.next().and_then(|m| m.to_str()).ok_or(format!("--module names a module\n{USAGE}"))?)
            }
            Some(format @ ("--json" | "--dot" | "--mermaid")) => output = &format[2..],
            Some(flag) if flag.starts_with('-') => return Err(format!("graph does not take {flag}\n{USAGE}")),
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return Err(format!("graph takes one project, directory or file\n{USAGE}")),
        }
    }
    if reverse && module.is_none() {
        return Err(format!("--reverse goes with --module <module>\n{USAGE}"));
    }
    let mut graph = tondo::graph::graph(&path.unwrap_or_else(|| PathBuf::from(".")))?;
    // A directory or file inside a project shows its module.
    if view != View::Packages
        && let Some(module) = module.map(str::to_string).or(graph.focus.clone())
    {
        graph.around(&module, reverse)?;
    }
    match output {
        "json" => print_json("graph", true, graph.json(view == View::Packages)),
        "dot" => print!("{}", graph.dot(view)),
        "mermaid" => print!("{}", graph.mermaid(view)),
        _ => print!("{}", graph.text(view)),
    }
    Ok(ExitCode::SUCCESS)
}

/// Rewrites files in the canonical format (21), or with `--check` reports
/// those that are not.
fn fmt_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let check = take_flag(&mut args, "--check");
    let compact = take_flag(&mut args, "--compact");
    if let Some(flag) = args.iter().find(|a| a.to_string_lossy().starts_with('-')) {
        return Err(format!("fmt does not take {}\n{USAGE}", flag.display()));
    }
    let files = args;
    if files.is_empty() {
        return Err(format!("expected a source file\n{USAGE}"));
    }
    let mut status = ExitCode::SUCCESS;
    // With `--json`: each file, whether it changed (or would, with
    // `--check`), its compact form, or why it is not Tondo.
    let mut reported: Vec<serde_json::Value> = Vec::new();
    for file in &files {
        let path = Path::new(file);
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut sources = tondo::source::SourceMap::default();
        let index = sources.add(path, text.clone());
        let mut diags = tondo::diag::Diagnostics::default();
        let source_id = format!("root:{}", path.display());
        if compact {
            match tondo::compact::compact(sources.file(index), &mut diags) {
                Some(text) if json => reported.push(serde_json::json!({ "file": path, "compact": text })),
                Some(text) => print!("{text}"),
                None => {
                    match json {
                        true => reported
                            .push(serde_json::json!({ "file": path, "diagnostics": diags.json(&sources, &source_id) })),
                        false => eprint!("{}", diags.render(&sources)),
                    }
                    status = ExitCode::FAILURE;
                }
            }
            continue;
        }
        let Some(formatted) = tondo::fmt::format(sources.file(index), &mut diags) else {
            match json {
                true => {
                    reported.push(serde_json::json!({ "file": path, "diagnostics": diags.json(&sources, &source_id) }))
                }
                false => eprint!("{}", diags.render(&sources)),
            }
            status = ExitCode::FAILURE;
            continue;
        };
        let changed = formatted != text;
        if json {
            reported.push(serde_json::json!({ "file": path, "changed": changed }));
        }
        if !changed {
            continue;
        }
        if check {
            if !json {
                eprintln!("{} is not formatted", path.display());
            }
            status = ExitCode::FAILURE;
        } else {
            std::fs::write(path, formatted).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }
    if json {
        print_json("fmt", status == ExitCode::SUCCESS, serde_json::json!({ "files": reported }));
    }
    Ok(status)
}

/// `tondo guide`: the overview, a topic, the topics' names, or the
/// overview as an agent skill.
fn guide_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
    let json = args.contains(&"--json");
    args.retain(|a| *a != "--json");
    if json {
        let fields = match args.as_slice() {
            [] => serde_json::json!({ "topic": tondo::guide::TOPICS[0].0, "text": tondo::guide::TOPICS[0].1 }),
            ["--list"] => {
                serde_json::json!({ "topics": tondo::guide::TOPICS.iter().map(|(name, _)| *name).collect::<Vec<_>>() })
            }
            ["--skill"] => serde_json::json!({ "skill": tondo::guide::skill() }),
            [name] => match tondo::guide::topic(name) {
                Some(text) => serde_json::json!({ "topic": name, "text": text }),
                None => return Err(format!("there is no topic `{name}`; `tondo guide --list` shows them")),
            },
            _ => return Err(format!("expected one topic\n{USAGE}")),
        };
        print_json("guide", true, fields);
        return Ok(ExitCode::SUCCESS);
    }
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
    let (mut private, mut list, mut json, mut project, mut target) = (false, false, false, PathBuf::from("."), None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--private") => private = true,
            Some("--list") => list = true,
            Some("--json") => json = true,
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
        let mut names: Vec<String> = tondo::api::standard_modules().iter().map(|name| name.to_string()).collect();
        if let Ok(modules) = modules() {
            names.extend(modules.into_iter().map(|(name, _, _)| name));
        }
        if json {
            print_json("api", true, serde_json::json!({ "modules": names }));
        } else {
            names.iter().for_each(|name| println!("{name}"));
        }
        return Ok(ExitCode::SUCCESS);
    }
    // What is shown: the module or file named, or each module of the
    // project.
    let (key, shown) = match target {
        Some(name) if name == "prelude" || name.starts_with("std.") => match tondo::api::standard(&name) {
            Some(text) => ("module", vec![(name, text.to_string())]),
            None => return Err(format!("there is no standard module `{name}`; `tondo api --list` shows them")),
        },
        Some(file) if file.ends_with(".to") => ("file", vec![(file.clone(), render(&[PathBuf::from(file)])?)]),
        Some(name) => {
            let modules = modules()?;
            let Some((_, paths, _)) = modules.iter().find(|(n, _, _)| *n == name) else {
                return Err(format!("the project has no module `{name}`; `tondo api --list` shows them"));
            };
            ("module", vec![(name, render(paths)?)])
        }
        None => {
            let mut shown = Vec::new();
            for (name, paths, own) in modules()? {
                let declarations = render(&paths)?;
                if own && !declarations.is_empty() {
                    shown.push((name, declarations));
                }
            }
            ("modules", shown)
        }
    };
    if json {
        let entries: Vec<serde_json::Value> = shown
            .iter()
            .map(|(name, text)| {
                let key = if key == "file" { "file" } else { "module" };
                serde_json::json!({ key: name, "declarations": tondo::api::declarations(text) })
            })
            .collect();
        let fields = match (key, entries.as_slice()) {
            ("modules", _) => serde_json::json!({ "modules": entries }),
            (_, [entry]) => entry.clone(),
            _ => unreachable!(),
        };
        print_json("api", true, fields);
    } else if key == "modules" {
        for (index, (name, declarations)) in shown.iter().enumerate() {
            print!("{}// module {name}\n\n{declarations}", if index == 0 { "" } else { "\n" });
        }
    } else {
        shown.iter().for_each(|(_, text)| print!("{text}"));
    }
    Ok(ExitCode::SUCCESS)
}

/// The `--link <library>` options before `--`, and the other arguments.
/// The native libraries `--link` adds, whether `--static` asks for a
/// static executable, and the other arguments.
/// How a program builds: its extra libraries, whether static, whether a
/// debug build (toolchain 7.6) or a live one (7.10), and for which target
/// (7.8).
#[derive(Default)]
struct BuildOptions {
    libraries: Vec<String>,
    statically: bool,
    debug: bool,
    live: bool,
    /// `run --watch`: reloads the program when its files change.
    watch: bool,
    target: Option<Target>,
    /// How diagnostics are shown.
    shown: Shown,
}

/// The object of a program; a live build's keeps its manifest.
fn object(analysis: &driver::Analysis, path: &Path, options: &BuildOptions) -> Result<Vec<u8>, String> {
    if !options.live {
        return driver::object(analysis, options.debug, options.target);
    }
    let live = tondo::live::Options {
        statically: options.statically,
        target: options.target,
        libraries: options.libraries.clone(),
    };
    tondo::live::compile(analysis, path, &live)
}

fn link_options(args: &[OsString]) -> Result<(BuildOptions, Vec<OsString>), String> {
    let (mut libraries, mut statically, mut debug, mut rest) = (Vec::new(), false, false, Vec::new());
    let (mut target, mut live, mut watch) = (None, false, false);
    let mut shown = Shown::Human;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--diagnostic-format" {
            shown = match args.next().and_then(|v| v.to_str()) {
                Some("human") => Shown::Human,
                Some("json") => Shown::Json,
                _ => return Err(format!("--diagnostic-format is human or json\n{USAGE}")),
            };
        } else if arg == "--target" && target.is_none() {
            let name = args
                .next()
                .ok_or_else(|| format!("--target needs a target: x86_64-linux or aarch64-linux\n{USAGE}"))?;
            target = Some(Target::parse(&name.to_string_lossy())?);
        } else if arg == "--static" && !statically {
            statically = true;
        } else if arg == "--debug" && !debug {
            debug = true;
        } else if arg == "--live" && !live {
            (live, debug) = (true, true);
        } else if arg == "--watch" && !watch {
            (watch, live, debug) = (true, true, true);
        } else if arg == "--" {
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
    driver::ready(target)?;
    Ok((BuildOptions { libraries, statically, debug, live, watch, target, shown }, rest))
}

fn build_command(args: &[OsString]) -> Result<ExitCode, String> {
    if let Some(at) = args.iter().position(|a| a == "--library") {
        let args = [&args[..at], &args[at + 1..]].concat();
        return library_command(&args);
    }
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let (path, rest) = source_argument(&args);
    let (options, rest) = link_options(rest)?;
    let mut output = None;
    let mut strip = false;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--output" | "-o") if output.is_none() => match rest.next() {
                Some(path) => output = Some(PathBuf::from(path)),
                None => return Err(format!("--output needs a path\n{USAGE}")),
            },
            Some("--strip") if !strip => strip = true,
            _ => return Err(format!("unexpected arguments\n{USAGE}")),
        }
    }
    let output = match output {
        Some(output) => output,
        None => program_name(&path)?,
    };
    let analysis = driver::analyze(&path)?;
    let diagnostics = analysis.diags.json(&analysis.sources, &analysis.source_id);
    if !json {
        show(&analysis, options.shown);
    }
    if analysis.program.is_none() {
        if json {
            print_json("build", false, serde_json::json!({ "output": null, "diagnostics": diagnostics }));
        }
        return Ok(ExitCode::FAILURE);
    }
    let object = object(&analysis, &path, &options)?;
    let libraries = [analysis.libraries.clone(), options.libraries].concat();
    let linking = driver::Linking { strip, statically: options.statically, target: options.target };
    driver::link(&object, &output, &libraries, linking)?;
    if json {
        print_json("build", true, serde_json::json!({ "output": output, "diagnostics": diagnostics }));
    }
    Ok(ExitCode::SUCCESS)
}

/// `tondo build --library`: a library for C (toolchain 7).
fn library_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let (path, rest) = source_argument(&args);
    let (mut output, mut debug) = (None, false);
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--output" | "-o") if output.is_none() => match rest.next() {
                Some(dir) => output = Some(PathBuf::from(dir)),
                None => return Err(format!("--output needs a directory\n{USAGE}")),
            },
            Some("--debug") if !debug => debug = true,
            Some("--static") => {
                return Err("--static builds executables; a library is linked into its host, as the host is".into());
            }
            Some("--link" | "--strip") => {
                return Err(format!(
                    "{} builds executables: a library lists its C libraries for its host",
                    arg.display()
                ));
            }
            _ => return Err(format!("unexpected arguments\n{USAGE}")),
        }
    }
    let name = program_name(&path)?.to_string_lossy().into_owned();
    let analysis = driver::analyze_library(&path)?;
    let diagnostics = analysis.diags.json(&analysis.sources, &analysis.source_id);
    if !json {
        show(&analysis, Shown::Human);
    }
    if analysis.program.is_none() {
        if json {
            print_json("build", false, serde_json::json!({ "files": [], "diagnostics": diagnostics }));
        }
        return Ok(ExitCode::FAILURE);
    }
    let object = driver::object(&analysis, debug, None)?;
    let dir = output.unwrap_or_else(|| PathBuf::from("."));
    let linking = driver::library(&analysis, &object, &dir, &name)?;
    let shared = format!("lib{name}.{}", if cfg!(target_os = "macos") { "dylib" } else { "so" });
    let wrote = [shared, format!("lib{name}.a"), format!("{name}.h")].map(|f| dir.join(f).display().to_string());
    if json {
        let fields = serde_json::json!({ "files": wrote, "linking": linking, "diagnostics": diagnostics });
        print_json("build", true, fields);
        return Ok(ExitCode::SUCCESS);
    }
    println!("wrote {}, {} and {}", wrote[0], wrote[1], wrote[2]);
    println!("{linking}");
    Ok(ExitCode::SUCCESS)
}

/// `tondo target pack [<dir>]`: writes the pack of this machine's target,
/// which a `tondo` elsewhere builds for it with (toolchain 7.8).
fn target_command(args: &[OsString]) -> Result<ExitCode, String> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    let dir = match &args[..] {
        [pack] if pack == "pack" => PathBuf::from("."),
        [pack, dir] if pack == "pack" => PathBuf::from(dir),
        _ => return Err(format!("unexpected arguments\n{USAGE}")),
    };
    let wrote = driver::pack(&dir)?;
    if json {
        print_json("target-pack", true, serde_json::json!({ "pack": wrote, "targets": dir }));
        return Ok(ExitCode::SUCCESS);
    }
    println!("wrote {}: set TONDO_TARGETS to {} where a tondo builds for it", wrote.display(), dir.display());
    Ok(ExitCode::SUCCESS)
}

fn run_command(args: &[OsString]) -> Result<ExitCode, String> {
    let (path, rest) = source_argument(args);
    let (options, rest) = link_options(rest)?;
    let program_args = match rest.as_slice() {
        [] => &[][..],
        [separator, rest @ ..] if separator == "--" => rest,
        _ => return Err(format!("program arguments go after `--`\n{USAGE}")),
    };
    if options.watch {
        return watch(&path, program_args, &options);
    }
    run(&path, program_args, &options)
}

/// `tondo run --watch` (toolchain 7.10): runs a live build of the program
/// and, when its files change, reloads it; it runs the program again when
/// it had ended or the change needs it, and goes on with the old code when
/// the files do not compile.
fn watch(path: &Path, program_args: &[OsString], options: &BuildOptions) -> Result<ExitCode, String> {
    let stamps = || -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
        let inputs = driver::inputs(path).unwrap_or_default();
        inputs.into_iter().map(|p| (p.clone(), std::fs::metadata(&p).and_then(|m| m.modified()).ok())).collect()
    };
    let start = || -> Result<Option<std::process::Child>, String> {
        let Some(executable) = cached_executable(path, options)? else { return Ok(None) };
        let launcher = options.target.map(Target::launcher).transpose()?.flatten();
        let mut command = match &launcher {
            Some(launcher) => {
                let mut command = std::process::Command::new(launcher);
                command.arg(&executable);
                command
            }
            None => std::process::Command::new(&executable),
        };
        command.args(program_args).spawn().map(Some).map_err(|e| format!("cannot start {}: {e}", executable.display()))
    };
    let mut child = start()?;
    let mut seen = stamps();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if let Some(status) = child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            eprintln!("tondo: the program ended ({status}); a change to its files runs it again");
            child = None;
        }
        let now = stamps();
        if now == seen {
            continue;
        }
        seen = now;
        let Some(running) = child.as_mut() else {
            child = start()?;
            continue;
        };
        match tondo::live::reload_named(&running.id().to_string()) {
            Ok(reloaded) => eprintln!("tondo: {}", reloaded.summary()),
            Err(tondo::live::Refused::Restart(message)) => {
                // Stopped before it says so, so that what comes next goes
                // to the program run again.
                let _ = running.kill();
                let _ = running.wait();
                eprintln!("tondo: not reloaded: {message}; running it again");
                child = start()?;
            }
            Err(refused) => eprintln!("tondo: {}", refused.message()),
        }
    }
}

/// Compiles (or reuses a cached build of) `path` and executes it, under
/// emulation when it is for another machine.
fn run(path: &Path, program_args: &[OsString], options: &BuildOptions) -> Result<ExitCode, String> {
    let launcher = options.target.map(Target::launcher).transpose()?.flatten();
    match cached_executable(path, options)? {
        Some(executable) => match launcher {
            Some(launcher) => exec(&launcher, &[&[executable.into_os_string()][..], program_args].concat()),
            None => exec(&executable, program_args),
        },
        None => Ok(ExitCode::FAILURE),
    }
}

/// The executable of `path` in the cache, built if it is not there; none
/// when the program does not compile, whose diagnostics are printed.
fn cached_executable(path: &Path, options: &BuildOptions) -> Result<Option<PathBuf>, String> {
    let BuildOptions { libraries, statically, debug, live, target, .. } = options;
    let identity = driver::compiler_identity();
    let linking = format!(
        "{}{}{}{}",
        if *statically { "static" } else { "" },
        if *debug { "debug" } else { "" },
        if *live { "live" } else { "" },
        target.map_or("", Target::name)
    );
    let mut parts = vec![identity.into_bytes(), libraries.join("\n").into_bytes(), linking.into_bytes()];
    // A live build is of the files where they are: its manifest says where
    // a reload reads them.
    if *live {
        parts.push(std::env::current_dir().map_err(|e| e.to_string())?.into_os_string().into_encoded_bytes());
    }
    for input in driver::inputs(path)? {
        parts.push(input.as_os_str().as_encoded_bytes().to_vec());
        parts.push(std::fs::read(&input).map_err(|e| format!("cannot read {}: {e}", input.display()))?);
    }
    let key = driver::fingerprint(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let dir = driver::cache_entry("run", &key);
    let executable = dir.join(program_name(path)?);
    if !executable.exists() {
        let Some(analysis) = analyze(path, false, options.shown)? else {
            return Ok(None);
        };
        let object = object(&analysis, path, options)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let partial = dir.join(format!(".partial-{}", std::process::id()));
        let libraries = [analysis.libraries.clone(), libraries.to_vec()].concat();
        let linking = driver::Linking { strip: false, statically: *statically, target: *target };
        driver::link(&object, &partial, &libraries, linking)?;
        std::fs::rename(&partial, &executable).map_err(|e| e.to_string())?;
    }
    Ok(Some(executable))
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
