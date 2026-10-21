//! Compilation pipeline, native linking and the build cache.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use crate::{ast, check, codegen, hir, lower, modules, mono, parser, project};
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
}

/// Reads, parses and checks a program: a project directory (toolchain 3)
/// or a single source file.
pub fn analyze(path: &Path) -> Result<Analysis, String> {
    let mut sources = SourceMap::default();
    let mut diags = Diagnostics::default();
    let module = if path.is_dir() {
        let project = project::discover(path)?;
        let mut modules: Vec<modules::ModuleSource> = Vec::new();
        let mut root = 0;
        for (index, file) in project.files.iter().enumerate() {
            let Some(parsed) = parse(&file.path, &mut sources, &mut diags)? else { continue };
            let position = match modules.iter().position(|m| m.path == file.module) {
                Some(position) => position,
                None => {
                    modules.push(modules::ModuleSource { path: file.module.clone(), files: Vec::new() });
                    modules.len() - 1
                }
            };
            if index == project.root {
                root = position;
            }
            modules[position].files.push((parsed, index == project.root));
        }
        if diags.has_errors() {
            return Ok(Analysis { sources, diags, program: None });
        }
        modules::link(modules, root, &mut diags)
    } else {
        match parse(path, &mut sources, &mut diags)? {
            Some(module) => module,
            None => return Ok(Analysis { sources, diags, program: None }),
        }
    };
    let program = if diags.has_errors() { None } else { check::check(&module, &sources, &mut diags) };
    let program = program.and_then(|program| mono::monomorphize(program, &mut diags));
    Ok(Analysis { sources, diags, program })
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
    let project = project::discover(path)?;
    let mut inputs: Vec<PathBuf> = project.files.into_iter().map(|f| f.path).collect();
    let manifest = path.join("tondo.toml");
    if manifest.is_file() {
        inputs.push(manifest);
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
