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
//! - `// expect-processors: <n>` — the executable runs on the first n
//!   processors alone, as in a container of that many, which its runtime
//!   counts as its own; on Linux only, so the case says `expect-platform:
//!   linux` too;
//! - `// expect-no-replay: <why>` — with `TONDO_CASES_REPLAY`, it is not
//!   recorded, for the reason given.
//! - `// expect-no-schedule: <why>` — with `TONDO_CASES_SCHEDULE`, its tasks
//!   run as ever, for the reason given.
//! - `// expect-no-static: <why>` — with `TONDO_CASES_STATIC`, it is built
//!   as ever, for the reason given, as one that loads libraries.
//! - `// expect-optimization: <line>:<column> <kind> [text]` — `tondo
//!   optimizations` tells, at that place of the case, a remark of that
//!   kind whose text contains `text` when given: what a release build
//!   makes of the code, which a change to the passes must keep. One of the
//!   kind `region` is looked for with `--kind region`, which tells too what
//!   the library's code makes on its way.
//! - `// expect-no-optimization: <line>:<column> <kind> [text]` — it tells
//!   no such remark there: of a place a copy of the code would hide, as
//!   one kept checked beside one that lost its check.
//!
//! Executables run with `TONDO_RT_CHECK_LEAKS=1`, so every case also checks
//! that reference counting frees each string exactly once, and, but with
//! `TONDO_CASES_STATIC`, which keeps the runtime's usual way, with
//! `TONDO_CHECK_LIFETIMES=1`, so that a value a release build puts in a region
//! it outlives faults where it is read. With
//! `TONDO_CASES_STATIC` set, cases build static executables (`--static`);
//! with `TONDO_CASES_DEBUG`, debug builds (`--debug`), whose runtime checks
//! as it runs what its scheduler promises (`TONDO_CHECK_SCHEDULER`, off by
//! itself under an order or a recording); with
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
//! With `TONDO_CASES_CPU=<level>`, builds are for that level of processor.
//! With `TONDO_CASES_ONLY=<text>`, only the cases whose path holds it, as
//! `framed_texts`, run: one case in each mode at once is a loop over the
//! variables of the modes. With `TONDO_CASES_STALLS`, a seed, the runner
//! stops each case's program, every thread at once, three times in its
//! first second, for 50 to 150 ms each, at moments that the seed, the case
//! and the round pick, as a busy machine stops a process: the clock runs on
//! while it stands, so a case that counts on a margin of time where it
//! should wait for a word fails, and says when it stood. With the checker
//! on, as in the debug step, its runtime also stops one thread at a time
//! where a seed of the same run says (`TONDO_RT_STALLS`), as a busy machine
//! runs one thread late while the others go on.
//!
//! Each directory under `tests/projects` is a project built as a whole,
//! with the expectations written in any of its files.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    processors: Option<usize>,
    no_replay: bool,
    no_schedule: bool,
    no_static: bool,
    /// Each remark expected, and whether it is to be told or not.
    optimizations: Vec<(String, bool)>,
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
            "no-static" => expect.no_static = true,
            "optimization" => expect.optimizations.push((value.trim().into(), true)),
            "no-optimization" => expect.optimizations.push((value.trim().into(), false)),
            "env" => {
                let (name, value) = value.split_once('=').expect("`NAME=value`");
                expect.env.push((name.trim().into(), value.trim().into()));
            }
            "processors" => expect.processors = Some(value.trim().parse().expect("a number of processors")),
            other => panic!("unknown expectation `{other}`"),
        }
    }
    expect
}

