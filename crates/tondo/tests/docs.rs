//! The manual in `docs/`: the reference of the standard library is what
//! `tondo api` gives, the reference of the commands is what `tondo help
//! --all` prints, every example compiles, an example followed by a block
//! of `text output` prints exactly that when it runs, and the tests of an
//! example pass, or fail when it says `tondo test failing`, as one that shows
//! what a failure looks like.
//!
//! A page's listings print what they show. A `tondo file=<name>` program is
//! an example with a name, and a `diff file=<new> from=<old>` fence makes the
//! variant `<new>` of `<old>`, or changes `<name>` itself without `from=`:
//! lines with `-` go, those with `+` come, the others, after one space, are
//! context, and what goes with its context is found exactly once. In a `text
//! listing`, each `$ tondo …` line is a command, split without a shell and
//! run in a directory of the page's own that holds the files it named so far;
//! what it prints, standard output and then standard error, is every line
//! up to the next `$` line, but for one blank line before it. Its exit
//! status is not compared. A plain `text` block right after a `tondo error
//! <code>` or `tondo warning <code>` program, or a `diff` with `error <code>`
//! or `warning <code>`, quotes its diagnostic: each of its lines is one of
//! what `tondo check` prints for it, in order.
//!
//! `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs` writes the reference,
//! the outputs and the listings again, after a change to the standard
//! library's API or documentation, or to what a program or a command prints.

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

/// The value of a word `key=value` of a fence's info string.
fn attribute<'a>(info: &'a str, key: &str) -> Option<&'a str> {
    info.split_whitespace().skip(1).find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

/// Whether a fence is a program whose output a `text output` after it holds.
fn prints(info: &str) -> bool {
    info == "tondo" || (info.split_whitespace().count() == 2 && attribute(info, "file").is_some())
}

/// A file as a `diff` fence makes it from another.
fn patched(old: &str, diff: &str) -> Result<String, String> {
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for line in diff.lines() {
        if let Some(gone) = line.strip_prefix('-') {
            before.push(gone);
        } else if let Some(new) = line.strip_prefix('+') {
            after.push(new);
        } else {
            let context = line.strip_prefix(' ').unwrap_or(line);
            before.push(context);
            after.push(context);
        }
    }
    let lines: Vec<&str> = old.lines().collect();
    let found: Vec<usize> = (0..=lines.len().saturating_sub(before.len()))
        .filter(|&at| !before.is_empty() && lines[at..].starts_with(&before))
        .collect();
    let [at] = found[..] else {
        return Err(format!(
            "what it takes away, with its context, is in the file {} times, and must be once:\n{}",
            found.len(),
            before.join("\n")
        ));
    };
    let new: Vec<&str> = lines[..at].iter().chain(&after).chain(&lines[at + before.len()..]).copied().collect();
    Ok(new.join("\n") + "\n")
}

/// The words of a command, split as a shell splits them without expanding
/// anything: at blanks, with quotes keeping theirs.
fn words(line: &str) -> Result<Vec<String>, String> {
    let (mut out, mut word, mut quote) = (Vec::new(), None::<String>, None);
    for c in line.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            (None, c) if c.is_whitespace() => out.extend(word.take()),
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(word);
    match out.iter().find(|word| ["|", ">", "<", ">>", "&&", "||", ";", "2>&1"].contains(&word.as_str())) {
        Some(shell) => Err(format!("`{line}` asks a shell for `{shell}`, and a listing runs its commands without one")),
        None if quote.is_some() => Err(format!("`{line}` leaves a quote open")),
        None => Ok(out),
    }
}

/// What a block of a page becomes: the lines it takes and what they say.
type Rewrite = (std::ops::Range<usize>, String);

/// What a page's listings need, in its order: a file it names, or a command
/// of a listing with the lines of what it prints.
enum Step {
    File { name: String, text: String },
    Command { words: Vec<String>, says: std::ops::Range<usize>, said: String },
}

