//! Compilation pipeline, native linking and the build cache.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use crate::target::Target;
use crate::{ast, check, codegen, deps, hir, lower, modules, mono, parser, project, stdlib};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
// The archives the compiler carries, compressed; each is written to the
// cache the first time a program links.
const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libtondo_runtime.a.gz"));
const RUNTIME_LINK_ARGS: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-link-args.txt"));
const RUNTIME_HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-hash.txt"));
/// The runtime's source and the compiler's version, which a target's pack
/// must agree with.
const RUNTIME_IDENTITY: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-identity.txt"));
/// Rust's libunwind for musl targets, which the runtime needs there; empty
/// elsewhere.
const UNWIND: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libunwind.a.gz"));
/// What a static executable links on Linux (`--static`): the runtime built
/// for musl, musl's C library, its unwinder and the start file that
/// relocates the executable; empty elsewhere.
const STATIC: [(&str, &[u8]); 4] = [
    ("libtondo_runtime-static", include_bytes!(concat!(env!("OUT_DIR"), "/libtondo_runtime-static.a.gz"))),
    ("libunwind-static", include_bytes!(concat!(env!("OUT_DIR"), "/libunwind-static.a.gz"))),
    ("libc-static", include_bytes!(concat!(env!("OUT_DIR"), "/libc-static.a.gz"))),
    ("rcrt1-static", include_bytes!(concat!(env!("OUT_DIR"), "/rcrt1-static.o.gz"))),
];

/// The archive `name` of the compiler, in the cache. Using it marks it,
/// so that another compiler, of another version, does not take it away
/// from a build that links with it.
fn carried(name: &str, compressed: &[u8]) -> Result<PathBuf, String> {
    let dir = cache_dir().join("runtime");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}-{RUNTIME_HASH}.a"));
    let now = std::time::SystemTime::now();
    let used = std::fs::File::open(&path).and_then(|file| file.set_modified(now));
    if used.is_err() {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(compressed), &mut bytes)
            .map_err(|e| format!("the compiler's {name} is damaged: {e}"))?;
        write_atomic(&path, &bytes)?;
        forget_others(&dir, name, RUNTIME_HASH, now);
    }
    Ok(path)
}

/// Removes the archives `name` of other compilers that no build used in a
/// day: one that another version, as an editor's, still uses stays.
fn forget_others(dir: &Path, name: &str, own: &str, now: std::time::SystemTime) {
    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = entry.file_name();
        let hash = file.to_str().and_then(|f| f.strip_prefix(name)?.strip_prefix('-')?.strip_suffix(".a"));
        let other = hash.is_some_and(|h| h.len() == 16 && h != own && h.bytes().all(|b| b.is_ascii_hexdigit()));
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        let unused = modified.is_some_and(|modified| now.duration_since(modified).unwrap_or_default() > DAY);
        if other && unused {
            let _ = std::fs::remove_file(entry.path());
            // With the index of its symbols the linker keeps beside it.
            let mut index = entry.path().into_os_string();
            index.push(".index");
            let _ = std::fs::remove_file(index);
        }
    }
}

/// The cache entry `key` of `kind` (`run`, `test` or `meta`), which holds
/// what a build can make again: using it marks it, and an entry no one
/// used in 30 days is removed, looking at most once a day.
/// The entry `key` of the builds of one program, as `run` and `test` keep
/// them, `owner` naming the program and how it is built: a new one lets go
/// of its older builds but the last, so that editing a program does not
/// fill the cache with every version of it.
pub fn build_entry(kind: &str, owner: &[&[u8]], key: &str) -> PathBuf {
    const KEPT: usize = 1;
    let prefix = format!("{}-", fingerprint(owner));
    let entry = cache_entry(kind, &format!("{prefix}{key}"));
    if !entry.exists() {
        let mut older: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(cache_dir().join(kind))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|name| name.starts_with(&prefix)))
            .filter_map(|e| Some((e.metadata().and_then(|m| m.modified()).ok()?, e.path())))
            .collect();
        older.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, path) in older.into_iter().skip(KEPT) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
    entry
}

pub fn cache_entry(kind: &str, key: &str) -> PathBuf {
    let dir = cache_dir().join(kind);
    let entry = dir.join(key);
    let now = std::time::SystemTime::now();
    if let Ok(file) = std::fs::File::open(&entry) {
        let _ = file.set_modified(now);
    }
    sweep(&dir, now);
    entry
}

fn sweep(dir: &Path, now: std::time::SystemTime) {
    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    // A time after `now`, as one this sweep just wrote, is no age.
    let age = |path: &Path| {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        Some(now.duration_since(modified).unwrap_or_default())
    };
    let stamp = dir.join(".swept");
    if age(&stamp).is_some_and(|age| age < DAY) || std::fs::write(&stamp, b"").is_err() {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() && age(&path).is_some_and(|age| age > 30 * DAY) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

pub struct Analysis {
    pub sources: SourceMap,
    pub diags: Diagnostics,
    pub program: Option<hir::Program>,
    /// Who owns the diagnostics (22.3): the package or the single file.
    pub source_id: String,
    /// The native libraries the packages link (`[native] libraries`).
    pub libraries: Vec<String>,
    /// A provider or generator, built for the `meta` profile (27.7).
    pub meta: bool,
    /// What each `derive` expanded to, for `tondo expand`.
    pub expansions: Vec<crate::meta::Expansion>,
    /// For an editor: each declaration and each name that refers to one.
    pub index: Option<crate::index::Index>,
    /// Where the program comes from, which its executable keeps; none with
    /// `--no-source-links`.
    pub project: Option<crate::roots::Project>,
}

thread_local! {
    /// The text of the files an editor has open, which may not be saved.
    static OPEN: std::cell::RefCell<std::collections::HashMap<PathBuf, String>> = Default::default();
    /// A function compiled though nothing calls it: a debugger's call.
    static ROOT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// The system a build is for, when it is not this machine's
    /// (language 6.8).
    static SYSTEM: std::cell::Cell<Option<project::System>> = const { std::cell::Cell::new(None) };
}

/// The system the builds of this thread are for: this machine's, or the
/// one `for_system` sets.
pub fn system() -> project::System {
    SYSTEM.get().unwrap_or_else(project::System::host)
}

/// Makes the builds of this thread for `system`, as `--target` does.
pub fn set_system(system: project::System) {
    SYSTEM.set(Some(system));
}

/// Runs `f` with the files `open` names read as it says, as an editor's
/// unsaved text, and the function `root` compiled though nothing calls it:
/// a debugger's `call` adds its function to a file so.
pub fn with_open<T>(open: std::collections::HashMap<PathBuf, String>, root: &str, f: impl FnOnce() -> T) -> T {
    let open = open.into_iter().map(|(path, text)| (path.canonicalize().unwrap_or(path), text)).collect();
    OPEN.with(|o| *o.borrow_mut() = open);
    ROOT.with(|r| *r.borrow_mut() = Some(root.to_string()));
    let out = f();
    OPEN.with(|o| o.borrow_mut().clear());
    ROOT.with(|r| *r.borrow_mut() = None);
    out
}

/// Runs `f` with builds for `system`, as `tondo check` does for each.
pub fn for_system<T>(system: project::System, f: impl FnOnce() -> T) -> T {
    let before = SYSTEM.replace(Some(system));
    let out = f();
    SYSTEM.set(before);
    out
}

/// Reads, parses and checks a program as an editor shows it: with the text
/// of the files it has open, and checked as far as it can be with errors,
/// to know what each name refers to. A test file is checked with its
/// project's test build.
pub fn analyze_for_editor(
    path: &Path,
    tests: bool,
    open: std::collections::HashMap<PathBuf, String>,
) -> Result<Analysis, String> {
    OPEN.with(|o| *o.borrow_mut() = open);
    let analysis = analyze_with(path, tests, false, true, Root::Checked);
    OPEN.with(|o| o.borrow_mut().clear());
    analysis
}

/// Reads, parses and checks a program: a project directory (toolchain 3)
/// or a single source file.
pub fn analyze(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, false, false, Root::Program)
}

/// Reads, parses and checks a program or a package as `tondo check` does:
/// without asking for `main`, which only what runs needs. A package with
/// files of a system is checked for each system it runs on, and an error
/// of only one says which (language 6.8).
pub fn check(path: &Path) -> Result<Analysis, String> {
    let systems = checked_systems(path)?;
    let carried = Carried::here();
    // The other systems' analyses and the tests' run beside this machine's,
    // each on a thread of its own as this one is set, and are merged in the
    // same order as one after another (docs/speed.md, B5).
    std::thread::scope(|scope| {
        let others: Vec<_> = systems[1..]
            .iter()
            .map(|&other| {
                beside(scope, &carried, move || {
                    for_system(other, || analyze_with(path, false, false, false, Root::Checked))
                })
            })
            .collect();
        let tests =
            path.is_dir().then(|| beside(scope, &carried, || analyze_with(path, true, false, false, Root::Checked)));
        let mut analysis = for_system(systems[0], || analyze_with(path, false, false, false, Root::Checked))?;
        for (handle, &other) in others.into_iter().zip(&systems[1..]) {
            let more = joined(handle)?;
            merge_system(&mut analysis, &more, other);
        }
        // A project's tests too, once its code is right, as an editor shows
        // them: what they alone say, in their own files.
        if let Some(handle) = tests {
            let tests = joined(handle);
            if !analysis.diags.has_errors() {
                let tests = tests?;
                // And of the code, what only the tests show: a mark in it of a
                // step no model in them has (`W1020`).
                merge(&mut analysis, &tests, None, |diagnostic, file| {
                    file.logical.ends_with("_test.to")
                        || Path::new(&file.logical).starts_with("tests")
                        || diagnostic.code == Some("W1020")
                });
            }
        }
        // What fails on one system does not build.
        if analysis.diags.has_errors() {
            analysis.program = None;
        }
        Ok(analysis)
    })
}

/// The stack of a thread that analyzes a program, as the main thread's: a
/// deeply nested program checks on it as on that one.
const ANALYSIS_STACK: usize = 8 << 20;

/// What the builds of a thread are set to, given to another.
#[derive(Clone)]
struct Carried {
    open: std::collections::HashMap<PathBuf, String>,
    root: Option<String>,
    system: Option<project::System>,
}

impl Carried {
    fn here() -> Carried {
        Carried {
            open: OPEN.with(|o| o.borrow().clone()),
            root: ROOT.with(|r| r.borrow().clone()),
            system: SYSTEM.get(),
        }
    }

    /// Runs `f` on this thread, a new one, set as the one it came from.
    fn set<T>(self, f: impl FnOnce() -> T) -> T {
        OPEN.with(|o| *o.borrow_mut() = self.open);
        ROOT.with(|r| *r.borrow_mut() = self.root);
        SYSTEM.set(self.system);
        f()
    }
}

/// Runs the analysis `f` on a thread of its own in `scope`, set as `carried`.
fn beside<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    carried: &Carried,
    f: impl FnOnce() -> Result<Analysis, String> + Send + 'scope,
) -> std::thread::ScopedJoinHandle<'scope, Result<Analysis, String>> {
    let carried = carried.clone();
    std::thread::Builder::new()
        .stack_size(ANALYSIS_STACK)
        .spawn_scoped(scope, move || carried.set(f))
        .expect("a thread for an analysis")
}

