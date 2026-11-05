//! `tondo test --bisect <good>` (testing 10.8): the first commit where a test
//! fails, in a worktree of its own, so that what is being edited stays. The
//! test comes from here: one written today, as `dump test` writes, still says
//! if an older commit fails.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let ran = Command::new("git").args(args).current_dir(dir).output().map_err(|e| format!("cannot run git: {e}"))?;
    match ran.status.success() {
        true => Ok(String::from_utf8_lossy(&ran.stdout).trim().to_string()),
        false => Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&ran.stderr).trim())),
    }
}

/// How a commit went: the test passes there, fails, or cannot say, as a
/// commit that does not compile.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Verdict {
    Passes,
    Fails,
    Unknown,
}

/// A worktree that goes away with its value.
pub(crate) struct Worktree {
    repository: PathBuf,
    pub(crate) dir: PathBuf,
}

impl Worktree {
    /// The project at `root` as it was at `rev`: its worktree, and where
    /// the project is in it.
    pub(crate) fn at(root: &Path, rev: &str) -> Result<(Worktree, PathBuf), String> {
        let root = if root.is_dir() { root } else { root.parent().unwrap_or(Path::new(".")) };
        let top = PathBuf::from(git(root, &["rev-parse", "--show-toplevel"])?);
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let inside = root.strip_prefix(&top).unwrap_or(Path::new("")).to_path_buf();
        let dir = std::env::temp_dir().join(format!("tondo-worktree-{}", std::process::id()));
        let shown = dir.to_string_lossy().into_owned();
        git(&top, &["worktree", "add", "--quiet", "--detach", &shown, rev])?;
        let project = dir.join(&inside);
        Ok((Worktree { repository: top, dir }, project))
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let dir = self.dir.to_string_lossy().into_owned();
        let _ = git(&self.repository, &["worktree", "remove", "--force", &dir]);
    }
}

/// The test files of the project here, to bring into each commit.
fn test_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let listed = git(root, &["ls-files", "--full-name", "--cached", "--others", "--exclude-standard"])?;
    Ok(listed.lines().filter(|file| file.ends_with("_test.to")).map(PathBuf::from).collect())
}

fn verdict(tree: &Path, inside: &Path, selection: &[String], tests: &[(PathBuf, Vec<u8>)]) -> Result<Verdict, String> {
    for (file, text) in tests {
        let path = tree.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    let tondo = std::env::current_exe().map_err(|e| e.to_string())?;
    let ran = Command::new(tondo)
        .arg("test")
        .args(selection)
        .arg("--json")
        .current_dir(tree.join(inside))
        .output()
        .map_err(|e| e.to_string())?;
    let report: Value = serde_json::from_slice(&ran.stdout).unwrap_or(Value::Null);
    let ran_any = report["summary"]["executed"].as_u64().unwrap_or(0) > 0;
    Ok(match (report["compiled"] == true && ran_any, ran.status.success()) {
        (false, _) => Verdict::Unknown,
        (true, true) => Verdict::Passes,
        (true, false) => Verdict::Fails,
    })
}

/// The first commit after `good` where the tests of `selection` fail.
pub fn bisect(root: &Path, good: &str, selection: &[String], json: bool) -> Result<(), String> {
    let root = if root.is_dir() { root } else { root.parent().unwrap_or(Path::new(".")) };
    let top = PathBuf::from(git(root, &["rev-parse", "--show-toplevel"])?);
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let inside = root.strip_prefix(&top).unwrap_or(Path::new("")).to_path_buf();
    let listed = git(&top, &["rev-list", "--first-parent", "--reverse", &format!("{good}..HEAD")])?;
    let commits: Vec<&str> = listed.lines().collect();
    if commits.is_empty() {
        return Err(format!("there are no commits after {good}"));
    }
    let tests: Vec<(PathBuf, Vec<u8>)> = test_files(&root)?
        .into_iter()
        .filter_map(|file| Some((file.clone(), std::fs::read(top.join(&file)).ok()?)))
        .collect();
    let dir = std::env::temp_dir().join(format!("tondo-bisect-{}", std::process::id()));
    let shown = dir.to_string_lossy().into_owned();
    git(&top, &["worktree", "add", "--quiet", "--detach", &shown, commits[commits.len() - 1]])?;
    let worktree = Worktree { repository: top, dir };
    let at = |commit: &str| -> Result<Verdict, String> {
        git(&worktree.dir, &["checkout", "--quiet", "--force", commit])?;
        git(&worktree.dir, &["clean", "--quiet", "-fd"])?;
        let verdict = verdict(&worktree.dir, &inside, selection, &tests)?;
        if !json {
            eprintln!("tondo: {} {verdict:?}", &commit[..commit.len().min(10)]);
        }
        Ok(verdict)
    };
    if at(commits[commits.len() - 1])? != Verdict::Fails {
        return Err("it does not fail at the last commit: there is nothing to look for".to_string());
    }
    if at(good)? == Verdict::Fails {
        return Err(format!("it fails at {good} too: give a commit where it passes"));
    }
    // The first that fails is in `low..=high`; a commit that cannot say
    // gives way to the next one that can.
    let (mut low, mut high) = (0, commits.len() - 1);
    while low < high {
        let middle = (low + high) / 2;
        let mut probe = middle;
        let mut found = Verdict::Unknown;
        while probe < high {
            found = at(commits[probe])?;
            if found != Verdict::Unknown {
                break;
            }
            probe += 1;
        }
        match found {
            Verdict::Fails => high = probe,
            Verdict::Passes => low = probe + 1,
            // None between here and what fails can say.
            Verdict::Unknown => high = middle.max(low),
        }
        if found == Verdict::Unknown {
            break;
        }
    }
    let first = commits[high];
    let title = git(&worktree.repository, &["log", "-1", "--format=%s", first])?;
    match json {
        true => crate::cli::print_json("test-bisect", true, json!({ "commit": first, "title": title })),
        false => println!("first fails at {first}: {title}"),
    }
    Ok(())
}
