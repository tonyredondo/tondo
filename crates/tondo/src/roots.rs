//! Where a program's sources came from (toolchain 7.6): a table of the
//! repository and commit of the program and of each dependency, with a hash
//! of each file, which the executable keeps and dumps carry. With it a tool
//! tells when the files at hand are not those of the build, and gets those
//! that were, from the place they came from.

use crate::source::SourceMap;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What starts the table in an executable, where a tool looks for it.
pub const MAGIC: &str = "TONDOROOTS1\n";

/// What ends it.
const END: &str = "END\n";

/// A repository at a commit: what its files are named from in the program,
/// and the directory of the package inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub prefix: String,
    pub repository: String,
    pub commit: String,
    pub directory: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roots {
    pub roots: Vec<Root>,
    /// Each file of the program but the standard library's, by its hash.
    pub files: BTreeMap<String, String>,
    /// The project's `[source] url`.
    pub template: Option<String>,
}

/// The repository of a directory: where its root is, its first remote
/// without credentials, and the commit it is at.
fn repository_of(dir: &Path) -> Option<(PathBuf, String, String)> {
    let git = crate::built::repository(dir)?;
    let commit = crate::built::commit(&git)?;
    let ask = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let remote = ask(&["config", "--get", "remote.origin.url"])?;
    let top = ask(&["rev-parse", "--show-toplevel"])?;
    Some((PathBuf::from(top), without_credentials(&remote), commit))
}

/// A repository's address as it can be shown: a token in it never leaves
/// the machine that built the program.
pub fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let Some((user, host)) = authority.rsplit_once('@') else { return url.to_string() };
    match scheme {
        // A name that says who, as `git@`, and no more.
        "ssh" => format!("ssh://{}@{host}/{path}", user.split(':').next().unwrap_or(user)),
        _ => format!("{scheme}://{host}/{path}"),
    }
}

/// A dependency's name, directory, and the repository and commit it was
/// fetched from.
pub type Dependency = (String, PathBuf, Option<(String, String)>);

/// What a build says of where it comes from: the project's directory, its
/// dependencies, and `[source] url`.
#[derive(Clone, Debug, Default)]
pub struct Project {
    pub dir: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub template: Option<String>,
}

/// The table of a build, as the executable keeps it: empty without any
/// repository, as a file on its own.
pub fn table(project: &Project, sources: &SourceMap) -> String {
    let mut roots = Vec::new();
    let mut root_of = |prefix: String, dir: &Path, pinned: Option<&(String, String)>| {
        let (repository, commit, directory) = match pinned {
            Some((url, commit)) => (without_credentials(url), commit.clone(), String::new()),
            None => {
                let Some((top, repository, commit)) = repository_of(dir) else { return };
                let inside = dir.canonicalize().ok().and_then(|d| Some(d.strip_prefix(&top).ok()?.to_path_buf()));
                (repository, commit, inside.unwrap_or_default().to_string_lossy().into_owned())
            }
        };
        roots.push(Root { prefix, repository, commit, directory });
    };
    root_of(String::new(), &project.dir, None);
    for (name, dir, origin) in &project.dependencies {
        root_of(format!("{name}/"), dir, origin.as_ref());
    }
    let files: BTreeMap<String, String> = sources
        .files()
        .filter(|file| !file.logical.starts_with('<') && !file.logical.is_empty())
        .map(|file| (file.logical.clone(), hash(file.text.as_bytes())))
        .collect();
    if roots.is_empty() {
        return String::new();
    }
    Roots { roots, files, template: project.template.clone() }.text()
}

/// The hash a table keeps of a file.
pub fn hash(bytes: &[u8]) -> String {
    crate::driver::fingerprint(&[bytes])
}

