//! Standard library modules written in Tondo, which the compiler carries
//! and links into a program that imports them, as any other module of a
//! package named `std` (standard library 12.1).

use crate::diag::Diagnostics;
use crate::modules::{ModuleSource, PackageInfo};
use crate::source::SourceMap;
use crate::{ast, parser};
use std::path::Path;

/// A file of the standard library, by its path in `std/` without `.to`.
macro_rules! file {
    ($path:literal) => {
        ($path, include_str!(concat!("../std/", $path, ".to")))
    };
}

/// Each module, by its path after `std`, and its files, by their path in
/// `std/` and their source: one file of its name, or the files of its
/// directory, which form one module as a package's do (language 6.1), the
/// one of its name first, with what the module is for.
const SOURCES: &[(&str, &[(&str, &str)])] = &[
    ("serialization", &[file!("serialization")]),
    ("json", &[file!("json")]),
    ("toml", &[file!("toml")]),
    ("yaml", &[file!("yaml")]),
    ("messagepack", &[file!("messagepack")]),
    ("cbor", &[file!("cbor")]),
    (
        "protobuf",
        &[
            file!("protobuf/protobuf"),
            file!("protobuf/wire"),
            file!("protobuf/wellknown"),
            file!("protobuf/reflection"),
            file!("protobuf/json"),
        ],
    ),
    (
        "grpc",
        &[
            file!("grpc/grpc"),
            file!("grpc/server"),
            file!("grpc/connect"),
            file!("grpc/client"),
            file!("grpc/health"),
            file!("grpc/reflection"),
        ],
    ),
    ("net", &[file!("net")]),
    ("ffi", &[file!("ffi")]),
    ("regex", &[file!("regex")]),
    ("reflect", &[file!("reflect")]),
    ("prelude", &[file!("prelude")]),
    ("panics", &[file!("panics")]),
    ("math", &[file!("math")]),
    ("meta", &[file!("meta")]),
    ("debug", &[file!("debug")]),
    ("encoding", &[file!("encoding")]),
    ("hash", &[file!("hash")]),
    ("crypto", &[file!("crypto")]),
    ("random", &[file!("random")]),
    ("format", &[file!("format")]),
    ("uuid", &[file!("uuid")]),
    ("datetime", &[file!("datetime")]),
    ("csv", &[file!("csv")]),
    ("log", &[file!("log")]),
    (
        "http",
        &[
            file!("http/http"),
            file!("http/client"),
            file!("http/websockets"),
            file!("http/requests"),
            file!("http/replies"),
            file!("http/cookies"),
            file!("http/app"),
            file!("http/openapi"),
            file!("http/server"),
        ],
    ),
    ("executor", &[file!("executor")]),
    ("diagnostics", &[file!("diagnostics")]),
    ("profile", &[file!("profile")]),
    ("embed", &[file!("embed")]),
    ("url", &[file!("url")]),
    ("zoned", &[file!("zoned")]),
    ("timetext", &[file!("timetext")]),
    ("files", &[file!("files")]),
    ("streams", &[file!("streams")]),
    ("paths", &[file!("paths")]),
    ("texts", &[file!("texts")]),
    ("environment", &[file!("environment")]),
    ("processes", &[file!("processes")]),
    ("pacing", &[file!("pacing")]),
    ("rules", &[file!("rules")]),
    ("validate", &[file!("validate")]),
    ("archive", &[file!("archive")]),
    (
        "sql",
        &[
            file!("sql/sql"),
            file!("sql/databases"),
            file!("sql/sqlite"),
            file!("sql/wire"),
            file!("sql/postgres"),
            file!("sql/mysql"),
        ],
    ),
    ("image", &[file!("image")]),
    ("acme", &[file!("acme")]),
    ("graphql", &[file!("graphql")]),
    ("gzip", &[file!("gzip")]),
    ("zstd", &[file!("zstd")]),
    ("xml", &[file!("xml")]),
    ("template", &[file!("template")]),
    ("formats", &[file!("formats")]),
    ("shapes", &[file!("shapes")]),
    ("cli", &[file!("cli")]),
    ("mcp", &[file!("mcp")]),
    ("model", &[file!("model")]),
    ("fuzz", &[file!("fuzz")]),
];

/// The modules the compiler implements whose rest is written in Tondo,
/// and the modules that have it: their `pub` functions and types are the
/// module's own. An import of the module brings them all.
pub(crate) const COMPANIONS: &[(&str, &[&str])] = &[
    ("time", &["datetime", "zoned"]),
    ("fs", &["files"]),
    ("path", &["paths"]),
    ("io", &["streams"]),
    ("text", &["texts"]),
    ("env", &["environment"]),
    ("process", &["processes"]),
    ("sync", &["pacing"]),
];

/// The module a companion completes: `time` for `datetime`.
pub(crate) fn completed(companion: &str) -> Option<&'static str> {
    COMPANIONS.iter().find(|(_, c)| c.contains(&companion)).map(|(module, _)| *module)
}

/// The companions of a module the compiler implements.
pub(crate) fn companions(module: &str) -> &'static [&'static str] {
    COMPANIONS.iter().find(|(m, _)| *m == module).map_or(&[], |(_, c)| c)
}

/// Each file of the standard library, by its path in `std/` without
/// `.to`, and its source.
pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    SOURCES.iter().flat_map(|(_, files)| files.iter().copied())
}

/// The files of a standard module written in Tondo.
pub fn files(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    SOURCES.iter().find(|(n, _)| *n == name).map(|(_, files)| *files)
}

/// The source of a file of the standard library, by its path in `std/`
/// without `.to`.
pub fn file(path: &str) -> Option<&'static str> {
    sources().find(|(p, _)| *p == path).map(|(_, text)| text)
}