/// What the thread of an analysis gave, or its panic, passed on.
fn joined(handle: std::thread::ScopedJoinHandle<'_, Result<Analysis, String>>) -> Result<Analysis, String> {
    handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The systems `tondo check` checks a project for: this machine's, first,
/// and those of the others the package runs on, when it has files of a
/// system; or only the first it runs on.
pub fn checked_systems(path: &Path) -> Result<Vec<project::System>, String> {
    let here = system();
    if !path.is_dir() {
        return Ok(vec![here]);
    }
    let packages = deps::resolve(path, None)?;
    let runs = |system: &project::System| packages[0].systems.is_empty() || packages[0].systems.contains(system);
    let mut systems: Vec<project::System> = project::System::ALL.into_iter().filter(runs).collect();
    systems.sort_by_key(|system| *system != here);
    let mut by_system = false;
    for package in &packages {
        let (files, _) = project::sources(&package.dir, &package.name, None)?;
        by_system |= files.iter().any(|file| project::systems_of(&file.path).is_some());
    }
    if !by_system {
        systems.truncate(1);
    }
    Ok(systems)
}

/// Adds to an analysis the diagnostics of another system's that it does
/// not have, each saying the system, with the files they are in.
pub fn merge_system(into: &mut Analysis, from: &Analysis, system: project::System) {
    merge(into, from, Some(system), |_, _| true);
}

/// Adds to an analysis the diagnostics of another that it does not have,
/// of the files `wanted` takes, with those files; of another system's, each
/// says which.
fn merge(
    into: &mut Analysis,
    from: &Analysis,
    system: Option<project::System>,
    wanted: impl Fn(&crate::diag::Diagnostic, &crate::source::SourceFile) -> bool,
) {
    let mut moved = |span: Span| -> Span {
        let file = from.sources.file_of(span.start);
        let found = (0..into.sources.len()).find(|&i| {
            let other = into.sources.file(i);
            other.path == file.path && other.logical == file.logical
        });
        let index = found.unwrap_or_else(|| {
            let index = into.sources.add(&file.path, file.text.clone());
            into.sources.set_identity(index, file.logical.clone(), file.module.clone());
            if file.own {
                into.sources.set_own(index);
            }
            index
        });
        let base = into.sources.file(index).base;
        Span::new(span.start - file.base + base, span.end - file.base + base)
    };
    let mut added = Vec::new();
    let taken = from.diags.list.iter().filter(|d| match d.span {
        Some(span) => wanted(d, from.sources.file_of(span.start)),
        None => system.is_some(),
    });
    for mut diagnostic in taken.cloned() {
        diagnostic.span = diagnostic.span.map(&mut moved);
        for fix in &mut diagnostic.fixes {
            for edit in &mut fix.edits {
                edit.0 = moved(edit.0);
            }
        }
        for related in &mut diagnostic.related {
            related.0 = moved(related.0);
        }
        let known = into
            .diags
            .list
            .iter()
            .any(|d| d.code == diagnostic.code && d.message == diagnostic.message && d.span == diagnostic.span);
        if !known {
            if let Some(system) = system {
                diagnostic.message = format!("on {}: {}", system.shown(), diagnostic.message);
            }
            added.push(diagnostic);
        }
    }
    into.diags.list.extend(added);
    into.diags.sort(&into.sources);
}

/// Reads, parses and checks a library for C (toolchain 7): without `main`,
/// it publishes `pub extern fn`s.
pub fn analyze_library(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, false, false, Root::CLibrary)
}

/// Reads, parses and checks the test build of a project (testing 5): its
/// sources, its unit test companions and its integration test roots. The
/// sources are checked first on their own, as a library for C or with no
/// need of `main` (testing 6.3), so tests cannot make code that is wrong
/// without them build.
/// The program at `path`, or, with `tests`, its tests as `tondo test`
/// builds them, as `tondo optimizations` and `tondo asm` read them.
pub fn analyzed(path: &Path, tests: bool) -> Result<Analysis, String> {
    match tests {
        true => analyze_tests(path, false),
        false => analyze(path),
    }
}

pub fn analyze_tests(path: &Path, library: bool) -> Result<Analysis, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a project: `tondo test` runs the tests of a project directory", path.display()));
    }
    let root = if library { Root::CLibrary } else { Root::Checked };
    let production = analyze_with(path, false, false, false, root)?;
    if production.diags.has_errors() {
        return Ok(production);
    }
    analyze_with(path, true, false, false, Root::Program)
}

/// Reads, parses and checks a provider for the `meta` profile (27.7).
pub fn analyze_meta(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, true, false, Root::Program)
}

/// What the root module of an analysis is.
#[derive(Clone, Copy, PartialEq)]
enum Root {
    /// A program, which runs `main` or its script; or a test build, which
    /// runs its tests.
    Program,
    /// Sources checked without being run, which need no `main`: what
    /// `tondo check` and editors see, and what a test build tests.
    Checked,
    /// A library for C (toolchain 7).
    CLibrary,
}

fn analyze_with(path: &Path, tests: bool, meta: bool, editor: bool, root: Root) -> Result<Analysis, String> {
    let parsing = crate::times::part("parse");
    let mut expansions = Vec::new();
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let mut source_id = format!("root:{}", path.file_name().unwrap_or_default().to_string_lossy());
    let mut libraries: Vec<String> = Vec::new();
    let mut elsewhere = Vec::new();
    // The dependencies, whose warnings their authors see, not this program's.
    let mut dependencies: Vec<String> = Vec::new();
    // A file alone is a program of its name, at version 0.0.0.
    let mut built = ast::Built {
        name: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        version: "0.0.0".into(),
        dir: path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf(),
        dependencies: Vec::new(),
    };
    let mut project = crate::roots::Project { dir: built.dir.clone(), ..Default::default() };
    let module = if path.is_dir() {
        let packages = deps::resolve(path, None)?;
        project.dir = path.to_path_buf();
        let system = system();
        for package in &packages {
            if !package.systems.is_empty() && !package.systems.contains(&system) {
                let named: Vec<&str> = package.systems.iter().map(|s| s.shown()).collect();
                return Err(format!(
                    "the package `{}` runs on {}, as `systems` in its tondo.toml says: it is not built for {}",
                    package.name,
                    named.join(" and "),
                    system.shown()
                ));
            }
        }
        project.dependencies = packages[1..]
            .iter()
            .filter(|p| !p.tests_only)
            .map(|p| (p.name.clone(), p.dir.clone(), p.origin.clone()))
            .collect();
        project.template = packages[0].source_url.clone();
        elsewhere = packages.iter().flat_map(|package| declared_elsewhere(package, system)).collect();
        dependencies = packages[1..].iter().map(|p| p.name.clone()).collect();
        built = ast::Built {
            name: packages[0].name.clone(),
            version: packages[0].version.clone(),
            dir: path.to_path_buf(),
            dependencies: packages[1..]
                .iter()
                .filter(|p| !p.tests_only)
                .map(|p| (p.name.clone(), p.version.clone()))
                .collect(),
        };
        for library in packages.iter().flat_map(|p| &p.libraries) {
            if !libraries.contains(library) {
                libraries.push(library.clone());
            }
        }
        source_id = format!("pkg:{}", packages[0].name);
        let mut modules: Vec<modules::ModuleSource> = Vec::new();
        let mut root = 0;
        for (package_index, package) in packages.iter().enumerate() {
            // The program's build leaves out what only its tests use.
            if package.tests_only && !tests {
                continue;
            }
            let (files, root_file) = project::sources(&package.dir, &package.name, Some(system))?;
            // `.proto` files are modules of their own, whose source the
            // compiler writes from them, all together.
            let root_path = files.get(root_file).map(|f| f.path.clone());
            let (protos, files): (Vec<_>, Vec<_>) =
                files.into_iter().partition(|f| f.path.extension().is_some_and(|e| e == "proto"));
            let root_file = files.iter().position(|f| Some(&f.path) == root_path.as_ref()).unwrap_or(0);
            let first = sources.len();
            let parsed_files = parse_files(files.iter().map(|f| f.path.as_path()), &mut sources, &mut diags)?;
            for (index, (file, parsed)) in files.iter().zip(parsed_files).enumerate() {
                // A dependency's files are shown under its package name.
                let relative = file.path.strip_prefix(&package.dir).unwrap_or(&file.path);
                let mut logical: Vec<String> =
                    relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if package_index > 0 {
                    logical.insert(0, package.name.clone());
                    if !package.tests_only {
                        sources.set_dependency(first + index);
                    }
                } else {
                    sources.set_own(first + index);
                }
                sources.set_identity(first + index, logical.join("/"), file.module.join("."));
                let Some(parsed) = parsed else { continue };
                let position = match modules.iter().position(|m| m.package == package_index && m.path == file.module) {
                    Some(position) => position,
                    None => {
                        let source = modules::ModuleSource {
                            path: file.module.clone(),
                            package: package_index,
                            files: Vec::new(),
                        };
                        modules.push(source);
                        modules.len() - 1
                    }
                };
                let is_root = package_index == 0 && index == root_file;
                if is_root {
                    root = position;
                }
                modules[position].files.push((parsed, is_root));
            }
            if !protos.is_empty() {
                let source_root =
                    if package.dir.join("src").is_dir() { package.dir.join("src") } else { package.dir.clone() };
                let mut read = Vec::new();
                for file in &protos {
                    let Some(id) = load(&file.path, &mut sources, &mut diags)? else { continue };
                    let relative = file.path.strip_prefix(&package.dir).unwrap_or(&file.path);
                    sources.set_identity(id, relative.display().to_string(), file.module.join("."));
                    if package_index > 0 {
                        sources.set_dependency(id);
                    } else {
                        sources.set_own(id);
                    }
                    let from_root = file.path.strip_prefix(&source_root).unwrap_or(&file.path);
                    let relative =
                        from_root.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                    read.push(crate::proto::ProtoSource { module: file.module.clone(), source: id, relative });
                }
                for (module, text) in crate::proto::translate(&read, &sources, &mut diags) {
                    let name = format!("{}.proto.to", module.join("."));
                    let id = sources.add(Path::new(&name), text);
                    sources.set_identity(id, format!("<proto>/{name}"), module.join("."));
                    let parsed = parser::parse_file(sources.file(id), &mut diags);
                    match modules.iter_mut().find(|m| m.package == package_index && m.path == module) {
                        Some(found) => found.files.push((parsed, false)),
                        None => modules.push(modules::ModuleSource {
                            path: module,
                            package: package_index,
                            files: vec![(parsed, false)],
                        }),
                    }
                }
            }
        }
        let tested = packages[0].name.clone();
        if tests {
            // Companions join the module of their directory; each root
            // under `tests/` is a module of a package that depends on the
            // tested one.
            let (companions, roots) = project::test_sources(&packages[0].dir, &tested, Some(system))?;
            let first = sources.len();
            let parsed_files = parse_files(companions.iter().map(|f| f.path.as_path()), &mut sources, &mut diags)?;
            for (index, (file, parsed)) in companions.into_iter().zip(parsed_files).enumerate() {
                let relative = file.path.strip_prefix(&packages[0].dir).unwrap_or(&file.path);
                sources.set_identity(first + index, relative.display().to_string(), file.module.join("."));
                let Some(mut parsed) = parsed else { continue };
                parsed.class = ast::SourceClass::Unit;
                match modules.iter_mut().find(|m| m.package == 0 && m.path == file.module) {
                    Some(module) => module.files.push((parsed, false)),
                    None => modules.push(modules::ModuleSource {
                        path: file.module,
                        package: 0,
                        files: vec![(parsed, false)],
                    }),
                }
            }
            let package = packages.len();
            let first = sources.len();
            let parsed_files = parse_files(roots.iter().map(|f| f.path.as_path()), &mut sources, &mut diags)?;
            for (index, (file, parsed)) in roots.into_iter().zip(parsed_files).enumerate() {
                let relative = file.path.strip_prefix(&packages[0].dir).unwrap_or(&file.path);
                sources.set_identity(first + index, relative.display().to_string(), file.module.join("."));
                let Some(mut parsed) = parsed else { continue };
                parsed.class = ast::SourceClass::Integration;
                let path = std::iter::once(format!("{tested}_tests")).chain(file.module).collect();
                modules.push(modules::ModuleSource { path, package, files: vec![(parsed, false)] });
            }
        }
        if diags.has_errors() && !editor {
            drop_dependency_warnings(&mut diags, &sources, &dependencies);
            diags.sort(&sources);
            let index = None;
            return Ok(Analysis {
                sources,
                diags,
                program: None,
                source_id,
                libraries,
                meta,
                expansions,
                index,
                project: Some(project.clone()),
            });
        }
        let mut providers: Vec<Vec<(String, PathBuf)>> = packages.iter().map(|p| p.providers.clone()).collect();
        let generators: Vec<Vec<deps::Generator>> = packages.iter().map(|p| p.generators.clone()).collect();
        let mut packages: Vec<modules::PackageInfo> = packages
            .into_iter()
            .map(|p| modules::PackageInfo {
                name: p.name,
                dependencies: p.dependencies.into_iter().collect(),
                dev_dependencies: p.dev_dependencies.into_iter().collect(),
            })
            .collect();
        if tests {
            // The integration tests see the project and its dev-dependencies.
            let dependencies = [(tested.clone(), 0)].into_iter().collect();
            let dev_dependencies = packages[0].dev_dependencies.clone();
            packages.push(modules::PackageInfo { name: format!("{tested}_tests"), dependencies, dev_dependencies });
            providers.push(Vec::new());
        }
        drop(parsing);
        let expanding = crate::times::part("meta, derive and templates");
        expansions = crate::meta::generate(&mut modules, &packages, &generators, &mut sources, &mut diags);
        expansions.extend(crate::meta::expand(&mut modules, &packages, &providers, &mut sources, &mut diags));
        crate::derive::derive(&mut modules, &mut sources, &mut diags);
        crate::templates::compile(&mut modules, &mut sources, &mut diags);
        drop(expanding);
        let loading = crate::times::part("std");
        stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
        drop(loading);
        let _linking = crate::times::part("modules");
        let mut module = modules::link(modules, root, &packages, &mut diags);
        module.test_package = tests.then_some(tested);
        module
    } else {
        let module = match parse(path, &mut sources, &mut diags)? {
            Some(module) => module,
            None => {
                let index = None;
                return Ok(Analysis {
                    sources,
                    diags,
                    program: None,
                    source_id,
                    libraries,
                    meta,
                    expansions,
                    index,
                    project: Some(project.clone()),
                });
            }
        };
        drop(parsing);
        if stdlib::needed(&module) {
            // The file links with the standard modules it imports.
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let mut modules =
                vec![modules::ModuleSource { path: vec![name.clone()], package: 0, files: vec![(module, true)] }];
            let mut packages = vec![modules::PackageInfo {
                name,
                dependencies: Default::default(),
                dev_dependencies: Default::default(),
            }];
            let expanding = crate::times::part("meta, derive and templates");
            expansions = crate::meta::expand(&mut modules, &packages, &[], &mut sources, &mut diags);
            crate::derive::derive(&mut modules, &mut sources, &mut diags);
            crate::templates::compile(&mut modules, &mut sources, &mut diags);
            drop(expanding);
            let loading = crate::times::part("std");
            stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
            drop(loading);
            let _linking = crate::times::part("modules");
            modules::link(modules, 0, &packages, &mut diags)
        } else {
            module
        }
    };
    let mut module = module;
    module.elsewhere = elsewhere;
    module.built = Some(built);
    module.library |= root != Root::Program;
    module.c_library = root == Root::CLibrary;
    if editor {
        // After errors that stop a build, checking still finds what names
        // refer to; its own errors would only follow from them.
        let earlier = diags.has_errors();
        let mut checked = Diagnostics::default();
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check::check_indexed(&module, &sources, &mut checked)
        }))
        .ok();
        let (program, index) = match found {
            // The program checked, unmonomorphized: what an editor asks of
            // it, as its tests.
            Some((program, index)) => (program.filter(|_| !earlier), Some(index)),
            None => (None, None),
        };
        if !earlier {
            diags.list.append(&mut checked.list);
        }
        drop_dependency_warnings(&mut diags, &sources, &dependencies);
        diags.sort(&sources);
        let program = program.filter(|_| !diags.has_errors());
        return Ok(Analysis {
            sources,
            diags,
            program,
            source_id,
            libraries,
            meta,
            expansions,
            index,
            project: Some(project.clone()),
        });
    }
    let checking = crate::times::part("check");
    let program = if diags.has_errors() { None } else { check::check(&module, &sources, &mut diags, meta) };
    drop(checking);
    let _monomorphizing = crate::times::part("mono");
    let program = program.and_then(|program| {
        // A test build has every function of the project that is not
        // generic, so that coverage counts those no test calls (testing
        // 10.3).
        let root = ROOT.with(|r| r.borrow().clone());
        let own = |f: &hir::Function| {
            let called = root.as_ref().is_some_and(|root| f.name == *root || f.name.ends_with(&format!(".{root}")));
            called || tests && f.generics.is_empty() && f.span != Span::default() && sources.file_of(f.span.start).own
        };
        let roots: Vec<hir::FnId> = (0..program.functions.len())
            .map(|i| hir::FnId(i as u32))
            .filter(|&id| own(&program.functions[id.0 as usize]) && !program.unchecked.contains(&id))
            .collect();
        mono::monomorphize_with(program, &roots, &mut diags)
    });
    drop_dependency_warnings(&mut diags, &sources, &dependencies);
    diags.sort(&sources);
    // A comment of `tondo: allow` is read last, and may be wrong.
    let program = program.filter(|_| !diags.has_errors());
    Ok(Analysis {
        sources,
        diags,
        program,
        source_id,
        libraries,
        meta,
        expansions,
        index: None,
        project: Some(project.clone()),
    })
}

