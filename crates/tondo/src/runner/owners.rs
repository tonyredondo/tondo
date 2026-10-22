//! The owners of each suite and test, from CODEOWNERS (testing 5.7): the
//! last rule that matches the file declaring the node gives all its owners.

use std::path::{Path, PathBuf};

pub enum Codeowners {
    /// The first of `.github/CODEOWNERS`, `CODEOWNERS` and `docs/CODEOWNERS`.
    Auto,
    None,
    /// A path from the repository's root.
    Path(String),
}

/// The rules of the CODEOWNERS file in use.
pub struct Owners {
    rules: Vec<(Rule, Vec<String>)>,
    /// The project's directory from the repository's root, with a final `/`.
    prefix: String,
}

impl Owners {
    /// The owners of a file of the project, given by its path there.
    pub fn of(&self, file: &str) -> Vec<String> {
        let path = format!("{}{file}", self.prefix);
        self.rules.iter().rev().find(|(rule, _)| rule.matches(&path)).map(|(_, o)| o.clone()).unwrap_or_default()
    }
}

/// Reads the CODEOWNERS of the repository holding the project at `dir`.
pub fn resolve(dir: &Path, mode: &Codeowners) -> Result<Owners, String> {
    let dir = std::fs::canonicalize(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    // The repository's root holds `.git`; without one, the project is the root.
    let root = dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(&dir).to_path_buf();
    let mut prefix: String = dir
        .strip_prefix(&root)
        .unwrap_or(Path::new(""))
        .components()
        .map(|c| format!("{}/", c.as_os_str().to_string_lossy()))
        .collect();
    if prefix == "/" {
        prefix.clear();
    }
    let none = Owners { rules: Vec::new(), prefix: prefix.clone() };
    let chosen = match mode {
        Codeowners::None => return Ok(none),
        Codeowners::Auto => {
            match [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"].into_iter().find(|p| root.join(p).exists()) {
                Some(path) => path.to_string(),
                None => return Ok(none),
            }
        }
        Codeowners::Path(path) => {
            let logical = path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
            if !logical || path.starts_with('/') {
                return Err(format!(
                    "--codeowners {path:?} is not a path from the repository's root without `.` or `..`"
                ));
            }
            path.clone()
        }
    };
    let file = root.join(&chosen);
    check_inside(&root, &file, &chosen)?;
    let bytes = std::fs::read(&file).map_err(|e| format!("cannot read the CODEOWNERS file {chosen}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("the CODEOWNERS file {chosen} is not UTF-8"))?;
    if text.starts_with('\u{feff}') {
        return Err(format!("the CODEOWNERS file {chosen} starts with a byte order mark; remove it"));
    }
    let mut rules = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut words = line.split([' ', '\t']).filter(|w| !w.is_empty());
        let Some(pattern) = words.next() else { continue };
        if pattern.starts_with('#') {
            continue;
        }
        let owners: Vec<String> = words.map(str::to_string).collect();
        let at = format!("{chosen}:{}", index + 1);
        if owners.is_empty() {
            return Err(format!("{at}: the rule `{pattern}` names no owner; add one, as `{pattern} @team`"));
        }
        rules.push((Rule::parse(pattern).map_err(|why| format!("{at}: {why}"))?, owners));
    }
    Ok(Owners { rules, prefix })
}

/// A CODEOWNERS file must be a regular file inside the repository.
fn check_inside(root: &Path, file: &Path, chosen: &str) -> Result<(), String> {
    let real: PathBuf =
        std::fs::canonicalize(file).map_err(|e| format!("cannot read the CODEOWNERS file {chosen}: {e}"))?;
    if !real.starts_with(root) || !real.is_file() {
        return Err(format!("the CODEOWNERS file {chosen} is not a regular file inside the repository"));
    }
    Ok(())
}

/// A pattern of the portable subset.
struct Rule {
    /// Its segments, with `**` for any number of them.
    pattern: Vec<char>,
    /// Matches any whole segment, and then its subtree.
    segment: bool,
    /// Starts at the first segment.
    anchored: bool,
}

impl Rule {
    fn parse(text: &str) -> Result<Rule, String> {
        if text.starts_with('!') || text.contains(['[', ']', '\\']) {
            return Err(format!("`{text}` uses `!`, `[]` or `\\`, which the portable subset leaves out"));
        }
        let anchored = text.starts_with('/');
        let mut body = text.trim_start_matches('/').to_string();
        // A final `/` covers the subtree.
        if body.ends_with('/') {
            body.push_str("**");
        }
        if body.is_empty() || body.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
            return Err(format!("`{text}` has an empty segment, `.` or `..`"));
        }
        let segment = !anchored && !body.contains('/');
        Ok(Rule { pattern: body.chars().collect(), segment, anchored })
    }

    fn matches(&self, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        if self.segment {
            // Any segment; a directory's match covers what it holds.
            return segments.iter().any(|s| glob(&self.pattern, &s.chars().collect::<Vec<_>>()));
        }
        if self.anchored && !self.pattern.contains(&'/') {
            return segments.first().is_some_and(|s| glob(&self.pattern, &s.chars().collect::<Vec<_>>()));
        }
        glob(&self.pattern, &path.chars().collect::<Vec<_>>())
    }
}

/// `*` and `?` do not cross `/`; `**` does.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let mut reach = vec![false; text.len() + 1];
    reach[0] = true;
    let mut i = 0;
    while i < pattern.len() {
        let mut next = vec![false; text.len() + 1];
        let double = pattern[i] == '*' && pattern.get(i + 1) == Some(&'*');
        for j in 0..=text.len() {
            if !reach[j] {
                continue;
            }
            match pattern[i] {
                '*' if double => next[j..].iter_mut().for_each(|n| *n = true),
                '*' => {
                    let end = text[j..].iter().position(|&c| c == '/').map_or(text.len(), |p| j + p);
                    next[j..=end].iter_mut().for_each(|n| *n = true);
                }
                '?' if j < text.len() && text[j] != '/' => next[j + 1] = true,
                c if j < text.len() && text[j] == c => next[j + 1] = true,
                _ => {}
            }
        }
        reach = next;
        i += if double { 2 } else { 1 };
    }
    reach[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(rules: &[(&str, &str)], path: &str) -> Option<String> {
        let rules = rules.iter().map(|(p, o)| (Rule::parse(p).unwrap(), vec![o.to_string()])).collect();
        Owners { rules, prefix: String::new() }.of(path).first().cloned()
    }

    #[test]
    fn the_last_matching_rule_wins() {
        let rules = [("*", "@all"), ("/src/", "@src"), ("*_test.to", "@tests"), ("docs", "@docs")];
        assert_eq!(owner(&rules, "src/main.to").as_deref(), Some("@src"));
        assert_eq!(owner(&rules, "src/main_test.to").as_deref(), Some("@tests"));
        assert_eq!(owner(&rules, "tests/api.to").as_deref(), Some("@all"));
        assert_eq!(owner(&rules, "lib/docs/x.to").as_deref(), Some("@docs"));
        assert_eq!(owner(&[("/src/*.to", "@a")], "src/a/b.to"), None);
        assert_eq!(owner(&[("src/**/b.to", "@a")], "src/a/c/b.to").as_deref(), Some("@a"));
        assert!(Rule::parse("!x").is_err() && Rule::parse("a//b").is_err() && Rule::parse("[ab]").is_err());
    }
}
