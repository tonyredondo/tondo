//! The manual in `docs/`: the reference of the standard library is what
//! `tondo api` gives, every example compiles, and an example followed by a
//! block of `text output` prints exactly that when it runs.
//!
//! `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs` writes the reference
//! and the outputs again, after a change to the standard library's API or
//! documentation, or to what a program prints.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn update() -> bool {
    std::env::var_os("TONDO_UPDATE_DOCS").is_some()
}

/// Where the part of a page of `docs/std` that `docs/std/generate.to` writes
/// begins: its examples are the standard library's own, held where they are
/// written.
const GENERATED: &str = "<!-- From here on, docs/std/generate.to writes this page";

/// A fenced block of a document.
struct Fence {
    info: String,
    code: String,
    /// The lines of its body in the document, from the first up to its end.
    body: std::ops::Range<usize>,
}

fn fences(text: &str) -> Vec<Fence> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if lines[at].starts_with(GENERATED) {
            break;
        }
        let Some(info) = lines[at].strip_prefix("```") else {
            at += 1;
            continue;
        };
        let start = at + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].starts_with("```") {
            end += 1;
        }
        let code = lines[start..end].iter().map(|line| format!("{line}\n")).collect();
        out.push(Fence { info: info.trim().to_string(), code, body: start..end });
        at = end + 1;
    }
    out
}

/// The Markdown files under a directory, in order.
fn markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            markdown(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        }
    }
}

/// The documents whose examples are held: the manual and the examples' pages,
/// not the records of decisions and plans, whose code is of its day.
fn documents() -> Vec<PathBuf> {
    let root = root();
    let mut out = vec![root.join("docs/tour.md"), root.join("docs/toolchain.md"), root.join("docs/comparison.md")];
    for dir in ["docs/language", "docs/toolchain", "docs/comparison", "docs/std", "examples"] {
        markdown(&root.join(dir), &mut out);
    }
    out
}

/// One example to check: where it is, what it is, and what it must print.
struct Job {
    name: String,
    document: usize,
    info: String,
    code: String,
    case: PathBuf,
    /// The lines of the output block that follows it, and what they say.
    output: Option<(std::ops::Range<usize>, String)>,
}

/// What an example did wrong, or what it printed when its output is recorded.
enum Outcome {
    Fine,
    Failed(String),
    Printed(String),
}

fn check(tondo: &str, job: &Job) -> Outcome {
    std::fs::create_dir_all(&job.case).unwrap();
    let test = job.info == "tondo test";
    if test {
        std::fs::create_dir_all(job.case.join("src")).unwrap();
        std::fs::write(job.case.join("tondo.toml"), "[package]\nname = \"guide\"\n").unwrap();
        std::fs::write(job.case.join("src/main.to"), "fn main() {}\n").unwrap();
        std::fs::write(job.case.join("src/main_test.to"), &job.code).unwrap();
    } else {
        std::fs::write(job.case.join("main.to"), &job.code).unwrap();
    }
    let run = |args: &[&str]| {
        Command::new(tondo).args(args).current_dir(&job.case).env("TONDO_DUMP", "off").output().unwrap()
    };
    let checks: &[&str] = if test { &["test", "--list"] } else { &["check", "main.to"] };
    let mut args = checks.to_vec();
    if job.info.ends_with(" library") {
        args.push("--library");
    }
    let output = run(&args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = job.info.strip_prefix("tondo error ").and_then(|rest| rest.split(' ').next());
    let ok = match expected {
        None => output.status.success(),
        Some(code) => !output.status.success() && stderr.contains(&format!("error[{code}]")),
    };
    if !ok {
        return Outcome::Failed(format!("{}:\n{}\n{stderr}", job.name, job.code));
    }
    let Some((_, says)) = &job.output else { return Outcome::Fine };
    let ran = run(&["run", "main.to"]);
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    if !ran.status.success() {
        let stderr = String::from_utf8_lossy(&ran.stderr);
        return Outcome::Failed(format!("{} does not end well:\n{printed}{stderr}", job.name));
    }
    if update() {
        return Outcome::Printed(printed);
    }
    if printed.trim_end() != says.trim_end() {
        return Outcome::Failed(format!(
            "{} prints\n{printed}\nand the document says\n{says}\nwrite it again with `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs`",
            job.name
        ));
    }
    Outcome::Fine
}

#[test]
fn the_reference_of_the_standard_library_is_current() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
    command.args(["run", "docs/std/generate.to"]);
    if !update() {
        command.args(["--", "--check"]);
    }
    let output = command.current_dir(root()).env("TONDO", env!("CARGO_BIN_EXE_tondo")).output().unwrap();
    assert!(
        output.status.success(),
        "{}{}after a change to the standard library's API or its documentation, write docs/std again with \
         `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs`",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn examples_compile_and_print_what_the_documents_say() {
    let root = root();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("docs");
    let _ = std::fs::remove_dir_all(&dir);
    let documents = documents();
    let texts: Vec<String> = documents.iter().map(|path| std::fs::read_to_string(path).unwrap()).collect();
    let mut jobs = Vec::new();
    for (document, text) in texts.iter().enumerate() {
        let name = documents[document].strip_prefix(&root).unwrap().display().to_string();
        let found = fences(text);
        for (index, fence) in found.iter().enumerate() {
            if !fence.info.starts_with("tondo") {
                continue;
            }
            // An output block belongs to the program right before it.
            let output = found
                .get(index + 1)
                .filter(|next| next.info == "text output" && fence.info == "tondo")
                .map(|next| (next.body.clone(), next.code.clone()));
            jobs.push(Job {
                name: format!("{name}, line {} ({})", fence.body.start, fence.info),
                document,
                info: fence.info.clone(),
                code: fence.code.clone(),
                case: dir.join(format!("{}-{index}", name.replace(['/', '.'], "-"))),
                output,
            });
        }
        if let Some(stray) = found
            .iter()
            .enumerate()
            .find(|(index, fence)| fence.info == "text output" && (*index == 0 || found[index - 1].info != "tondo"))
        {
            panic!("{name}, line {}: a block of `text output` follows no program", stray.1.body.start);
        }
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let outcomes: Vec<(usize, Outcome)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(job) = jobs.get(index) else { break };
                        done.push((index, check(env!("CARGO_BIN_EXE_tondo"), job)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect()
    });
    let mut failures = Vec::new();
    // What each document's output blocks become, last block first so that
    // the lines of those above stay where they were.
    let mut rewrites: Vec<Vec<(std::ops::Range<usize>, String)>> = vec![Vec::new(); documents.len()];
    for (index, outcome) in outcomes {
        match outcome {
            Outcome::Fine => {}
            Outcome::Failed(why) => failures.push((index, why)),
            Outcome::Printed(printed) => {
                let (lines, says) = jobs[index].output.clone().unwrap();
                if printed.trim_end() != says.trim_end() {
                    rewrites[jobs[index].document].push((lines, printed));
                }
            }
        }
    }
    for (document, mut blocks) in rewrites.into_iter().enumerate() {
        if blocks.is_empty() {
            continue;
        }
        blocks.sort_by_key(|(lines, _)| std::cmp::Reverse(lines.start));
        let mut lines: Vec<String> = texts[document].lines().map(str::to_string).collect();
        for (range, printed) in blocks {
            lines.splice(range, printed.trim_end().lines().map(str::to_string));
        }
        std::fs::write(&documents[document], lines.join("\n") + "\n").unwrap();
        println!("wrote the outputs of {}", documents[document].display());
    }
    failures.sort_by_key(|(index, _)| *index);
    let failures: Vec<String> = failures.into_iter().map(|(_, why)| why).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
