//! Conventional projects: which files form a package and which module each
//! belongs to.

use std::path::{Path, PathBuf};

/// A system Tondo builds for (language 6.8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum System {
    Linux,
    Macos,
}

impl System {
    pub const ALL: [System; 2] = [System::Linux, System::Macos];

    /// As a file's name and `tondo.toml` write it: `linux`, `macos`.
    pub fn name(self) -> &'static str {
        match self {
            System::Linux => "linux",
            System::Macos => "macos",
        }
    }

    /// As a person reads it: `Linux`, `macOS`.
    pub fn shown(self) -> &'static str {
        match self {
            System::Linux => "Linux",
            System::Macos => "macOS",
        }
    }

    pub fn parse(name: &str) -> Option<System> {
        System::ALL.into_iter().find(|system| system.name() == name)
    }

    /// The system this `tondo` runs on.
    pub fn host() -> System {
        if cfg!(target_os = "macos") { System::Macos } else { System::Linux }
    }
}

/// The systems a file is for, by its name: `watch_linux.to`,
/// `watch_macos.to`, `signals_unix.to`, and the same before `_test.to`;
/// none for a common file, which is for all.
pub fn systems_of(path: &Path) -> Option<&'static [System]> {
    let name = path.file_name()?.to_str()?.strip_suffix(".to")?;
    let name = name.strip_suffix("_test").unwrap_or(name);
    if name.ends_with("_linux") {
        Some(&[System::Linux])
    } else if name.ends_with("_macos") {
        Some(&[System::Macos])
    } else if name.ends_with("_unix") {
        Some(&System::ALL)
    } else {
        None
    }
}

/// Whether a file belongs to a build for `system`, `None` for every one.
pub fn belongs(path: &Path, system: Option<System>) -> bool {
    match (systems_of(path), system) {
        (Some(systems), Some(system)) => systems.contains(&system),
        _ => true,
    }
}

pub struct ProjectFile {
    pub path: PathBuf,
    /// The package name, then the directories below the source root.
    pub module: Vec<String>,
}

/// The sources of the package `name` in `dir`: the `.to` files under
/// `src/` or, without it, those next to `tondo.toml` or `main.to`, those
/// of another system than `system` left out (all with `None`). Test files,
/// hidden entries, symlinks, `target/` and `vendor/` are skipped. Also
/// returns the index of the root file: `main.to`, or else the first.
pub fn sources(dir: &Path, name: &str, system: Option<System>) -> Result<(Vec<ProjectFile>, usize), String> {
    let source = dir.join("src");
    let mut paths = Vec::new();
    let root_dir = if source.is_dir() {
        walk(&source, true, &mut paths)?;
        source
    } else if dir.join("tondo.toml").is_file() || dir.join("main.to").is_file() {
        walk(dir, false, &mut paths)?;
        dir.to_path_buf()
    } else {
        return Err(format!("{} is not a Tondo project: add src/main.to or main.to", dir.display()));
    };
    paths.retain(|path| belongs(path, system));
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{} has no source files", root_dir.display()));
    }
    let mut files = Vec::new();
    for path in paths {
        let relative = path.parent().and_then(|p| p.strip_prefix(&root_dir).ok()).unwrap_or(Path::new(""));
        let mut module = vec![name.to_string()];
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
    Ok((files, root))
}

/// The test files of the package `name` in `dir` (testing 5.2): the unit
/// test companions (`*_test.to` beside the sources), with the module each
/// joins, and the integration roots under `tests/`, each with its path
/// there as a module path; those of another system than `system` left out.
pub fn test_sources(
    dir: &Path,
    name: &str,
    system: Option<System>,
) -> Result<(Vec<ProjectFile>, Vec<ProjectFile>), String> {
    let source = dir.join("src");
    let (root_dir, recursive) = if source.is_dir() { (source, true) } else { (dir.to_path_buf(), false) };
    let mut paths = Vec::new();
    walk_tests(&root_dir, recursive, &mut paths)?;
    paths.retain(|path| belongs(path, system));
    paths.sort();
    let mut companions = Vec::new();
    for path in paths {
        let relative = path.parent().and_then(|p| p.strip_prefix(&root_dir).ok()).unwrap_or(Path::new(""));
        let mut module = vec![name.to_string()];
        module.extend(relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
        companions.push(ProjectFile { path, module });
    }
    let tests = dir.join("tests");
    let mut paths = Vec::new();
    if tests.is_dir() {
        walk(&tests, true, &mut paths)?;
        walk_tests(&tests, true, &mut paths)?;
    }
    paths.retain(|path| belongs(path, system));
    paths.sort();
    let mut roots = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(&tests).unwrap_or(&path).with_extension("");
        let mut module = Vec::new();
        for part in relative.components() {
            let part = part.as_os_str().to_str().unwrap_or_default();
            if !is_identifier(part) {
                return Err(format!(
                    "the test file {} cannot be a module: `{part}` is not an identifier",
                    path.display()
                ));
            }
            module.push(part.to_string());
        }
        roots.push(ProjectFile { path, module });
    }
    Ok((companions, roots))
}

/// The `*_test.to` files under `dir`.
fn walk_tests(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
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
                walk_tests(&entry.path(), true, out)?;
            }
        } else if name.ends_with("_test.to") {
            out.push(entry.path());
        }
    }
    Ok(())
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

pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(unicode_ident::is_xid_start) && chars.all(unicode_ident::is_xid_continue)
}