/// The steps of a `text listing`.
fn commands(page: &str, fence: &Fence, lines: &[&str]) -> Result<Vec<Step>, String> {
    let starts: Vec<usize> = fence.body.clone().filter(|&at| lines[at].starts_with("$ ")).collect();
    if starts.first() != Some(&fence.body.start) {
        return Err(format!("{page}, line {}: a `text listing` starts with its `$ tondo` line", fence.body.start));
    }
    let mut steps = Vec::new();
    for (index, &at) in starts.iter().enumerate() {
        let command = &lines[at][2..];
        let words = words(command).map_err(|why| format!("{page}, line {}: {why}", at + 1))?;
        if words.first().map(String::as_str) != Some("tondo") {
            return Err(format!("{page}, line {}: `{command}` is not a command of tondo", at + 1));
        }
        let end = starts.get(index + 1).map_or(fence.body.end, |&next| match lines[next - 1].is_empty() {
            true if next - 1 > at => next - 1,
            _ => next,
        });
        let said = lines[at + 1..end].iter().map(|line| format!("{line}\n")).collect();
        steps.push(Step::Command { words, says: at + 1..end, said });
    }
    Ok(steps)
}

/// What a command printed, standard output and then standard error, with no
/// blanks at the ends of its lines.
fn printed(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    text.lines().map(|line| format!("{}\n", line.trim_end())).collect::<String>().trim_end().to_string()
}