/// The names of the standard modules written in Tondo.
pub fn names() -> impl Iterator<Item = &'static str> {
    SOURCES.iter().map(|(name, _)| *name)
}

/// The source of a standard module written in Tondo, its files one after
/// another.
pub fn text(name: &str) -> Option<std::borrow::Cow<'static, str>> {
    match files(name)? {
        [(_, text)] => Some(std::borrow::Cow::Borrowed(text)),
        files => Some(files.iter().map(|(_, text)| *text).collect::<Vec<_>>().join("\n").into()),
    }
}

/// The module written in Tondo an import brings.
pub(crate) fn source(path: &[ast::Ident]) -> Option<&'static str> {
    match path {
        [std, name] if std.name == "std" => {
            // `std.testing` brings the loop of `testing.fuzz`, and with
            // it `std.json`, which `testing.snapshot` encodes values with;
            // and a module the compiler implements, its parts in Tondo.
            let name = match name.name.as_str() {
                "testing" => "fuzz",
                name => companions(name).first().copied().unwrap_or(name),
            };
            SOURCES.iter().find(|(n, _)| *n == name).map(|(name, _)| *name)
        }
        _ => None,
    }
}

/// The standard modules that import `std.reflect` only to implement it for
/// their own types: they do not ask it of the program's.
const OWN_REFLECTION: [&str; 9] = ["datetime", "json", "math", "pacing", "toml", "url", "uuid", "xml", "yaml"];

/// Whether the standard module `name` is `target` or imports it, directly
/// or through another standard module.
pub(crate) fn brings(name: &str, target: &str) -> bool {
    let mut pending = vec![name];
    let mut seen: Vec<&str> = Vec::new();
    while let Some(next) = pending.pop() {
        if next == target {
            return true;
        }
        if seen.contains(&next) {
            continue;
        }
        seen.push(next);
        let Some(files) = files(next) else { continue };
        for line in files.iter().flat_map(|(_, text)| text.lines()) {
            let Some(path) = line.strip_prefix("import std.") else { continue };
            let module = path.split([' ', '.']).next().unwrap_or_default();
            if module == "reflect" && OWN_REFLECTION.contains(&next) {
                continue;
            }
            pending.push(SOURCES.iter().find(|(n, _)| *n == module).map_or(module, |(n, _)| n));
        }
    }
    false
}

/// Whether a program links with standard modules written in Tondo: always,
/// for the prelude, which has the methods of the intrinsic types.
pub fn needed(_module: &ast::Module) -> bool {
    true
}

/// Adds the standard modules written in Tondo that the program imports,
/// directly or through one another, as the modules of a package `std`.
pub fn add(
    modules: &mut Vec<ModuleSource>,
    packages: &mut Vec<PackageInfo>,
    sources: &mut SourceMap,
    diags: &mut Diagnostics,
) {
    add_except(modules, packages, sources, diags, &[]);
}

/// Like `add`, without the modules `replaced` names, which the caller adds.
pub fn add_except(
    modules: &mut Vec<ModuleSource>,
    packages: &mut Vec<PackageInfo>,
    sources: &mut SourceMap,
    diags: &mut Diagnostics,
    replaced: &[&str],
) {
    let package = packages.len();
    let first = modules.len();
    let mut pending: Vec<&str> = vec!["prelude"];
    let wanted = |module: &ast::Module, pending: &mut Vec<&str>| {
        for import in &module.imports {
            let Some(name) = source(&import.path) else { continue };
            // A module replaced comes without its parts in Tondo.
            if completed(name).is_some_and(|module| replaced.contains(&module)) {
                continue;
            }
            let others = completed(name).map_or(&[][..], companions);
            for name in std::iter::once(name).chain(others.iter().copied()) {
                if !pending.contains(&name) && !replaced.contains(&name) {
                    pending.push(name);
                }
            }
        }
    };
    for module in modules.iter() {
        for (file, _) in &module.files {
            wanted(file, &mut pending);
        }
    }
    packages.push(PackageInfo {
        name: "std".into(),
        dependencies: Default::default(),
        dev_dependencies: Default::default(),
    });
    // In rounds: the modules wanted so far are parsed on every core, and
    // what they import is wanted next, in the order the files were added.
    let mut done = 0;
    while done < pending.len() {
        let round: Vec<&str> = pending[done..].to_vec();
        done = pending.len();
        // Each file with its own identity, `<std>/http/server.to`.
        let files: Vec<(&str, usize)> = round
            .iter()
            .flat_map(|name| files(name).unwrap_or_default().iter().map(move |file| (*name, file)))
            .map(|(name, (path, text))| {
                let file = sources.add(Path::new(&format!("{path}.to")), text.to_string());
                sources.set_identity(file, format!("<std>/{path}.to"), format!("std.{name}"));
                (name, file)
            })
            .collect();
        let parsed = crate::parallel::map(&files, 1, |&(_, file)| {
            let mut found = Diagnostics::default();
            (parser::parse_file(sources.file(file), &mut found), found)
        });
        let mut parts: Vec<(&str, Vec<(ast::Module, bool)>)> = round.iter().map(|name| (*name, Vec::new())).collect();
        for ((name, _), (parsed, mut found)) in files.into_iter().zip(parsed) {
            diags.list.append(&mut found.list);
            wanted(&parsed, &mut pending);
            if let Some((_, part)) = parts.iter_mut().find(|(n, _)| *n == name) {
                part.push((parsed, false));
            }
        }
        for (name, files) in parts {
            modules.push(ModuleSource { path: vec!["std".into(), name.into()], package, files });
        }
    }
    // Their enums of plain variants show their names, as the program's do.
    crate::derive::derive_display(&mut modules[first..], sources, diags);
}