/// Checks an example of documentation (21.6) as the root file of a
/// program: `source`, with the declarations of its fixture at its end
/// (`tail`), and the fixture's modules of `std` (by name, with their
/// source) in place of the standard ones. Without `main` or statements it
/// is checked as a library.
pub fn analyze_fence(
    logical: &str,
    source: &str,
    tail: &str,
    universe: &[(String, String)],
) -> (SourceMap, Diagnostics) {
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let mut parse = |name: &str, logical: String, module: &str, text: &str| {
        let id = sources.add(Path::new(name), text.to_string());
        sources.set_identity(id, logical, module.to_string());
        parser::parse_file(sources.file(id), &mut diags)
    };
    let mut module = parse(logical, logical.to_string(), "doc", source);
    let fixture = parse("fixture.to", "<fixture>/fixture.to".into(), "doc", tail);
    let main = |item: &ast::Item| matches!(item, ast::Item::Fn(f) if f.name.name == "main" && f.owner.is_none());
    let library = module.statements.is_empty() && !module.items.iter().any(main);
    module.imports.extend(fixture.imports);
    module.items.extend(fixture.items);
    let stubs: Vec<ast::Module> = universe
        .iter()
        .map(|(name, text)| {
            parse(&format!("{name}.to"), format!("<fixture>/std.{name}.to"), &format!("std.{name}"), text)
        })
        .collect();
    let mut modules = vec![modules::ModuleSource { path: vec!["doc".into()], package: 0, files: vec![(module, true)] }];
    let mut packages = vec![modules::PackageInfo {
        name: "doc".into(),
        dependencies: Default::default(),
        dev_dependencies: Default::default(),
    }];
    crate::meta::expand(&mut modules, &packages, &[], &mut sources, &mut diags);
    crate::derive::derive(&mut modules, &mut sources, &mut diags);
    crate::templates::compile(&mut modules, &mut sources, &mut diags);
    let replaced: Vec<&str> = universe.iter().map(|(name, _)| name.as_str()).collect();
    stdlib::add_except(&mut modules, &mut packages, &mut sources, &mut diags, &replaced);
    for ((name, _), stub) in universe.iter().zip(stubs) {
        let path = vec!["std".to_string(), name.clone()];
        modules.push(modules::ModuleSource { path, package: packages.len() - 1, files: vec![(stub, false)] });
    }
    let mut module = modules::link(modules, 0, &packages, &mut diags);
    module.library = library;
    if !diags.has_errors()
        && let Some(program) = check::check(&module, &sources, &mut diags, false)
    {
        mono::monomorphize(program, &mut diags);
    }
    diags.sort(&sources);
    (sources, diags)
}

fn drop_dependency_warnings(diags: &mut Diagnostics, sources: &SourceMap, dependencies: &[String]) {
    diags.drop_warnings(sources, |file| {
        let module = &file.module;
        dependencies.iter().any(|d| module == d || module.strip_prefix(d.as_str()).is_some_and(|r| r.starts_with('.')))
    });
}

/// Reads and parses one source file; `None` when it cannot be lexed.
/// What the files of a package that are not for `system` declare at their
/// top, for an error about a name only they have (language 6.8).
fn declared_elsewhere(package: &deps::Package, system: project::System) -> Vec<ast::Elsewhere> {
    let Ok((files, _)) = project::sources(&package.dir, &package.name, None) else { return Vec::new() };
    let mut found = Vec::new();
    for file in files {
        let Some(systems) = project::systems_of(&file.path).filter(|systems| !systems.contains(&system)) else {
            continue;
        };
        let (mut sources, mut diags) = (SourceMap::default(), Diagnostics::default());
        let Ok(Some(module)) = parse(&file.path, &mut sources, &mut diags) else { continue };
        let source = sources.file(0);
        let head = |span: Span, end: Option<Span>| {
            let end = end.map_or(span.end, |end| end.start);
            let text = source.slice(Span::new(span.start, end.max(span.start)));
            text.lines().next().unwrap_or_default().trim().trim_end_matches('{').trim_end().to_string()
        };
        let systems: Vec<&str> = systems.iter().map(|system| system.shown()).collect();
        let relative = file.path.strip_prefix(&package.dir).unwrap_or(&file.path);
        for item in &module.items {
            let (name, head) = match item {
                ast::Item::Fn(f) if f.owner.is_none() => (&f.name.name, head(f.span, f.body.as_ref().map(|b| b.span))),
                ast::Item::Const(c) => (&c.name.name, head(c.span, None)),
                ast::Item::Type(t) => (&t.name.name, head(t.span, None)),
                ast::Item::Enum(e) => (&e.name.name, head(e.span, None)),
                ast::Item::Alias(a) => (&a.name.name, head(a.span, None)),
                _ => continue,
            };
            found.push(ast::Elsewhere {
                module: file.module.join("."),
                name: name.clone(),
                file: relative.display().to_string(),
                systems: systems.join(" and "),
                head,
            });
        }
    }
    found
}

fn parse(path: &Path, sources: &mut SourceMap, diags: &mut Diagnostics) -> Result<Option<ast::Module>, String> {
    let parsed = parse_files(std::iter::once(path), sources, diags)?;
    Ok(parsed.into_iter().next().flatten())
}

/// Reads files into `sources`, one each, in order, and parses them on every
/// core: each one's module, or none when it is not valid UTF-8 or starts
/// with a byte order mark.
fn parse_files<'p>(
    paths: impl Iterator<Item = &'p Path>,
    sources: &mut SourceMap,
    diags: &mut Diagnostics,
) -> Result<Vec<Option<ast::Module>>, String> {
    let mut loaded = Vec::new();
    for path in paths {
        loaded.push(load(path, sources, diags)?);
    }
    let parsed = crate::parallel::map(&loaded, 1, |file| {
        let mut found = Diagnostics::default();
        file.map(|file| (parser::parse_file(sources.file(file), &mut found), found))
    });
    Ok(parsed
        .into_iter()
        .map(|parsed| {
            parsed.map(|(module, mut found)| {
                diags.list.append(&mut found.list);
                module
            })
        })
        .collect())
}

