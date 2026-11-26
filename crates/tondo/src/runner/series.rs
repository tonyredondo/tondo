//! A series of compilers (testing 10.4): `tondo test <projects> --bench
//! --series <base>..<head>` measures the benchmarks with the `tondo` of
//! each revision from base to head that changes what tondo is built from,
//! says for each commit what it moved against the one before, and then
//! measures head against base, in as many orders of the code as
//! `--layouts` gives.
//!
//! Every `tondo` measures the same sources, the projects as they are where
//! the command runs. Each measure is kept in the cache under what decides
//! it, so a series measured again measures only what is new.

use super::Benchmark;
use super::bench;
use crate::hash::sha256_hex;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// What `tondo test --bench --series` was given.
pub struct Series {
    /// `<base>..<head>`: two revisions of tondo's repository.
    pub range: String,
    pub projects: Vec<PathBuf>,
    /// The options every `tondo` measures with, as given: the tests chosen,
    /// `--cpu`, `--bench-time`.
    pub measuring: Vec<String>,
    /// What only the whole takes: `--layouts`, `--max-regression`,
    /// `--regression-by` and `--baseline`.
    pub whole: Vec<String>,
    /// Measures every revision again, whatever the cache keeps.
    pub fresh: bool,
    pub json: bool,
}

/// A revision of the series.
struct Revision {
    commit: String,
    subject: String,
    /// Whether it changes what tondo is built from; the base does.
    changes: bool,
}

/// A `tondo` the series measures with.
struct Tondo {
    /// The first revision that builds it.
    revision: String,
    executable: PathBuf,
    sha256: String,
}

/// The benchmarks of one project as one `tondo` measured them.
struct Measure {
    benchmarks: HashMap<(String, String), Benchmark>,
    /// The tests that failed, whose benchmarks have no measure.
    failed: Vec<String>,
    /// The bytes of the executable the benchmarks run in.
    size: Option<u64>,
    /// When it was measured, in seconds since 1970, and whether now.
    measured: u64,
    now: bool,
}

/// A benchmark that moved at a step: its instructions by half a percent or
/// more, or its allocations or bytes by as much as `--max-regression` sees.
struct Moved {
    project: String,
    test: String,
    name: String,
    before: Benchmark,
    after: Benchmark,
}