/// Runs `command` on the first `count` processors alone: the child's
/// affinity, set between its fork and its exec.
#[cfg(target_os = "linux")]
fn on_processors(command: &mut Command, count: usize) {
    use std::os::unix::process::CommandExt;
    let set = unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        (0..count).for_each(|cpu| libc::CPU_SET(cpu, &mut set));
        set
    };
    unsafe {
        command.pre_exec(move || match libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn on_processors(_: &mut Command, _: usize) {
    panic!("`expect-processors` runs on Linux only: the case says `expect-platform: linux` too");
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

/// Runs one case, built with `extra` beside what the variables of the
/// modes ask.
fn run_case(path: &Path, scratch: &Path, extra: &[String]) -> Result<(), String> {
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
    let binary = scratch.join(own(path)).join(path.file_stem().unwrap());
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    // From its directory, so that locations read `case.to:4:5`.
    let mut build = common::command(compiler);
    build
        .current_dir(path.parent().unwrap())
        .arg("build")
        .arg(path.file_name().unwrap())
        .arg("--output")
        .arg(&binary)
        .args(std::env::var_os("TONDO_CASES_STATIC").filter(|_| !expect.no_static).map(|_| "--static"))
        .args(std::env::var_os("TONDO_CASES_DEBUG").or(std::env::var_os("TONDO_CASES_REPLAY")).map(|_| "--debug"))
        .args(std::env::var_os("TONDO_CASES_LIVE").map(|_| "--live"))
        .args(extra)
        .env("XDG_CACHE_HOME", scratch.join("cache"));
    let coverage = std::env::var_os("TONDO_CASES_COVERAGE")
        .map(|dir| Path::new(&dir).join(scratch.file_name().unwrap()).join(own(path)));
    if let Some(dir) = &coverage {
        std::fs::create_dir_all(dir.join("counts")).unwrap();
        build.env("TONDO_COVERAGE_STD", dir.join("counted"));
    }
    // Long enough for a debug build of `tondo` under an emulator, where a
    // case that uses much of the standard library takes minutes: a build
    // that hangs names its case.
    let build = run_with_timeout(&mut build, 1200, &[]).map_err(|e| format!("its build: {e}"))?;
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
    if !expect.optimizations.is_empty()
        && release.iter().all(|mode| std::env::var_os(mode).is_none())
        && extra.is_empty()
    {
        optimized(path, scratch, &expect.optimizations)?;
    }

    // A crash writes no core file, which takes the system a while.
    let program = || {
        let mut command = common::command(&binary);
        if expect.crash {
            command = common::command("sh");
            command.args(["-c", "ulimit -c 0; exec \"$0\""]).arg(&binary);
        }
        // A case that panics on purpose leaves no dump in the user's cache.
        command.env("TONDO_DUMP", "off");
        command.envs(expect.env.iter().map(|(name, value)| (name, value)));
        if let Some(count) = expect.processors {
            on_processors(&mut command, count);
        }
        command.env("TONDO_RT_CHECK_LEAKS", "1").current_dir(scratch);
        if std::env::var_os("TONDO_CASES_STATIC").is_none() {
            command.env("TONDO_CHECK_LIFETIMES", "1");
        }
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
        // Unless the runner is given it, as `0` to measure what it catches.
        if std::env::var_os("TONDO_CASES_DEBUG").is_some() && std::env::var_os("TONDO_CHECK_SCHEDULER").is_none() {
            command.env("TONDO_CHECK_SCHEDULER", "1");
        }
        command
    };
    // With `TONDO_CASES_REPEAT`, a case that starts tasks runs that many
    // times, as memory that orders weakly, as arm64's, brings out what one run
    // rarely shows (docs/refactor.md, 0.9).
    let tasks = std::fs::read_to_string(path).is_ok_and(|text| text.contains("spawn"));
    let repeat = std::env::var("TONDO_CASES_REPEAT").ok().and_then(|n| n.parse().ok()).filter(|_| tasks).unwrap_or(1);
    for round in 1..=repeat {
        let stops = stops(path, round);
        let stalls = thread_stalls(path, round);
        let once = || -> Result<(), String> {
            let mut command = program();
            if let Some(seed) = stalls {
                command.env("TONDO_RT_STALLS", seed.to_string());
            }
            // A seed of its own each time, which a failure says.
            let recording = std::env::var_os("TONDO_CASES_REPLAY").filter(|_| !expect.no_replay).map(|_| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                (binary.with_extension("calls"), format!("{:x}", now.as_nanos() as u64))
            });
            if let Some((calls, seed)) = &recording {
                command.env("TONDO_RT_RECORD", calls).env("TONDO_RT_SCHEDULE", seed);
            }
            let run = run_with_timeout(&mut command, 60, &stops)?;
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
                let again =
                    run_with_timeout(replay.env("TONDO_RT_REPLAY", calls).env("TONDO_RT_SCHEDULE", seed), 60, &stops)
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
        let once = once().map_err(|e| if stops.is_empty() { e } else { format!("{e}\n{}", stood(&stops)) });
        once.map_err(|e| if repeat > 1 { format!("run {round} of {repeat}: {e}") } else { e })?;
    }
    Ok(())
}

/// A stop of a program, every thread at once: from `at` after it starts,
/// for `lasting`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stop {
    at: Duration,
    lasting: Duration,
}

/// With `TONDO_CASES_STALLS`, the stops of a case's run: three in its first
/// second, of 50 to 150 ms each, one after another, that the seed, the
/// case's path and the round pick, so that a seed repeats them.
fn stops(path: &Path, round: u32) -> Vec<Stop> {
    let Some(seed) = std::env::var_os("TONDO_CASES_STALLS") else {
        return Vec::new();
    };
    let seed: u64 = seed.to_str().and_then(|seed| seed.parse().ok()).expect("TONDO_CASES_STALLS is a seed, a number");
    stops_of(seed, &own(path).to_string_lossy(), round)
}

/// The seed of a case's run: FNV-1a of the case under the seed and the round.
fn run_seed(seed: u64, case: &str, round: u32) -> u64 {
    case.bytes().fold(seed ^ (u64::from(round) << 32) ^ 0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The seed of the stops of one thread at a time the runtime makes, with the
/// checker on (`TONDO_RT_STALLS`), from the same seed of the case's run.
fn thread_stalls(path: &Path, round: u32) -> Option<u64> {
    let seed: u64 = std::env::var("TONDO_CASES_STALLS").ok()?.trim().parse().ok()?;
    Some(run_seed(seed, &own(path).to_string_lossy(), round).rotate_left(17))
}

fn stops_of(seed: u64, case: &str, round: u32) -> Vec<Stop> {
    // The run's seed, then splitmix64.
    let mut state = run_seed(seed, case, round);
    let mut next = move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut ats: Vec<u64> = (0..3).map(|_| next() % 1000).collect();
    ats.sort_unstable();
    let mut free = 0;
    ats.into_iter()
        .map(|at| {
            let (at, lasting) = (at.max(free), 50 + next() % 101);
            free = at + lasting;
            Stop { at: Duration::from_millis(at), lasting: Duration::from_millis(lasting) }
        })
        .collect()
}

/// What a failure says of the stops, to repeat them.
fn stood(stops: &[Stop]) -> String {
    let each: Vec<String> =
        stops.iter().map(|stop| format!("at {} ms for {} ms", stop.at.as_millis(), stop.lasting.as_millis())).collect();
    let seed = std::env::var("TONDO_CASES_STALLS").unwrap_or_default();
    format!("it stood still {}, with TONDO_CASES_STALLS={seed}", each.join(", "))
}

#[test]
fn stops_are_three_in_the_first_second_one_after_another() {
    for seed in 0..200 {
        let stops = stops_of(seed, "cases/run/tasks.to", 1);
        assert_eq!(stops, stops_of(seed, "cases/run/tasks.to", 1));
        assert_eq!(stops.len(), 3);
        let mut free = Duration::ZERO;
        for stop in &stops {
            assert!(stop.at >= free && stop.at < Duration::from_millis(1300), "{stops:?}");
            assert!((50..=150).contains(&stop.lasting.as_millis()), "{stops:?}");
            free = stop.at + stop.lasting;
        }
    }
    assert_ne!(stops_of(1, "cases/run/tasks.to", 1), stops_of(1, "cases/run/tasks.to", 2));
    assert_ne!(stops_of(1, "cases/run/tasks.to", 1), stops_of(1, "cases/run/select.to", 1));
}

/// A case's own directory under a scratch directory, which its executable
/// goes in: its path under `tests`, as `cases/run/regions/tasks.to`, which
/// no directory of cases is named as. Cases of one name in two
/// directories, as run/tasks.to and run/regions/tasks.to, run on two
/// threads at once, and one built where the other ran would find its
/// executable busy, or run the other's.
fn own(path: &Path) -> PathBuf {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    path.strip_prefix(&tests).unwrap_or(path).to_path_buf()
}

/// Whether `tondo optimizations` tells each remark a case expects.
fn optimized(path: &Path, scratch: &Path, expected: &[(String, bool)]) -> Result<(), String> {
    // What the view tells, by default or of one kind.
    let view = |kind: Option<&str>| -> Result<Vec<serde_json::Value>, String> {
        let mut args = vec![std::ffi::OsStr::new("optimizations"), path.file_name().unwrap()];
        args.push(std::ffi::OsStr::new("--json"));
        if let Some(kind) = kind {
            args.extend([std::ffi::OsStr::new("--kind"), std::ffi::OsStr::new(kind)]);
        }
        let told = common::command(env!("CARGO_BIN_EXE_tondo"))
            .current_dir(path.parent().unwrap())
            .args(args)
            .env("XDG_CACHE_HOME", scratch.join("cache"))
            .output()
            .map_err(|e| format!("tondo optimizations: {e}"))?;
        let told: serde_json::Value = serde_json::from_slice(&told.stdout)
            .map_err(|e| format!("tondo optimizations: {e}\n{}", String::from_utf8_lossy(&told.stderr)))?;
        Ok(told["optimizations"].as_array().cloned().unwrap_or_default())
    };
    let shown = view(None)?;
    // Where a region puts what the library's code makes the view tells by
    // default only of what reaches the program: an expectation of the kind
    // `region` is held to the view of that kind, which tells all of it.
    let of_regions = |(expected, _): &(String, bool)| expected.split(' ').nth(1) == Some("region");
    let regions = match expected.iter().any(of_regions) {
        true => view(Some("region"))?,
        false => Vec::new(),
    };
    for (expected, told) in expected {
        let (at, rest) = expected.split_once(' ').unwrap_or((expected, ""));
        let (kind, text) = rest.split_once(' ').unwrap_or((rest, ""));
        let found = if kind == "region" { &regions } else { &shown };
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
        if !told && found.iter().any(here) {
            return Err(format!("an optimization `{expected}` a release build was not to make"));
        }
        if *told && !found.iter().any(here) {
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
    let dumped = common::command(tondo).args(["dump", &pid.to_string(), "--output"]).arg(&dump).output();
    match &dumped {
        Ok(output) if output.status.success() => {}
        // What kept the dump from coming, said rather than left out.
        Ok(output) => return format!("\n--- no dump: {}", String::from_utf8_lossy(&output.stderr).trim()),
        Err(error) => return format!("\n--- no dump: tondo dump did not start: {error}"),
    }
    let read = |view: &str| {
        let shown = common::command(tondo).args(["dump", "read"]).arg(&dump).arg(view).output();
        shown.map_or(String::new(), |output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    format!("\n--- why\n{}\n--- tasks\n{}\n--- dump: {}", read("why"), read("tasks"), dump.display())
}

/// A program that does not end says why in the failure of its case. It is
/// asked once it says it waits, by its own word: asked at a time, as when
/// the cases' runner gave it two seconds, a program not yet started that
/// far, under the load of every test at once in alpine's container, served
/// no dump, and the case said only that it timed out.
#[test]
fn a_case_that_hangs_says_why() {
    use std::io::BufRead;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hangs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = "import std.console\nimport std.time\n\nfn main() {\n    console.eprintln(\"waits\")\n    time.sleep(time.seconds(120))\n}\n";
    std::fs::write(dir.join("hangs.to"), source).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    let built =
        common::command(compiler).args(["build", "hangs.to", "--output", "hangs"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut program = common::command(dir.join("hangs"))
        .env("TONDO_DIAGNOSTICS", "1")
        .env("TONDO_CI_DUMPS", &dir)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufReader::new(program.stderr.take().unwrap()).read_line(&mut said).unwrap();
    assert_eq!(said.trim(), "waits");
    let why = why_it_hangs(program.id());
    let _ = program.kill();
    let _ = program.wait();
    assert!(why.contains("hangs.to:6 in main"), "{why}");
}

/// Runs a program, failing the case when it has not finished in time, and
/// stops it as `stops` say.
fn run_with_timeout(command: &mut Command, seconds: u64, stops: &[Stop]) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::process::Stdio;
    // A program that cannot start fails its case, which says which.
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} does not start: {e}", command.get_program().to_string_lossy()))?;
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
    // Its end, which a thread waits for; the time only bounds a hang.
    let pid = child.id();
    let started = std::time::Instant::now();
    // Whether it ended, which a stop reads before it signals: the waiter
    // learns of the end before it reaps the process, so that no stop
    // signals another process given its number since.
    let over = std::sync::Arc::new(std::sync::Mutex::new(false));
    if !stops.is_empty() {
        let (over, stops) = (over.clone(), stops.to_vec());
        std::thread::spawn(move || {
            let signal = |signal| {
                let over = over.lock().unwrap();
                (!*over).then(|| unsafe { libc::kill(pid as i32, signal) }).is_some()
            };
            for stop in stops {
                // Time is what is under test: the moment the program stops.
                std::thread::sleep((started + stop.at).saturating_duration_since(std::time::Instant::now()));
                if !signal(libc::SIGSTOP) {
                    return;
                }
                // Time is what is under test: how long it stands.
                std::thread::sleep(stop.lasting);
                if !signal(libc::SIGCONT) {
                    return;
                }
            }
        });
    }
    let (done, ended) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn({
        let over = over.clone();
        let watched = !stops.is_empty();
        move || {
            if watched {
                ended_unreaped(pid);
                *over.lock().unwrap() = true;
            }
            done.send(child.wait())
        }
    });
    let status = match ended.recv_timeout(std::time::Duration::from_secs(seconds)) {
        Ok(status) => status.unwrap(),
        Err(_) => {
            let why = why_it_hangs(pid);
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            let _ = waiter.join();
            return Err(format!("timed out after {seconds}s{why}"));
        }
    };
    Ok(std::process::Output { status, stdout: stdout.join().unwrap(), stderr: stderr.join().unwrap() })
}

/// Returns once the process has ended, and before it is reaped.
#[cfg(not(target_os = "macos"))]
fn ended_unreaped(pid: u32) {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let flags = libc::WEXITED | libc::WNOWAIT;
    while unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) } == -1
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    {}
}

/// Returns once the process has ended, and before it is reaped: through
/// kqueue, as macOS's waitid returns for a stop too, though only an end is
/// asked for. One that has already ended gives ESRCH, which ends the wait.
#[cfg(target_os = "macos")]
fn ended_unreaped(pid: u32) {
    let queue = unsafe { libc::kqueue() };
    if queue == -1 {
        return;
    }
    let mut change: libc::kevent = unsafe { std::mem::zeroed() };
    change.ident = pid as libc::uintptr_t;
    change.filter = libc::EVFILT_PROC;
    change.flags = libc::EV_ADD | libc::EV_ONESHOT;
    change.fflags = libc::NOTE_EXIT;
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    while unsafe { libc::kevent(queue, &change, 1, &mut event, 1, std::ptr::null()) } == -1
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    {}
    unsafe { libc::close(queue) };
}

#[test]
fn cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases");
    let mut paths = Vec::new();
    collect(&root, &mut paths);
    let cpu = std::env::var("TONDO_CASES_CPU").ok().filter(|cpu| !cpu.is_empty());
    run_all(paths, "cases", &cpu.map(|cpu| vec!["--cpu".to_string(), cpu]).unwrap_or_default());
}

/// The cases of vectors where the processor has `x86-64-v3`, built for that
/// level, at which vectors of 256 bits are registers of 256 bits (step 22b
/// of `std.simd`): in a release, a debug and a live build, as the cases of
/// every other build run at the baseline.
#[test]
fn vectors_at_the_level_of_avx2() {
    #[cfg(target_arch = "x86_64")]
    let here = {
        use std::arch::is_x86_feature_detected as has;
        has!("avx")
            && has!("avx2")
            && has!("bmi1")
            && has!("bmi2")
            && has!("fma")
            && has!("f16c")
            && has!("lzcnt")
            && has!("movbe")
    };
    #[cfg(not(target_arch = "x86_64"))]
    let here = false;
    // It builds in its own modes, which a variable of another mode would
    // give twice.
    let modes =
        ["TONDO_CASES_STATIC", "TONDO_CASES_DEBUG", "TONDO_CASES_LIVE", "TONDO_CASES_REPLAY", "TONDO_CASES_CPU"];
    if !here || modes.iter().any(|mode| std::env::var_os(mode).is_some()) {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/run");
    let mut paths = Vec::new();
    collect(&root, &mut paths);
    let vectors = |name: &str| name.starts_with("simd_") || name.starts_with("tensor_");
    paths.retain(|path| path.file_name().is_some_and(|name| vectors(&name.to_string_lossy())));
    for (name, build) in
        [("vectors-v3", None), ("vectors-v3-debug", Some("--debug")), ("vectors-v3-live", Some("--live"))]
    {
        let extra: Vec<String> = ["--cpu", "x86-64-v3"].into_iter().chain(build).map(str::to_string).collect();
        run_all(paths.clone(), name, &extra);
    }
}

#[test]
fn projects() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/projects");
    let paths = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    run_all(paths, "projects", &[]);
}

fn run_all(mut paths: Vec<PathBuf>, name: &str, extra: &[String]) {
    paths.sort();
    assert!(!paths.is_empty(), "no {name} found");
    if let Some(only) = std::env::var("TONDO_CASES_ONLY").ok().filter(|only| !only.is_empty()) {
        paths.retain(|path| path.to_string_lossy().contains(&only));
        if paths.is_empty() {
            return;
        }
    }
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
    let _held = hold(&scratch);
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
                        .filter_map(|path| {
                            run_case(path, scratch, extra).err().map(|e| format!("{}: {e}", path.display()))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{} of {} cases failed:\n\n{}", failures.len(), paths.len(), failures.join("\n\n"));
}

/// A lock on a scratch directory for a run of the cases, held while the
/// file it gives is open: a second run on the same directory, as two
/// `cargo test` in one tree, would clear it under the first, whose
/// programs would vanish as they start. That second run fails at once,
/// saying which run holds it.
fn hold(scratch: &Path) -> std::fs::File {
    use std::io::{Read, Write};
    let path = scratch.with_extension("lock");
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).unwrap();
    if let Err(std::fs::TryLockError::WouldBlock) = file.try_lock() {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        panic!("another run of the cases uses {}: {}; wait for its end", scratch.display(), holder.trim());
    }
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    file.set_len(0).unwrap();
    write!(file, "process {}, since {since} seconds after the epoch", std::process::id()).unwrap();
    file
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
    let output = common::command(env!("CARGO_BIN_EXE_tondo")).arg("check").arg(&path).arg("--json").output().unwrap();
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

/// Runs git in `dir`, with an identity and no signing of its own.
fn git_in(dir: &Path, args: &[&str]) {
    let status = common::command("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

/// Writes `text` into `root`'s `path`, making its directories.
fn write_under(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// What the tests of dependencies fetch, under `root`: a repository
/// `greet`, whose tag `v1` says "hello" and whose head says "HELLO", and an
/// archive of a package `math`; their URLs. A package `units` in `units`
/// depends on the archive.
fn packages_to_fetch(root: &Path) -> (String, String) {
    write_under(root, "greet/src/greet.to", "pub fn hello(name: String): String {\n    \"hello {name}\"\n}\n");
    write_under(root, "greet/tondo.toml", "[package]\nname = \"greet\"\n");
    let greet = root.join("greet");
    git_in(&greet, &["init", "-q"]);
    git_in(&greet, &["add", "-A"]);
    git_in(&greet, &["commit", "-qm", "v1"]);
    git_in(&greet, &["tag", "-a", "v1", "-m", "v1"]);
    write_under(root, "greet/src/greet.to", "pub fn hello(name: String): String {\n    \"HELLO {name}\"\n}\n");
    git_in(&greet, &["commit", "-qam", "v2"]);
    write_under(root, "math-1.0/tondo.toml", "[package]\nname = \"math\"\n");
    write_under(root, "math-1.0/src/math.to", "pub fn twice(value: Int): Int {\n    value * 2\n}\n");
    let tar = common::command("tar").args(["-czf", "math-1.0.tar.gz", "math-1.0"]).current_dir(root).status().unwrap();
    assert!(tar.success());
    let url = format!("file://{}/math-1.0.tar.gz", root.display());
    write_under(
        root,
        "units/src/units.to",
        "import math\n\npub fn meters(km: Int): Int {\n    math.twice(km) * 500\n}\n",
    );
    write_under(
        root,
        "units/tondo.toml",
        &format!("[package]\nname = \"units\"\n\n[dependencies]\nmath = {{ url = \"{url}\" }}\n"),
    );
    (format!("file://{}/greet", root.display()), url)
}

/// Dependencies from a git repository (by tag) and an archive, reached
/// through a path dependency; `tondo.lock` pins them until `tondo update`.
#[test]
fn dependencies() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dependencies");
    let _ = std::fs::remove_dir_all(&root);
    let (repository, _) = packages_to_fetch(&root);
    let write = |path: &str, text: &str| write_under(&root, path, text);
    write(
        "app/tondo.toml",
        &format!(
            "[dependencies]\ngreet = {{ git = \"{repository}\", tag = \"v1\" }}\nunits = {{ path = \"../units\" }}\n"
        ),
    );
    write(
        "app/src/main.to",
        "import std.console\nimport greet\nimport units\n\nconsole.println(\"{greet.hello(\"ada\")} {units.meters(3)}\")\n",
    );
    let tondo = |args: &[&str]| {
        let output = common::command(env!("CARGO_BIN_EXE_tondo"))
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
    git_in(&root.join("greet"), &["tag", "-f", "-a", "v1", "-m", "moved"]);
    assert_eq!(tondo(&["run"]), "hello ada 3000\n");
    tondo(&["update"]);
    assert_eq!(tondo(&["run"]), "HELLO ada 3000\n");
}

/// A script declares its dependencies in its header (toolchain 2): each
/// package fetched says so once on stderr, and the lock beside the script
/// pins them, found by the script's path from another directory. A script
/// that cannot write its lock runs, and says so once.
#[test]
fn script_dependencies() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("script-dependencies");
    let _ = std::fs::remove_dir_all(&root);
    let (repository, url) = packages_to_fetch(&root);
    write_under(
        &root,
        "scripts/deploy.to",
        &format!(
            "#!/usr/bin/env tondo\n\n// tondo: dependency greet = {{ git = \"{repository}\", tag = \"v1\" }}\n// tondo: dependency units = {{ path = \"../units\" }}\n\nimport greet\nimport std.console\nimport units\n\nconsole.println(\"{{greet.hello(\"ada\")}} {{units.meters(3)}}\")\n"
        ),
    );
    // From the root, as a shebang runs it from wherever its user is.
    let tondo = |args: &[&str]| {
        let output = common::command(env!("CARGO_BIN_EXE_tondo"))
            .args(args)
            .current_dir(&root)
            .env("XDG_CACHE_HOME", root.join("cache"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "tondo {args:?}: {stderr}");
        (String::from_utf8(output.stdout).unwrap(), stderr)
    };
    let fetched = format!("fetching greet: git {repository}, tag v1\nfetching math: {url}\n");
    assert_eq!(tondo(&["run", "scripts/deploy.to"]), ("hello ada 3000\n".into(), fetched.clone()));
    let lock = root.join("scripts/deploy.to.lock");
    let pins = std::fs::read_to_string(&lock).unwrap();
    assert!(pins.contains("commit = ") && pins.contains("sha256 = "), "{pins}");
    // With the lock and the cache, nothing is fetched and nothing said.
    assert_eq!(tondo(&["scripts/deploy.to"]), ("hello ada 3000\n".into(), String::new()));
    // Moving the tag changes nothing until the lock is updated.
    git_in(&root.join("greet"), &["tag", "-f", "-a", "v1", "-m", "moved"]);
    assert_eq!(tondo(&["run", "scripts/deploy.to"]).0, "hello ada 3000\n");
    tondo(&["update", "scripts/deploy.to"]);
    assert_eq!(tondo(&["run", "scripts/deploy.to"]).0, "HELLO ada 3000\n");
    // Installed where its user cannot write, without its lock.
    std::fs::remove_file(&lock).unwrap();
    let scripts = root.join("scripts");
    let mode = |mode| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&scripts, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    mode(0o555);
    // The superuser writes anyway: then there is nothing to see.
    if std::fs::write(scripts.join("probe"), "").is_err() {
        let (out, err) = tondo(&["run", "scripts/deploy.to"]);
        mode(0o755);
        assert_eq!(out, "HELLO ada 3000\n");
        let warning = "warning: cannot write scripts/deploy.to.lock (Permission denied): the dependencies were resolved for this run only; install the script with its lock beside it to fix them\n";
        assert_eq!(err, format!("{fetched}{warning}"));
        assert!(!lock.exists());
    }
    mode(0o755);
}