/// Reads a file into `sources`, or the text an editor has of it: the file,
/// unless it cannot be parsed, which `diags` says.
fn load(path: &Path, sources: &mut SourceMap, diags: &mut Diagnostics) -> Result<Option<usize>, String> {
    let open = OPEN.with(|o| {
        let open = o.borrow();
        if open.is_empty() {
            return None;
        }
        open.get(&path.canonicalize().unwrap_or_else(|_| path.to_path_buf())).cloned()
    });
    let bytes = match open {
        Some(text) => text.into_bytes(),
        None => std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?,
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let offset = error.utf8_error().valid_up_to() as u32;
            let text = String::from_utf8_lossy(error.as_bytes()).into_owned();
            let file = sources.add(path, text);
            let base = sources.file(file).base;
            // The byte is a replacement character in the text that is shown.
            let end = base + offset + char::REPLACEMENT_CHARACTER.len_utf8() as u32;
            diags.error("E0001", Span::new(base + offset, end), "invalid UTF-8 in source file");
            return Ok(None);
        }
    };
    let has_bom = text.starts_with('\u{feff}');
    let file = sources.add(path, text);
    if has_bom {
        let base = sources.file(file).base;
        diags.error("E0001", Span::new(base, base + 3), "source files must not start with a byte order mark");
        return Ok(None);
    }
    Ok(Some(file))
}

/// The source files a build of `path` reads, for cache keys.
pub fn inputs(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut inputs = sources(path)?;
    embedded(&mut inputs);
    Ok(inputs)
}

/// Adds the files the sources among `inputs` embed (standard library
/// 14.27), and those of the repository whose commit `env.build()` reports
/// (14.3.6).
fn embedded(inputs: &mut Vec<PathBuf>) {
    let mut found = Vec::new();
    let mut reports = None;
    for input in inputs.iter().filter(|p| p.extension().is_some_and(|e| e == "to")) {
        if let Ok(text) = std::fs::read_to_string(input) {
            let embedded = crate::embed::inputs(input, &text);
            found.extend(crate::templates::inputs(input, &embedded));
            found.extend(embedded);
            if text.contains("build()") {
                reports = input.parent().map(Path::to_path_buf);
            }
        }
    }
    if let Some(dir) = reports {
        found.extend(crate::built::inputs(&dir));
    }
    inputs.extend(found);
}

/// The files of a build but those its sources embed.
fn sources(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut inputs = Vec::new();
    for package in deps::resolve(path, None)? {
        let (files, _) = project::sources(&package.dir, &package.name, None)?;
        inputs.extend(files.into_iter().map(|f| f.path));
        let manifest = package.dir.join("tondo.toml");
        if manifest.is_file() {
            inputs.push(manifest);
        }
        inputs.extend(package.providers.into_iter().map(|(_, file)| file));
        for generator in package.generators {
            inputs.push(generator.program);
            inputs.extend(generator.inputs.into_iter().map(|(_, file)| file));
        }
    }
    Ok(inputs)
}

/// The files a test build of the project at `path` reads: those of its
/// build, its unit test companions and its integration test roots.
pub fn test_inputs(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut inputs = sources(path)?;
    let (companions, roots) = project::test_sources(path, "", None)?;
    inputs.extend(companions.into_iter().chain(roots).map(|f| f.path));
    embedded(&mut inputs);
    Ok(inputs)
}

/// Writes a library for C (toolchain 7.7) into `dir`: the shared library
/// `lib<name>.so` (`.dylib` on macOS) and the static `lib<name>.a`, each
/// with the program's object and the runtime, and `<name>.h`, which
/// declares what they publish. Gives how a C program links them.
pub fn library(analysis: &Analysis, object: &[u8], dir: &Path, name: &str) -> Result<String, String> {
    let program = analysis.program.as_ref().expect("library() requires a checked program");
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let (archive, shared) =
        (format!("lib{name}.a"), format!("lib{name}.{}", if cfg!(target_os = "macos") { "dylib" } else { "so" }));
    let path = carried("libtondo_runtime", RUNTIME)?;
    // With musl, the runtime's unwinder comes with the compiler.
    let mut runtime_args: Vec<String> = RUNTIME_LINK_ARGS.split_whitespace().map(String::from).collect();
    let mut archives = vec![path];
    if !UNWIND.is_empty() {
        runtime_args.retain(|a| a != "-lunwind");
        archives.push(carried("libunwind", UNWIND)?);
    }
    // The shared library, which publishes the functions alone.
    let exports: Vec<String> = program.c_api.functions.iter().map(|f| f.symbol.clone()).collect();
    let product = crate::link::Product::Library { exports: &exports };
    crate::link::link(object, &archives, &runtime_args, &analysis.libraries, &dir.join(&shared), product)?;
    // The static one, whose host links what it needs.
    let mut contents = Vec::new();
    for path in &archives {
        contents.push(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
    }
    let mut members = vec![(format!("{name}.o"), object)];
    for content in &contents {
        let parsed = object::read::archive::ArchiveFile::parse(&content[..]).map_err(|e| e.to_string())?;
        for member in parsed.members() {
            let member = member.map_err(|e| e.to_string())?;
            let data = member.data(&content[..]).map_err(|e| e.to_string())?;
            members.push((String::from_utf8_lossy(member.name()).into_owned(), data));
        }
    }
    write_atomic(&dir.join(&archive), &crate::link::archive(&members, cfg!(target_os = "macos")))?;
    let mut native: Vec<String> = Vec::new();
    for library in &analysis.libraries {
        native.push(match library.strip_prefix("static:") {
            Some(name) => format!("-l{name}"),
            None if library.contains('/') => library.clone(),
            None => format!("-l{library}"),
        });
    }
    let absolute = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let strip = if cfg!(target_os = "macos") { "-Wl,-dead_strip" } else { "-Wl,--gc-sections" };
    let system: Vec<&str> = native.iter().map(String::as_str).chain(runtime_args.iter().map(String::as_str)).collect();
    let linking = format!(
        "Link the shared library: cc app.c -L{0} -l{name} -Wl,-rpath,{1}\nor the static one: cc app.c {2} {strip} {3}",
        dir.display(),
        absolute.display(),
        dir.join(&archive).display(),
        system.join(" ")
    );
    write_atomic(&dir.join(format!("{name}.h")), crate::header::header(name, &program.c_api, &linking).as_bytes())?;
    Ok(linking)
}

/// Lowers a checked program to a native object file, for this machine or
/// a target; a debug build has what a debugger needs (toolchain 7.6) and
/// is not optimized.
pub fn object(analysis: &Analysis, debug: bool, target: Option<Target>) -> Result<Vec<u8>, String> {
    compile(analysis, debug, target, None)
}

/// The literals of the code the search of `--fuzz` counts: the texts
/// without interpolation and the integers, each once, which mutations take
/// values from (testing 10.6); the project's and its dependencies' first.
pub fn literals(analysis: &Analysis) -> (Vec<String>, Vec<i64>) {
    let (mut texts, mut ints) = (Vec::new(), Vec::new());
    let mut seen = (std::collections::HashSet::new(), std::collections::HashSet::new());
    let first = analysis.sources.files().filter(|f| f.own || f.dependency);
    for file in first.chain(analysis.sources.files().filter(|f| !f.own && !f.dependency && f.searched())) {
        for token in crate::lexer::lex(file, None, &mut Diagnostics::default()) {
            match token.tok {
                crate::lexer::Tok::Str(parts) => {
                    if let [crate::lexer::StrPart::Text(text)] = &parts[..]
                        && (1..=64).contains(&text.len())
                        && seen.0.insert(text.clone())
                    {
                        texts.push(text.clone());
                    }
                }
                crate::lexer::Tok::Int { value, .. }
                    if value > 1 && value <= i64::MAX as u128 && seen.1.insert(value as i64) =>
                {
                    ints.push(value as i64);
                }
                _ => {}
            }
        }
    }
    texts.truncate(4096);
    ints.truncate(4096);
    (texts, ints)
}

/// The object of a test build that counts what runs of the project's code
/// (testing 10.3), with what each of its counters counts; for the search
/// of `--fuzz`, of its dependencies' too, telling the runtime what that
/// code compares (10.6).
pub fn object_counting(
    analysis: &Analysis,
    debug: bool,
    target: Option<Target>,
    fuzz: bool,
) -> Result<(Vec<u8>, Vec<codegen::Counted>), String> {
    let program = analysis.program.as_ref().expect("object_counting() requires a checked program");
    let bodies = lower::lower(program, false, debug, None);
    let mut counted = Vec::new();
    let triple = target.map(Target::triple);
    let options = codegen::Options {
        meta: analysis.meta,
        debug,
        target: triple,
        live: None,
        roots: "",
        coverage: Some(&mut counted),
        fuzz,
    };
    let object = codegen::compile(program, &bodies, &analysis.sources, options)?;
    Ok((object, counted))
}

/// The object of a program, live (toolchain 7.10) when `live` says how.
pub fn compile(
    analysis: &Analysis,
    debug: bool,
    target: Option<Target>,
    live: Option<&mut crate::live::Build>,
) -> Result<Vec<u8>, String> {
    let program = analysis.program.as_ref().expect("object() requires a checked program");
    let debug = debug || live.is_some();
    let lowering = crate::times::part("lower");
    let mut bodies = lower::lower(program, live.is_some(), debug, live.as_ref().map(|build| &build.catch_ups));
    drop(lowering);
    let triple = target.map(Target::triple);
    let (types, sources) = (&program.types, &analysis.sources);
    after("lower", &bodies, types, sources);
    if !debug && !analysis.meta {
        let _passes = crate::times::part("MIR passes");
        optimize(&mut bodies, types, sources);
    }
    let roots = analysis.project.as_ref().map_or(String::new(), |p| crate::roots::table(p, &analysis.sources));
    let options = codegen::Options {
        meta: analysis.meta,
        debug,
        target: triple,
        live,
        coverage: None,
        fuzz: false,
        roots: &roots,
    };
    codegen::compile(program, &bodies, &analysis.sources, options)
}

/// The files of the project or file at `path` as the git revision
/// `reference` has them, extracted once per commit into the cache: the
/// commit, and where `path` is in them (testing 10.4, toolchain 7.18).
pub fn revision(path: &Path, reference: &str) -> Result<(String, PathBuf), String> {
    use std::process::{Command, Stdio};
    let git = |dir: &Path, args: &[&str]| -> Option<String> {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let given = path.canonicalize().map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let place = if given.is_file() { given.parent().unwrap_or(&given).to_path_buf() } else { given.clone() };
    let root = git(&place, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .ok_or(format!("a revision is one of a git repository's, and {} is in none", path.display()))?;
    let commit = git(&root, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
        .filter(|c| !c.is_empty())
        .ok_or(format!("git knows no revision {reference}: give a branch, a tag or a commit, as `main`"))?;
    let within = given.strip_prefix(root.canonicalize().unwrap_or(root.clone())).unwrap_or(Path::new("")).to_path_buf();
    let tree = cache_entry("revision", &commit);
    if !tree.join(".complete").is_file() {
        let _ = std::fs::remove_dir_all(&tree);
        std::fs::create_dir_all(&tree).map_err(|e| format!("cannot create {}: {e}", tree.display()))?;
        let mut archive = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["archive", "--format=tar", &commit])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run git: {e}"))?;
        let stdout = archive.stdout.take().expect("piped");
        let extracted = Command::new("tar").arg("-x").arg("-C").arg(&tree).stdin(stdout).status();
        let archived = archive.wait().map_err(|e| e.to_string())?;
        if !archived.success() || !extracted.is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_dir_all(&tree);
            return Err(format!("cannot extract {reference} from git"));
        }
        std::fs::write(tree.join(".complete"), &commit).map_err(|e| e.to_string())?;
    }
    Ok((commit, tree.join(within)))
}

/// A `tondo` to compare with (toolchain 7.19): an executable, as given, or
/// a revision of tondo's own repository, where `near` is, built once in
/// release and kept in the cache. What names it, its commit or its path,
/// and where it is.
pub fn compiler(reference: &str, near: &Path) -> Result<(String, PathBuf), String> {
    use std::process::{Command, Stdio};
    let given = Path::new(reference);
    if given.is_file() {
        let path = given.canonicalize().map_err(|e| format!("cannot read {reference}: {e}"))?;
        return Ok((path.display().to_string(), path));
    }
    let place = near.canonicalize().unwrap_or(near.to_path_buf());
    let place = if place.is_file() { place.parent().unwrap_or(&place).to_path_buf() } else { place };
    let toplevel = Command::new("git").arg("-C").arg(&place).args(["rev-parse", "--show-toplevel"]).output().ok();
    let root = toplevel
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .ok_or(format!(
            "{reference} is no tondo executable, nor a revision: {} is in no git repository",
            near.display()
        ))?;
    let (commit, tree) = revision(&root, reference)?;
    if !tree.join("crates/tondo/Cargo.toml").is_file() {
        return Err(format!(
            "{reference} is a revision of {}, which is not tondo's repository: give a tondo executable, or run this in tondo's",
            root.display()
        ));
    }
    let kept = cache_entry("compiler", &commit);
    let executable = kept.join("tondo");
    if executable.is_file() {
        return Ok((commit, executable));
    }
    eprintln!("tondo: building the tondo of {reference} ({}), once", &commit[..commit.len().min(12)]);
    // One target for every revision, which builds again only what differs.
    let target = cache_dir().join("compiler-target");
    let built = Command::new("cargo")
        .args(["build", "--release", "--locked", "--quiet", "-p", "tondo"])
        .current_dir(&tree)
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("TONDO_RUNTIME_FAST")
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("building the tondo of {reference} needs cargo, from https://rustup.rs: {e}"))?;
    if !built.success() {
        return Err(format!("the tondo of {reference} does not build: cargo says why above"));
    }
    std::fs::create_dir_all(&kept).map_err(|e| format!("cannot create {}: {e}", kept.display()))?;
    let partial = kept.join(format!("tondo.{}", std::process::id()));
    std::fs::copy(target.join("release/tondo"), &partial).map_err(|e| format!("cannot keep the tondo built: {e}"))?;
    std::fs::rename(&partial, &executable).map_err(|e| format!("cannot keep the tondo built: {e}"))?;
    Ok((commit, executable))
}

/// The MIR passes of a release build, in their order, each followed by
/// what `after` does.
fn optimize(bodies: &mut Vec<crate::mir::Body>, types: &crate::types::Types, sources: &SourceMap) {
    crate::inline::inline(bodies);
    after("inline", bodies, types, sources);
    crate::elide::elide(bodies, types);
    after("elide", bodies, types, sources);
    crate::thread::thread(bodies, types);
    after("thread", bodies, types, sources);
    crate::lends::lends(bodies);
    after("lends", bodies, types, sources);
    crate::moves::moves(bodies, types);
    after("moves", bodies, types, sources);
    crate::reserve::reserve(bodies, types);
    after("reserve", bodies, types, sources);
    crate::unroll::unroll(bodies, types);
    after("unroll", bodies, types, sources);
    crate::prove::prove(bodies, types);
    lower::drop_dead_checks(bodies);
    after("prove", bodies, types, sources);
    crate::stack::stack(bodies, types);
    after("stack", bodies, types, sources);
}

/// After lowering and each pass: the MIR checked, in a debug build of
/// `tondo`, and written as `TONDO_DUMP_MIR` asks.
fn after(stage: &str, bodies: &[crate::mir::Body], types: &crate::types::Types, sources: &SourceMap) {
    crate::verify::verify(stage, bodies, types, sources);
    crate::mir_text::dump(stage, bodies, types, sources);
}

/// What a pass of a release build did, or could not do, at a place in the
/// program's own code (toolchain 7.18).
#[derive(Clone)]
pub struct Optimization {
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// The functions whose code it is in: its own, and those it is inlined
    /// into.
    pub functions: Vec<String>,
    /// `inlined` or `call`, `frame` or `heap`, `unchecked` or `checked`,
    /// `arithmetic`, `unrolled` or `looped`, `reserved` or `grown`, `lent`
    /// or `counted`.
    pub kind: &'static str,
    /// What was done, to read after `source`.
    pub text: String,
    /// The first line of the code it is about.
    pub source: String,
}

/// The kinds of `Optimization`, a pass's done and its not done side by side.
pub const OPTIMIZATION_KINDS: &[&str] = &[
    "inlined",
    "call",
    "frame",
    "heap",
    "unchecked",
    "checked",
    "arithmetic",
    "unrolled",
    "looped",
    "reserved",
    "grown",
    "lent",
    "counted",
];

impl Optimization {
    /// The line `tondo optimizations` writes.
    pub fn line(&self) -> String {
        let (file, line, column) = (&self.file, self.line, self.column);
        format!("{file}:{line}:{column}: `{}` {} (in {})", self.source, self.text, self.functions.join(", "))
    }

    /// As data (toolchain 7.12).
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "file": self.file, "line": self.line, "column": self.column, "functions": self.functions,
            "kind": self.kind, "text": self.text, "source": self.source,
        })
    }
}