impl Roots {
    fn text(&self) -> String {
        let mut text = MAGIC.to_string();
        for root in &self.roots {
            let Root { prefix, repository, commit, directory } = root;
            text.push_str(&format!("R\t{prefix}\t{repository}\t{commit}\t{directory}\n"));
        }
        for (file, hash) in &self.files {
            text.push_str(&format!("F\t{file}\t{hash}\n"));
        }
        if let Some(template) = &self.template {
            text.push_str(&format!("U\t{template}\n"));
        }
        text.push_str(END);
        text
    }

    pub fn parse(text: &str) -> Roots {
        let mut roots = Roots::default();
        for line in text.lines() {
            match line.split('\t').collect::<Vec<_>>().as_slice() {
                ["R", prefix, repository, commit, directory] => roots.roots.push(Root {
                    prefix: prefix.to_string(),
                    repository: repository.to_string(),
                    commit: commit.to_string(),
                    directory: directory.to_string(),
                }),
                ["F", file, hash] => drop(roots.files.insert(file.to_string(), hash.to_string())),
                ["U", template] => roots.template = Some(template.to_string()),
                _ => {}
            }
        }
        roots
    }

    /// The table in an executable.
    pub fn of_executable(path: &Path) -> Result<Roots, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let magic = MAGIC.as_bytes();
        let start = bytes.windows(magic.len()).position(|window| window == magic);
        let start = start.ok_or_else(|| format!("{} says nothing of its sources", path.display()))?;
        let end = END.as_bytes();
        let length = bytes[start..].windows(end.len()).position(|window| window == end).unwrap_or(0);
        Ok(Roots::parse(&String::from_utf8_lossy(&bytes[start..start + length])))
    }

    /// The repository a file of the program is in, and its path there.
    pub fn place(&self, file: &str) -> Option<(&Root, String)> {
        let root = self.roots.iter().filter(|r| file.starts_with(&r.prefix)).max_by_key(|r| r.prefix.len())?;
        let inside = &file[root.prefix.len()..];
        let path = match root.directory.as_str() {
            "" => inside.to_string(),
            directory => format!("{directory}/{inside}"),
        };
        Some((root, path))
    }

    /// The page of a line of a file at its provider.
    pub fn link(&self, file: &str, line: u64) -> Option<String> {
        let (root, path) = self.place(file)?;
        let own = self.template.as_deref().filter(|_| root.prefix.is_empty());
        let (host, repository) = address(&root.repository)?;
        let template = own.map(str::to_string).or_else(|| provider(&host))?;
        Some(
            template
                .replace("{host}", &host)
                .replace("{repo}", &repository)
                .replace("{commit}", &root.commit)
                .replace("{path}", &path)
                .replace("{line}", &line.to_string()),
        )
    }

    /// One line of what a build came from, for a person.
    pub fn summary(&self) -> Option<String> {
        let root = self.roots.first().filter(|root| root.prefix.is_empty())?;
        let (host, repository) = address(&root.repository).unwrap_or((String::new(), root.repository.clone()));
        let short = &root.commit[..root.commit.len().min(10)];
        let place = if host.is_empty() { repository } else { format!("{host}/{repository}") };
        Some(format!("built from {place} at {short}"))
    }
}

/// The host of a repository and its path there, without `.git`.
fn address(url: &str) -> Option<(String, String)> {
    let (host, path) = match url.split_once("://") {
        Some(("file", _)) => return None,
        Some((_, rest)) => rest.split_once('/')?,
        None => url.split_once(':')?,
    };
    let host = host.rsplit('@').next()?.split(':').next()?;
    let path = path.trim_matches('/').trim_end_matches(".git");
    // Azure's SSH address, as its web pages have it.
    if let Some(rest) = path.strip_prefix("v3/").filter(|_| host == "ssh.dev.azure.com") {
        let (project, name) = rest.rsplit_once('/')?;
        return Some(("dev.azure.com".into(), format!("{project}/_git/{name}")));
    }
    Some((host.to_string(), path.to_string()))
}

