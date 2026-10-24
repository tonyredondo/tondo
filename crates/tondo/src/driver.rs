//! Compilation pipeline, native linking and the build cache.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use crate::{ast, check, codegen, deps, hir, lower, modules, mono, parser, project, stdlib};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
// The archives the compiler carries, compressed; each is written to the
// cache the first time a program links.
const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libtondo_runtime.a.gz"));
const RUNTIME_LINK_ARGS: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-link-args.txt"));
const RUNTIME_HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-hash.txt"));
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

/// The archive `name` of the compiler, in the cache.
fn carried(name: &str, compressed: &[u8]) -> Result<PathBuf, String> {
    let dir = cache_dir().join("runtime");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}-{RUNTIME_HASH}.a"));
    if !path.exists() {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(compressed), &mut bytes)
            .map_err(|e| format!("the compiler's {name} is damaged: {e}"))?;
        write_atomic(&path, &bytes)?;
        // Those of other compilers go: a compiler writes its own again.
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let file = entry.file_name();
            let hash = file.to_str().and_then(|f| f.strip_prefix(name)?.strip_prefix('-')?.strip_suffix(".a"));
            if hash.is_some_and(|h| h.len() == 16 && h != RUNTIME_HASH && h.bytes().all(|b| b.is_ascii_hexdigit())) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(path)
}

/// The cache entry `key` of `kind` (`run`, `test` or `meta`), which holds
/// what a build can make again: using it marks it, and an entry no one
/// used in 30 days is removed, looking at most once a day.
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
}

thread_local! {
    /// The text of the files an editor has open, which may not be saved.
    static OPEN: std::cell::RefCell<std::collections::HashMap<PathBuf, String>> = Default::default();
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
    let analysis = analyze_with(path, tests, false, true, false);
    OPEN.with(|o| o.borrow_mut().clear());
    analysis
}

/// Reads, parses and checks a program: a project directory (toolchain 3)
/// or a single source file.
pub fn analyze(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, false, false, false)
}

/// Reads, parses and checks a library for C (toolchain 7): without `main`,
/// it publishes `pub extern fn`s.
pub fn analyze_library(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, false, false, true)
}

/// Reads, parses and checks the test build of a project (testing 5): its
/// sources, its unit test companions and its integration test roots. The
/// sources are checked first on their own, as a program or as a library,
/// so tests cannot make code that is wrong without them build.
pub fn analyze_tests(path: &Path, library: bool) -> Result<Analysis, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a project: `tondo test` runs the tests of a project directory", path.display()));
    }
    let production = analyze_with(path, false, false, false, library)?;
    if production.diags.has_errors() {
        return Ok(production);
    }
    analyze_with(path, true, false, false, false)
}

/// Reads, parses and checks a provider for the `meta` profile (27.7).
pub fn analyze_meta(path: &Path) -> Result<Analysis, String> {
    analyze_with(path, false, true, false, false)
}

fn analyze_with(path: &Path, tests: bool, meta: bool, editor: bool, library: bool) -> Result<Analysis, String> {
    let mut expansions = Vec::new();
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let mut source_id = format!("root:{}", path.file_name().unwrap_or_default().to_string_lossy());
    let mut libraries: Vec<String> = Vec::new();
    // The dependencies, whose warnings their authors see, not this program's.
    let mut dependencies: Vec<String> = Vec::new();
    let module = if path.is_dir() {
        let packages = deps::resolve(path, None)?;
        dependencies = packages[1..].iter().map(|p| p.name.clone()).collect();
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
            let (files, root_file) = project::sources(&package.dir, &package.name)?;
            for (index, file) in files.iter().enumerate() {
                let parsed = parse(&file.path, &mut sources, &mut diags)?;
                // A dependency's files are shown under its package name.
                let relative = file.path.strip_prefix(&package.dir).unwrap_or(&file.path);
                let mut logical: Vec<String> =
                    relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if package_index > 0 {
                    logical.insert(0, package.name.clone());
                }
                sources.set_identity(sources.len() - 1, logical.join("/"), file.module.join("."));
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
        }
        let tested = packages[0].name.clone();
        if tests {
            // Companions join the module of their directory; each root
            // under `tests/` is a module of a package that depends on the
            // tested one.
            let (companions, roots) = project::test_sources(&packages[0].dir, &tested)?;
            for file in companions {
                let parsed = parse(&file.path, &mut sources, &mut diags)?;
                let relative = file.path.strip_prefix(&packages[0].dir).unwrap_or(&file.path);
                sources.set_identity(sources.len() - 1, relative.display().to_string(), file.module.join("."));
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
            for file in roots {
                let parsed = parse(&file.path, &mut sources, &mut diags)?;
                let relative = file.path.strip_prefix(&packages[0].dir).unwrap_or(&file.path);
                sources.set_identity(sources.len() - 1, relative.display().to_string(), file.module.join("."));
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
            return Ok(Analysis { sources, diags, program: None, source_id, libraries, meta, expansions, index });
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
        expansions = crate::meta::generate(&mut modules, &packages, &generators, &mut sources, &mut diags);
        expansions.extend(crate::meta::expand(&mut modules, &packages, &providers, &mut sources, &mut diags));
        crate::derive::derive(&mut modules, &mut sources, &mut diags);
        stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
        let mut module = modules::link(modules, root, &packages, &mut diags);
        module.test_package = tests.then_some(tested);
        module
    } else {
        let module = match parse(path, &mut sources, &mut diags)? {
            Some(module) => module,
            None => {
                let index = None;
                return Ok(Analysis { sources, diags, program: None, source_id, libraries, meta, expansions, index });
            }
        };
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
            expansions = crate::meta::expand(&mut modules, &packages, &[], &mut sources, &mut diags);
            crate::derive::derive(&mut modules, &mut sources, &mut diags);
            stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
            modules::link(modules, 0, &packages, &mut diags)
        } else {
            module
        }
    };
    if editor {
        // After errors that stop a build, checking still finds what names
        // refer to; its own errors would only follow from them.
        let earlier = diags.has_errors();
        let mut checked = Diagnostics::default();
        let index = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check::check_indexed(&module, &sources, &mut checked).1
        }))
        .ok();
        if !earlier {
            diags.list.append(&mut checked.list);
        }
        drop_dependency_warnings(&mut diags, &sources, &dependencies);
        diags.sort(&sources);
        return Ok(Analysis { sources, diags, program: None, source_id, libraries, meta, expansions, index });
    }
    let mut module = module;
    module.library |= library;
    module.c_library = library;
    let program = if diags.has_errors() { None } else { check::check(&module, &sources, &mut diags, meta) };
    let program = program.and_then(|program| mono::monomorphize(program, &mut diags));
    drop_dependency_warnings(&mut diags, &sources, &dependencies);
    diags.sort(&sources);
    Ok(Analysis { sources, diags, program, source_id, libraries, meta, expansions, index: None })
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
fn parse(path: &Path, sources: &mut SourceMap, diags: &mut Diagnostics) -> Result<Option<ast::Module>, String> {
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
            diags.error("E0001", Span::new(base + offset, base + offset + 1), "invalid UTF-8 in source file");
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
    Ok(Some(parser::parse_file(sources.file(file), diags)))
}

