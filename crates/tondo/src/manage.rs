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

fn size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size(&e.path())).sum()
        }
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}