/// How the known providers write the page of a line of a file.
fn provider(host: &str) -> Option<String> {
    let page = if host.contains("github") {
        "https://{host}/{repo}/blob/{commit}/{path}#L{line}"
    } else if host.contains("gitlab") {
        "https://{host}/{repo}/-/blob/{commit}/{path}#L{line}"
    } else if host.contains("bitbucket") {
        "https://{host}/{repo}/src/{commit}/{path}#lines-{line}"
    } else if host.contains("codeberg") || host.contains("gitea") || host.contains("forgejo") {
        "https://{host}/{repo}/src/commit/{commit}/{path}#L{line}"
    } else if host == "dev.azure.com" {
        "https://{host}/{repo}?path=/{path}&version=GC{commit}&line={line}"
    } else {
        return None;
    };
    Some(page.to_string())
}

/// Where a file of a build is for a tool that shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// Here, as it was built.
    Same(PathBuf),
    /// Here, but it changed since the build, and nothing else has it: with
    /// the build's hash and the file's.
    Changed(PathBuf),
    /// Not here, or not as it was: at a commit of a repository.
    Elsewhere,
    /// Neither here nor anywhere the table knows.
    Lost,
}

impl Roots {
    /// Where `file` is, for a tool that runs in `root`.
    pub fn find(&self, root: &Path, file: &str) -> Found {
        let here = root.join(file);
        let known = self.files.get(file);
        match (std::fs::read(&here), known, self.place(file)) {
            (Ok(text), Some(built), _) if hash(&text) == *built => Found::Same(here),
            (Ok(_), None, _) => Found::Same(here),
            (_, _, Some((root, _))) if !root.repository.is_empty() => Found::Elsewhere,
            (Ok(_), ..) => Found::Changed(here),
            _ => Found::Lost,
        }
    }

    /// The text of `file` at its commit, from what `tondo` has of its
    /// repository, or from there: only what that file needs, with the
    /// credentials `git` has. It stays in the cache, by its commit.
    pub fn fetch(&self, file: &str) -> Result<PathBuf, String> {
        let (root, path) = self.place(file).ok_or_else(|| format!("no repository is known for {file}"))?;
        let kept = crate::driver::cache_dir().join("sources").join(&root.commit).join(file);
        if kept.is_file() {
            return Ok(kept);
        }
        let checkout = crate::driver::cache_dir().join("deps").join(format!("git-{}", root.commit)).join(&path);
        let bytes = match std::fs::read(&checkout) {
            Ok(bytes) => bytes,
            Err(_) => from_repository(&root.repository, &root.commit, &path)?,
        };
        let parent = kept.parent().expect("a file in the cache");
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        std::fs::write(&kept, bytes).map_err(|e| format!("cannot write {}: {e}", kept.display()))?;
        Ok(kept)
    }
}

