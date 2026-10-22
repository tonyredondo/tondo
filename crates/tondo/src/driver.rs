//! Compilation pipeline, native linking and the build cache.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use crate::{ast, check, codegen, deps, hir, lower, modules, mono, parser, project, stdlib};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libtondo_runtime.a"));
const RUNTIME_LINK_ARGS: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-link-args.txt"));
const RUNTIME_HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-hash.txt"));
/// Rust's libunwind for musl targets, which the runtime needs there; empty
/// elsewhere.
const UNWIND: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libunwind.a"));

pub struct Analysis {
    pub sources: SourceMap,
    pub diags: Diagnostics,
    pub program: Option<hir::Program>,
    /// Who owns the diagnostics (22.3): the package or the single file.
    pub source_id: String,
    /// The native libraries the packages link (`[native] libraries`).
    pub libraries: Vec<String>,
}

/// Reads, parses and checks a program: a project directory (toolchain 3)
/// or a single source file.
pub fn analyze(path: &Path) -> Result<Analysis, String> {
    analyze_build(path, false)
}

/// Reads, parses and checks the test build of a project (testing 5): its
/// sources, its unit test companions and its integration test roots. The
/// sources are checked first on their own, so tests cannot make code that
/// is wrong without them build.
pub fn analyze_tests(path: &Path) -> Result<Analysis, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a project: `tondo test` runs the tests of a project directory", path.display()));
    }
    let production = analyze(path)?;
    if production.diags.has_errors() {
        return Ok(production);
    }
    analyze_build(path, true)
}

fn analyze_build(path: &Path, tests: bool) -> Result<Analysis, String> {
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let mut source_id = format!("root:{}", path.file_name().unwrap_or_default().to_string_lossy());
    let mut libraries: Vec<String> = Vec::new();
    let module = if path.is_dir() {
        let packages = deps::resolve(path, None)?;
        for library in packages.iter().flat_map(|p| &p.libraries) {
            if !libraries.contains(library) {
                libraries.push(library.clone());
            }
        }
        source_id = format!("pkg:{}", packages[0].name);
        let mut modules: Vec<modules::ModuleSource> = Vec::new();
        let mut root = 0;
        for (package_index, package) in packages.iter().enumerate() {
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
        if diags.has_errors() {
            diags.sort(&sources);
            return Ok(Analysis { sources, diags, program: None, source_id, libraries });
        }
        let mut packages: Vec<modules::PackageInfo> = packages
            .into_iter()
            .map(|p| modules::PackageInfo { name: p.name, dependencies: p.dependencies.into_iter().collect() })
            .collect();
        if tests {
            let dependencies = [(tested.clone(), 0)].into_iter().collect();
            packages.push(modules::PackageInfo { name: format!("{tested}_tests"), dependencies });
        }
        stdlib::derive_json(&mut modules, &mut sources, &mut diags);
        stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
        let mut module = modules::link(modules, root, &packages, &mut diags);
        module.test_package = tests.then_some(tested);
        module
    } else {
        let module = match parse(path, &mut sources, &mut diags)? {
            Some(module) => module,
            None => return Ok(Analysis { sources, diags, program: None, source_id, libraries }),
        };
        if stdlib::needed(&module) {
            // The file links with the standard modules it imports.
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let mut modules =
                vec![modules::ModuleSource { path: vec![name.clone()], package: 0, files: vec![(module, true)] }];
            let mut packages = vec![modules::PackageInfo { name, dependencies: Default::default() }];
            stdlib::derive_json(&mut modules, &mut sources, &mut diags);
            stdlib::add(&mut modules, &mut packages, &mut sources, &mut diags);
            modules::link(modules, 0, &packages, &mut diags)
        } else {
            module
        }
    };
    let program = if diags.has_errors() { None } else { check::check(&module, &sources, &mut diags) };
    let program = program.and_then(|program| mono::monomorphize(program, &mut diags));
    diags.sort(&sources);
    Ok(Analysis { sources, diags, program, source_id, libraries })
}

/// Reads and parses one source file; `None` when it cannot be lexed.
fn parse(path: &Path, sources: &mut SourceMap, diags: &mut Diagnostics) -> Result<Option<ast::Module>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
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
    }
    Ok(inputs)
}

/// Lowers a checked program to a native object file.
pub fn object(analysis: &Analysis) -> Result<Vec<u8>, String> {
    let program = analysis.program.as_ref().expect("object() requires a checked program");
    let bodies = lower::lower(program);
    codegen::compile(program, &bodies, &analysis.sources)
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

/// Links an object file with the embedded runtime and the native
/// libraries (`-lname` or files) into an executable.
pub fn link(object: &[u8], output: &Path, libraries: &[String]) -> Result<(), String> {
    let dir = cache_dir().join("runtime");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let runtime = dir.join(format!("libtondo_runtime-{RUNTIME_HASH}.a"));
    if !runtime.exists() {
        write_atomic(&runtime, RUNTIME)?;
    }
    let object_path = dir.join(format!("object-{}.o", std::process::id()));
    std::fs::write(&object_path, object).map_err(|e| format!("cannot write {}: {e}", object_path.display()))?;
    let linker = std::env::var("TONDO_CC").unwrap_or_else(|_| "cc".into());
    let mut runtime_args: Vec<String> = RUNTIME_LINK_ARGS.split_whitespace().map(String::from).collect();
    if !UNWIND.is_empty() {
        let unwind = dir.join(format!("libunwind-{RUNTIME_HASH}.a"));
        if !unwind.exists() {
            write_atomic(&unwind, UNWIND)?;
        }
        for arg in &mut runtime_args {
            if arg == "-lunwind" {
                *arg = unwind.display().to_string();
            }
        }
    }
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
        Err(e) => Err(format!("cannot run the linker `{linker}`: {e}; install a C toolchain or set TONDO_CC")),
    }
}
