//! Conventional projects (toolchain 3): which files form the program and
//! which module each belongs to.

use std::path::{Path, PathBuf};

pub struct Project {
    /// The package name, the first segment of its module paths.
    pub name: String,
    /// Production sources ordered by path, with their module paths.
    pub files: Vec<ProjectFile>,
    /// The index in `files` of the root file, which holds `main` or the
    /// script.
    pub root: usize,
}

pub struct ProjectFile {
    pub path: PathBuf,
    /// The package name, then the directories below the source root.
    pub module: Vec<String>,
}

/// Finds the sources of the project in `dir`: the `.to` files under `src/`
/// or, without it, those next to `tondo.toml` or `main.to`. Test files,
/// hidden entries, symlinks, `target/` and `vendor/` are skipped.
pub fn discover(dir: &Path) -> Result<Project, String> {
    let manifest = dir.join("tondo.toml");
    let declared = if manifest.is_file() {
        let text =
            std::fs::read_to_string(&manifest).map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
        package_name(&text).map_err(|e| format!("{}: {e}", manifest.display()))?
    } else {
        None
    };
    let name = match declared {
        Some(name) => name,
        None => {
            let base = std::fs::canonicalize(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
            let name = base.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !is_identifier(&name) {
                return Err(format!(
                    "the directory name `{name}` is not a valid package name; declare one in tondo.toml: [package] name = \"app\""
                ));
            }
            name
        }
    };
    let source = dir.join("src");
    let mut paths = Vec::new();
    let root_dir = if source.is_dir() {
        walk(&source, true, &mut paths)?;
        source
    } else if manifest.is_file() || dir.join("main.to").is_file() {
        walk(dir, false, &mut paths)?;
        dir.to_path_buf()
    } else {
        return Err(format!("{} is not a Tondo project: add src/main.to or main.to", dir.display()));
    };
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{} has no source files", root_dir.display()));
    }
    let mut files = Vec::new();
    for path in paths {
        let relative = path.parent().and_then(|p| p.strip_prefix(&root_dir).ok()).unwrap_or(Path::new(""));
        let mut module = vec![name.clone()];
        for part in relative.components() {
            let part = part.as_os_str().to_str().unwrap_or_default();
            if !is_identifier(part) {
                return Err(format!(
                    "the directory `{}` cannot be a module: its name `{part}` is not an identifier",
                    path.parent().unwrap().display()
                ));
            }
            module.push(part.to_string());
        }
        files.push(ProjectFile { path, module });
    }
    let main = root_dir.join("main.to");
    let root = files.iter().position(|f| f.path == main).unwrap_or(0);
    Ok(Project { name, files, root })
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if name.starts_with('.') || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if recursive && name != "target" && name != "vendor" {
                walk(&entry.path(), true, out)?;
            }
        } else if name.ends_with(".to") && !name.ends_with("_test.to") {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(unicode_ident::is_xid_start) && chars.all(unicode_ident::is_xid_continue)
}

/// The package name `tondo.toml` declares, if any. Only `[package]` with
/// `name` and `edition = "0.1"` is understood so far.
fn package_name(text: &str) -> Result<Option<String>, String> {
    let mut table = String::new();
    let mut name = None;
    for (number, line) in text.lines().enumerate() {
        let line = line.split_once('#').map_or(line, |(code, _)| code).trim();
        if line.is_empty() {
            continue;
        }
        let at = |message: String| format!("line {}: {message}", number + 1);
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = header.trim().to_string();
            if table != "package" {
                return Err(at(format!("`[{table}]` is not supported yet; only `[package]` is")));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(at("expected `key = \"value\"`".into()));
        };
        let (key, value) = (key.trim(), value.trim());
        let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
            return Err(at(format!("the value of `{key}` must be a string")));
        };
        match (table.as_str(), key) {
            ("package", "name") if is_identifier(value) => name = Some(value.to_string()),
            ("package", "name") => return Err(at(format!("`{value}` is not a valid package name"))),
            ("package", "edition") if value == "0.1" => {}
            ("package", "edition") => return Err(at(format!("edition `{value}` is not supported; use \"0.1\""))),
            ("package", _) => return Err(at(format!("unknown key `{key}` in `[package]`"))),
            _ => return Err(at(format!("`{key}` must be inside `[package]`"))),
        }
    }
    Ok(name)
}
