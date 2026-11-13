//! What a contract of `std.plugin` gives its module (standard library
//! 14.48), written as Tondo source, as `derive.rs` writes implementations:
//! for a trait `Name` with `@plugin.Contract`, the type `NamePlugin`, which
//! implements it by calling a plugin, its `load` and `start`, and
//! `serveName`. The checker holds the contract to what it may be.

use crate::ast;
use crate::diag::Diagnostics;
use crate::modules::ModuleSource;
use crate::parser;
use crate::source::SourceMap;
use std::path::Path;

/// The entries a library that offers a plugin publishes (14.48).
pub const ENTRIES: [&str; 3] = ["tondo_plugin_contract", "tondo_plugin_call", "tondo_plugin_free"];

/// The contracts of every module, each given its declarations.
pub fn derive(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    for module in modules.iter_mut() {
        let module_name = module.path.join(".");
        for (file, _) in module.files.iter_mut() {
            let Some(plugin) = crate::derive::alias_of(file, "plugin") else { continue };
            let mut text = String::new();
            let mut exported: Option<crate::source::Span> = None;
            for item in &file.items {
                if let ast::Item::Fn(f) = item
                    && f.uses.iter().any(
                        |u| matches!(u.path.as_slice(), [alias, name] if alias.name == plugin && name.name == "Export"),
                    )
                {
                    if let Some(first) = exported {
                        let line = sources.position(first.start).1;
                        let message = format!(
                            "a library offers one plugin, and `@{plugin}.Export` is on another function, on line {line}: keep one"
                        );
                        diags.error("E1119", f.name.span, message);
                        continue;
                    }
                    exported = Some(f.name.span);
                    match entries(f, &plugin, sources) {
                        Ok(written) => text.push_str(&written),
                        Err(message) => diags.error("E1119", f.name.span, message),
                    }
                }
                let ast::Item::Trait(contract) = item else { continue };
                let marked = contract.uses.iter().any(
                    |u| matches!(u.path.as_slice(), [alias, name] if alias.name == plugin && name.name == "Contract"),
                );
                if marked && let Some(written) = declarations(contract, &plugin, &module_name, sources) {
                    text.push_str(&written);
                }
            }
            if text.is_empty() {
                continue;
            }
            let span = file.imports.first().map_or(crate::source::Span::new(0, 0), |i| i.span);
            if text.contains("__messagepack.") && crate::derive::alias_of(file, "messagepack").is_none() {
                let ident = |name: &str| ast::Ident { name: name.into(), span };
                file.imports.push(ast::Import {
                    path: vec![ident("std"), ident("messagepack")],
                    alias: Some(ident("__messagepack")),
                    span,
                    line: span,
                });
            }
            if text.contains("__ffi.") && crate::derive::alias_of(file, "ffi").is_none() {
                let ident = |name: &str| ast::Ident { name: name.into(), span };
                file.imports.push(ast::Import {
                    path: vec![ident("std"), ident("ffi")],
                    alias: Some(ident("__ffi")),
                    span,
                    line: span,
                });
            }
            let messagepack = crate::derive::alias_of(file, "messagepack").unwrap_or_else(|| "__messagepack".into());
            let ffi = crate::derive::alias_of(file, "ffi").unwrap_or_else(|| "__ffi".into());
            let text = text.replace("__messagepack.", &format!("{messagepack}.")).replace("__ffi.", &format!("{ffi}."));
            let name = format!("{module_name}.plugin.derived.to");
            let id = sources.add(Path::new(&name), text);
            sources.set_identity(id, format!("<derived>/{name}"), module_name.clone());
            let generated = parser::parse_file(sources.file(id), diags);
            file.items.extend(generated.items);
        }
    }
}

/// A method of a contract as the declarations need it: its name, its
/// parameters' names and types, its result's type, and its own error's,
/// beside `plugin.Error`, when it has one.
struct Method {
    name: String,
    params: Vec<(String, String)>,
    result: String,
    own: Option<String>,
}

