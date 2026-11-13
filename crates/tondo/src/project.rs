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

/// A processor architecture Tondo builds for (language 6.8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::X64, Arch::Arm64];

    /// As a file's name, `tondo.toml` and a diagnostic write it: `x64`,
    /// `arm64`.
    pub fn name(self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
        }
    }

    pub fn parse(name: &str) -> Option<Arch> {
        Arch::ALL.into_iter().find(|arch| arch.name() == name)
    }

    /// The architecture this `tondo` runs on.
    pub fn host() -> Arch {
        if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X64 }
    }
}

/// What a build is for: a system and an architecture (language 6.8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Target {
    pub system: System,
    pub arch: Arch,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target { system: System::Linux, arch: Arch::X64 },
        Target { system: System::Linux, arch: Arch::Arm64 },
        Target { system: System::Macos, arch: Arch::X64 },
        Target { system: System::Macos, arch: Arch::Arm64 },
    ];

    /// The target this `tondo` runs on.
    pub fn host() -> Target {
        Target { system: System::host(), arch: Arch::host() }
    }

    /// As a diagnostic names it: `Linux arm64`.
    pub fn shown(self) -> String {
        format!("{} {}", self.system.shown(), self.arch.name())
    }
}

/// Some of the targets `all`, as a diagnostic names them: a system alone
/// when all of its architectures there are, an architecture alone when all
/// of its systems are, and the rest each by itself, as `Linux and macOS
/// arm64` or `arm64`.
pub fn shown_targets(targets: &[Target], all: &[Target]) -> String {
    let has = |t: &Target| targets.contains(t);
    let whole = |of: &dyn Fn(&Target) -> bool| {
        let mut them = all.iter().filter(|t| of(t)).peekable();
        them.peek().is_some() && them.all(has)
    };
    let mut parts = Vec::new();
    let mut named: Vec<Target> = Vec::new();
    for system in System::ALL {
        if whole(&|t: &Target| t.system == system) {
            parts.push(system.shown().to_string());
            named.extend(all.iter().filter(|t| t.system == system));
        }
    }
    // An architecture alone when no system named covers any of it.
    for arch in Arch::ALL {
        let of = |t: &Target| t.arch == arch;
        let untouched = all.iter().filter(|t| of(t)).all(|t| !named.contains(t));
        if all.iter().filter(|t| of(t)).count() > 1 && untouched && whole(&of) {
            parts.push(arch.name().to_string());
            named.extend(all.iter().filter(|t| of(t)));
        }
    }
    parts.extend(targets.iter().filter(|t| !named.contains(t)).map(|t| t.shown()));
    parts.join(" and ")
}

/// What the name of a file says it is for, after `.to` and a final `_test`:
/// its systems, `_linux`, `_macos` or `_unix`, and its architecture, `_x64`
/// or `_arm64`, the system first; each `None` when the name says none.
pub fn suffixes(path: &Path) -> (Option<&'static [System]>, Option<&'static [Arch]>) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".to")) else {
        return (None, None);
    };
    let name = name.strip_suffix("_test").unwrap_or(name);
    let (name, archs): (&str, Option<&'static [Arch]>) = if let Some(rest) = name.strip_suffix("_x64") {
        (rest, Some(&[Arch::X64]))
    } else if let Some(rest) = name.strip_suffix("_arm64") {
        (rest, Some(&[Arch::Arm64]))
    } else {
        (name, None)
    };
    let systems: Option<&'static [System]> = if name.ends_with("_linux") {
        Some(&[System::Linux])
    } else if name.ends_with("_macos") {
        Some(&[System::Macos])
    } else if name.ends_with("_unix") {
        Some(&System::ALL)
    } else {
        None
    };
    (systems, archs)
}

/// The targets a file is for, by its name: `watch_linux.to`,
/// `crc_arm64.to`, `poll_linux_x64.to`, and the same before `_test.to`;
/// none for a common file, which is for all.
pub fn targets_of(path: &Path) -> Option<Vec<Target>> {
    let (systems, archs) = suffixes(path);
    if systems.is_none() && archs.is_none() {
        return None;
    }
    let found = Target::ALL
        .into_iter()
        .filter(|t| systems.is_none_or(|s| s.contains(&t.system)) && archs.is_none_or(|a| a.contains(&t.arch)))
        .collect();
    Some(found)
}

