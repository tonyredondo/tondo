//! `std.embed` (standard library 14.27): files of the package that a program
//! keeps in its executable, read when it compiles. The checker reads them for
//! `embed.text`, `embed.bytes` and `embed.files`; the build's inputs,
//! `--watch` and `tondo graph` find them in the sources without compiling.

use crate::diag::Diagnostics;
use crate::lexer::{StrPart, Tok};
use crate::source::SourceMap;
use std::path::{Path, PathBuf};

/// What a call of `std.embed` reads.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Text,
    Bytes,
    Files,
}

impl Kind {
    pub fn of(name: &str) -> Option<Kind> {
        match name {
            "text" => Some(Kind::Text),
            "bytes" => Some(Kind::Bytes),
            "files" => Some(Kind::Files),
            _ => None,
        }
    }
}

/// What a call reads: a file's text or bytes, or the files under a
/// directory by their paths under it.
pub enum Read {
    Text(String),
    Bytes(Vec<u8>),
    Files(Vec<(String, Vec<u8>)>),
}

/// Reads what `embed.<kind>(written)` names from the file `source`, or says
/// why it cannot and what to write.
pub fn read(source: &Path, written: &str, kind: Kind) -> Result<Read, String> {
    let target = resolve(source, written)?;
    let root = package(source);
    let shown = |path: &Path| shown(&root, path);
    match kind {
        Kind::Files if !target.is_dir() => Err(format!(
            "`{written}` is a file: `embed.bytes(\"{written}\")` or `embed.text(\"{written}\")` reads it, and `embed.files` a directory"
        )),
        Kind::Files => {
            let mut files = Vec::new();
            for (name, path) in walk(&target) {
                let bytes = std::fs::read(&path).map_err(|e| format!("cannot read `{}`: {e}", shown(&path)))?;
                files.push((name, bytes));
            }
            Ok(Read::Files(files))
        }
        _ if target.is_dir() => {
            Err(format!("`{written}` is a directory: `embed.files(\"{written}\")` reads the files in it"))
        }
        Kind::Bytes => {
            std::fs::read(&target).map(Read::Bytes).map_err(|e| format!("cannot read `{}`: {e}", shown(&target)))
        }
        Kind::Text => {
            let bytes = std::fs::read(&target).map_err(|e| format!("cannot read `{}`: {e}", shown(&target)))?;
            String::from_utf8(bytes)
                .map(Read::Text)
                .map_err(|_| format!("`{written}` is not UTF-8 text: `embed.bytes(\"{written}\")` gives its bytes"))
        }
    }
}

/// The file or directory a path written in `source` names: relative to the
/// directory of `source`, with `/`, and inside its package.
pub fn resolve(source: &Path, written: &str) -> Result<PathBuf, String> {
    if written.is_empty() || written.starts_with('/') || written.contains('\\') {
        return Err(format!(
            "`{written}` is not a path relative to this file: write it from the file's directory with `/`, as `\"static/index.html\"`"
        ));
    }
    let root = package(source);
    let dir = source.parent().unwrap_or(Path::new("."));
    let Ok(found) = dir.join(written).canonicalize() else {
        // The deepest directory of the path there is, and what it has.
        let mut there = dir.to_path_buf();
        for part in written.split('/') {
            let next = there.join(part);
            if !next.is_dir() {
                break;
            }
            there = next;
        }
        let mut names: Vec<String> = std::fs::read_dir(&there)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().into_string().ok()?;
                        let slash = if e.path().is_dir() { "/" } else { "" };
                        (!name.starts_with('.')).then(|| format!("`{name}{slash}`"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        let has = match names.len() {
            0 => "nothing".to_string(),
            1..=12 => names.join(", "),
            n => format!("{} and {} more", names[..12].join(", "), n - 12),
        };
        let place = shown(&root, &there);
        return Err(format!("there is no `{written}` next to this file: `{place}` has {has}"));
    };
    let inside = root.canonicalize().is_ok_and(|root| found.starts_with(root));
    if !inside {
        return Err(format!(
            "`{written}` is outside the package in `{}`: a program embeds the files of its own package",
            root.display()
        ));
    }
    Ok(found)
}

/// The directory of the package a source file is in: the nearest one up
/// with `tondo.toml` or `src/main.to`, or the file's own.
fn package(source: &Path) -> PathBuf {
    let source = source.canonicalize().unwrap_or_else(|_| source.to_path_buf());
    let ancestors = || source.ancestors().skip(1);
    ancestors()
        .find(|dir| dir.join("tondo.toml").is_file())
        .or_else(|| ancestors().find(|dir| dir.join("src").join("main.to").is_file()))
        .or_else(|| source.parent())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// A path as the package names it: relative to its directory.
fn shown(root: &Path, path: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match path.strip_prefix(&root) {
        Ok(relative) if relative.as_os_str().is_empty() => {
            root.file_name().map_or_else(|| ".".to_string(), |name| name.to_string_lossy().into_owned())
        }
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// The files under `dir`, by their paths under it with `/`, in order, but
/// those whose name, or a directory's on the way, starts with `.`.
pub fn walk(dir: &Path) -> Vec<(String, PathBuf)> {
    fn visit(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            let path = entry.path();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                visit(&path, &format!("{prefix}{name}/"), out);
            } else if path.is_file() {
                out.push((format!("{prefix}{name}"), path));
            }
        }
    }
    let mut out = Vec::new();
    visit(dir, "", &mut out);
    out.sort();
    out
}

/// The calls of `std.embed` in a source file whose path is a literal: what
/// each reads, its path as written and its line, from one.
pub fn calls(path: &Path, text: &str) -> Vec<(Kind, String, usize)> {
    // The names the file imports `std.embed` under.
    let aliases: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("import std.embed")?;
            match rest.trim() {
                "" => Some("embed"),
                rest => rest.strip_prefix("as ").map(str::trim),
            }
        })
        .collect();
    if aliases.is_empty() {
        return Vec::new();
    }
    let mut sources = SourceMap::default();
    let file = sources.add(path, text.to_string());
    let file = sources.file(file);
    let tokens = crate::lexer::lex(file, None, &mut Diagnostics::default());
    let mut found = Vec::new();
    for window in tokens.windows(5) {
        let [module, dot, name, open, literal] = window else { continue };
        let (Tok::Ident(module), Tok::Dot, Tok::Ident(name), Tok::LParen, Tok::Str(parts)) =
            (&module.tok, &dot.tok, &name.tok, &open.tok, &literal.tok)
        else {
            continue;
        };
        let (Some(kind), [StrPart::Text(written)]) = (Kind::of(name), parts.as_slice()) else { continue };
        if aliases.contains(&module.as_str()) {
            found.push((kind, written.clone(), file.line_col(literal.span.start).0));
        }
    }
    found
}

/// The files a source file embeds, that a build reads: each it names, and
/// those under each directory it names.
pub fn inputs(path: &Path, text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (_, written, _) in calls(path, text) {
        match resolve(path, &written) {
            Ok(found) if found.is_dir() => out.extend(walk(&found).into_iter().map(|(_, file)| file)),
            Ok(found) => out.push(found),
            Err(_) => {}
        }
    }
    out
}