/// A file at a commit: a repository without its files is cloned once, and
/// git brings each file as it is asked for.
fn from_repository(url: &str, commit: &str, path: &str) -> Result<Vec<u8>, String> {
    let cache = crate::driver::cache_dir().join("git");
    std::fs::create_dir_all(&cache).map_err(|e| format!("cannot create {}: {e}", cache.display()))?;
    let repository = cache.join(crate::driver::fingerprint(&[url.as_bytes()]));
    let ran = |command: &mut Command| -> Result<Vec<u8>, String> {
        let output = command.output().map_err(|e| format!("cannot run `git`: {e}"))?;
        match output.status.success() {
            true => Ok(output.stdout),
            false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        }
    };
    if !repository.is_dir() {
        let partial = repository.with_extension(format!("partial{}", std::process::id()));
        let mut clone = Command::new("git");
        clone.args(["clone", "--quiet", "--bare", "--filter=blob:none", url]).arg(&partial);
        ran(&mut clone).map_err(|e| format!("cannot reach {url}: {e}"))?;
        std::fs::rename(&partial, &repository).map_err(|e| e.to_string())?;
    }
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command.arg("-C").arg(&repository).args(args);
        ran(&mut command)
    };
    if git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_err() {
        // By its name, which most servers take; else all there is.
        git(&["fetch", "--quiet", "--filter=blob:none", "origin", commit])
            .or_else(|_| git(&["fetch", "--quiet", "--filter=blob:none", "origin", "+refs/*:refs/*"]))
            .map_err(|e| format!("cannot reach {url}: {e}"))?;
    }
    git(&["show", &format!("{commit}:{path}")]).map_err(|_| format!("{url} has no {path} at {commit}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(repository: &str, directory: &str) -> Roots {
        let root = Root {
            prefix: String::new(),
            repository: repository.into(),
            commit: "abc123".into(),
            directory: directory.into(),
        };
        Roots { roots: vec![root], ..Roots::default() }
    }

    #[test]
    fn no_credential_is_kept() {
        assert_eq!(without_credentials("https://user:token@github.com/a/b.git"), "https://github.com/a/b.git");
        assert_eq!(without_credentials("https://token@gitlab.com/a/b"), "https://gitlab.com/a/b");
        assert_eq!(without_credentials("ssh://git:secret@host.com/a/b.git"), "ssh://git@host.com/a/b.git");
        assert_eq!(without_credentials("git@github.com:a/b.git"), "git@github.com:a/b.git");
        assert_eq!(without_credentials("https://github.com/a/b"), "https://github.com/a/b");
    }

    #[test]
    fn links_are_those_of_each_provider() {
        let link = |url: &str| roots(url, "services/api").link("src/main.to", 12);
        let expected = "https://github.com/a/b/blob/abc123/services/api/src/main.to#L12";
        assert_eq!(link("git@github.com:a/b.git").unwrap(), expected);
        assert_eq!(link("https://github.com/a/b.git").unwrap(), expected);
        assert_eq!(
            link("https://gitlab.com/g/s/b.git").unwrap(),
            "https://gitlab.com/g/s/b/-/blob/abc123/services/api/src/main.to#L12"
        );
        assert_eq!(
            link("git@bitbucket.org:a/b.git").unwrap(),
            "https://bitbucket.org/a/b/src/abc123/services/api/src/main.to#lines-12"
        );
        assert_eq!(
            link("https://codeberg.org/a/b").unwrap(),
            "https://codeberg.org/a/b/src/commit/abc123/services/api/src/main.to#L12"
        );
        let azure = "https://dev.azure.com/o/p/_git/b?path=/services/api/src/main.to&version=GCabc123&line=12";
        assert_eq!(link("git@ssh.dev.azure.com:v3/o/p/b").unwrap(), azure);
        assert_eq!(link("https://dev.azure.com/o/p/_git/b").unwrap(), azure);
        assert_eq!(link("https://git.acme.com/a/b.git"), None);
        assert_eq!(link("file:///repos/b"), None);
    }

    #[test]
    fn a_project_writes_how_its_own_host_links() {
        let mut roots = roots("https://git.acme.com/a/b.git", "");
        roots.template = Some("https://git.acme.com/{repo}/blob/{commit}/{path}#L{line}".into());
        let expected = "https://git.acme.com/a/b/blob/abc123/src/main.to#L3";
        assert_eq!(roots.link("src/main.to", 3).unwrap(), expected);
    }

    #[test]
    fn the_table_reads_as_it_was_written() {
        let mut written = roots("https://github.com/a/b", "api");
        written.roots.push(Root {
            prefix: "greet/".into(),
            repository: "https://github.com/c/greet".into(),
            commit: "def".into(),
            directory: String::new(),
        });
        written.files.insert("src/main.to".into(), "1234".into());
        assert_eq!(Roots::parse(&written.text()), written);
        // The file of a dependency is in its repository, from its root.
        let (root, path) = written.place("greet/src/greet.to").unwrap();
        assert_eq!((root.commit.as_str(), path.as_str()), ("def", "src/greet.to"));
        assert_eq!(written.place("src/main.to").unwrap().1, "api/src/main.to");
    }
}