/// What the passes of a release build of a checked program did in its own
/// code, in the functions whose names hold `function` and of the `kind`
/// when they are given, in
/// the order of the source, without generating the code. What is done in
/// one copy of a place, of a loop unrolled or code inlined twice, and not
/// in another, is told as not done when it is a call or the heap, and as
/// both when it is a check.
pub fn optimizations(
    analysis: &Analysis,
    function: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<Optimization>, String> {
    if let Some(kind) = kind.filter(|kind| !OPTIMIZATION_KINDS.contains(kind)) {
        return Err(format!("no kind `{kind}`: {}", OPTIMIZATION_KINDS.join(", ")));
    }
    let program = analysis.program.as_ref().expect("optimizations() requires a checked program");
    let sources = &analysis.sources;
    crate::mir_text::ask_remarks(true);
    let mut bodies = lower::lower(program, false, false, None);
    if !analysis.meta {
        optimize(&mut bodies, &program.types, sources);
    }
    crate::mir_text::ask_remarks(false);
    let mut found = Vec::new();
    for body in bodies.iter().filter(|body| function.is_none_or(|name| body.name.contains(name))) {
        // What a loop unrolled or code inlined twice does in one copy of
        // a place and not in another: a call or the heap is told as such,
        // and a check one copy drops and another keeps as both.
        let worse = |kind: &str| match kind {
            "inlined" => "call",
            "frame" => "heap",
            "checked" => "unchecked",
            "unrolled" => "looped",
            "reserved" => "grown",
            "lent" => "counted",
            _ => "",
        };
        let told: std::collections::HashSet<_> =
            body.remarks.iter().map(|r| (r.kind, r.span.start, r.span.end)).collect();
        let kept = |remark: &crate::mir::Remark| told.contains(&("checked", remark.span.start, remark.span.end));
        for remark in &body.remarks {
            let file = sources.file_of(remark.span.start);
            if file.dependency || file.logical.starts_with("<std>/") {
                continue;
            }
            if told.contains(&(worse(remark.kind), remark.span.start, remark.span.end))
                || kind.is_some_and(|kind| kind != remark.kind)
            {
                continue;
            }
            let (name, line, column) = sources.position(remark.span.start);
            let first = sources.slice(remark.span).lines().next().unwrap_or("").trim();
            let source = match first.chars().count() > 60 {
                true => format!("{}…", first.chars().take(59).collect::<String>()),
                false => first.to_string(),
            };
            let text = match remark.kind == "unchecked" && kept(remark) {
                true => format!("{}, in one copy of the code; another keeps the check", remark.text),
                false => remark.text.clone(),
            };
            found.push(Optimization {
                file: name,
                line,
                column,
                functions: vec![body.name.clone()],
                kind: remark.kind,
                text,
                source,
            });
        }
    }
    let key = |o: &Optimization| (o.file.clone(), o.line, o.column, o.kind, o.text.clone());
    found.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.functions.cmp(&b.functions)));
    let mut grouped: Vec<Optimization> = Vec::new();
    for o in found {
        match grouped.last_mut() {
            Some(last) if key(last) == key(&o) => {
                if !last.functions.contains(&o.functions[0]) {
                    last.functions.extend(o.functions);
                }
            }
            _ => grouped.push(o),
        }
    }
    Ok(grouped)
}

/// What a release build made of the code at `path` as the git revision
/// `reference` has it, and of the code as it is: what only the revision
/// has, and what only the code has. A place is known by its file, code,
/// kind, text and functions rather than its line, which a change moves.
pub fn optimizations_against(
    path: &Path,
    reference: &str,
    function: Option<&str>,
    kind: Option<&str>,
    tests: bool,
) -> Result<(String, Vec<Optimization>, Vec<Optimization>), String> {
    let now = analyzed(path, tests)?;
    if now.program.is_none() {
        return Err(format!("{} does not compile: `tondo check` says why", path.display()));
    }
    let now = optimizations(&now, function, kind)?;
    let (commit, dir) = revision(path, reference)?;
    let then = analyzed(&dir, tests)?;
    if then.program.is_none() {
        return Err(format!("the code of {reference} does not compile, so there is nothing to compare with"));
    }
    // A file on its own is named as given, the revision's in the cache.
    let (cached, given) = (dir.display().to_string(), path.display().to_string());
    let mut then = optimizations(&then, function, kind)?;
    for optimization in &mut then {
        if let Some(rest) = optimization.file.strip_prefix(&cached) {
            optimization.file = format!("{given}{rest}");
        }
    }
    let (gone, new) = optimization_changes(then, now);
    Ok((commit, gone, new))
}

/// What a release build made that it no longer makes, and what it makes
/// that it did not, of the same places.
fn optimization_changes(then: Vec<Optimization>, now: Vec<Optimization>) -> (Vec<Optimization>, Vec<Optimization>) {
    let key = |o: &Optimization| (o.file.clone(), o.source.clone(), o.kind, o.text.clone(), o.functions.clone());
    let only = |these: Vec<Optimization>, others: &[Optimization]| {
        let mut left: std::collections::HashMap<_, usize> = std::collections::HashMap::new();
        others.iter().for_each(|o| *left.entry(key(o)).or_default() += 1);
        these
            .into_iter()
            .filter(|o| match left.get_mut(&key(o)) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    false
                }
                _ => true,
            })
            .collect::<Vec<_>>()
    };
    let gone = only(then.clone(), &now);
    let new = only(now, &then);
    (gone, new)
}

/// What another `tondo` makes of the code and this one does not, and the
/// other way round (toolchain 7.18): it says what it made with `--json`.
pub fn optimizations_compiler(
    path: &Path,
    compiler: &Path,
    function: Option<&str>,
    kind: Option<&str>,
    tests: bool,
) -> Result<(Vec<Optimization>, Vec<Optimization>), String> {
    let now = analyzed(path, tests)?;
    if now.program.is_none() {
        return Err(format!("{} does not compile: `tondo check` says why", path.display()));
    }
    let now = optimizations(&now, function, kind)?;
    let mut command = std::process::Command::new(compiler);
    command.arg("optimizations").arg(path).arg("--json");
    if tests {
        command.arg("--tests");
    }
    function.map(|name| command.args(["--function", name]));
    kind.map(|kind| command.args(["--kind", kind]));
    let ran = command.output().map_err(|e| format!("cannot run {}: {e}", compiler.display()))?;
    let told: serde_json::Value = serde_json::from_slice(&ran.stdout).map_err(|_| {
        format!("{} said nothing of the code: {}", compiler.display(), String::from_utf8_lossy(&ran.stderr).trim())
    })?;
    if told["ok"] != true {
        return Err(format!("{} could not build the code: its diagnostics say why", compiler.display()));
    }
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_string();
    let then = told["optimizations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let kind = OPTIMIZATION_KINDS.iter().find(|k| **k == o["kind"].as_str().unwrap_or_default())?;
            Some(Optimization {
                file: text(&o["file"]),
                line: o["line"].as_u64().unwrap_or(0) as usize,
                column: o["column"].as_u64().unwrap_or(0) as usize,
                functions: o["functions"].as_array().into_iter().flatten().map(text).collect(),
                kind,
                text: text(&o["text"]),
                source: text(&o["source"]),
            })
        })
        .collect();
    Ok(optimization_changes(then, now))
}