/// Runs a page's listings in its order, in a directory of its own with a
/// cache of its own; gives what fails, and what each listing printed that
/// differs from what the page says.
fn listings(page: &str, steps: &[Step], dir: &Path) -> (Vec<String>, Vec<Rewrite>) {
    let cache = dir.with_extension("cache");
    std::fs::create_dir_all(dir).unwrap();
    let (mut failures, mut rewrites) = (Vec::new(), Vec::new());
    for step in steps {
        match step {
            Step::File { name, text } => {
                let path = dir.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            Step::Command { words, says, said } => {
                let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
                command.args(&words[1..]).current_dir(dir).env("XDG_CACHE_HOME", &cache);
                let shown = printed(&command.stdin(std::process::Stdio::null()).output().unwrap());
                let written: String = said.lines().map(|line| format!("{}\n", line.trim_end())).collect();
                if shown == written.trim_end() {
                    continue;
                }
                match update() {
                    true => rewrites.push((says.clone(), shown)),
                    false => failures.push(format!(
                        "{page}, line {}: `{}` prints\n{shown}\nand the page says\n{said}write it again with \
                         `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs`",
                        says.start,
                        words.join(" ")
                    )),
                }
            }
        }
    }
    (failures, rewrites)
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
    /// What a `text` block after a wrong program quotes of its diagnostics,
    /// and where.
    quoted: Option<(usize, String)>,
}

/// What an example did wrong, or what it printed when its output is recorded.
enum Outcome {
    Fine,
    Failed(String),
    Printed(String),
}

fn check(tondo: &str, job: &Job) -> Outcome {
    std::fs::create_dir_all(&job.case).unwrap();
    let test = job.info.starts_with("tondo test");
    if test {
        std::fs::create_dir_all(job.case.join("src")).unwrap();
        std::fs::write(job.case.join("tondo.toml"), "[package]\nname = \"guide\"\n").unwrap();
        std::fs::write(job.case.join("src/main.to"), "fn main() {}\n").unwrap();
        std::fs::write(job.case.join("src/main_test.to"), &job.code).unwrap();
    }
    // One that imports a module of one system, as `std.os.linux`, or of one
    // architecture, as `std.cpu.x64`, is a file of it, which `tondo check`
    // checks for it on any machine.
    let system = ["linux", "macos"].into_iter().find(|s| job.code.contains(&format!("import std.os.{s}\n")));
    let arch = ["x64", "arm64"].into_iter().find(|a| job.code.contains(&format!("import std.cpu.{a}\n")));
    let suffix = |name: Option<&str>| name.map(|name| format!("_{name}")).unwrap_or_default();
    let main = match attribute(&job.info, "file") {
        Some(named) => named.to_string(),
        None => format!("main{}{}.to", suffix(system), suffix(arch)),
    };
    if !test {
        let path = job.case.join(&main);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &job.code).unwrap();
    }
    // With the memory of each region that ends made unreadable, so that a
    // value put in a region it outlives faults.
    let run = |args: &[&str]| {
        let mut command = Command::new(tondo);
        command.args(args).current_dir(&job.case).env("TONDO_DUMP", "off").env("TONDO_CHECK_LIFETIMES", "1");
        command.output().unwrap()
    };
    let checks: &[&str] = if test { &["test", "--list"] } else { &["check", &main] };
    let mut args = checks.to_vec();
    if job.info.ends_with(" library") {
        args.push("--library");
    }
    let output = run(&args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = ["error", "warning"].into_iter().find_map(|severity| {
        let code = job.info.strip_prefix(&format!("tondo {severity} "))?.split(' ').next()?;
        Some((severity, code))
    });
    let ok = match expected {
        None => output.status.success(),
        Some((severity, code)) => {
            output.status.success() == (severity == "warning") && stderr.contains(&format!("{severity}[{code}]"))
        }
    };
    if !ok {
        return Outcome::Failed(format!("{}:\n{}\n{stderr}", job.name, job.code));
    }
    // What the page quotes of its diagnostics is there, line by line, in
    // order: the first line alone, or the message with its excerpt.
    if let Some((line, quoted)) = &job.quoted {
        let mut said = stderr.lines().map(str::trim_end);
        if let Some(missing) = quoted.lines().map(str::trim_end).find(|quote| !said.any(|line| line == *quote)) {
            return Outcome::Failed(format!(
                "{}, line {line}: the diagnostic it quotes has no line `{missing}` here, in its order; `tondo check \
                 {main}` prints\n{stderr}",
                job.name.split(',').next().unwrap_or_default()
            ));
        }
    }
    if test {
        // The tests run too: an example that only compiled could fail unseen.
        let ran = run(&["test"]);
        let fails = job.info == "tondo test failing";
        if ran.status.success() == fails {
            let said = String::from_utf8_lossy(&ran.stdout).into_owned() + &String::from_utf8_lossy(&ran.stderr);
            let expected = if fails { "fail, as `tondo test failing` says" } else { "pass" };
            return Outcome::Failed(format!("the tests of {} should {expected}:\n{said}", job.name));
        }
        return Outcome::Fine;
    }
    let Some((_, says)) = &job.output else { return Outcome::Fine };
    // What it prints, where it runs.
    let here = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let here_arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    if system.is_some_and(|system| system != here) || arch.is_some_and(|arch| arch != here_arch) {
        return Outcome::Fine;
    }
    let ran = run(&["run", &main]);
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

/// Where `docs/toolchain/commands.md` stops being written by hand: what
/// follows it is what `tondo help --all` prints.
const COMMANDS: &str = "<!-- From here on, this page is what `tondo help --all` prints";

#[test]
fn the_reference_of_the_commands_is_current() {
    let path = root().join("docs/toolchain/commands.md");
    let page = std::fs::read_to_string(&path).unwrap();
    let help = Command::new(env!("CARGO_BIN_EXE_tondo")).args(["help", "--all"]).output().unwrap();
    assert!(help.status.success(), "{}", String::from_utf8_lossy(&help.stderr));
    let (top, rest) =
        page.split_once(COMMANDS).expect("docs/toolchain/commands.md has the line that starts its listing");
    let marker = rest.lines().next().unwrap_or("");
    let listing = String::from_utf8_lossy(&help.stdout);
    let current = format!("{top}{COMMANDS}{marker}\n\n```text\n{}\n```\n", listing.trim_end());
    if update() {
        std::fs::write(&path, current).unwrap();
        return;
    }
    assert!(
        page == current,
        "docs/toolchain/commands.md is not what `tondo help --all` prints: after a change to a command, write it \
         again with `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs`"
    );
}

/// The examples of a page, the files and listings its listings need, in its
/// order, and what is wrong in them before anything runs.
fn read_page(name: &str, document: usize, text: &str, dir: &Path) -> (Vec<Job>, Vec<Step>, Vec<String>) {
    let (mut jobs, mut steps, mut wrong) = (Vec::new(), Vec::new(), Vec::new());
    let found = fences(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut files: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for (index, fence) in found.iter().enumerate() {
        let at = format!("{name}, line {}", fence.body.start);
        let case = dir.join(format!("{}-{index}", name.replace(['/', '.'], "-")));
        let named = attribute(&fence.info, "file");
        match fence.info.split_whitespace().next().unwrap_or_default() {
            "tondo" => {
                let next = found.get(index + 1);
                // An output block belongs to the program right before it,
                // and a plain block after a wrong one quotes its diagnostic.
                let output = next
                    .filter(|next| next.info == "text output" && prints(&fence.info))
                    .map(|next| (next.body.clone(), next.code.clone()));
                let wrong_one = ["tondo error ", "tondo warning "].iter().any(|form| fence.info.starts_with(form));
                let quoted = next
                    .filter(|next| next.info == "text" && wrong_one)
                    .map(|next| (next.body.start, next.code.clone()));
                if let Some(file) = named {
                    files.insert(file, fence.code.clone());
                    steps.push(Step::File { name: file.to_string(), text: fence.code.clone() });
                }
                let (info, code) = (fence.info.clone(), fence.code.clone());
                jobs.push(Job { name: format!("{at} ({info})"), document, info, code, case, output, quoted });
            }
            "diff" => {
                let Some(file) = named else {
                    wrong.push(format!("{at}: a `diff` names the file it makes, with `file=`"));
                    continue;
                };
                let from = attribute(&fence.info, "from").unwrap_or(file);
                let made = match files.get(from) {
                    Some(old) => patched(old, &fence.code),
                    None => Err(format!("no `file={from}` comes before it")),
                };
                let text = match made {
                    Ok(text) => text,
                    Err(why) => {
                        wrong.push(format!("{at} ({}): {why}", fence.info));
                        continue;
                    }
                };
                files.insert(file, text.clone());
                steps.push(Step::File { name: file.to_string(), text: text.clone() });
                // The variant is an example too, wrong as `error <code>` or
                // `warning <code>` says, and then quoted by a plain block.
                if file.ends_with(".to") {
                    let words: Vec<&str> = fence.info.split_whitespace().collect();
                    let wrong = words.windows(2).find(|pair| matches!(pair[0], "error" | "warning"));
                    let info = match wrong {
                        Some(pair) => format!("tondo {} {} file={file}", pair[0], pair[1]),
                        None => format!("tondo file={file}"),
                    };
                    let quoted = found
                        .get(index + 1)
                        .filter(|next| next.info == "text" && wrong.is_some())
                        .map(|next| (next.body.start, next.code.clone()));
                    let name = format!("{at} ({})", fence.info);
                    jobs.push(Job { name, document, info, code: text, case, output: None, quoted });
                }
            }
            _ if fence.info == "text listing" => match commands(name, fence, &lines) {
                Ok(commands) => steps.extend(commands),
                Err(why) => wrong.push(why),
            },
            _ => {}
        }
    }
    if let Some(stray) = found
        .iter()
        .enumerate()
        .find(|(index, fence)| fence.info == "text output" && (*index == 0 || !prints(&found[index - 1].info)))
    {
        panic!("{name}, line {}: a block of `text output` follows no program", stray.1.body.start);
    }
    (jobs, steps, wrong)
}

#[test]
fn examples_compile_and_print_what_the_documents_say() {
    let root = root();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("docs");
    let _ = std::fs::remove_dir_all(&dir);
    let documents = documents();
    let texts: Vec<String> = documents.iter().map(|path| std::fs::read_to_string(path).unwrap()).collect();
    let mut jobs = Vec::new();
    // The pages with listings, each with the steps they take in its order.
    let mut pages: Vec<(usize, Vec<Step>)> = Vec::new();
    let mut wrong = Vec::new();
    for (document, text) in texts.iter().enumerate() {
        let name = documents[document].strip_prefix(&root).unwrap().display().to_string();
        let (found, steps, failed) = read_page(&name, document, text, &dir);
        jobs.extend(found);
        wrong.extend(failed);
        if steps.iter().any(|step| matches!(step, Step::Command { .. })) {
            pages.push((document, steps));
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
    // The listings, each page's in its order, the pages side by side.
    let listed: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = pages
            .iter()
            .map(|(document, steps)| {
                let name = documents[*document].strip_prefix(&root).unwrap().display().to_string();
                let page = dir.join("listings").join(name.replace(['/', '.'], "-"));
                scope.spawn(move || (*document, listings(&name, steps, &page)))
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    let mut failures = Vec::new();
    // What each document's output blocks and listings become, last block
    // first so that the lines of those above stay where they were.
    let mut rewrites: Vec<Vec<Rewrite>> = vec![Vec::new(); documents.len()];
    for (document, (failed, rewritten)) in listed {
        wrong.extend(failed);
        rewrites[document].extend(rewritten);
    }
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
        println!("wrote the outputs and listings of {}", documents[document].display());
    }
    failures.sort_by_key(|(index, _)| *index);
    let failures: Vec<String> = failures.into_iter().map(|(_, why)| why).chain(wrong).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A page's named programs, the variant a `diff` makes, its listing and the
/// diagnostic it quotes, read and run as the manual's are; and what a page
/// says wrong fails, saying where.
#[test]
fn a_page_holds_its_listings_variants_and_quotes() {
    let page = r#"A program:

```tondo file=hello.to
import std.console

fn main() {
    console.println("hello")
}
```

```diff file=bye.to from=hello.to
 fn main() {
-    console.println("hello")
+    console.println("bye")
 }
```

```text listing
$ tondo run bye.to
bye

$ tondo check hello.to

$ tondo run 'hello.to'
hello
```

```diff file=broken.to from=hello.to error E1001
-    console.println("hello")
+    undefined()
```

```text
error[E1001]: unknown name `undefined`; Tondo writes `none`, the `T?` without a value
 --> broken.to:4:5
```

```tondo error E1001 file=wrong.to
fn main() {
    undefined()
}
```

```text
 --> wrong.to:2:5
2 |     undefined()
```
"#;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("docs-page");
    let _ = std::fs::remove_dir_all(&dir);
    let read = |page: &str| read_page("page.md", 0, page, &dir);
    let (jobs, steps, wrong) = read(page);
    assert!(wrong.is_empty(), "{wrong:?}");
    assert_eq!(
        jobs.iter().map(|job| job.info.as_str()).collect::<Vec<_>>(),
        [
            "tondo file=hello.to",
            "tondo file=bye.to",
            "tondo error E1001 file=broken.to",
            "tondo error E1001 file=wrong.to"
        ]
    );
    for job in &jobs {
        if let Outcome::Failed(why) = check(env!("CARGO_BIN_EXE_tondo"), job) {
            panic!("{why}");
        }
    }
    let (failed, rewritten) = listings("page.md", &steps, &dir.join("listings"));
    assert!(failed.is_empty() && rewritten.is_empty(), "{failed:?}");

    // A listing that says otherwise, or a quote that is not in the diagnostic.
    let (jobs, steps, _) = read(&page.replace("bye\n\n$", "goodbye\n\n$").replace("wrong.to:2:5", "wrong.to:3:5"));
    let (failed, rewritten) = listings("page.md", &steps, &dir.join("again"));
    match update() {
        true => assert_eq!(rewritten, [(19..20, "bye".to_string())]),
        false => assert!(
            failed.len() == 1 && failed[0].starts_with("page.md, line 19: `tondo run bye.to` prints\nbye\n"),
            "{failed:?}"
        ),
    }
    let Outcome::Failed(why) = check(env!("CARGO_BIN_EXE_tondo"), &jobs[3]) else { panic!("the quote held") };
    assert!(why.contains("no line ` --> wrong.to:3:5`"), "{why}");
    // A wrong variant that compiles is wrong.
    let (jobs, _, _) = read(&page.replace("+    undefined()", "+    console.println(\"hi\")"));
    let Outcome::Failed(why) = check(env!("CARGO_BIN_EXE_tondo"), &jobs[2]) else { panic!("the variant held") };
    assert!(why.contains("(diff file=broken.to from=hello.to error E1001)"), "{why}");

    // A variant whose lines are not there, and a command that needs a shell.
    let (_, _, wrong) = read(
        &page.replace(" fn main() {\n-    console.println(\"hello\")", " fn main() {\n-    console.println(\"hi\")"),
    );
    assert!(wrong.len() == 1 && wrong[0].contains("is in the file 0 times"), "{wrong:?}");
    let (_, _, wrong) = read(&page.replace("$ tondo check hello.to", "$ tondo check hello.to | head"));
    assert!(wrong.len() == 1 && wrong[0].contains("asks a shell for `|`"), "{wrong:?}");
}
