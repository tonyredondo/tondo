//! Managing projects from the command line (toolchain 7.14): `tondo new`
//! and `init` make one, `add` and `remove` change its dependencies, and
//! `clean` empties the cache of builds.

use std::path::{Path, PathBuf};

/// The files of a new project, its name in them.
fn files(name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("tondo.toml", format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")),
        (
            "src/main.to",
            "import std.console\n\nfn main() {\n    console.println(greeting(\"world\"))\n}\n\n/// What the program says to someone.\nfn greeting(name: String): String {\n    \"Hello, {name}!\"\n}\n"
                .into(),
        ),
        ("src/main_test.to", "test greetsByName {\n    assert(greeting(\"Ada\") == \"Hello, Ada!\")\n}\n".into()),
        (".gitignore", "target/\n".into()),
    ]
}

/// Makes a project in `dir`, named `name`, which must be an identifier:
/// `tondo new` in a new directory, `tondo init` in one that is not a
/// project yet. Gives the files it wrote.
pub fn create(dir: &Path, name: &str, new: bool) -> Result<Vec<PathBuf>, String> {
    if !crate::project::is_identifier(name) || crate::lexer::Kw::lookup(name).is_some() {
        return Err(format!("`{name}` cannot name a package: a package's name is an identifier, as `my_app`"));
    }
    if new && dir.read_dir().is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(format!("{} is not empty: `tondo init` makes a project of an existing directory", dir.display()));
    }
    if ["tondo.toml", "src/main.to", "main.to"].iter().any(|f| dir.join(f).exists()) {
        return Err(format!("{} is a project already", dir.display()));
    }
    let mut written = Vec::new();
    for (file, text) in files(name) {
        let path = dir.join(file);
        // A file of the same name that is there stays.
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// Where a dependency comes from, as `tondo add` takes it.
pub enum Source {
    Path(String),
    Git { url: String, reference: Option<(&'static str, String)> },
    Archive { url: String, sha256: Option<String> },
}

impl Source {
    fn entry(&self) -> String {
        let quote = crate::toml::quote;
        match self {
            Source::Path(path) => format!("{{ path = {} }}", quote(path)),
            Source::Git { url, reference: None } => format!("{{ git = {} }}", quote(url)),
            Source::Git { url, reference: Some((kind, value)) } => {
                format!("{{ git = {}, {kind} = {} }}", quote(url), quote(value))
            }
            Source::Archive { url, sha256: None } => format!("{{ url = {} }}", quote(url)),
            Source::Archive { url, sha256: Some(sha256) } => {
                format!("{{ url = {}, sha256 = {} }}", quote(url), quote(sha256))
            }
        }
    }
}

/// Adds a dependency to the project's manifest, under `[dependencies]` or
/// `[dev-dependencies]`, and fetches it; the manifest stays as it was when
/// it cannot be fetched.
pub fn add(project: &Path, alias: &str, source: &Source, dev: bool) -> Result<(), String> {
    if !crate::project::is_identifier(alias) {
        return Err(format!("`{alias}` cannot name a dependency: an alias is an identifier, as `http`"));
    }
    if !project.is_dir() {
        return add_to_script(project, alias, source, dev);
    }
    let path = project.join("tondo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let tables = crate::toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let table = if dev { "dev-dependencies" } else { "dependencies" };
    for t in tables.iter().filter(|t| t.name == "dependencies" || t.name == "dev-dependencies") {
        if t.entries.iter().any(|(key, ..)| key == alias) {
            return Err(format!(
                "the project depends on `{alias}` already, in [{}]: `tondo remove {alias}` first",
                t.name
            ));
        }
    }
    let line = format!("{alias} = {}", source.entry());
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match tables.iter().find(|t| t.name == table) {
        Some(found) => {
            // After its last entry, or its header.
            let after = found.entries.iter().map(|(_, _, line)| *line).max();
            let header = lines.iter().position(|l| l.trim() == format!("[{table}]")).map(|i| i + 1);
            let at = after.or(header).unwrap_or(lines.len());
            lines.insert(at, line);
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{table}]"));
            lines.push(line);
        }
    }
    std::fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    if let Err(message) = crate::deps::resolve(project, None) {
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        return Err(format!("{message}; tondo.toml is as it was"));
    }
    Ok(())
}

/// Takes a dependency out of the project's manifest, and out of
/// `tondo.lock`.
pub fn remove(project: &Path, alias: &str) -> Result<(), String> {
    if !project.is_dir() {
        return remove_from_script(project, alias);
    }
    let path = project.join("tondo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let tables = crate::toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let found = tables
        .iter()
        .filter(|t| t.name == "dependencies" || t.name == "dev-dependencies")
        .find_map(|t| t.entries.iter().find(|(key, ..)| key == alias).map(|(_, _, line)| *line));
    let Some(line) = found else {
        return Err(format!(
            "the project does not depend on `{alias}`: `tondo graph --packages` shows what it depends on"
        ));
    };
    let mut lines: Vec<&str> = text.lines().collect();
    lines.remove(line - 1);
    std::fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    crate::deps::resolve(project, None).map(|_| ())
}

/// What a script that needs more than its dependencies is (toolchain 2).
pub const SCRIPT_AS_PROJECT: &str = "a project, which `tondo new <name>` makes, with this script as its src/main.to and its dependencies in its tondo.toml";

/// The lines of a script's header that declare a dependency, by index, with
/// their alias.
fn script_dependencies(lines: &[String]) -> Vec<(usize, String)> {
    let header = crate::deps::header_lines(&lines.join("\n"));
    let alias = |rest: &str| rest.split(['=', ' ']).next().unwrap_or_default().to_string();
    lines[..header.min(lines.len())]
        .iter()
        .enumerate()
        .filter_map(|(index, line)| match crate::deps::directive(line) {
            Some(("dependency", rest)) => Some((index, alias(rest))),
            _ => None,
        })
        .collect()
}

/// Adds a dependency to a script's header (toolchain 2), under the others,
/// or at its top, under the shebang, with a blank line after it as the
/// formatter writes it; the script stays as it was when it cannot be fetched.
fn add_to_script(script: &Path, alias: &str, source: &Source, dev: bool) -> Result<(), String> {
    if dev {
        return Err(format!("a script has no dev-dependencies: one whose tests need them is {SCRIPT_AS_PROJECT}"));
    }
    let text = std::fs::read_to_string(script).map_err(|e| format!("cannot read {}: {e}", script.display()))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let declared = script_dependencies(&lines);
    if let Some((index, _)) = declared.iter().find(|(_, a)| a == alias) {
        return Err(format!(
            "the script depends on `{alias}` already, on line {}: `tondo remove {alias} --project {}` first",
            index + 1,
            script.display()
        ));
    }
    let line = format!("// tondo: dependency {alias} = {}", source.entry());
    match declared.last() {
        Some((index, _)) => lines.insert(index + 1, line),
        None => {
            let at = usize::from(lines.first().is_some_and(|l| l.starts_with("#!")));
            while lines.get(at).is_some_and(|l| l.trim().is_empty()) {
                lines.remove(at);
            }
            let rest = at < lines.len();
            lines
                .splice(at..at, [(at > 0).then(String::new), Some(line), rest.then(String::new)].into_iter().flatten());
        }
    }
    std::fs::write(script, lines.join("\n") + "\n").map_err(|e| format!("cannot write {}: {e}", script.display()))?;
    if let Err(message) = crate::deps::resolve(script, None) {
        std::fs::write(script, text).map_err(|e| format!("cannot write {}: {e}", script.display()))?;
        return Err(format!("{message}; {} is as it was", script.display()));
    }
    Ok(())
}

/// Takes a dependency out of a script's header, and out of its lock.
fn remove_from_script(script: &Path, alias: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(script).map_err(|e| format!("cannot read {}: {e}", script.display()))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let Some((index, _)) = script_dependencies(&lines).into_iter().find(|(_, a)| a == alias) else {
        return Err(format!(
            "the script does not depend on `{alias}`: its `// tondo: dependency` lines say what it does"
        ));
    };
    lines.remove(index);
    // The blank line after the last one goes with it.
    if lines.get(index).is_some_and(|l| l.trim().is_empty()) && (index == 0 || lines[index - 1].trim().is_empty()) {
        lines.remove(index);
    }
    std::fs::write(script, lines.join("\n") + "\n").map_err(|e| format!("cannot write {}: {e}", script.display()))?;
    crate::deps::resolve(script, None).map(|_| ())
}

/// Empties the cache of builds, and with `all` the dependencies fetched
/// too: what it took, in bytes.
pub fn clean(all: bool) -> Result<u64, String> {
    let cache = crate::driver::cache_dir();
    let mut freed = 0;
    let Ok(entries) = std::fs::read_dir(&cache) else { return Ok(0) };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !all && (name == "deps" || name == "git") {
            continue;
        }
        let path = entry.path();
        freed += size(&path);
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    }
    Ok(freed)
}

/// What each part of the cache holds, without emptying it: its name,
/// bytes, what it is, whether it counts toward the limit on the builds, and
/// whether `clean` empties it without `--all`; the largest first.
pub fn listed() -> serde_json::Value {
    let cache = crate::driver::cache_dir();
    let what = |name: &str| match name {
        "run" => "builds of programs",
        "test" => "builds of tests",
        "meta" => "builds of what derives and generators run",
        "runtime" => "the runtime, built once for each toolchain",
        "compiler" | "compiler-target" => "the tondo of a revision, for --compiler",
        "revision" => "the code of a revision, for --against",
        "deps" => "fetched dependencies",
        "git" => "the repositories of dependencies",
        "dumps" => "dumps of programs that failed",
        "live" => "live builds",
        "plugins" => "built plugins",
        "sources" => "files of builds, from their repositories",
        name if name.starts_with("std-") => "the standard library's sources",
        _ => "",
    };
    let mut parts: Vec<(String, u64)> = std::fs::read_dir(&cache)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), size(&entry.path())))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    parts.sort_by_key(|(name, bytes)| (std::cmp::Reverse(*bytes), name.clone()));
    let counted = |name: &str| matches!(name, "run" | "test" | "meta");
    let total: u64 = parts.iter().map(|(_, bytes)| bytes).sum();
    let builds: u64 = parts.iter().filter(|(name, _)| counted(name)).map(|(_, bytes)| bytes).sum();
    let parts: Vec<serde_json::Value> = parts
        .iter()
        .map(|(name, bytes)| {
            serde_json::json!({
                "part": name, "bytes": bytes, "what": what(name), "counted": counted(name),
                "cleaned": !matches!(name.as_str(), "deps" | "git"),
            })
        })
        .collect();
    serde_json::json!({
        "dir": cache, "total": total, "parts": parts, "builds": builds, "limit": crate::driver::cache_limit(),
    })
}

/// What `listed` says, for a person.
pub fn listed_text(listed: &serde_json::Value) -> String {
    let bytes = |value: &serde_json::Value| crate::size::bytes(value.as_u64().unwrap_or(0));
    let dir = listed["dir"].as_str().unwrap_or_default();
    let mut out = format!("the cache in {dir} holds {}\n", bytes(&listed["total"]));
    for part in listed["parts"].as_array().into_iter().flatten() {
        let name = part["part"].as_str().unwrap_or_default();
        let what = part["what"].as_str().unwrap_or_default();
        let note = match (part["counted"] == true, part["cleaned"] == true) {
            (true, _) => ", toward the limit",
            (false, false) => ", emptied only with --all",
            _ => "",
        };
        let said = format!("{what}{note}");
        out.push_str(&format!("{:>11}  {name:<16}{}\n", bytes(&part["bytes"]), said.trim_start_matches(", ")));
    }
    out.push_str(&format!(
        "the builds hold {} of a limit of {} (TONDO_CACHE_LIMIT): once a day the least used past it go\n",
        bytes(&listed["builds"]),
        bytes(&listed["limit"])
    ));
    out
}

fn size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size(&e.path())).sum()
        }
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}
