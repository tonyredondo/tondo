//! What `env.build()` reports (standard library 14.3.6): the program's
//! name and version from `tondo.toml`, the commit of its git repository
//! and whether its tracked files changed since, and its dependencies.

use crate::ast::Built;
use std::path::{Path, PathBuf};

/// The lines `std/environment.to` reads: name, version, commit (empty
/// without one), `true` when tracked files changed, the compiler, then a
/// `name\tversion` line per dependency.
pub fn describe(built: &Built) -> String {
    let (commit, modified) = match repository(&built.dir) {
        Some(git) => (commit(&git).unwrap_or_default(), modified(&built.dir)),
        None => (String::new(), false),
    };
    let mut text =
        format!("{}\n{}\n{commit}\n{modified}\ntondo {}\n", built.name, built.version, crate::driver::VERSION);
    for (name, version) in &built.dependencies {
        text.push_str(&format!("{name}\t{version}\n"));
    }
    text
}

/// The files whose changes change what `describe` says, for cache keys.
pub fn inputs(dir: &Path) -> Vec<PathBuf> {
    let Some(git) = repository(dir) else { return Vec::new() };
    let mut found = vec![git.join("HEAD"), git.join("index"), git.join("packed-refs")];
    if let Ok(head) = std::fs::read_to_string(git.join("HEAD"))
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        found.push(git.join(reference));
    }
    found.into_iter().filter(|path| path.is_file()).collect()
}

/// The `.git` directory of the repository that holds `dir`, following the
/// `gitdir:` of a worktree.
fn repository(dir: &Path) -> Option<PathBuf> {
    let dir = dir.canonicalize().ok()?;
    let found = dir.ancestors().map(|d| d.join(".git")).find(|git| git.exists())?;
    if found.is_dir() {
        return Some(found);
    }
    let pointer = std::fs::read_to_string(&found).ok()?;
    let target = pointer.trim().strip_prefix("gitdir: ")?;
    Some(found.parent()?.join(target))
}

/// The commit `HEAD` names, read from the repository's files.
fn commit(git: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let Some(reference) = head.trim().strip_prefix("ref: ") else { return Some(head.trim().to_string()) };
    // A worktree keeps its refs in the main repository.
    let common = std::fs::read_to_string(git.join("commondir")).ok().map(|c| git.join(c.trim()));
    for base in [Some(git.to_path_buf()), common].into_iter().flatten() {
        if let Ok(loose) = std::fs::read_to_string(base.join(reference)) {
            return Some(loose.trim().to_string());
        }
        if let Ok(packed) = std::fs::read_to_string(base.join("packed-refs")) {
            let line = packed.lines().find(|line| line.ends_with(&format!(" {reference}")));
            if let Some((hash, _)) = line.and_then(|line| line.split_once(' ')) {
                return Some(hash.to_string());
            }
        }
    }
    None
}

/// Whether tracked files differ from the commit, as `git status` says;
/// without git, `false`.
fn modified(dir: &Path) -> bool {
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(dir)
        .stderr(std::process::Stdio::null())
        .output();
    status.is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}
