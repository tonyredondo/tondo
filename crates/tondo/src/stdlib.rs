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
];

/// The module written in Tondo an import brings: `std.testing` brings
/// `std.json`, which `testing.snapshot` encodes values with.
pub(crate) fn source(path: &[ast::Ident]) -> Option<(&'static str, &'static str)> {
    match path {
        [std, name] if std.name == "std" => {
            let name = if name.name == "testing" { "json" } else { name.name.as_str() };
            SOURCES.iter().find(|(n, _)| *n == name).copied()
        }
        _ => None,
    }
}

/// Whether a module imports a standard module written in Tondo.
pub fn needed(module: &ast::Module) -> bool {
    module.imports.iter().any(|import| source(&import.path).is_some())
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
    let mut pending: Vec<&str> = Vec::new();
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
    if pending.is_empty() {
        return;
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
