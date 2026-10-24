//! The C header of a library (toolchain 7): the functions it publishes and
//! the `extern type`s they reach, with their documentation, and in a
//! comment how to link the library.

use crate::hir::{CApi, CType};
use std::fmt::Write;

/// The header of the library `name`.
pub fn header(name: &str, api: &CApi, linking: &str) -> String {
    let guard = format!("{}_H", name.to_uppercase());
    let mut out = format!("/* {name}.h: the C functions of the Tondo library `{name}`, written by tondo.\n *\n");
    for line in linking.lines() {
        let _ = writeln!(out, " * {line}");
    }
    let _ = write!(
        out,
        " */\n\n#ifndef {guard}\n#define {guard}\n\n#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n"
    );
    let mut types: Vec<&CType> =
        api.functions.iter().flat_map(|f| f.params.iter().map(|(_, t)| t).chain([&f.result])).collect();
    types.extend(api.structs.iter().flat_map(|s| s.fields.iter().map(|(_, t)| t)));
    if types.iter().any(|ty| mentions(ty, "ssize_t")) {
        out.push_str("#include <sys/types.h>\n");
    }
    out.push_str("\n#ifdef __cplusplus\nextern \"C\" {\n#endif\n");
    if !api.structs.is_empty() {
        out.push('\n');
    }
    for decl in &api.structs {
        let keyword = if decl.layout.union { "union" } else { "struct" };
        let _ = writeln!(out, "typedef {keyword} {0} {0};", decl.name);
    }
    for decl in api.structs.iter().filter(|s| !s.layout.opaque) {
        out.push('\n');
        docs(&mut out, decl.docs.as_deref());
        let keyword = if decl.layout.union { "union" } else { "struct" };
        let mut attributes = Vec::new();
        if decl.layout.packed {
            attributes.push("packed".to_string());
        }
        if decl.layout.align > 0 {
            attributes.push(format!("aligned({})", decl.layout.align));
        }
        let attributes =
            if attributes.is_empty() { String::new() } else { format!("__attribute__(({})) ", attributes.join(", ")) };
        let _ = writeln!(out, "{keyword} {attributes}{} {{", decl.name);
        for (field, ty) in &decl.fields {
            let _ = writeln!(out, "    {};", declare(ty, field));
        }
        out.push_str("};\n");
    }
    for function in &api.functions {
        out.push('\n');
        docs(&mut out, function.docs.as_deref());
        let _ = writeln!(
            out,
            "{};",
            declare(&function.result, &format!("{}({})", function.symbol, params(&function.params)))
        );
    }
    let _ = write!(out, "\n#ifdef __cplusplus\n}}\n#endif\n\n#endif\n");
    out
}

/// A declaration of `name` with type `ty`, in C's inside-out syntax.
fn declare(ty: &CType, name: &str) -> String {
    match ty {
        CType::Named(c) if name.is_empty() => c.to_string(),
        CType::Named(c) => format!("{c} {name}"),
        CType::Struct(c) if name.is_empty() => c.clone(),
        CType::Struct(c) => format!("{c} {name}"),
        CType::Pointer(inner) => declare(inner, &format!("*{name}")),
        CType::Array(inner, count) => declare(inner, &format!("{name}[{count}]")),
        CType::Function(parameters, result) => {
            let list = if parameters.is_empty() {
                "void".to_string()
            } else {
                parameters.iter().map(|p| declare(p, "")).collect::<Vec<_>>().join(", ")
            };
            declare(result, &format!("(*{name})({list})"))
        }
    }
}

fn params(params: &[(String, CType)]) -> String {
    if params.is_empty() {
        return "void".into();
    }
    params.iter().map(|(name, ty)| declare(ty, name)).collect::<Vec<_>>().join(", ")
}

fn mentions(ty: &CType, c: &str) -> bool {
    match ty {
        CType::Named(name) => *name == c,
        CType::Struct(_) => false,
        CType::Pointer(inner) | CType::Array(inner, _) => mentions(inner, c),
        CType::Function(params, result) => params.iter().chain([&**result]).any(|p| mentions(p, c)),
    }
}

/// `///` documentation as a C comment.
fn docs(out: &mut String, docs: Option<&str>) {
    let Some(docs) = docs else { return };
    let mut lines = docs.lines();
    let first = lines.next().unwrap_or_default().replace("*/", "* /");
    let rest: Vec<String> = lines.map(|l| l.replace("*/", "* /")).collect();
    if rest.is_empty() {
        let _ = writeln!(out, "/** {first} */");
        return;
    }
    let _ = writeln!(out, "/** {first}");
    for line in rest {
        let _ = writeln!(out, "{}", format!(" * {line}").trim_end());
    }
    out.push_str(" */\n");
}
