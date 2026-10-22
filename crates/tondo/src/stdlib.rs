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
    ("json", include_str!("../std/json.to")),
    ("ffi", include_str!("../std/ffi.to")),
    ("regex", include_str!("../std/regex.to")),
    ("reflect", include_str!("../std/reflect.to")),
    ("prelude", include_str!("../std/prelude.to")),
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
];

/// The module written in Tondo an import brings.
pub(crate) fn source(path: &[ast::Ident]) -> Option<(&'static str, &'static str)> {
    match path {
        [std, name] if std.name == "std" => {
            // `std.testing` brings `std.json`, which `testing.snapshot`
            // encodes values with, and `std.time` its parts in Tondo.
            let name = match name.name.as_str() {
                "testing" => "json",
                "time" => "datetime",
                name => name,
            };
            SOURCES.iter().find(|(n, _)| *n == name).copied()
        }
        _ => None,
    }
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
    let package = packages.len();
    let mut pending: Vec<&str> = vec!["prelude"];
    let wanted = |module: &ast::Module, pending: &mut Vec<&str>| {
        for import in &module.imports {
            if let Some((name, _)) = source(&import.path)
                && !pending.contains(&name)
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
    packages.push(PackageInfo { name: "std".into(), dependencies: Default::default() });
    let mut done = 0;
    while done < pending.len() {
        let name = pending[done];
        done += 1;
        let text = SOURCES.iter().find(|(n, _)| *n == name).unwrap().1;
        let file = sources.add(Path::new(&format!("{name}.to")), text.to_string());
        sources.set_identity(file, format!("<std>/{name}.to"), format!("std.{name}"));
        let parsed = parser::parse_file(sources.file(file), diags);
        wanted(&parsed, &mut pending);
        modules.push(ModuleSource { path: vec!["std".into(), name.into()], package, files: vec![(parsed, false)] });
    }
}