/// The name to write for a file whose architecture comes before its system,
/// as `crc_arm64_linux.to`, which would otherwise be a file of Linux named
/// `crc_arm64`: `crc_linux_arm64.to`.
pub fn misordered(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.strip_suffix(".to")?;
    let (name, test) = match name.strip_suffix("_test") {
        Some(rest) => (rest, "_test"),
        None => (name, ""),
    };
    let system = ["_linux", "_macos", "_unix"].into_iter().find(|s| name.ends_with(s))?;
    let rest = &name[..name.len() - system.len()];
    let arch = ["_x64", "_arm64"].into_iter().find(|a| rest.ends_with(a))?;
    let stem = &rest[..rest.len() - arch.len()];
    Some(format!("{stem}{system}{arch}{test}.to"))
}

/// An error for the first file whose architecture comes before its system.
fn misordered_names(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if let Some(name) = misordered(path) {
            return Err(format!(
                "{}: the system goes before the architecture, as `{name}`: rename it, or it is a file of that system named for the architecture",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Whether a file belongs to a build for `target`, `None` for every one.
pub fn belongs(path: &Path, target: Option<Target>) -> bool {
    match (targets_of(path), target) {
        (Some(targets), Some(target)) => targets.contains(&target),
        _ => true,
    }
}

pub struct ProjectFile {
    pub path: PathBuf,
    /// The package name, then the directories below the source root.
    pub module: Vec<String>,
}

/// The sources of the package `name` in `dir`: the `.to` and `.proto` files
/// under `src/` or, without it, those next to `tondo.toml` or `main.to`,
/// those of another target than `target` left out (all with `None`). Test
/// files, hidden entries, symlinks, `target/` and `vendor/` are skipped. A
/// `.proto` file is a module of its own, named by its file. Also returns the
/// index of the root file: `main.to`, or else the first.
pub fn sources(dir: &Path, name: &str, target: Option<Target>) -> Result<(Vec<ProjectFile>, usize), String> {
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
    misordered_names(&paths)?;
    paths.retain(|path| belongs(path, target));
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
        if path.extension().is_some_and(|e| e == "proto") {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if !is_identifier(stem) {
                return Err(format!("{} cannot be a module: its name `{stem}` is not an identifier", path.display()));
            }
            module.push(stem.to_string());
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
    target: Option<Target>,
) -> Result<(Vec<ProjectFile>, Vec<ProjectFile>), String> {
    let source = dir.join("src");
    let (root_dir, recursive) = if source.is_dir() { (source, true) } else { (dir.to_path_buf(), false) };
    let mut paths = Vec::new();
    walk_tests(&root_dir, recursive, &mut paths)?;
    misordered_names(&paths)?;
    paths.retain(|path| belongs(path, target));
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
    misordered_names(&paths)?;
    paths.retain(|path| belongs(path, target));
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
        } else if (name.ends_with(".to") && !name.ends_with("_test.to")) || name.ends_with(".proto") {
            out.push(entry.path());
        }
    }
    Ok(())
}

pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(unicode_ident::is_xid_start) && chars.all(unicode_ident::is_xid_continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(system: System, arch: Arch) -> Target {
        Target { system, arch }
    }

    #[test]
    fn a_name_says_its_system_and_architecture() {
        let of = |name: &str| suffixes(Path::new(name));
        assert_eq!(of("watch_linux.to"), (Some(&[System::Linux][..]), None));
        assert_eq!(of("crc_arm64.to"), (None, Some(&[Arch::Arm64][..])));
        assert_eq!(of("poll_unix_x64_test.to"), (Some(&System::ALL[..]), Some(&[Arch::X64][..])));
        assert_eq!(of("linux.to"), (None, None));
        let poll = targets_of(Path::new("poll_macos_arm64.to")).unwrap();
        assert_eq!(poll, [target(System::Macos, Arch::Arm64)]);
        assert!(belongs(Path::new("crc_arm64.to"), Some(target(System::Linux, Arch::Arm64))));
        assert!(!belongs(Path::new("crc_arm64.to"), Some(target(System::Macos, Arch::X64))));
        assert_eq!(misordered(Path::new("crc_arm64_linux_test.to")).as_deref(), Some("crc_linux_arm64_test.to"));
        assert_eq!(misordered(Path::new("crc_linux_arm64.to")), None);
    }

    #[test]
    fn targets_are_named_by_what_they_cover() {
        let all = Target::ALL;
        let (lx, la, mx, ma) = (all[0], all[1], all[2], all[3]);
        assert_eq!(shown_targets(&[mx, ma], &all), "macOS");
        assert_eq!(shown_targets(&[la, ma], &all), "arm64");
        assert_eq!(shown_targets(&[la], &all), "Linux arm64");
        assert_eq!(shown_targets(&[lx, la, ma], &all), "Linux and macOS arm64");
        // With one system checked, its architecture's target is named whole.
        assert_eq!(shown_targets(&[la], &[lx, la]), "Linux arm64");
    }
}
