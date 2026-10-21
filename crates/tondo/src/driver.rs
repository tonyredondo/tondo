//! Compilation pipeline, native linking and the build cache.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use crate::{ast, check, codegen, deps, hir, lower, modules, mono, parser, project};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libtondo_runtime.a"));
const RUNTIME_LINK_ARGS: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-link-args.txt"));
const RUNTIME_HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-hash.txt"));

pub struct Analysis {
    pub sources: SourceMap,
    pub diags: Diagnostics,
    pub program: Option<hir::Program>,
    /// Who owns the diagnostics (22.3): the package or the single file.
    pub source_id: String,
}

/// Reads, parses and checks a program: a project directory (toolchain 3)
/// or a single source file.
pub fn analyze(path: &Path) -> Result<Analysis, String> {
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let mut source_id = format!("root:{}", path.file_name().unwrap_or_default().to_string_lossy());
    let module = if path.is_dir() {
        let packages = deps::resolve(path, None)?;
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
        if diags.has_errors() {
            diags.sort(&sources);
            return Ok(Analysis { sources, diags, program: None, source_id });
        }
        let packages: Vec<modules::PackageInfo> = packages
            .into_iter()
            .map(|p| modules::PackageInfo { name: p.name, dependencies: p.dependencies.into_iter().collect() })
            .collect();
        modules::link(modules, root, &packages, &mut diags)
    } else {
        match parse(path, &mut sources, &mut diags)? {
            Some(module) => module,
            None => return Ok(Analysis { sources, diags, program: None, source_id }),
        }
    };
    let program = if diags.has_errors() { None } else { check::check(&module, &sources, &mut diags) };
    let program = program.and_then(|program| mono::monomorphize(program, &mut diags));
    diags.sort(&sources);
    Ok(Analysis { sources, diags, program, source_id })
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

/// Links an object file with the embedded runtime into an executable.
pub fn link(object: &[u8], output: &Path) -> Result<(), String> {
    let dir = cache_dir().join("runtime");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let runtime = dir.join(format!("libtondo_runtime-{RUNTIME_HASH}.a"));
    if !runtime.exists() {
        write_atomic(&runtime, RUNTIME)?;
    }
    let object_path = dir.join(format!("object-{}.o", std::process::id()));
    std::fs::write(&object_path, object).map_err(|e| format!("cannot write {}: {e}", object_path.display()))?;
    let linker = std::env::var("TONDO_CC").unwrap_or_else(|_| "cc".into());
    let result = Command::new(&linker)
        .arg("-o")
        .arg(output)
        .arg(&object_path)
        .arg(&runtime)
        .args(RUNTIME_LINK_ARGS.split_whitespace())
        // Unused runtime code and the standard library's debug info would
        // otherwise make up most of every executable.
        .args(["-Wl,--gc-sections", "-Wl,--strip-debug"])
        .output();
    let _ = std::fs::remove_file(&object_path);
    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!("linking failed:\n{}", String::from_utf8_lossy(&out.stderr))),
        Err(e) => Err(format!("cannot run the linker `{linker}`: {e}; install a C toolchain or set TONDO_CC")),
    }
}