/// Measures the series and says what each step did; fails if a revision
/// does not build or does not pass the first run, if a benchmark's test
/// fails with some revision, or if the whole regressed.
pub fn run(series: &Series) -> Result<ExitCode, String> {
    let Some((base, head)) = series.range.split_once("..").filter(|(b, h)| !b.is_empty() && !h.is_empty()) else {
        return Err(format!(
            "--series takes <base>..<head>, two revisions of tondo's repository, as main..HEAD, not {}",
            series.range
        ));
    };
    let say = |line: String| match series.json {
        true => eprintln!("{line}"),
        false => println!("{line}"),
    };
    let here = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    let root = git(&here, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| format!("--series measures revisions of tondo's repository, and {} is in none", here.display()))?;
    let revisions = revisions(&root, base, head)?;
    let after = revisions.len() - 1;
    say(format!(
        "series: {} to {}, {after} commit{} after the base, {} of them changing what tondo is built from",
        short(&revisions[0].commit),
        short(&revisions[after].commit),
        if after == 1 { "" } else { "s" },
        revisions.iter().skip(1).filter(|r| r.changes).count()
    ));

    // Each revision's tondo, built or taken with the record of its build.
    let mut tondos: Vec<Tondo> = Vec::new();
    let mut uses = Vec::new();
    for revision in &revisions {
        if !revision.changes {
            say(format!("  {}  the same tondo as its parent: {}", short(&revision.commit), revision.subject));
            uses.push(tondos.len() - 1);
            continue;
        }
        let (_, executable) = crate::driver::compiler(&revision.commit, &root)
            .map_err(|why| format!("{why}\nthe series stops before measuring anything"))?;
        let bytes = std::fs::read(&executable).map_err(|e| format!("cannot read {}: {e}", executable.display()))?;
        let sha256 = sha256_hex(&bytes);
        match tondos.last() {
            Some(last) if last.sha256 == sha256 => say(format!(
                "  {}  builds the tondo of {} (sha256 {}): {}",
                short(&revision.commit),
                short(&last.revision),
                &sha256[..16],
                revision.subject
            )),
            _ => {
                say(format!("  {}  sha256 {}: {}", short(&revision.commit), &sha256[..16], revision.subject));
                tondos.push(Tondo { revision: revision.commit.clone(), executable, sha256 });
            }
        }
        uses.push(tondos.len() - 1);
    }

    // The sources every tondo measures, the same for all.
    let mut sources = Vec::new();
    for project in &series.projects {
        sources.push(sources_hash(project)?);
    }
    let environment = environment();
    say(format!(
        "measuring {} as they are here, sources {}, in an environment of {} variables",
        names(&series.projects),
        &sha256_hex(sources.join(" ").as_bytes())[..16],
        environment.count
    ));

    // One benchmark of each project with every tondo, before the long run.
    let selection = selection(&series.measuring);
    for project in &series.projects {
        let first = first_benchmark(&tondos[tondos.len() - 1], project, &selection)?;
        for tondo in &tondos {
            smoke(tondo, project, &first, &series.measuring)?;
        }
    }
    say(format!("first run: {} with every tondo, each passed", names(&series.projects)));

    // Every tondo measures every project once, or the cache keeps it.
    let machine = machine();
    let mut measures: Vec<Vec<Measure>> = Vec::new();
    for tondo in &tondos {
        let mut row = Vec::new();
        for (project, sources) in series.projects.iter().zip(&sources) {
            let key =
                key(&tondo.sha256, &project.display().to_string(), sources, &series.measuring, &environment, &machine);
            let kept = crate::driver::cache_dir().join("series").join(format!("{key}.json"));
            let measure = measure(tondo, project, &series.measuring, &kept, series.fresh)?;
            say(format!(
                "  {} {}: {}",
                short(&tondo.revision),
                project.display(),
                match measure.now {
                    true => "measured now".to_string(),
                    false => format!("kept, measured {} ago", ago(measure.measured)),
                }
            ));
            row.push(measure);
        }
        measures.push(row);
    }

    // A test that fails with a tondo leaves its benchmarks without a
    // measure there, and the series fails at its end.
    let mut failing = BTreeSet::new();
    for (tondo, row) in tondos.iter().zip(&measures) {
        for (project, measure) in series.projects.iter().zip(row) {
            for test in &measure.failed {
                failing.insert(format!("{}: {test} with the tondo of {}", project.display(), short(&tondo.revision)));
            }
        }
    }

    // Each step against the one before.
    let mut steps = Vec::new();
    for (index, revision) in revisions.iter().enumerate().skip(1) {
        let (before, after) = (uses[index - 1], uses[index]);
        if before == after {
            continue;
        }
        let mut moved = Vec::new();
        let mut lines = vec![format!(
            "{}  {} (sha256 {} against {})",
            short(&revision.commit),
            revision.subject,
            &tondos[after].sha256[..16],
            &tondos[before].sha256[..16]
        )];
        let mut sizes = Vec::new();
        for (project, (was, now)) in series.projects.iter().zip(measures[before].iter().zip(&measures[after])) {
            let name = project.display().to_string();
            for test in was.failed.iter().filter(|t| !now.failed.contains(t)) {
                lines.push(format!(
                    "    {name}: {test} failed with the tondo before, so its benchmarks have no measure there"
                ));
            }
            for test in &now.failed {
                lines.push(format!("    {name}: {test} fails with this tondo, so its benchmarks have no measure"));
            }
            let found = moved_between(&name, &was.benchmarks, &now.benchmarks);
            for row in &found {
                lines.push(format!("    {}", moved_line(row)));
            }
            moved.extend(found);
            if let (Some(was), Some(now)) = (was.size, now.size) {
                if was != now {
                    lines.push(format!(
                        "    {name}: its tests' executable {was} bytes, now {now} ({:+.2}%)",
                        percent(now as f64, was as f64)
                    ));
                }
                sizes.push(json!({ "project": name, "before": was, "after": now }));
            }
        }
        if moved.is_empty() {
            lines.push("    no benchmark moved by its instructions, allocations or bytes".to_string());
        }
        for line in lines {
            say(line);
        }
        steps.push((index, before, after, moved, sizes));
    }

    // The whole, head against base, in turns, as one comparison of tondos.
    let (first, last) = (uses[0], uses[uses.len() - 1]);
    let mut whole = Vec::new();
    let mut whole_failed = false;
    if first == last {
        say(format!("head builds the base's tondo (sha256 {}): no whole to measure", &tondos[first].sha256[..16]));
    } else {
        say(format!(
            "whole: the tondo of {} against that of {}",
            short(&tondos[last].revision),
            short(&tondos[first].revision)
        ));
        for project in &series.projects {
            let (passed, report) = compare_whole(&tondos[last], &tondos[first], project, series)?;
            whole_failed |= !passed;
            whole.push((project.display().to_string(), report));
        }
    }

    // Each row the whole calls regressed, put down to the step that moved it
    // most the same way.
    let mut regressions = Vec::new();
    for (project, report) in &whole {
        for (test, name, by) in regressed(report) {
            let moved = steps.iter().map(|(index, _, _, moved, _)| (*index, moved.as_slice()));
            let commit = put_down(moved, project, &test, &name, by).map(|index| revisions[index].commit.clone());
            say(match &commit {
                Some(commit) => format!("regressed: {project}: {test} {name}, by {}", short(commit)),
                None => format!("regressed: {project}: {test} {name}, by no step's count of it: where its code lies"),
            });
            regressions.push(json!({ "project": project, "test": test, "name": name, "commit": commit }));
        }
    }
    for failed in &failing {
        say(format!("failed: {failed}"));
    }
    if regressions.is_empty() && failing.is_empty() && !whole.is_empty() {
        say("no benchmark regressed, and every test passed with every revision".to_string());
    }

    if series.json {
        let report = json!({
            "format": "tondo-series-0.1/1",
            "series": {
                "base": revisions[0].commit,
                "head": revisions[revisions.len() - 1].commit,
                "variables": environment.count,
                "sources": series.projects.iter().zip(&sources)
                    .map(|(p, s)| json!({ "project": p.display().to_string(), "sha256": s }))
                    .collect::<Vec<_>>(),
                "revisions": revisions.iter().zip(&uses).map(|(r, &u)| json!({
                    "revision": r.commit,
                    "subject": r.subject,
                    "sha256": tondos[u].sha256,
                    "changes": r.changes,
                    "measured": measures[u].iter().map(|m| json!({ "at": m.measured, "now": m.now })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "steps": steps.iter().map(|(index, before, after, moved, sizes)| json!({
                    "revision": revisions[*index].commit,
                    "parent": revisions[index - 1].commit,
                    "sha256": tondos[*after].sha256,
                    "parent_sha256": tondos[*before].sha256,
                    "rows": moved.iter().map(row_json).collect::<Vec<_>>(),
                    "sizes": sizes,
                })).collect::<Vec<_>>(),
                "whole": whole.iter().map(|(p, r)| json!({ "project": p, "report": r })).collect::<Vec<_>>(),
                "regressions": regressions,
                "failed": failing.iter().collect::<Vec<_>>(),
            }
        });
        println!("{report}");
    }
    Ok(match failing.is_empty() && !whole_failed {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    })
}

/// The revisions from `base` to `head` along head's first parents, base
/// first, each with whether it changes what tondo is built from.
fn revisions(root: &Path, base: &str, head: &str) -> Result<Vec<Revision>, String> {
    let commit = |name: &str| {
        git(root, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")])
            .map_err(|_| format!("{name} is no revision of {}", root.display()))
    };
    let (base, head) = (commit(base)?, commit(head)?);
    let subject = |commit: &str| git(root, &["log", "-1", "--format=%s", commit]);
    let mut revisions = vec![Revision { subject: subject(&base)?, commit: base.clone(), changes: true }];
    let listed = git(root, &["rev-list", "--first-parent", "--reverse", &format!("{base}..{head}")])?;
    let mut parent = base.clone();
    for commit in listed.lines() {
        if git(root, &["rev-parse", &format!("{commit}^")])? != parent {
            return Err(format!(
                "{} is not on the line of {}'s first parents: a series measures one commit after another",
                short(&base),
                short(&head)
            ));
        }
        let mut diff = vec!["diff", "--quiet", &parent, commit, "--"];
        diff.extend(crate::driver::TONDO_SOURCES);
        let same =
            Command::new("git").arg("-C").arg(root).args(&diff).status().map_err(|e| format!("cannot run git: {e}"))?;
        let changes = match same.code() {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(format!("git cannot compare {} with its parent", short(commit))),
        };
        revisions.push(Revision { commit: commit.to_string(), subject: subject(commit)?, changes });
        parent = commit.to_string();
    }
    if revisions.len() == 1 && base != head {
        return Err(format!("{} is not an ancestor of {}", short(&base), short(&head)));
    }
    Ok(revisions)
}

/// What decides a measure: the `tondo`'s bytes, the sources measured, the
/// options that change what is measured, the environment's `TONDO_*` and
/// how many variables it has, as a start of a program copies them all, and
/// the machine. Two measures with one key measure the same thing.
fn key(
    sha256: &str,
    project: &str,
    sources: &str,
    measuring: &[String],
    environment: &Environment,
    machine: &str,
) -> String {
    let mut text = format!("tondo {sha256}\nproject {project} {sources}\noptions {}\n", measuring.join(" "));
    for (name, value) in &environment.tondo {
        text.push_str(&format!("env {name}={value}\n"));
    }
    text.push_str(&format!("variables {}\n", environment.count));
    text.push_str(&format!("machine {machine}\n"));
    sha256_hex(text.as_bytes())
}

/// The sha256 of every file a build of the project's tests reads, by its
/// path in the project and its bytes.
fn sources_hash(project: &Path) -> Result<String, String> {
    let mut inputs = crate::driver::test_inputs(project)?;
    inputs.sort();
    inputs.dedup();
    let mut text = Vec::new();
    for input in inputs {
        let name = input.strip_prefix(project).unwrap_or(&input);
        let bytes = std::fs::read(&input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
        text.extend(format!("{} {}\n", name.display(), sha256_hex(&bytes)).into_bytes());
    }
    Ok(sha256_hex(&text))
}

/// What of the environment decides a measure: the variables a `tondo`
/// reads, `TONDO_*`, by name, and how many there are, which the benchmarks
/// that start a program depend on. Not their bytes, which every shell's `_`
/// and `OLDPWD` would change at each run.
struct Environment {
    tondo: Vec<(String, String)>,
    count: usize,
}

fn environment() -> Environment {
    let mut tondo: Vec<_> = std::env::vars().filter(|(name, _)| name.starts_with("TONDO_")).collect();
    tondo.sort();
    Environment { tondo, count: std::env::vars_os().count() }
}

/// The processor, the kernel's release and the C library's version.
fn machine() -> String {
    format!("{} | {} | {}", bench::cpu(), kernel_release(), libc_version())
}

#[cfg(unix)]
fn kernel_release() -> String {
    // SAFETY: `uname` fills the struct it is given, and `release` is a
    // C string it ends.
    unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        match libc::uname(&mut name) {
            0 => std::ffi::CStr::from_ptr(name.release.as_ptr()).to_string_lossy().into_owned(),
            _ => String::new(),
        }
    }
}

#[cfg(not(unix))]
fn kernel_release() -> String {
    String::new()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn libc_version() -> String {
    // SAFETY: glibc returns a string of its own, which lives as long as it.
    let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) };
    format!("glibc {}", version.to_string_lossy())
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn libc_version() -> String {
    format!("{} {}", std::env::consts::OS, if cfg!(target_env = "musl") { "musl" } else { "libc" })
}

/// The options that choose the tests, which the first run replaces with
/// one test.
fn selection(measuring: &[String]) -> Vec<String> {
    let mut chosen = Vec::new();
    let mut given = measuring.iter();
    while let Some(option) = given.next() {
        if ["--filter", "--glob", "--exact"].contains(&option.as_str()) {
            chosen.push(option.clone());
            chosen.extend(given.next().cloned());
        }
    }
    chosen
}

/// The options but those that choose the tests.
fn unselected(measuring: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut given = measuring.iter();
    while let Some(option) = given.next() {
        match ["--filter", "--glob", "--exact"].contains(&option.as_str()) {
            true => {
                given.next();
            }
            false => kept.push(option.clone()),
        }
    }
    kept
}

/// The first test of the project that measures, among those chosen.
fn first_benchmark(tondo: &Tondo, project: &Path, selection: &[String]) -> Result<String, String> {
    let listed = Command::new(&tondo.executable)
        .arg("test")
        .arg(project)
        .args(["--list", "--json"])
        .args(selection)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", tondo.executable.display()))?;
    let list: Value = serde_json::from_slice(&listed.stdout).map_err(|_| {
        format!(
            "the tondo of {} cannot list the tests of {}: {}",
            short(&tondo.revision),
            project.display(),
            String::from_utf8_lossy(&listed.stderr).trim_end()
        )
    })?;
    list["tests"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|test| test["bench"] == true)
        .and_then(|test| test["id"].as_str())
        .map(str::to_string)
        .ok_or(format!("{} has no benchmark among the tests chosen", project.display()))
}

/// One benchmark measured briefly, to know the long run can run.
fn smoke(tondo: &Tondo, project: &Path, test: &str, measuring: &[String]) -> Result<(), String> {
    let mut options = unselected(measuring);
    let time = options.iter().position(|o| o == "--bench-time");
    if let Some(at) = time {
        options.drain(at..(at + 2).min(options.len()));
    }
    let ran = Command::new(&tondo.executable)
        .arg("test")
        .arg(project)
        .args(["--bench", "--bench-time", "0.01", "--exact", test])
        .args(&options)
        .env(super::REGIONS_UNCOUNTED, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", tondo.executable.display()))?;
    if ran.status.success() {
        return Ok(());
    }
    Err(format!(
        "the tondo of {} does not pass {test} of {}: the series measures nothing more\n{}",
        short(&tondo.revision),
        project.display(),
        String::from_utf8_lossy(&ran.stderr).trim_end()
    ))
}

/// The project measured with the tondo, or as the cache keeps it at
/// `kept`. A measure is kept only when every test of its run passed: one
/// that failed for a reason of the moment, a busy machine or a port, would
/// otherwise fail every later series over that revision unmeasured.
fn measure(tondo: &Tondo, project: &Path, measuring: &[String], kept: &Path, fresh: bool) -> Result<Measure, String> {
    if !fresh && let Some(measure) = read_kept(kept, &tondo.sha256) {
        return Ok(measure);
    }
    let folder = kept.parent().unwrap_or(Path::new("."));
    let scratch = folder.join(format!("measuring-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
    let measured = run_measure(tondo, project, measuring, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    let (entry, passed) = measured?;
    if passed {
        crate::driver::write_atomic(kept, format!("{entry}\n").as_bytes())?;
    }
    let mut measure = measure_of(&entry, &tondo.sha256).ok_or("a measure that reads as none".to_string())?;
    measure.now = true;
    Ok(measure)
}

/// One run of the project's benchmarks with the tondo, in `scratch`: what
/// it measured, as the cache keeps it, and whether every test passed.
fn run_measure(tondo: &Tondo, project: &Path, measuring: &[String], scratch: &Path) -> Result<(Value, bool), String> {
    let (baseline, report) = (scratch.join("baseline.json"), scratch.join("report.json"));
    let ran = Command::new(&tondo.executable)
        .arg("test")
        .arg(project)
        .arg("--bench")
        .args(measuring)
        .arg("--save-baseline")
        .arg(&baseline)
        .arg("--report")
        .arg(format!("json={}", report.display()))
        .env(super::REGIONS_UNCOUNTED, "1")
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {}: {e}", tondo.executable.display()))?;
    let saved = std::fs::read_to_string(&baseline).map_err(|_| {
        format!(
            "the tondo of {} measured nothing of {} (it ended with {ran})",
            short(&tondo.revision),
            project.display()
        )
    })?;
    let failed = std::fs::read_to_string(&report)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|report| failed_tests(&report))
        .unwrap_or_default();
    let entry = json!({
        "format": "tondo-series-measure-0.1/1",
        "revision": tondo.revision,
        "sha256": tondo.sha256,
        "project": project.display().to_string(),
        "measured": now(),
        "failed": failed,
        "size": size(tondo, project),
        "baseline": serde_json::from_str::<Value>(&saved).map_err(|e| format!("the baseline measured is not JSON: {e}"))?,
    });
    Ok((entry, ran.success() && failed.is_empty()))
}

/// A measure the cache keeps, if it reads as one of the tondo of these
/// bytes.
fn read_kept(path: &Path, sha256: &str) -> Option<Measure> {
    let entry: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    measure_of(&entry, sha256)
}

/// A measure as the cache keeps it, if it is one of the tondo of these
/// bytes.
fn measure_of(entry: &Value, sha256: &str) -> Option<Measure> {
    if entry["format"] != "tondo-series-measure-0.1/1" || entry["sha256"] != sha256 {
        return None;
    }
    Some(Measure {
        benchmarks: bench::measures(&entry["baseline"]),
        failed: entry["failed"].as_array()?.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        size: entry["size"].as_u64(),
        measured: entry["measured"].as_u64()?,
        now: false,
    })
}

/// The tests a report says did not pass.
fn failed_tests(report: &Value) -> Vec<String> {
    report["tests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|test| !matches!(test["status"].as_str(), Some("passed" | "skipped")))
        .filter_map(|test| test["id"].as_str().map(str::to_string))
        .collect()
}

/// The bytes of the executable the project's benchmarks run in, its
/// tests', as the tondo builds it.
fn size(tondo: &Tondo, project: &Path) -> Option<u64> {
    let output = Command::new(&tondo.executable)
        .arg("size")
        .arg(project)
        .args(["--tests", "--json"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    serde_json::from_slice::<Value>(&output.stdout).ok()?["size"].as_u64()
}

/// The benchmarks of a project that moved from one measure to the next:
/// their instructions by half a percent or more, their allocations by one
/// in a hundred runs and their bytes by eight a run, the least a count
/// moves by from the code (testing 10.4).
fn moved_between(
    project: &str,
    before: &HashMap<(String, String), Benchmark>,
    after: &HashMap<(String, String), Benchmark>,
) -> Vec<Moved> {
    let mut found: Vec<Moved> = after
        .iter()
        .filter_map(|(key, now)| {
            let was = before.get(key)?;
            let instructions = match (now.instructions, was.instructions) {
                (Some(now), Some(was)) if was > 0.0 => ((now - was) / was).abs() >= bench::MOVED,
                _ => false,
            };
            let counted = (now.allocations - was.allocations).abs() >= 0.01 || (now.bytes - was.bytes).abs() >= 8.0;
            (instructions || counted).then(|| Moved {
                project: project.to_string(),
                test: key.0.clone(),
                name: key.1.clone(),
                before: was.clone(),
                after: now.clone(),
            })
        })
        .collect();
    found.sort_by(|a, b| (&a.test, &a.name).cmp(&(&b.test, &b.name)));
    found
}

/// The step that made a benchmark grow the most by what it regressed by,
/// by its index among the revisions: what a regression of the whole is put
/// down to, from each revision's own measure.
fn put_down<'a>(
    steps: impl Iterator<Item = (usize, &'a [Moved])>,
    project: &str,
    test: &str,
    name: &str,
    by: Grew,
) -> Option<usize> {
    steps
        .filter_map(|(index, moved)| {
            let row = moved.iter().find(|m| m.project == project && m.test == test && m.name == name)?;
            let rise = rise(&row.before, &row.after, by);
            (rise > 0.0).then_some((index, rise))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// How much a benchmark's count grew at a step, as a fraction of what it
/// was, or by what it now has where it had none.
fn rise(before: &Benchmark, after: &Benchmark, by: Grew) -> f64 {
    let grown = |now: f64, was: f64| match was > 0.0 {
        true => now / was - 1.0,
        false => now,
    };
    match by {
        Grew::Instructions | Grew::Time => match (after.instructions, before.instructions) {
            (Some(now), Some(was)) => grown(now, was),
            _ => grown(bench::stats(&after.samples).median, bench::stats(&before.samples).median),
        },
        Grew::Allocations => grown(after.allocations, before.allocations),
        Grew::Bytes => grown(after.bytes, before.bytes),
    }
}

/// A moved benchmark, as a step says it: the counts that judge it, and its
/// two times with no verdict, as one run of each says little.
fn moved_line(row: &Moved) -> String {
    let (before, after) = (&row.before, &row.after);
    let instructions = match (before.instructions, after.instructions) {
        (Some(was), Some(now)) => {
            format!("{} → {} ins/op ({:+.1}%)", bench::many(was), bench::many(now), percent(now, was))
        }
        _ => "instructions not counted".to_string(),
    };
    let mut line = format!("{}: {} {}  {instructions}", row.project, row.test, row.name);
    if (after.allocations - before.allocations).abs() >= 0.01 {
        line.push_str(&format!(
            ", {} → {} allocs/op",
            bench::amount(before.allocations),
            bench::amount(after.allocations)
        ));
    }
    if (after.bytes - before.bytes).abs() >= 8.0 {
        line.push_str(&format!(", {} → {} B/op", bench::amount(before.bytes), bench::amount(after.bytes)));
    }
    let median = |b: &Benchmark| bench::stats(&b.samples).median;
    line.push_str(&format!(", {} and {}", bench::time(median(before)), bench::time(median(after))));
    line
}

fn row_json(row: &Moved) -> Value {
    let side = |b: &Benchmark| {
        json!({
            "instructions_per_op": b.instructions,
            "allocations_per_op": b.allocations,
            "bytes_per_op": b.bytes,
            "ns_per_op": bench::stats(&b.samples).median,
        })
    };
    json!({
        "project": row.project,
        "test": row.test,
        "name": row.name,
        "before": side(&row.before),
        "after": side(&row.after),
    })
}

/// Head's tondo against base's, measuring the project in turns, with what
/// only the whole takes; its report, and whether it passed.
fn compare_whole(head: &Tondo, base: &Tondo, project: &Path, series: &Series) -> Result<(bool, Value), String> {
    let report = crate::driver::cache_dir().join("series").join(format!("whole-{}.json", std::process::id()));
    let mut command = Command::new(&head.executable);
    command
        .arg("test")
        .arg(project)
        .arg("--bench")
        .args(&series.measuring)
        .args(&series.whole)
        .arg("--compiler")
        .arg(&base.executable)
        .arg("--report")
        .arg(format!("json={}", report.display()));
    if series.json {
        command.stdout(std::io::stderr());
    }
    let ran = command.status().map_err(|e| format!("cannot run {}: {e}", head.executable.display()))?;
    let read = std::fs::read_to_string(&report).ok().and_then(|text| serde_json::from_str(&text).ok());
    let _ = std::fs::remove_file(&report);
    Ok((ran.success(), read.unwrap_or(Value::Null)))
}

/// What a benchmark regressed by, as the whole's report says why: the step
/// it is put down to is the one that grew the same count most.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grew {
    Instructions,
    /// Slower by the clock, which a step's one run cannot judge: its
    /// instructions stand for it.
    Time,
    Allocations,
    Bytes,
}

/// The kind of a reason the report gives (testing 10.4): "211.04 allocs/op,
/// was 111.05", "11856.32 B/op, was 8266.38", "3.4% more instructions",
/// "12.5% slower", or that the instructions were not counted.
fn grew(reason: &str) -> Grew {
    if reason.contains("allocs/op") {
        Grew::Allocations
    } else if reason.contains("B/op") {
        Grew::Bytes
    } else if reason.contains("slower") {
        Grew::Time
    } else {
        Grew::Instructions
    }
}

/// The benchmarks a whole's report says regressed, by test and name, with
/// what they regressed by: against the other tondo, or against the saved
/// baseline, which judges instructions.
fn regressed(report: &Value) -> Vec<(String, String, Grew)> {
    let mut found = Vec::new();
    for test in report["tests"].as_array().into_iter().flatten() {
        let id = test["id"].as_str().unwrap_or_default();
        let attempts = test["attempts"].as_array().into_iter().flatten();
        for benchmark in attempts.flat_map(|a| a["benchmarks"].as_array().into_iter().flatten()) {
            let Some(name) = benchmark["name"].as_str() else { continue };
            if let Some(reason) = benchmark["baseline"]["regressed"].as_str() {
                found.push((id.to_string(), name.to_string(), grew(reason)));
            } else if benchmark["since"]["regressed"] == true {
                found.push((id.to_string(), name.to_string(), Grew::Instructions));
            }
        }
    }
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    found
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output =
        Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("cannot run git: {e}"))?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

fn names(projects: &[PathBuf]) -> String {
    projects.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn percent(now: f64, was: f64) -> f64 {
    if was > 0.0 { (now / was - 1.0) * 100.0 } else { 0.0 }
}

/// Seconds since 1970, now.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// How long ago a moment in seconds since 1970 was, in days, hours or
/// minutes.
fn ago(seconds: u64) -> String {
    match now().saturating_sub(seconds) {
        s if s >= 86_400 => format!("{} d {} h", s / 86_400, s % 86_400 / 3600),
        s if s >= 3600 => format!("{} h {} min", s / 3600, s % 3600 / 60),
        s => format!("{} min", s / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["commit", "-q", "-m", file]);
        git_in(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn a_series_skips_the_commits_that_build_their_parents_tondo() {
        let dir = std::env::temp_dir().join(format!("tondo-series-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q"]);
        let base = commit(&dir, "crates/tondo/src/lib.rs", "a");
        let code = commit(&dir, "crates/tondo/src/lib.rs", "b");
        let docs = commit(&dir, "docs/notes.md", "c");
        let tests = commit(&dir, "crates/tondo/tests/cli.rs", "d");
        let lock = commit(&dir, "Cargo.lock", "e");
        let embedded = commit(&dir, "conformance/fixtures.txt", "f");
        let found = revisions(&dir, &base, "HEAD").unwrap();
        let seen: Vec<(&str, bool)> = found.iter().map(|r| (r.commit.as_str(), r.changes)).collect();
        assert_eq!(
            seen,
            [
                (base.as_str(), true),
                (code.as_str(), true),
                (docs.as_str(), false),
                (tests.as_str(), false),
                (lock.as_str(), true),
                (embedded.as_str(), true)
            ]
        );
        assert_eq!(found[2].subject, "docs/notes.md");
        assert!(revisions(&dir, &code, &base).is_err(), "a base after its head");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_part_of_a_measure_s_key_changes_it() {
        let tondo = vec![("TONDO_WORKERS".to_string(), "4".to_string())];
        let environment = Environment { tondo: tondo.clone(), count: 55 };
        let options = vec!["--filter".to_string(), "texts".to_string()];
        let key = |sha: &str, sources: &str, options: &[String], environment: &Environment, machine: &str| {
            super::key(sha, "bench/tondo", sources, options, environment, machine)
        };
        let first = key("aa", "s1", &options, &environment, "m1");
        assert_eq!(first, key("aa", "s1", &options, &environment, "m1"));
        assert_ne!(first, key("ab", "s1", &options, &environment, "m1"), "the tondo");
        assert_ne!(first, key("aa", "s2", &options, &environment, "m1"), "the sources");
        assert_ne!(first, key("aa", "s1", &options[..1], &environment, "m1"), "the options");
        let none = Environment { tondo: Vec::new(), count: 55 };
        assert_ne!(first, key("aa", "s1", &options, &none, "m1"), "the TONDO_* variables");
        let more = Environment { tondo, count: 200 };
        assert_ne!(first, key("aa", "s1", &options, &more, "m1"), "how many variables");
        assert_ne!(first, key("aa", "s1", &options, &environment, "m2"), "the machine");
        assert_ne!(first, super::key("aa", "examples/chat", "s1", &options, &environment, "m1"), "the project");
    }

    fn measured(instructions: f64, allocations: f64, bytes: f64) -> Benchmark {
        Benchmark {
            name: "row".to_string(),
            samples: vec![100.0],
            instructions: Some(instructions),
            allocations,
            bytes,
            ..Benchmark::default()
        }
    }

    #[test]
    fn a_row_moves_by_half_a_percent_of_instructions_or_by_what_it_allocates() {
        let row = |b: Benchmark| HashMap::from([(("t".to_string(), "row".to_string()), b)]);
        let before = row(measured(1000.0, 1.0, 64.0));
        let moved = |after: Benchmark| moved_between("p", &before, &row(after)).len();
        assert_eq!(moved(measured(1004.0, 1.0, 64.0)), 0, "0.4% more instructions");
        assert_eq!(moved(measured(1005.0, 1.0, 64.0)), 1, "0.5% more");
        assert_eq!(moved(measured(995.0, 1.0, 64.0)), 1, "0.5% fewer");
        assert_eq!(moved(measured(1000.0, 1.005, 64.0)), 0, "an allocation in two hundred runs");
        assert_eq!(moved(measured(1000.0, 1.01, 64.0)), 1, "one in a hundred");
        assert_eq!(moved(measured(1000.0, 1.0, 71.0)), 0, "seven bytes");
        assert_eq!(moved(measured(1000.0, 1.0, 72.0)), 1, "eight bytes");
    }

    #[test]
    fn a_regression_is_put_down_to_the_step_that_grew_it_most() {
        let row = |before: f64, after: f64| Moved {
            project: "p".to_string(),
            test: "t".to_string(),
            name: "row".to_string(),
            before: measured(before, 0.0, 0.0),
            after: measured(after, 0.0, 0.0),
        };
        let (one, two, three) = (vec![row(100.0, 102.0)], vec![row(102.0, 101.0)], vec![row(101.0, 109.0)]);
        let steps = [(1, one.as_slice()), (2, two.as_slice()), (3, three.as_slice())];
        let by = Grew::Instructions;
        assert_eq!(put_down(steps.iter().copied(), "p", "t", "row", by), Some(3));
        assert_eq!(put_down(steps[..2].iter().copied(), "p", "t", "row", by), Some(1));
        assert_eq!(put_down(steps[1..2].iter().copied(), "p", "t", "row", by), None, "it only fell");
        assert_eq!(put_down(steps.iter().copied(), "p", "t", "other", by), None);
    }

    #[test]
    fn a_regression_is_put_down_by_the_count_it_regressed_by() {
        // The allocations rise at the second step and the instructions at
        // the first: each kind names its own step.
        let row = |before: Benchmark, after: Benchmark| Moved {
            project: "p".to_string(),
            test: "t".to_string(),
            name: "row".to_string(),
            before,
            after,
        };
        let one = vec![row(measured(100.0, 111.0, 800.0), measured(101.0, 111.0, 800.0))];
        let two = vec![row(measured(101.0, 111.0, 800.0), measured(84.0, 211.0, 1100.0))];
        let steps = [(1, one.as_slice()), (2, two.as_slice())];
        let named = |by| put_down(steps.iter().copied(), "p", "t", "row", by);
        assert_eq!(named(Grew::Allocations), Some(2));
        assert_eq!(named(Grew::Bytes), Some(2));
        assert_eq!(named(Grew::Instructions), Some(1));
        assert_eq!(named(Grew::Time), Some(1));
        assert_eq!(grew("211.04 allocs/op, was 111.05"), Grew::Allocations);
        assert_eq!(grew("11856.32 B/op, was 8266.38"), Grew::Bytes);
        assert_eq!(grew("3.4% more instructions"), Grew::Instructions);
        assert_eq!(grew("12.5% slower"), Grew::Time);
    }

    /// A stand-in for a tondo, as a script: `test` saves a baseline of one
    /// benchmark and a report whose test has `status`, and ends with
    /// `code`; `size` gives a size.
    #[cfg(unix)]
    fn stand_in(dir: &Path, status: &str, code: i32) -> Tondo {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            "#!/bin/sh\n\
             [ \"$1\" = size ] && {{ echo '{{\"size\": 1000}}'; exit 0; }}\n\
             while [ $# -gt 0 ]; do\n\
             case \"$1\" in --save-baseline) baseline=$2; shift ;; --report) report=${{2#json=}}; shift ;; esac\n\
             shift\n\
             done\n\
             echo '{{\"format\":\"tondo-bench-baseline-0.1/1\",\"environment\":{{}},\"benchmarks\":\
             [{{\"test\":\"p::t\",\"name\":\"row\",\"instructions_per_op\":100.0,\"samples_ns_per_op\":[10.0]}}]}}' > \"$baseline\"\n\
             echo '{{\"tests\":[{{\"id\":\"p::t\",\"status\":\"{status}\"}}]}}' > \"$report\"\n\
             exit {code}\n"
        );
        std::fs::create_dir_all(dir).unwrap();
        let executable = dir.join("tondo");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Tondo { revision: "f".repeat(40), executable, sha256: "ab".repeat(32) }
    }

    #[cfg(unix)]
    #[test]
    fn a_measure_is_kept_only_when_every_test_of_its_run_passed() {
        let dir = std::env::temp_dir().join(format!("tondo-series-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        // A run whose tests pass is kept, and a second series takes it.
        let (passing, kept) = (stand_in(&dir.join("passing"), "passed", 0), dir.join("cache/passing.json"));
        let first = measure(&passing, &project, &[], &kept, false).unwrap();
        assert!(first.now && first.failed.is_empty() && first.size == Some(1000));
        assert_eq!(first.benchmarks[&("p::t".to_string(), "row".to_string())].instructions, Some(100.0));
        assert!(!measure(&passing, &project, &[], &kept, false).unwrap().now, "kept");
        assert!(measure(&passing, &project, &[], &kept, true).unwrap().now, "--fresh measures again");
        // A run with a failed test gives its measure to this series alone.
        let (failing, kept) = (stand_in(&dir.join("failing"), "failed", 1), dir.join("cache/failing.json"));
        let first = measure(&failing, &project, &[], &kept, false).unwrap();
        assert_eq!(first.failed, ["p::t"]);
        assert!(!kept.exists(), "a measure with a failed test is kept");
        assert!(measure(&failing, &project, &[], &kept, false).unwrap().now, "the next series measures it again");
        let left: Vec<_> = dir.join("cache").read_dir().unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(!left.iter().any(|name| name.to_string_lossy().starts_with("measuring-")), "{left:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_kept_measure_is_only_the_tondo_s_of_its_bytes() {
        let entry = json!({
            "format": "tondo-series-measure-0.1/1",
            "sha256": "aa",
            "measured": 1,
            "failed": [],
            "size": null,
            "baseline": {"benchmarks": []},
        });
        assert!(measure_of(&entry, "aa").is_some());
        assert!(measure_of(&entry, "bb").is_none(), "another tondo's bytes");
    }
}