/// What changed against a revision, as `tondo optimizations --against`
/// writes it: what only the revision has, then what only the code has.
pub fn changed_text(reference: &str, commit: &str, gone: &[Optimization], new: &[Optimization]) -> String {
    if gone.is_empty() && new.is_empty() {
        return format!("a release build makes of the code what it made of {reference}\n");
    }
    let short = match commit.len() {
        0 => String::new(),
        _ => format!(" ({})", &commit[..commit.len().min(12)]),
    };
    let mut text = String::new();
    if !gone.is_empty() {
        text.push_str(&format!("only in {reference}{short}:\n"));
        gone.iter().for_each(|o| text.push_str(&format!("  {}\n", o.line())));
    }
    if !new.is_empty() {
        text.push_str("only now:\n");
        new.iter().for_each(|o| text.push_str(&format!("  {}\n", o.line())));
    }
    text
}

/// The machine code of the runtime's functions named `function` (toolchain
/// 7.19), as this `tondo` links it into programs: by the name, demangled,
/// or by a part of the names of a few. What each calls or reads is named
/// by its relocation, so that a diff compares what it calls.
pub fn runtime_assembly(function: &str) -> Result<Vec<Assembly>, String> {
    use object::{Object, ObjectSection, ObjectSymbol};
    let path = carried("libtondo_runtime", RUNTIME)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the runtime: {e}"))?;
    let archive = object::read::archive::ArchiveFile::parse(&*bytes).map_err(|e| format!("the runtime: {e}"))?;
    let name = |symbol: &str| format!("{:#}", rustc_demangle::demangle(symbol));
    let mut found: Vec<Assembly> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for member in archive.members().flatten() {
        let Ok(data) = member.data(&*bytes) else { continue };
        let Ok(file) = object::File::parse(data) else { continue };
        for symbol in file.symbols() {
            if symbol.kind() != object::SymbolKind::Text || symbol.is_undefined() || symbol.size() == 0 {
                continue;
            }
            let Ok(raw) = symbol.name() else { continue };
            let shown = name(raw);
            let exact = shown == function || raw == function;
            if !exact && !shown.contains(function) {
                continue;
            }
            if exact {
                (found, named) = (Vec::new(), vec![shown.clone()]);
            } else if named.len() == 1 && (named[0] == function) {
                continue;
            } else {
                named.push(shown.clone());
            }
            let Some(section) = symbol.section_index().and_then(|index| file.section_by_index(index).ok()) else {
                continue;
            };
            let Ok(code) = section.data() else { continue };
            let start = symbol.address() as usize;
            let Some(code) = code.get(start..start + symbol.size() as usize) else { continue };
            // What each place of it calls or reads, by the relocations.
            let targets: Vec<(usize, String)> = section
                .relocations()
                .filter(|(at, _)| (start..start + code.len()).contains(&(*at as usize)))
                .filter_map(|(at, relocation)| match relocation.target() {
                    object::RelocationTarget::Symbol(index) => {
                        let target = file.symbol_by_index(index).ok()?;
                        Some((at as usize - start, name(target.name().ok()?)))
                    }
                    _ => None,
                })
                .collect();
            let instructions = crate::disassembly::decode(code)
                .into_iter()
                .map(|(offset, length, text)| Instruction {
                    offset,
                    bytes: code[offset..offset + length].iter().map(|b| format!("{b:02x}")).collect(),
                    text,
                    target: targets
                        .iter()
                        .find(|(at, _)| (offset..offset + length).contains(at))
                        .map(|(_, name)| name.clone()),
                    at: None,
                })
                .collect();
            found.push(Assembly {
                function: shown,
                size: code.len(),
                constants: 0,
                instructions,
                lines: Vec::new(),
                ir: None,
                rules: Vec::new(),
            });
        }
    }
    match found.len() {
        0 => Err(format!("the runtime has no function named `{function}`")),
        1..=8 => Ok(found),
        many => Err(format!(
            "{many} functions of the runtime hold `{function}` in their names: name one, as `{}`",
            named.iter().take(5).cloned().collect::<Vec<_>>().join("`, `")
        )),
    }
}

/// An instruction of a function a release build made (toolchain 7.19):
/// where it is in the function, its bytes and text, what it calls or
/// reads when a relocation names it, and the place of the program its
/// statement is at.
pub struct Instruction {
    pub offset: usize,
    pub bytes: String,
    pub text: String,
    pub target: Option<String>,
    pub at: Option<(String, usize)>,
}

/// The machine code a release build makes of a function, by instruction,
/// with what the passes said of its lines, and its IR before and after
/// Cranelift optimizes it when asked.
pub struct Assembly {
    pub function: String,
    pub size: usize,
    /// The bytes of constants after its instructions, which they read.
    pub constants: usize,
    pub instructions: Vec<Instruction>,
    /// The code of each line its instructions are at, and what `tondo
    /// optimizations` tells of it.
    pub lines: Vec<((String, usize), String, Vec<String>)>,
    pub ir: Option<(String, String)>,
    /// The ISLE rules Cranelift applied to it, with `--rules`: the term,
    /// where the rule is, how many times, and whether it is Tondo's.
    pub rules: Vec<(String, String, u64, bool)>,
}

/// The machine code a release build of a checked program makes of the
/// functions named `function`, or, when none is, of those whose names hold
/// it; or with no `function`, of every function of the program's own
/// code; without linking it, and with their IR when `ir` asks.
pub fn assembly(analysis: &Analysis, function: Option<&str>, ir: bool) -> Result<Vec<Assembly>, String> {
    assembly_with(analysis, function, ir, false)
}

/// As `assembly`, with the ISLE rules Cranelift applied to each function
/// when `rules` asks, in a `tondo` built with its feature `rules`.
pub fn assembly_with(
    analysis: &Analysis,
    function: Option<&str>,
    ir: bool,
    rules: bool,
) -> Result<Vec<Assembly>, String> {
    if rules && !cfg!(feature = "rules") {
        return Err("this tondo does not log the rules Cranelift applies: build one that does with `cargo build --release -p tondo --features rules`".into());
    }
    let program = analysis.program.as_ref().expect("assembly() requires a checked program");
    let sources = &analysis.sources;
    let own = |body: &crate::mir::Body| {
        let file = sources.file_of(body.span.start);
        body.span != crate::source::Span::default() && !file.dependency && !file.logical.starts_with("<std>/")
    };
    let bodies = lower::lower(program, false, false, None);
    let mut names: std::collections::HashSet<String> = match function {
        Some(name) if bodies.iter().any(|body| body.name == name) => std::iter::once(name.to_string()).collect(),
        Some(name) => bodies.iter().filter(|body| body.name.contains(name)).map(|body| body.name.clone()).collect(),
        None => bodies.iter().filter(|body| own(body)).map(|body| body.name.clone()).collect(),
    };
    names.retain(|name| !name.is_empty());
    crate::codegen::keep_code(Some(crate::codegen::Asked { names, ir, rules }));
    let built = compile(analysis, false, None, None);
    crate::codegen::keep_code(None);
    built?;
    let mut kept = crate::codegen::kept_code();
    if kept.is_empty() {
        let named = function.map_or("of the program's own".to_string(), |name| format!("named `{name}`"));
        return Err(format!(
            "a release build has no code of a function {named}: no function holds that name, or every call of it is inlined, which `tondo optimizations` tells"
        ));
    }
    kept.sort_by(|a, b| a.name.cmp(&b.name));
    let sources = &analysis.sources;
    let told = optimizations(analysis, None, None)?;
    let mut assemblies = Vec::new();
    for kept in kept {
        let place = |start: u32| {
            let (file, line, _) = sources.position(start);
            (file, line)
        };
        let at = |offset: usize| {
            let run = kept.lines.partition_point(|&(start, _)| start as usize <= offset).checked_sub(1)?;
            kept.lines[run].1.map(place)
        };
        let code = &kept.bytes[..kept.code_end.min(kept.bytes.len())];
        let instructions: Vec<Instruction> = crate::disassembly::decode(code)
            .into_iter()
            .map(|(offset, length, text)| Instruction {
                offset,
                bytes: kept.bytes[offset..offset + length].iter().map(|b| format!("{b:02x}")).collect(),
                text,
                target: kept
                    .targets
                    .iter()
                    .find(|(at, _)| (offset..offset + length).contains(&(*at as usize)))
                    .map(|(_, name)| name.clone()),
                at: at(offset),
            })
            .collect();
        let mut lines: Vec<((String, usize), String, Vec<String>)> = Vec::new();
        for start in kept.lines.iter().filter_map(|&(_, start)| start) {
            let (file, line) = place(start);
            if lines.iter().any(|(at, ..)| *at == (file.clone(), line)) {
                continue;
            }
            let code = sources.file_of(start).line_text(line).trim().to_string();
            let remarks = told
                .iter()
                .filter(|o| o.file == file && o.line == line && o.functions.contains(&kept.name))
                .map(|o| format!("`{}` {}", o.source, o.text))
                .collect();
            lines.push(((file, line), code, remarks));
        }
        let constants = kept.bytes.len() - code.len();
        let (function, size, ir, rules) = (kept.name, kept.bytes.len(), kept.ir, kept.rules);
        assemblies.push(Assembly { function, size, constants, instructions, lines, ir, rules });
    }
    Ok(assemblies)
}

/// A function whose code changed, and the lines of its diff.
pub type Changed = (String, Vec<String>);

/// What changed in the machine code of the functions named `function`
/// since the git revision `reference`, function by function, as the lines
/// of a diff: ` ` the same, `-` only in the revision, `+` only now. The
/// distance of a jump is left out, which code before it moves.
pub fn assembly_against(
    path: &Path,
    reference: &str,
    function: Option<&str>,
    tests: bool,
) -> Result<(String, Vec<Changed>, usize), String> {
    let now = compiled_assembly(path, function, "the code", tests)?;
    let (commit, dir) = revision(path, reference)?;
    let then = compiled_assembly(&dir, function, &format!("the code of {reference}"), tests)?;
    let (changes, same) = assembly_changes(&then, &now);
    Ok((commit, changes, same))
}

/// The machine code of the functions named, or of all the program's own,
/// of the code at `path`, which must compile.
fn compiled_assembly(path: &Path, function: Option<&str>, what: &str, tests: bool) -> Result<Vec<Assembly>, String> {
    let analysis = analyzed(path, tests)?;
    if analysis.program.is_none() {
        return Err(format!("{what} does not compile, so there is nothing to compare"));
    }
    assembly(&analysis, function, false)
}

/// What changed in the machine code of the code at `path` from what the
/// `tondo` at `compiler` makes of it to what this one does, function by
/// function as `assembly_against` tells it, and how many are the same.
pub fn assembly_compiler(
    path: &Path,
    compiler: &Path,
    function: Option<&str>,
    tests: bool,
) -> Result<(Vec<Changed>, usize), String> {
    let now = compiled_assembly(path, function, "the code", tests)?;
    let mut command = std::process::Command::new(compiler);
    command.arg("asm").arg(path);
    if tests {
        command.arg("--tests");
    }
    match function {
        Some(name) => command.arg(name),
        None => command.arg("--all"),
    };
    let then = told_assembly(command, compiler)?;
    Ok(assembly_changes(&then, &now))
}

/// What changed in the machine code of the runtime's functions named
/// `function` from the runtime of the `tondo` at `compiler` to this one's.
pub fn runtime_assembly_compiler(compiler: &Path, function: &str) -> Result<(Vec<Changed>, usize), String> {
    let now = runtime_assembly(function)?;
    let mut command = std::process::Command::new(compiler);
    command.args(["asm", "--runtime", function]);
    let then = told_assembly(command, compiler)?;
    Ok(assembly_changes(&then, &now))
}