/// The source files a build of `path` reads, for cache keys.
pub fn inputs(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut inputs = Vec::new();
    for package in deps::resolve(path, None)? {
        let (files, _) = project::sources(&package.dir, &package.name)?;
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
    let mut inputs = inputs(path)?;
    let (companions, roots) = project::test_sources(path, "")?;
    inputs.extend(companions.into_iter().chain(roots).map(|f| f.path));
    Ok(inputs)
}

/// Writes a library for C (toolchain 7) into `dir`: `lib<name>.a`, with
/// the program's object and the runtime, and `<name>.h`, which declares
/// what it publishes. Gives how a C program links it.
pub fn library(analysis: &Analysis, object: &[u8], dir: &Path, name: &str) -> Result<String, String> {
    let program = analysis.program.as_ref().expect("library() requires a checked program");
    let archive = format!("lib{name}.a");
    let mut native: Vec<String> = Vec::new();
    for library in &analysis.libraries {
        native.extend(
            match library.strip_prefix("static:") {
                Some(name) => format!("-l{name}"),
                None if library.contains('/') => library.clone(),
                None => format!("-l{library}"),
            }
            .split(' ')
            .map(str::to_string),
        );
    }
    let strip = if cfg!(target_os = "macos") { "-Wl,-dead_strip" } else { "-Wl,--gc-sections" };
    let linking = format!(
        "Link it: cc app.c {} {strip} {}",
        dir.join(&archive).display(),
        native.iter().map(String::as_str).chain(RUNTIME_LINK_ARGS.split_whitespace()).collect::<Vec<_>>().join(" ")
    );
    let path = carried("libtondo_runtime", RUNTIME)?;
    let runtime = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let parsed = object::read::archive::ArchiveFile::parse(&runtime[..]).map_err(|e| e.to_string())?;
    let mut members = vec![(format!("{name}.o"), object)];
    for member in parsed.members() {
        let member = member.map_err(|e| e.to_string())?;
        let data = member.data(&runtime[..]).map_err(|e| e.to_string())?;
        members.push((String::from_utf8_lossy(member.name()).into_owned(), data));
    }
    let bytes = crate::link::archive(&members, cfg!(target_os = "macos"));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    write_atomic(&dir.join(&archive), &bytes)?;
    write_atomic(&dir.join(format!("{name}.h")), crate::header::header(name, &program.c_api, &linking).as_bytes())?;
    Ok(linking)
}

/// Lowers a checked program to a native object file; a debug build has
/// what a debugger needs (toolchain 7.6) and is not optimized.
pub fn object(analysis: &Analysis, debug: bool) -> Result<Vec<u8>, String> {
    let program = analysis.program.as_ref().expect("object() requires a checked program");
    let bodies = lower::lower(program);
    codegen::compile(program, &bodies, &analysis.sources, analysis.meta, debug)
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
    format!("{VERSION}:{}:{stamp}", RUNTIME_HASH)
}

/// Writes `bytes` to `path` atomically.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|e| format!("cannot write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
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

/// How an executable links: `strip` leaves out its symbol table, and
/// `statically` links the C library into it.
#[derive(Clone, Copy, Default)]
pub struct Linking {
    pub strip: bool,
    pub statically: bool,
}

/// Links an object file with the embedded runtime and the native
/// libraries (`-lname` or files) into an executable.
pub fn link(object: &[u8], output: &Path, libraries: &[String], linking: Linking) -> Result<(), String> {
    if linking.statically {
        if STATIC[2].1.is_empty() {
            let why = if cfg!(target_os = "linux") {
                "this tondo was built without Rust's musl target, which a static executable needs"
            } else {
                "a static executable is for Linux: this system has no static C library; build without --static"
            };
            return Err(why.into());
        }
        let archives = STATIC.iter().map(|(name, bytes)| carried(name, bytes)).collect::<Result<Vec<_>, _>>()?;
        return crate::link::link(object, &archives, &[], libraries, output, linking.strip, true);
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
        return crate::link::link(object, &archives, &runtime_args, libraries, output, linking.strip, false);
    };
    let dir = cache_dir().join("runtime");
    let object_path = dir.join(format!("object-{}.o", std::process::id()));
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
}