/// The declarations of a contract, or none when its shape is not one the
/// checker allows, which the checker says.
fn declarations(contract: &ast::TraitDecl, plugin: &str, module: &str, sources: &SourceMap) -> Option<String> {
    if !contract.generics.is_empty() || !contract.requires.is_empty() {
        return None;
    }
    let text = |ty: &ast::TypeExpr| sources.slice(ty.span).to_string();
    let is_error = |ty: &ast::TypeExpr| {
        matches!(&ty.kind, ast::TypeKind::Path { segments, args } if args.is_empty()
            && matches!(segments.as_slice(), [alias, name] if alias.name == plugin && name.name == "Error"))
    };
    let mut methods = Vec::new();
    for f in &contract.methods {
        let [receiver, params @ ..] = f.params.as_slice() else { return None };
        // A plain `self`, which lends the receiver: not `mut self` nor `var self`.
        if receiver.ty.is_some() || receiver.mode != ast::Mode::Ref || !f.generics.is_empty() {
            return None;
        }
        let ast::TypeKind::Result { ok, err } = &f.result.as_ref()?.kind else { return None };
        let result = ok.as_ref().map_or("Unit".to_string(), |ok| text(ok));
        let own = match &err.kind {
            _ if is_error(err) => None,
            ast::TypeKind::Union(members) if members.iter().any(is_error) => {
                let others: Vec<String> = members.iter().filter(|m| !is_error(m)).map(text).collect();
                Some(others.join(" | "))
            }
            _ => return None,
        };
        let mut named = Vec::new();
        for param in params {
            if param.mode != ast::Mode::Value || param.variadic {
                return None;
            }
            named.push((param.name.name.clone(), text(param.ty.as_ref()?)));
        }
        methods.push(Method { name: f.name.name.clone(), params: named, result, own });
    }
    let name = contract.name.name.rsplit('.').next().unwrap_or(&contract.name.name).to_string();
    let p = plugin;
    let qualified = format!("{module}.{name}");
    let names: Vec<String> = methods.iter().map(|m| format!("{:?}", m.name)).collect();
    let mut out = format!(
        "/// A `{name}` that was loaded while the program runs: from a library with
/// `{name}Plugin.load(path)`, or from a program with `{name}Plugin.start(command)`.
pub type {name}Plugin = {{
    priv link: {p}.Link
}}

/// What `{name}` is, as both ends of a plugin compare it.
pub fn {name}Plugin.contract(): {p}.Description {{
    {p}.described({qualified:?}, {p}.contractText[{name}Plugin](), [{names}])
}}

/// Loads the plugin of the library at `path`, which must offer `{name}`.
pub fn {name}Plugin.load(path: String): {name}Plugin ! {p}.Error {{
    {name}Plugin {{ link: {p}.Link.load(path, {name}Plugin.contract())? }}
}}

/// Starts `command`, which must serve `{name}`, and talks to the plugin
/// it serves.
pub fn {name}Plugin.start(command: Command): {name}Plugin ! {p}.Error {{
    {name}Plugin {{ link: {p}.Link.start(command, {name}Plugin.contract())? }}
}}

impl {name} for {name}Plugin {{
",
        names = names.join(", "),
    );
    for (index, method) in methods.iter().enumerate() {
        let params: String = method.params.iter().map(|(n, t)| format!(", {n}: {t}")).collect();
        let error = match &method.own {
            Some(own) => format!("({own} | {p}.Error)"),
            None => format!("{p}.Error"),
        };
        let values: String = method.params.iter().map(|(n, _)| format!("        __arguments.value({n})\n")).collect();
        let own = match &method.own {
            Some(own) => format!(
                "        if __status == {p}.Failed {{\n            fail {p}.decoded[{own}](__bytes)?\n        }}\n"
            ),
            None => String::new(),
        };
        out.push_str(&format!(
            "    fn {}(self{params}): {} ! {error} {{
        let __arguments = __messagepack.writer()
        __arguments.beginArray({})
{values}        let (__status, __bytes) = self.link.call({index}, __arguments.finish())?
{own}        {}
    }}
",
            method.name,
            method.result,
            method.params.len(),
            // A method of no result gives nil, which is nothing to read.
            match method.result.as_str() {
                "Unit" => "_ = __bytes".to_string(),
                result => format!("{p}.decoded[{result}](__bytes)?"),
            },
        ));
    }
    out.push_str(&format!(
        "}}

/// Serves `implementation` to the program that started this one, over
/// its standard input and output, until that program closes its end.
pub fn serve{name}[__Implementation: {name} + Send + Share](implementation: __Implementation): !{p}.Error {{
    {p}.serve({name}Plugin.contract(), (method, given) {{ {name}Plugin.answer(implementation, method, given) }})
}}

/// The answer of `implementation` to a call of method number `method` with
/// the bytes of its arguments, for `serve{name}` and a library's entries.
pub fn {name}Plugin.answer[__Implementation: {name}](implementation: __Implementation, method: Int, given: Array[Byte]): (Int, Array[Byte]) {{
    match method {{
"
    ));
    for (index, method) in methods.iter().enumerate() {
        let mut arm = format!("        {index} => {{\n");
        for (at, (n, t)) in method.params.iter().enumerate() {
            arm.push_str(&format!(
                "            let __{n}: {t} = match {p}.argument[{t}](given, {at}) {{\n                ok(value) => value\n                err(error) => return {p}.broken(error)\n            }}\n"
            ));
        }
        let args: Vec<String> = method.params.iter().map(|(n, _)| format!("__{n}")).collect();
        let failed = match &method.own {
            Some(_) => format!(
                "                err({p}.Error(error)) => {p}.broken(error)\n                err(error) => ({p}.Failed, __messagepack.encode(error))\n"
            ),
            None => format!("                err(error) => {p}.broken(error)\n"),
        };
        arm.push_str(&format!(
            "            match implementation.{}({}) {{\n                ok(value) => {}\n{failed}            }}\n        }}\n",
            method.name,
            args.join(", "),
            match method.result.as_str() {
                "Unit" => format!("{{\n                    _ = value\n                    ({p}.Returned, {p}.nothing())\n                }}"),
                _ => format!("({p}.Returned, __messagepack.encode(value))"),
            },
        ));
        out.push_str(&arm);
    }
    out.push_str(&format!(
        "        _ => {p}.broken({p}.Error.Format(\"no method {{method}} in `{name}`\"))\n    }}\n}}\n"
    ));
    Some(out)
}

/// The entries of rule 12 for a library whose function `f` gives the
/// implementation it offers, `fn f(): impl module.Contract`; or what is
/// wrong with `f`.
fn entries(f: &ast::FnDecl, plugin: &str, sources: &SourceMap) -> Result<String, String> {
    let form = format!(
        "`@{plugin}.Export` goes on a function with no parameters that gives an implementation of a contract, as `fn exporter(): impl api.Exporter {{ Csv {{}} }}`"
    );
    if !f.params.is_empty() || f.owner.is_some() || !f.generics.is_empty() {
        return Err(form);
    }
    let Some(ast::TypeExpr { kind: ast::TypeKind::Opaque(bounds), .. }) = &f.result else { return Err(form) };
    let [contract] = bounds.as_slice() else { return Err(form) };
    let ast::TypeKind::Path { segments, args } = &contract.kind else { return Err(form) };
    if !args.is_empty() {
        return Err(form);
    }
    let written = sources.slice(contract.span).to_string();
    let plugin_type = format!("{written}Plugin");
    let _ = segments;
    let p = plugin;
    let name = &f.name.name;
    Ok(format!(
        "/// The contract this library offers, `{written}` (14.48).
pub extern fn tondo_plugin_contract(bytes: Pointer[Pointer[Byte]], length: Pointer[__ffi.CSize]) {{
    let implementation = {name}()
    unsafe {{
        {p}.offer({plugin_type}.contract(), (method, given) {{ {plugin_type}.answer(implementation, method, given) }}, bytes, length)
    }}
}}

/// Calls a method of what this library offers (14.48).
pub extern fn tondo_plugin_call(
    method: UInt32,
    input: Pointer[Byte],
    inputLength: __ffi.CSize,
    output: Pointer[Pointer[Byte]],
    outputLength: Pointer[__ffi.CSize],
): Int32 {{
    unsafe {{ {p}.offered(method, input, inputLength, output, outputLength) }}
}}

/// Frees what the other entries wrote (14.48).
pub extern fn tondo_plugin_free(bytes: Pointer[Byte], length: __ffi.CSize) {{
    unsafe {{ {p}.release(bytes, length) }}
}}
"
    ))
}