/// The machine code another `tondo` says it makes, with `asm --json`.
fn told_assembly(mut command: std::process::Command, compiler: &Path) -> Result<Vec<Assembly>, String> {
    let ran = command.arg("--json").output().map_err(|e| format!("cannot run {}: {e}", compiler.display()))?;
    let told: serde_json::Value = serde_json::from_slice(&ran.stdout).map_err(|_| {
        let said = String::from_utf8_lossy(&ran.stderr);
        format!("{} gave no machine code: {}", compiler.display(), said.trim())
    })?;
    if told["ok"] != true {
        return Err(format!(
            "{} could not build the code: {}",
            compiler.display(),
            told["message"].as_str().unwrap_or("its diagnostics say why")
        ));
    }
    Ok(told["functions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|function| Assembly {
            function: function["function"].as_str().unwrap_or_default().to_string(),
            size: function["size"].as_u64().unwrap_or(0) as usize,
            constants: function["constants"].as_u64().unwrap_or(0) as usize,
            instructions: function["instructions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| Instruction {
                    offset: i["offset"].as_u64().unwrap_or(0) as usize,
                    bytes: i["bytes"].as_str().unwrap_or_default().to_string(),
                    text: i["instruction"].as_str().unwrap_or_default().to_string(),
                    target: i["target"].as_str().map(str::to_string),
                    at: None,
                })
                .collect(),
            lines: Vec::new(),
            ir: None,
            rules: Vec::new(),
        })
        .collect())
}

/// The functions whose code differs from `then` to `now`, with their diffs,
/// and how many are the same.
fn assembly_changes(then: &[Assembly], now: &[Assembly]) -> (Vec<Changed>, usize) {
    let listing = |assembly: Option<&Assembly>| -> Vec<String> {
        assembly.map_or(Vec::new(), |assembly| assembly.instructions.iter().map(normalized).collect())
    };
    let mut names: Vec<&String> = now.iter().chain(then).map(|a| &a.function).collect();
    names.sort();
    names.dedup();
    let all = names.len();
    let changes: Vec<Changed> = names
        .into_iter()
        .map(|name| {
            let before = listing(then.iter().find(|a| &a.function == name));
            let after = listing(now.iter().find(|a| &a.function == name));
            (name.clone(), line_diff(&before, &after))
        })
        .filter(|(_, lines)| lines.iter().any(|line| !line.starts_with(' ')))
        .collect();
    let same = all - changes.len();
    (changes, same)
}

/// An instruction as a diff compares it: a jump without its distance, a
/// call by what it calls.
fn normalized(instruction: &Instruction) -> String {
    let mnemonic = instruction.text.split_whitespace().next().unwrap_or("");
    let jumps = mnemonic.starts_with('j')
        || mnemonic.starts_with("b.")
        || matches!(mnemonic, "b" | "bl" | "cbz" | "cbnz" | "tbz" | "tbnz" | "call");
    match (&instruction.target, jumps) {
        (Some(target), _) => format!("{mnemonic} {target}"),
        (None, true) => mnemonic.to_string(),
        (None, false) => instruction.text.clone(),
    }
}

/// The lines of a diff of two listings: what they start and end with
/// alike, and between, their longest common part.
fn line_diff(before: &[String], after: &[String]) -> Vec<String> {
    let start = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let (before_rest, after_rest) = (&before[start..], &after[start..]);
    let end = before_rest.iter().rev().zip(after_rest.iter().rev()).take_while(|(a, b)| a == b).count();
    let mut lines: Vec<String> = before[..start].iter().map(|l| format!(" {l}")).collect();
    lines.extend(common_diff(&before_rest[..before_rest.len() - end], &after_rest[..after_rest.len() - end]));
    lines.extend(before_rest[before_rest.len() - end..].iter().map(|l| format!(" {l}")));
    lines
}

/// The lines of a diff by the longest common part, or, for parts too long
/// to compare so, all of the one taken out and all of the other put in.
fn common_diff(before: &[String], after: &[String]) -> Vec<String> {
    let (n, m) = (before.len(), after.len());
    if n.saturating_mul(m) > 10_000_000 {
        let mut lines: Vec<String> = before.iter().map(|l| format!("-{l}")).collect();
        lines.extend(after.iter().map(|l| format!("+{l}")));
        return lines;
    }
    let mut common = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] =
                if before[i] == after[j] { common[i + 1][j + 1] + 1 } else { common[i + 1][j].max(common[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut lines) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && before[i] == after[j] {
            lines.push(format!(" {}", before[i]));
            (i, j) = (i + 1, j + 1);
        } else if j < m && (i == n || common[i][j + 1] >= common[i + 1][j]) {
            lines.push(format!("+{}", after[j]));
            j += 1;
        } else {
            lines.push(format!("-{}", before[i]));
            i += 1;
        }
    }
    lines
}

/// What `tondo asm --against` and `--compiler` write: each function that
/// changed since `from`, its diff with a few lines around each change, and
/// how many are the same.
pub fn assembly_changes_text(from: &str, changes: &[Changed], same: usize) -> String {
    if changes.is_empty() {
        return format!("a release build makes the same code as {from}, in {same} functions\n");
    }
    let mut out = String::new();
    for (name, lines) in changes {
        out.push_str(&format!("{name}, against {from}:\n"));
        let near =
            |at: usize| lines[at.saturating_sub(3)..(at + 4).min(lines.len())].iter().any(|l| !l.starts_with(' '));
        let mut skipped = false;
        for (at, line) in lines.iter().enumerate() {
            if near(at) {
                out.push_str(&format!("  {line}\n"));
                skipped = false;
            } else if !skipped {
                out.push_str("  ...\n");
                skipped = true;
            }
        }
    }
    let changed = match changes.len() {
        1 => "1 function changed".to_string(),
        n => format!("{n} functions changed"),
    };
    out.push_str(&format!("{changed}, {same} the same\n"));
    out
}

impl Assembly {
    /// As `tondo asm` writes it: each line of the program the code is at,
    /// with what the passes said of it, then its instructions.
    pub fn text(&self) -> String {
        let constants = match self.constants {
            0 => String::new(),
            n => format!(", {n} of them constants"),
        };
        let mut out = format!("{} ({} bytes{constants})\n", self.function, self.size);
        let mut line = None;
        for instruction in &self.instructions {
            if instruction.at.is_some() && instruction.at != line {
                line = instruction.at.clone();
                let (file, number) = instruction.at.clone().unwrap_or_default();
                let found = self.lines.iter().find(|(at, ..)| *at == (file.clone(), number));
                let code = found.map_or("", |(_, code, _)| code.as_str());
                out.push_str(&format!("{file}:{number}  {code}\n"));
                for remark in found.map_or(&[][..], |(.., remarks)| remarks.as_slice()) {
                    out.push_str(&format!("        ; {remark}\n"));
                }
            }
            let target = instruction.target.as_ref().map_or(String::new(), |name| format!("  ; {name}"));
            out.push_str(&format!("  {:>5x}  {}{target}\n", instruction.offset, instruction.text));
        }
        if let Some((before, after)) = &self.ir {
            out.push_str(&format!("\nIR before Cranelift optimizes it:\n{before}\nIR after:\n{after}"));
        }
        if !self.rules.is_empty() {
            out.push_str("\nISLE rules Cranelift applied, the most first:\n");
            for (term, place, count, tondo) in &self.rules {
                let own = if *tondo { "  Tondo" } else { "" };
                out.push_str(&format!("  {count:>6}  {place}  {term}{own}\n"));
            }
        }
        out
    }

    /// As data (toolchain 7.12).
    pub fn json(&self) -> serde_json::Value {
        let instructions: Vec<serde_json::Value> = self
            .instructions
            .iter()
            .map(|i| {
                let (file, line) = i.at.clone().map_or((None, None), |(file, line)| (Some(file), Some(line)));
                serde_json::json!({
                    "offset": i.offset, "bytes": i.bytes, "instruction": i.text, "target": i.target,
                    "file": file, "line": line,
                })
            })
            .collect();
        let lines: Vec<serde_json::Value> = self
            .lines
            .iter()
            .map(|((file, line), code, remarks)| {
                serde_json::json!({ "file": file, "line": line, "source": code, "optimizations": remarks })
            })
            .collect();
        serde_json::json!({
            "function": self.function, "size": self.size, "constants": self.constants,
            "instructions": instructions, "lines": lines,
            "ir_before": self.ir.as_ref().map(|ir| &ir.0), "ir_after": self.ir.as_ref().map(|ir| &ir.1),
            "rules": self.rules.iter().map(|(term, place, count, tondo)| {
                serde_json::json!({ "term": term, "rule": place, "count": count, "tondo": tondo })
            }).collect::<Vec<_>>(),
        })
    }
}

/// Whether this tondo can build for `target`: its own machine's, or one
/// whose pack it finds; asked before compiling, which the error saves.
pub fn ready(target: Option<Target>) -> Result<(), String> {
    match target.filter(|t| !t.native()) {
        Some(target) => target.pack(RUNTIME_IDENTITY.trim()).map(|_| ()),
        None => Ok(()),
    }
}

/// Writes the pack of this machine's target in `dir` (toolchain 7.8): what
/// a `tondo` elsewhere links a static executable for it with. Gives the
/// directory it wrote.
pub fn pack(dir: &Path) -> Result<PathBuf, String> {
    let target = Target::machine().filter(|_| !STATIC[2].1.is_empty());
    let Some(target) = target else {
        return Err("a pack holds the static runtime of a Linux target, which this tondo does not have".into());
    };
    let out = dir.join(target.name());
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    for ((name, bytes), file) in STATIC.iter().zip(crate::target::PACK) {
        let path = carried(name, bytes)?;
        std::fs::copy(&path, out.join(file)).map_err(|e| format!("cannot write {}: {e}", out.join(file).display()))?;
    }
    write_atomic(&out.join("identity"), RUNTIME_IDENTITY.as_bytes())?;
    Ok(out)
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("tondo")
}

