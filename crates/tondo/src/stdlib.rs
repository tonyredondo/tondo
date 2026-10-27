//! Standard library modules written in Tondo, which the compiler carries
//! and links into a program that imports them, as any other module of a
//! package named `std` (standard library 12.1).

use crate::diag::Diagnostics;
use crate::modules::{ModuleSource, PackageInfo};
use crate::source::SourceMap;
use crate::{ast, parser};
use std::path::Path;

/// Each module, by its path after `std`, and its source.
const SOURCES: &[(&str, &str)] = &[
    ("serialization", include_str!("../std/serialization.to")),
    ("json", include_str!("../std/json.to")),
    ("toml", include_str!("../std/toml.to")),
    ("yaml", include_str!("../std/yaml.to")),
    ("messagepack", include_str!("../std/messagepack.to")),
    ("cbor", include_str!("../std/cbor.to")),
    ("net", include_str!("../std/net.to")),
    ("ffi", include_str!("../std/ffi.to")),
    ("regex", include_str!("../std/regex.to")),
    ("reflect", include_str!("../std/reflect.to")),
    ("prelude", include_str!("../std/prelude.to")),
    ("panics", include_str!("../std/panics.to")),
    ("math", include_str!("../std/math.to")),
    ("meta", include_str!("../std/meta.to")),
    ("encoding", include_str!("../std/encoding.to")),
    ("hash", include_str!("../std/hash.to")),
    ("random", include_str!("../std/random.to")),
    ("format", include_str!("../std/format.to")),
    ("uuid", include_str!("../std/uuid.to")),
    ("datetime", include_str!("../std/datetime.to")),
    ("csv", include_str!("../std/csv.to")),
    ("log", include_str!("../std/log.to")),
    ("http", include_str!("../std/http.to")),
    ("executor", include_str!("../std/executor.to")),
    ("diagnostics", include_str!("../std/diagnostics.to")),
    ("profile", include_str!("../std/profile.to")),
    ("fuzz", include_str!("../std/fuzz.to")),
];

/// Each standard module's name and source.
pub fn sources() -> &'static [(&'static str, &'static str)] {
    SOURCES
}

/// The names of the standard modules written in Tondo.
pub fn names() -> impl Iterator<Item = &'static str> {
    SOURCES.iter().map(|(name, _)| *name)
}

/// The source of a standard module written in Tondo.
pub fn text(name: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}

/// The module written in Tondo an import brings.
pub(crate) fn source(path: &[ast::Ident]) -> Option<(&'static str, &'static str)> {
    match path {
        [std, name] if std.name == "std" => {
            // `std.testing` brings the loop of `testing.fuzz`, and with
            // it `std.json`, which `testing.snapshot` encodes values with;
            // and `std.time` its parts in Tondo.
            let name = match name.name.as_str() {
                "testing" => "fuzz",
                "time" => "datetime",
                name => name,
            };
            SOURCES.iter().find(|(n, _)| *n == name).copied()
        }
        _ => None,
    }
}

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
        let Some(text) = text(next) else { continue };
        for line in text.lines() {
            let Some(path) = line.strip_prefix("import std.") else { continue };
            let module = path.split([' ', '.']).next().unwrap_or_default();
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
    let mut pending: Vec<&str> = vec!["prelude"];
    let wanted = |module: &ast::Module, pending: &mut Vec<&str>| {
        for import in &module.imports {
            if let Some((name, _)) = source(&import.path)
                && !pending.contains(&name)
                && !replaced.contains(&name)
            {
                pending.push(name);
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
        let files: Vec<usize> = round
            .iter()
            .map(|name| {
                let text = SOURCES.iter().find(|(n, _)| n == name).unwrap().1;
                let file = sources.add(Path::new(&format!("{name}.to")), text.to_string());
                sources.set_identity(file, format!("<std>/{name}.to"), format!("std.{name}"));
                file
            })
            .collect();
        let parsed = crate::parallel::map(&files, 1, |&file| {
            let mut found = Diagnostics::default();
            (parser::parse_file(sources.file(file), &mut found), found)
        });
        for (name, (parsed, mut found)) in round.into_iter().zip(parsed) {
            diags.list.append(&mut found.list);
            wanted(&parsed, &mut pending);
            modules.push(ModuleSource { path: vec!["std".into(), name.into()], package, files: vec![(parsed, false)] });
        }
    }
}