/// FNV-1a over the given parts, as hex. Identifies cache entries only.
pub fn fingerprint(parts: &[&[u8]]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for &byte in part.iter().chain(&[0xff]) {
            hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Identity of this compiler build, for cache keys.
pub fn compiler_identity() -> String {
    let exe = std::env::current_exe().ok().and_then(|p| std::fs::metadata(p).ok());
    let stamp = exe.map(|m| format!("{}:{:?}", m.len(), m.modified().ok())).unwrap_or_default();
    let layout = layout_seed().map(|seed| format!(":layout {seed}")).unwrap_or_default();
    let sites = if logpoint_sites() { "" } else { ":no logpoint sites" };
    format!("{VERSION}:{}:{stamp}{layout}{sites}", RUNTIME_HASH)
}

/// `TONDO_LOGPOINT_SITES=0`: a build without probes leaves no place for a
/// logpoint at the start of each line, which costs the instructions of a
/// `nop` each time the line runs: a function that does little but call,
/// as a recursive one, runs a few percent faster, and `tondo logpoint`
/// finds no place in it.
pub fn logpoint_sites() -> bool {
    static SITES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SITES.get_or_init(|| std::env::var("TONDO_LOGPOINT_SITES").map_or(true, |value| value.trim() != "0"))
}

/// `TONDO_LAYOUT_SEED`: the order a build lays the program's functions out
/// in, other than the one they are compiled in, to tell how much of a
/// benchmark's time is where its code happens to lie. A build with none
/// keeps the order it always has.
pub fn layout_seed() -> Option<u64> {
    static SEED: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    if let Some(seed) = *LAYOUT.lock().unwrap_or_else(|e| e.into_inner()) {
        return seed;
    }
    *SEED.get_or_init(|| std::env::var("TONDO_LAYOUT_SEED").ok()?.trim().parse().ok())
}

/// The layout the next builds use instead of `TONDO_LAYOUT_SEED`'s, as
/// `tondo test --layouts` builds the tests in several (testing 10.4).
static LAYOUT: std::sync::Mutex<Option<Option<u64>>> = std::sync::Mutex::new(None);

/// Builds in the layout of `seed`, `Some(None)` the usual one, from now
/// on; `None`, as `TONDO_LAYOUT_SEED` says again.
pub fn set_layout_seed(seed: Option<Option<u64>>) {
    *LAYOUT.lock().unwrap_or_else(|e| e.into_inner()) = seed;
}

/// Writes `bytes` to `path` atomically.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp{}", aside()));
    std::fs::write(&temporary, bytes).map_err(|e| format!("cannot write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// What tells a file a build writes aside, to rename it into place, from
/// another build's: of another process, or of this one, as `tondo check`
/// analyzes on several threads at once.
pub fn aside() -> String {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!("{}-{}", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// What the linker receives for a library entry (see
/// `deps::library_entry`): `-lname`; the archive of `static:name` in the
/// system's library paths; a static archive; or a shared library with the
/// run-time search path set to its directory, so the executable finds it.
fn link_arguments(linker: &str, library: &str) -> Result<Vec<String>, String> {
    if let Some(name) = library.strip_prefix("static:") {
        let archive = format!("lib{name}.a");
        let found = Command::new(linker).arg(format!("-print-file-name={archive}")).output();
        let path = found.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if path.is_empty() || path == archive || !Path::new(&path).is_file() {
            return Err(format!(
                "no static library {archive} in the system's library paths; give its file instead, as \"vendor/{archive}\""
            ));
        }
        return Ok(vec![path]);
    }
    let path = Path::new(library);
    let shared = library.ends_with(".so") || library.ends_with(".dylib") || library.contains(".so.");
    if shared {
        if !path.is_file() {
            return Err(format!("the library {library} does not exist"));
        }
        let dir = path.parent().unwrap_or(Path::new("."));
        return Ok(vec![library.to_string(), format!("-Wl,-rpath,{}", dir.display())]);
    }
    if library.contains('/') || library.ends_with(".a") {
        if !path.is_file() {
            return Err(format!("the library {library} does not exist"));
        }
        return Ok(vec![library.to_string()]);
    }
    Ok(vec![format!("-l{library}")])
}

/// How an executable links: `strip` leaves out its symbol table,
/// `statically` links the C library into it, a `target` makes it for
/// another machine, statically, and a debug build's runs can be recorded
/// (toolchain 7.6).
#[derive(Clone, Copy, Default)]
pub struct Linking {
    pub strip: bool,
    pub statically: bool,
    pub target: Option<Target>,
    pub debug: bool,
}

/// Links an object file with the embedded runtime and the native
/// libraries (`-lname` or files) into an executable.
/// Whether this is a `tondo` for musl on a system without it, as an old one
/// with glibc takes: what it builds then carries its C library.
fn no_system_libc() -> bool {
    cfg!(target_env = "musl") && !Path::new(crate::link::host_interpreter()).exists()
}

/// The runtime's functions in the archive a build links, as a live build
/// lists them for its patches; read once for each runtime.
pub fn runtime_functions(statically: bool, target: Option<Target>) -> Result<Vec<String>, String> {
    let (archive, kind) = match target.filter(|t| !t.native()) {
        Some(target) => (target.pack(RUNTIME_IDENTITY.trim())?.remove(0), target.name()),
        None if statically || target.is_some() || no_system_libc() => (carried(STATIC[0].0, STATIC[0].1)?, "static"),
        None => (carried("libtondo_runtime", RUNTIME)?, "dynamic"),
    };
    let listed = cache_dir().join("live").join(format!("runtime-{RUNTIME_HASH}-{kind}.txt"));
    if let Ok(text) = std::fs::read_to_string(&listed) {
        return Ok(text.lines().map(String::from).collect());
    }
    use object::{Object, ObjectSymbol};
    let bytes = std::fs::read(&archive).map_err(|e| format!("cannot read {}: {e}", archive.display()))?;
    let parsed = object::read::archive::ArchiveFile::parse(&bytes[..]).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for member in parsed.members() {
        let member = member.map_err(|e| e.to_string())?;
        let Ok(file) = object::File::parse(member.data(&bytes[..]).map_err(|e| e.to_string())?) else { continue };
        for symbol in file.symbols() {
            let name = symbol.name().unwrap_or_default();
            let name = name.strip_prefix('_').filter(|_| file.format() == object::BinaryFormat::MachO).unwrap_or(name);
            if symbol.is_global()
                && symbol.is_definition()
                && symbol.kind() == object::SymbolKind::Text
                && let Some(rest) = name.strip_prefix("tondo_rt_")
            {
                names.push(format!("tondo_rt_{rest}"));
            }
        }
    }
    names.sort();
    names.dedup();
    let _ = std::fs::create_dir_all(listed.parent().expect("a directory"));
    let _ = write_atomic(&listed, names.join("\n").as_bytes());
    Ok(names)
}

pub fn link(object: &[u8], output: &Path, libraries: &[String], linking: Linking) -> Result<(), String> {
    let _linking = crate::times::part("link");
    if let Some(target) = linking.target.filter(|t| !t.native()) {
        let archives = target.pack(RUNTIME_IDENTITY.trim())?;
        let product = crate::link::Product::Executable {
            strip: linking.strip,
            statically: true,
            target: Some(target),
            recordable: linking.debug,
        };
        return crate::link::link(object, &archives, &[], libraries, output, product);
    }
    if linking.statically || linking.target.is_some() || no_system_libc() {
        if STATIC[2].1.is_empty() {
            let why = if cfg!(target_os = "linux") {
                "this tondo was built without Rust's musl target, which a static executable needs"
            } else {
                "a static executable is for Linux: this system has no static C library; build without --static"
            };
            return Err(why.into());
        }
        let archives = STATIC.iter().map(|(name, bytes)| carried(name, bytes)).collect::<Result<Vec<_>, _>>()?;
        let product = crate::link::Product::Executable {
            strip: linking.strip,
            statically: true,
            target: None,
            recordable: linking.debug,
        };
        return crate::link::link(object, &archives, &[], libraries, output, product);
    }
    let runtime = carried("libtondo_runtime", RUNTIME)?;
    let mut runtime_args: Vec<String> = RUNTIME_LINK_ARGS.split_whitespace().map(String::from).collect();
    if !UNWIND.is_empty() {
        let unwind = carried("libunwind", UNWIND)?;
        for arg in &mut runtime_args {
            if arg == "-lunwind" {
                *arg = unwind.display().to_string();
            }
        }
    }
    // `TONDO_CC` links with the system's C compiler instead.
    let Ok(linker) = std::env::var("TONDO_CC") else {
        let mut archives = vec![runtime];
        let unwind = runtime_args.iter().position(|a| a.ends_with(".a")).map(|i| runtime_args.remove(i));
        archives.extend(unwind.map(PathBuf::from));
        let product = crate::link::Product::Executable {
            strip: linking.strip,
            statically: false,
            target: None,
            recordable: linking.debug,
        };
        return crate::link::link(object, &archives, &runtime_args, libraries, output, product);
    };
    let dir = cache_dir().join("runtime");
    let object_path = dir.join(format!("object-{}.o", aside()));
    std::fs::write(&object_path, object).map_err(|e| format!("cannot write {}: {e}", object_path.display()))?;
    let mut native = Vec::new();
    for library in libraries {
        native.extend(link_arguments(&linker, library)?);
    }
    let result = Command::new(&linker)
        .arg("-o")
        .arg(output)
        .arg(&object_path)
        .args(native)
        .arg(&runtime)
        .args(&runtime_args)
        // Unused runtime code and the standard library's debug info would
        // otherwise make up most of every executable.
        .args(if cfg!(target_os = "macos") {
            ["-Wl,-dead_strip", "-Wl,-S"]
        } else {
            ["-Wl,--gc-sections", "-Wl,--strip-debug"]
        })
        .args(linking.strip.then_some(if cfg!(target_os = "macos") { "-Wl,-x" } else { "-Wl,--strip-all" }))
        .output();
    let _ = std::fs::remove_file(&object_path);
    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // A C function no linked library defines.
            let mut missing: Vec<&str> = stderr
                .lines()
                .filter_map(|line| line.split("undefined reference to `").nth(1)?.split('\'').next())
                .collect();
            missing.dedup();
            if missing.is_empty() {
                return Err(format!("linking failed:\n{stderr}"));
            }
            let names: Vec<String> = missing.iter().map(|m| format!("`{m}`")).collect();
            Err(format!(
                "no linked library defines the C function {}: add its library to `[native] libraries` in tondo.toml, or pass `--link <library>` to `tondo run` or `tondo build`",
                names.join(", ")
            ))
        }
        Err(e) => {
            Err(format!("cannot run the linker `{linker}`: {e}; install it, or unset TONDO_CC to link without one"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    /// Two long listings alike show no change, and one instruction changed
    /// in the middle of them shows as that one alone.
    #[test]
    fn diffs_long_listings_by_what_changed() {
        let listing: Vec<String> = (0..12_000).map(|i| format!("add rax, {i}")).collect();
        let changed =
            |lines: &[String]| lines.iter().filter(|line| !line.starts_with(' ')).cloned().collect::<Vec<_>>();
        assert!(changed(&super::line_diff(&listing, &listing)).is_empty());
        let mut other = listing.clone();
        other[6_000] = "sub rax, 1".to_string();
        let diff = super::line_diff(&listing, &other);
        assert_eq!(changed(&diff), ["+sub rax, 1", "-add rax, 6000"]);
        assert_eq!(diff.len(), 12_001);
    }

    #[test]
    fn sweeps_entries_unused_for_30_days() {
        let dir = std::env::temp_dir().join(format!("tondo-sweep-{}", std::process::id()));
        let now = SystemTime::now();
        for (name, days) in [("old", 31), ("recent", 29)] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            let file = std::fs::File::open(dir.join(name)).unwrap();
            file.set_modified(now - Duration::from_secs(days * 24 * 60 * 60)).unwrap();
        }
        super::sweep(&dir, now);
        assert!(!dir.join("old").exists() && dir.join("recent").exists());
        // Once a day at most.
        std::fs::File::open(dir.join("recent")).unwrap().set_modified(now - Duration::from_secs(40 * 86400)).unwrap();
        super::sweep(&dir, now);
        assert!(dir.join("recent").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Threads that write the same file at once, as two analyses extracting
    /// the runtime, each write it whole.
    #[test]
    fn threads_write_one_file_at_once() {
        let dir = std::env::temp_dir().join(format!("tondo-aside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.a");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        super::write_atomic(&path, &[7; 4096]).unwrap();
                    }
                });
            }
        });
        assert_eq!(std::fs::read(&path).unwrap(), [7; 4096]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A runtime of another compiler that a build used today stays, as one
    /// of the editor's while the terminal's builds; an unused one goes.
    #[test]
    fn keeps_the_runtimes_other_compilers_use() {
        let dir = std::env::temp_dir().join(format!("tondo-runtimes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        let runtime = |hash: &str| dir.join(format!("libtondo_runtime-{hash}.a"));
        for (hash, hours) in [("00000000000000aa", 2), ("00000000000000bb", 30), ("00000000000000cc", 30)] {
            std::fs::write(runtime(hash), b"").unwrap();
            let file = std::fs::File::open(runtime(hash)).unwrap();
            file.set_modified(now - Duration::from_secs(hours * 60 * 60)).unwrap();
        }
        super::forget_others(&dir, "libtondo_runtime", "00000000000000cc", now);
        assert!(runtime("00000000000000aa").exists(), "used two hours ago");
        assert!(!runtime("00000000000000bb").exists(), "unused for a day");
        assert!(runtime("00000000000000cc").exists(), "its own");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
