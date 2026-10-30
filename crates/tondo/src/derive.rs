//! Implementations the compiler writes as Tondo source for every record,
//! enum and newtype whose parts allow them, unless the program writes its
//! own: `serialization.Encode` and `serialization.Decode` when the program
//! brings `std.serialization`, as every format does (standard library
//! 14.7), and `reflect.Reflect` when it imports `std.reflect` (14.8). They go into the type's module, and the rest of the
//! compiler checks them like any other source.

use crate::diag::Diagnostics;
use crate::modules::ModuleSource;
use crate::source::SourceMap;
use crate::stdlib::source;
use crate::{ast, parser};
use std::path::Path;

/// One kind of automatic implementation.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Serialization,
    Reflect,
}

impl Kind {
    /// The standard module whose traits it implements.
    fn module(self) -> &'static str {
        match self {
            Kind::Serialization => "serialization",
            Kind::Reflect => "reflect",
        }
    }

    fn traits(self) -> &'static [&'static str] {
        match self {
            Kind::Serialization => &["Encode", "Decode"],
            Kind::Reflect => &["Reflect"],
        }
    }

    /// Whether a file brings the module: by importing it, or a standard
    /// module that imports it, as `std.http` imports `std.json`, which
    /// imports `std.serialization`.
    fn brought(self, file: &ast::Module) -> bool {
        file.imports.iter().any(|i| source(&i.path).is_some_and(|(name, _)| crate::stdlib::brings(name, self.module())))
    }

    /// Whether an intrinsic type with these arguments has the
    /// implementations when its arguments do; `None` for another name.
    fn builtin(self, name: &str, args: &[ast::TypeExpr]) -> Option<bool> {
        let string = |ty: &ast::TypeExpr| matches!(&ty.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == "String");
        Some(match (self, name, args) {
            (Kind::Serialization, name, []) if crate::types::IntTy::from_name(name).is_some() => true,
            (Kind::Serialization, "Byte" | "Float" | "Float64" | "Float32" | "Bool" | "String", []) => true,
            (Kind::Serialization, "Array" | "Option", [_]) => true,
            (Kind::Serialization, "Map", [key, _]) => string(key),
            (
                Kind::Reflect,
                "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "Int128" | "UInt" | "UInt8" | "UInt16" | "UInt32"
                | "UInt64" | "UInt128" | "Float" | "Float32" | "Float64" | "Bool" | "Char" | "Byte" | "String" | "Unit",
                [],
            ) => true,
            (Kind::Reflect, "Array" | "Option" | "Deque" | "Set", [_]) => true,
            (Kind::Reflect, "Map", [_, _]) => true,
            _ => return None,
        })
    }

    /// Whether a type of a standard module has the implementations.
    fn standard(self, module: &str, name: &str) -> bool {
        match (self, module) {
            (Kind::Serialization, "json") => matches!(name, "Value" | "Number" | "Raw"),
            (Kind::Serialization, "toml" | "yaml" | "messagepack" | "cbor") => name == "Value",
            (Kind::Serialization, "serialization") => name == "Bytes",
            (Kind::Serialization, "time") => name == "DateTime",
            (Kind::Reflect, "reflect") => name == "Value",
            (Kind::Reflect | Kind::Serialization, "template") => name == "Html",
            (Kind::Reflect | Kind::Serialization, "math") => matches!(name, "BigInt" | "Decimal"),
            (Kind::Reflect | Kind::Serialization, "uuid") => name == "Uuid",
            _ => false,
        }
    }
}

/// A record, enum or newtype of the program.
struct Declared {
    module: usize,
    file: usize,
    name: String,
    generics: Vec<String>,
}

/// A `pub` method that reflection can call: it takes `self` or `mut self`
/// and parameters by value whose types, like its result, implement
/// `Reflect`.
struct Method {
    name: String,
    mutating: bool,
    /// The types of its parameters after the receiver, and of its result,
    /// as written.
    params: Vec<String>,
    result: String,
    /// The names of its parameters after the receiver.
    names: Vec<String>,
    /// Whether the program annotates it, and each of its parameters after
    /// the receiver (27.13).
    annotated: bool,
    annotated_params: Vec<bool>,
    /// Its `///` documentation.
    docs: Option<String>,
}

/// A variant as the generator matches it: the pattern that binds its
/// payload to `__part` locals, and the payload's labels with their locals.
struct Arm<'a> {
    variant: &'a str,
    pattern: String,
    parts: Vec<(String, String)>,
    payload: &'a ast::VariantPayload,
}

/// What an arm of a generated `match self` gives, from its variant.
type ArmBody<'a> = &'a dyn Fn(&str, &[(String, String)], &ast::VariantPayload) -> String;

/// The alias a file imports a standard module under, if any.
fn alias_of(file: &ast::Module, module: &str) -> Option<String> {
    file.imports
        .iter()
        .find(|i| matches!(&i.path[..], [s, m] if s.name == "std" && m.name == module))
        .map(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name.clone())
}

/// Writes the automatic implementations of every kind the program asks for.
pub fn derive(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    for kind in [Kind::Serialization, Kind::Reflect] {
        if modules.iter().any(|m| m.files.iter().any(|(f, _)| kind.brought(f))) {
            derive_kind(kind, modules, sources, diags);
        }
    }
}

fn derive_kind(kind: Kind, modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    let mut declared: Vec<Declared> = Vec::new();
    for (module, source) in modules.iter().enumerate() {
        for (file, (parsed, _)) in source.files.iter().enumerate() {
            for item in &parsed.items {
                let (name, generics) = match item {
                    // C structs cross to C, not to other formats or reflection.
                    ast::Item::Type(t) if t.foreign => continue,
                    ast::Item::Type(t) => (&t.name.name, &t.generics),
                    ast::Item::Enum(e) => (&e.name.name, &e.generics),
                    _ => continue,
                };
                let generics = generics.iter().map(|g| g.name.name.clone()).collect();
                declared.push(Declared { module, file, name: name.clone(), generics });
            }
        }
    }
    let paths: Vec<Vec<String>> = modules.iter().map(|m| m.path.clone()).collect();
    let find = |module: usize, name: &str| declared.iter().position(|d| d.module == module && d.name == name);
    // Which types qualify: all their parts do, computed as a greatest fixed
    // point so that recursive types qualify.
    let mut qualifies = vec![true; declared.len()];
    loop {
        let mut changed = false;
        for index in 0..declared.len() {
            if !qualifies[index] {
                continue;
            }
            let d = &declared[index];
            let file = &modules[d.module].files[d.file].0;
            let context = Context { kind, module: d.module, file, generics: &d.generics, paths: &paths };
            let ok = parts(file, &d.name)
                .iter()
                .all(|ty| context.allowed(ty, &|m, n| find(m, n).is_some_and(|i| qualifies[i])))
                && (kind != Kind::Serialization
                    || fields(file, &d.name)
                        .iter()
                        .all(|f| !ignored(f) || matches!(f.ty.kind, ast::TypeKind::Optional(_))));
            if !ok {
                qualifies[index] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // The methods reflection calls, by declaration.
    let methods: Vec<Vec<Method>> = declared
        .iter()
        .map(|d| match kind {
            Kind::Serialization => Vec::new(),
            Kind::Reflect => {
                let allowed = |file: &ast::Module, ty: &ast::TypeExpr| {
                    let context = Context { kind, module: d.module, file, generics: &d.generics, paths: &paths };
                    context.allowed(ty, &|m, n| find(m, n).is_some_and(|i| qualifies[i]))
                };
                callable(&modules[d.module], &d.name, &allowed, sources)
            }
        })
        .collect();
    let module_name = kind.module();
    let fallback = format!("__{module_name}");
    for (module_index, module) in modules.iter_mut().enumerate() {
        for (file_index, (file, _)) in module.files.iter_mut().enumerate() {
            let mut text = String::new();
            let alias = alias_of(file, module_name).unwrap_or_else(|| fallback.clone());
            for (index, d) in declared.iter().enumerate() {
                if d.module != module_index || d.file != file_index || !qualifies[index] {
                    continue;
                }
                let implemented = |name: &str| {
                    file.items.iter().any(|item| match item {
                        ast::Item::Impl(i) => {
                            matches!(&i.trait_ref.kind, ast::TypeKind::Path { segments, .. } if segments.last().is_some_and(|s| s.name == name))
                                && matches!(&i.target.kind, ast::TypeKind::Path { segments, .. } if segments.len() == 1 && segments[0].name == d.name)
                        }
                        _ => false,
                    })
                };
                let item = file.items.iter().find(|item| match item {
                    ast::Item::Type(t) => t.name.name == d.name,
                    ast::Item::Enum(e) => e.name.name == d.name,
                    _ => false,
                });
                let Some(item) = item else { continue };
                let noted = NOTED.iter().filter_map(|&format| Some((format, alias_of(file, format)?))).collect();
                let displayed = implemented("Display");
                let validate = alias_of(file, "validate");
                let generator = Generator { alias: &alias, noted, sources, declared: d, displayed, validate };
                for &name in kind.traits() {
                    if implemented(name) {
                        continue;
                    }
                    text.push_str(&match name {
                        "Encode" => generator.encode(item),
                        "Decode" => generator.decode(item),
                        _ => generator.reflect(item, &methods[index]),
                    });
                }
            }
            if text.is_empty() {
                continue;
            }
            let span = file.imports.first().map_or(crate::source::Span::new(0, 0), |i| i.span);
            let ident = |name: &str| ast::Ident { name: name.into(), span };
            if alias_of(file, module_name).is_none() {
                file.imports.push(ast::Import {
                    path: vec![ident("std"), ident(module_name)],
                    alias: Some(ident(&fallback)),
                    span,
                    line: span,
                });
            }
            // The rules the code checks, from a module of their own.
            if text.contains("__rules.") && alias_of(file, "rules").is_none() {
                file.imports.push(ast::Import {
                    path: vec![ident("std"), ident("rules")],
                    alias: Some(ident("__rules")),
                    span,
                    line: span,
                });
            }
            let name = match kind {
                Kind::Serialization => format!("{}.derived.to", module.path.join(".")),
                Kind::Reflect => format!("{}.reflect.derived.to", module.path.join(".")),
            };
            let id = sources.add(Path::new(&name), text);
            sources.set_identity(id, format!("<derived>/{name}"), module.path.join("."));
            let generated = parser::parse_file(sources.file(id), diags);
            file.items.extend(generated.items);
        }
    }
}

/// The `pub` methods of a type that reflection can call, from every file
/// of its module.
fn callable(
    module: &ModuleSource,
    name: &str,
    allowed: &dyn Fn(&ast::Module, &ast::TypeExpr) -> bool,
    sources: &SourceMap,
) -> Vec<Method> {
    let mut found = Vec::new();
    for (file, _) in &module.files {
        for item in &file.items {
            let ast::Item::Fn(f) = item else { continue };
            if !f.public || f.owner.as_ref().is_none_or(|o| o.name != name) || !f.generics.is_empty() {
                continue;
            }
            let Some((receiver, params)) = f.params.split_first() else { continue };
            let mutating = match (&receiver.ty, receiver.mode) {
                (None, ast::Mode::Ref) => false,
                (None, ast::Mode::Mut) => true,
                _ => continue,
            };
            let plain = params
                .iter()
                .all(|p| p.mode == ast::Mode::Value && !p.variadic && p.ty.as_ref().is_some_and(|t| allowed(file, t)));
            let result = f.result.as_ref().is_none_or(|r| allowed(file, r));
            if plain && result {
                let text = |ty: &ast::TypeExpr| sources.slice(ty.span).to_string();
                found.push(Method {
                    name: f.name.name.clone(),
                    mutating,
                    params: params.iter().filter_map(|p| p.ty.as_ref()).map(text).collect(),
                    result: f.result.as_ref().map_or_else(|| "Unit".to_string(), text),
                    names: params.iter().map(|p| p.name.name.clone()).collect(),
                    annotated: !f.uses.is_empty(),
                    annotated_params: params.iter().map(|p| !p.uses.is_empty()).collect(),
                    docs: docs(sources, f.name.span),
                });
            }
        }
    }
    found
}

/// The `///` lines just before a declaration, without their markers.
pub(crate) fn docs(sources: &SourceMap, span: crate::source::Span) -> Option<String> {
    let file = sources.file_of(span.start);
    let before = &file.text[..(span.start - file.base) as usize];
    // The text before the name on its own line, such as `priv `, goes first.
    let mut lines: Vec<&str> = before.lines().collect();
    if !before.ends_with('\n') {
        lines.pop();
    }
    let mut found: Vec<&str> = Vec::new();
    for line in lines.iter().rev() {
        match line.trim_start().strip_prefix("///") {
            Some(doc) => found.push(doc.strip_prefix(' ').unwrap_or(doc)),
            // The program's annotations go between the documentation and
            // what it documents.
            None if line.trim_start().starts_with('@') => {}
            None => break,
        }
    }
    found.reverse();
    (!found.is_empty()).then(|| found.join("\n"))
}

/// `.documented(some("..."))` for the documentation of a variant or a
/// method, nothing without it.
fn documented(docs: Option<String>) -> String {
    docs.map_or(String::new(), |docs| format!(".documented(some({}))", literal(&docs)))
}

/// A Tondo string literal with this text: escaped, and with its braces
/// doubled so that they are not interpolation.
fn literal(text: &str) -> String {
    format!("{text:?}").replace('{', "{{").replace('}', "}}")
}

/// The types of the parts of a declaration.
fn parts<'a>(file: &'a ast::Module, name: &str) -> Vec<&'a ast::TypeExpr> {
    for item in &file.items {
        match item {
            ast::Item::Type(t) if t.name.name == name => {
                return match &t.body {
                    ast::TypeBody::Record(fields) => fields.iter().map(|f| &f.ty).collect(),
                    ast::TypeBody::Newtype(inner) => vec![inner],
                    ast::TypeBody::Opaque => Vec::new(),
                };
            }
            ast::Item::Enum(e) if e.name.name == name => {
                return e
                    .variants
                    .iter()
                    .flat_map(|v| match &v.payload {
                        ast::VariantPayload::Unit => Vec::new(),
                        ast::VariantPayload::Tuple(types) => types.iter().collect(),
                        ast::VariantPayload::Record(fields) => fields.iter().map(|f| &f.ty).collect(),
                    })
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// The fields of a record, or of the record variants of an enum.
fn fields<'a>(file: &'a ast::Module, name: &str) -> Vec<&'a ast::FieldDecl> {
    for item in &file.items {
        match item {
            ast::Item::Type(t) if t.name.name == name => {
                if let ast::TypeBody::Record(fields) = &t.body {
                    return fields.iter().collect();
                }
            }
            ast::Item::Enum(e) if e.name.name == name => {
                return e
                    .variants
                    .iter()
                    .flat_map(|v| match &v.payload {
                        ast::VariantPayload::Record(fields) => fields.iter().collect(),
                        _ => Vec::new(),
                    })
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Where a type expression is written: its module, file and the type
/// parameters in scope.
struct Context<'a> {
    kind: Kind,
    module: usize,
    file: &'a ast::Module,
    generics: &'a [String],
    paths: &'a [Vec<String>],
}

impl Context<'_> {
    /// Whether values of the type have the implementations, given which
    /// declarations of the program do.
    fn allowed(&self, ty: &ast::TypeExpr, declared: &dyn Fn(usize, &str) -> bool) -> bool {
        let each = |types: &[ast::TypeExpr]| types.iter().all(|t| self.allowed(t, declared));
        match &ty.kind {
            ast::TypeKind::Optional(inner) => self.allowed(inner, declared),
            ast::TypeKind::Result { ok, err } if self.kind == Kind::Reflect => {
                ok.as_ref().is_none_or(|ok| self.allowed(ok, declared)) && self.allowed(err, declared)
            }
            ast::TypeKind::Path { segments, args } => {
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                match (names.as_slice(), args.as_slice()) {
                    ([name], []) if self.generics.iter().any(|g| g == name) => true,
                    ([name], args) if let Some(builtin) = self.kind.builtin(name, args) => builtin && each(args),
                    ([name], args) => {
                        if let Some(alias) = self.file.items.iter().find_map(|i| match i {
                            ast::Item::Alias(a) if a.name.name == *name => Some(a),
                            _ => None,
                        }) {
                            return args.is_empty() && self.allowed(&alias.target, declared);
                        }
                        declared(self.module, name) && each(args)
                    }
                    ([module, name], args) => {
                        let import = self
                            .file
                            .imports
                            .iter()
                            .find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == *module);
                        let Some(import) = import else { return false };
                        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
                        if let ["std", standard] = path.as_slice() {
                            return args.is_empty() && self.kind.standard(standard, name);
                        }
                        match self.paths.iter().position(|p| p.iter().map(String::as_str).eq(path.iter().copied())) {
                            Some(target) => declared(target, name) && each(args),
                            None => false,
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

/// The name a field has in every format, `@name` or its own.
fn wire(field: &ast::FieldDecl) -> String {
    field
        .annotations
        .iter()
        .find(|a| a.name.name == "name")
        .and_then(|a| match &a.argument {
            Some(ast::AnnotationArg::Text(text)) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| field.name.name.clone())
}

fn ignored(field: &ast::FieldDecl) -> bool {
    field.annotations.iter().any(|a| a.name.name == "ignore")
}

/// The formats whose annotations the implementations give them as notes
/// (standard library 14.7).
const NOTED: &[&str] = &["xml"];

/// Writes the source of the implementations for one declaration.
struct Generator<'a> {
    /// The alias of the standard module in the type's file.
    alias: &'a str,
    /// The formats of `NOTED` the type's file imports, and their aliases.
    noted: Vec<(&'static str, String)>,
    /// Whether the type's file implements `Display` for it, which
    /// reflection shows it with.
    displayed: bool,
    /// The alias of `std.validate` in the type's file, whose rules `Decode`
    /// checks (standard library 14.35).
    validate: Option<String>,
    sources: &'a SourceMap,
    declared: &'a Declared,
}

impl Generator<'_> {
    fn text(&self, ty: &ast::TypeExpr) -> String {
        self.sources.slice(ty.span).to_string()
    }

    /// The notes of the annotations of formats among some, as an array:
    /// `serialization.Note { format: "xml", name: "Name", value: "item" }`
    /// for `@xml.Name("item")`.
    fn notes(&self, uses: &[ast::AnnotationUse]) -> String {
        let s = self.alias;
        let notes: Vec<String> = uses
            .iter()
            .filter_map(|annotation| {
                let [module, name] = &annotation.path[..] else { return None };
                let (format, _) = self.noted.iter().find(|(_, alias)| *alias == module.name)?;
                let value = annotation.args.first().map_or(String::new(), |arg| match &arg.value.kind {
                    ast::ExprKind::Str(segments) => match &segments[..] {
                        [ast::StrSegment::Text(text)] => text.clone(),
                        _ => String::new(),
                    },
                    _ => self.sources.slice(arg.value.span).to_string(),
                });
                Some(format!("{s}.Note {{ format: {format:?}, name: {:?}, value: {} }}", name.name, literal(&value)))
            })
            .collect();
        format!("[{}]", notes.join(", "))
    }

    /// `impl[T: module.Trait] module.Trait for Name[T]`
    fn header(&self, name: &str) -> String {
        let j = self.alias;
        let d = self.declared;
        if d.generics.is_empty() {
            return format!("impl {j}.{name} for {} {{\n", d.name);
        }
        let binders: Vec<String> = d.generics.iter().map(|g| format!("{g}: {j}.{name}")).collect();
        format!("impl[{}] {j}.{name} for {}[{}] {{\n", binders.join(", "), d.name, d.generics.join(", "))
    }

    fn target(&self) -> String {
        let d = self.declared;
        if d.generics.is_empty() { d.name.clone() } else { format!("{}[{}]", d.name, d.generics.join(", ")) }
    }

    fn encode(&self, item: &ast::Item) -> String {
        let s = self.alias;
        let mut out = self.header("Encode");
        out.push_str(&format!("    fn encode[__S: {s}.Sink](self, out: __S) {{\n"));
        match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) => {
                    out.push_str(&self.write_fields((&t.name.name, &t.uses), fields, "self.", "        "))
                }
                ast::TypeBody::Newtype(_) => out.push_str(&format!("        {s}.Encode.encode(self.value, out)\n")),
                ast::TypeBody::Opaque => unreachable!("extern types are not derived"),
            },
            ast::Item::Enum(e) => {
                out.push_str("        match self {\n");
                for variant in &e.variants {
                    let path = format!("{}.{}", e.name.name, variant.name.name);
                    let key = format!("{:?}", variant.name.name);
                    match &variant.payload {
                        ast::VariantPayload::Unit => out.push_str(&format!("            {path} => out.text({key})\n")),
                        ast::VariantPayload::Tuple(types) => {
                            let names: Vec<String> = (0..types.len()).map(|i| format!("__{i}")).collect();
                            out.push_str(&format!("            {path}({}) => {{\n", names.join(", ")));
                            out.push_str(&format!("                out.beginMap(1)\n                out.key({key})\n"));
                            if types.len() > 1 {
                                out.push_str(&format!("                out.beginList({})\n", types.len()));
                            }
                            for name in &names {
                                out.push_str(&format!("                {s}.Encode.encode({name}, out)\n"));
                            }
                            if types.len() > 1 {
                                out.push_str("                out.endList()\n");
                            }
                            out.push_str("                out.endMap()\n            }\n");
                        }
                        ast::VariantPayload::Record(fields) => {
                            let names: Vec<String> =
                                fields.iter().map(|f| format!("{}: __field_{}", f.name.name, f.name.name)).collect();
                            out.push_str(&format!("            {path} {{ {} }} => {{\n", names.join(", ")));
                            out.push_str(&format!("                out.beginMap(1)\n                out.key({key})\n"));
                            let record = (variant.name.name.as_str(), variant.uses.as_slice());
                            out.push_str(&self.write_fields(record, fields, "__field_", "                "));
                            out.push_str("                out.endMap()\n            }\n");
                        }
                    }
                }
                out.push_str("        }\n");
            }
            _ => {}
        }
        out.push_str("    }\n}\n\n");
        out
    }

    /// The statements that write the fields of a record as a map, each
    /// value read as `{prefix}name`, after the record's name and notes. A
    /// format without null leaves out the optional ones that are `none`.
    fn write_fields(
        &self,
        (name, uses): (&str, &[ast::AnnotationUse]),
        fields: &[ast::FieldDecl],
        prefix: &str,
        indent: &str,
    ) -> String {
        let s = self.alias;
        let written: Vec<&ast::FieldDecl> = fields.iter().filter(|f| !ignored(f)).collect();
        let optional = |f: &ast::FieldDecl| self.text(&f.ty).ends_with('?');
        let mut out = format!("{indent}out.record({name:?}, {})\n", self.notes(uses));
        // A `none` asks the format whether it leaves the member out; a
        // value asks nothing.
        let count = if written.iter().any(|f| optional(f)) {
            out.push_str(&format!("{indent}var __count = {}\n", written.len()));
            for field in written.iter().filter(|f| optional(f)) {
                let name = &field.name.name;
                out.push_str(&format!(
                    "{indent}let __skip_{name} = match {prefix}{name} {{\n{indent}    none => out.omitsNone()\n{indent}    _ => false\n{indent}}}\n{indent}if __skip_{name} {{\n{indent}    __count -= 1\n{indent}}}\n"
                ));
            }
            "__count".to_string()
        } else {
            written.len().to_string()
        };
        out.push_str(&format!("{indent}out.beginMap({count})\n"));
        for field in written {
            let value = format!("{prefix}{}", field.name.name);
            let mut write = format!("out.key({:?})\n{indent}{s}.Encode.encode({value}, out)", wire(field));
            let notes = self.notes(&field.uses);
            if notes != "[]" {
                write = format!("out.field({notes})\n{indent}{write}");
            }
            if optional(field) {
                out.push_str(&format!(
                    "{indent}if not __skip_{} {{\n{indent}    {}\n{indent}}}\n",
                    field.name.name,
                    write.replace(&format!("\n{indent}"), &format!("\n{indent}    "))
                ));
            } else {
                out.push_str(&format!("{indent}{write}\n"));
            }
        }
        out.push_str(&format!("{indent}out.endMap()\n"));
        out
    }

    /// The statements that read the members of a map into `__field_name`
    /// locals, one per field, until its end: named apart from the modules
    /// the prelude imports as `__name`, as `__text`. The source finds each
    /// key among the fields' and skips the members of none of them.
    fn read_fields(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let s = self.alias;
        let mut out = String::new();
        let read: Vec<&ast::FieldDecl> = fields.iter().filter(|f| !ignored(f)).collect();
        for field in &read {
            let ty = self.text(&field.ty);
            let local = if ty.ends_with('?') { format!("({ty})?") } else { format!("{ty}?") };
            out.push_str(&format!("{indent}var __field_{}: {local} = none\n", field.name.name));
        }
        let names: Vec<String> = read.iter().map(|field| format!("{:?}", wire(field))).collect();
        out.push_str(&format!("{indent}let __fieldNames: Array[String] = [{}]\n", names.join(", ")));
        let notes: Vec<String> = read.iter().map(|field| self.notes(&field.uses)).collect();
        if notes.iter().any(|notes| notes != "[]") {
            out.push_str(&format!("{indent}input.fields([{}])\n", notes.join(", ")));
        }
        out.push_str(&format!("{indent}for {{\n{indent}    match input.nextField(__fieldNames)? {{\n"));
        for (index, field) in read.iter().enumerate() {
            out.push_str(&format!(
                "{indent}        some({index}) => {{\n{indent}            __field_{} = some({s}.Decode.decode[{}](input, input.value()?)?)\n{indent}        }}\n",
                field.name.name,
                self.text(&field.ty)
            ));
        }
        out.push_str(&format!(
            "{indent}        some(_) => {{}}\n{indent}        none => break\n{indent}    }}\n{indent}}}\n"
        ));
        out
    }

    /// The fields of a record literal from the `__field_name` locals.
    fn fields(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let mut out = String::new();
        for field in fields {
            let name = &field.name.name;
            let value = if ignored(field) {
                "none".to_string()
            } else if self.text(&field.ty).ends_with('?') {
                format!(
                    "match __field_{name} {{\n{indent}    some(__value) => __value\n{indent}    none => none\n{indent}}}"
                )
            } else {
                format!("input.required(__field_{name}, {:?})?", wire(field))
            };
            out.push_str(&format!("{indent}{name}: {value}\n"));
        }
        out
    }

    /// The checks of the rules of `std.validate` on the fields of a record,
    /// one element of an array each, for `__rules.found`; the checker says
    /// which rules do not go with their field (`E1908`), and those are left
    /// out here.
    fn rules(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let Some(validate) = &self.validate else { return String::new() };
        let mut out = String::new();
        for field in fields {
            let written = self.text(&field.ty);
            let optional = written.ends_with('?');
            let inner = written.trim_end_matches('?').trim_start_matches('(').trim_end_matches(')').trim().to_string();
            let path = wire(field);
            let value = if optional { "__v".to_string() } else { format!("__value.{}", field.name.name) };
            let text = matches!(inner.as_str(), "String");
            let counted = text || ["Array[", "Map[", "Set["].iter().any(|p| inner.starts_with(p));
            let integer = crate::types::IntTy::from_name(&inner).is_some() || inner == "Byte";
            let float = matches!(inner.as_str(), "Float" | "Float32" | "Float64");
            for rule in &field.uses {
                let [module, name] = &rule.path[..] else { continue };
                if module.name != *validate {
                    continue;
                }
                let argument = |field: &str, position: usize| {
                    let named = rule.args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == field));
                    named.or_else(|| rule.args.iter().filter(|a| a.name.is_none()).nth(position))
                };
                let source = |arg: &ast::Arg| self.sources.slice(arg.value.span).to_string();
                let check = match name.name.as_str() {
                    "Length" if counted => {
                        let min = argument("min", 0).map_or("0".to_string(), source);
                        let max = argument("max", 1).map_or("none".to_string(), |a| format!("some({})", source(a)));
                        let unit = if text { "character" } else { "element" };
                        format!("__rules.length({path:?}, {value}.length(), {min}, {max}, {unit:?})")
                    }
                    "Min" | "Max" if integer || float || !(text || counted || inner == "Bool") => {
                        let Some(bound) = argument("value", 0).map(source) else { continue };
                        let fractional = bound.contains(['.', 'e', 'E']);
                        if integer && fractional {
                            continue;
                        }
                        let bound = if float && !fractional { format!("{bound}.0") } else { bound };
                        let which = if name.name == "Min" { "atLeast" } else { "atMost" };
                        format!("__rules.{which}({path:?}, {value}, {bound})")
                    }
                    "Matches" if text => {
                        let Some(pattern) = argument("pattern", 0).map(source) else { continue };
                        format!("__rules.matching({path:?}, {value}, {pattern})")
                    }
                    "Email" if text => format!("__rules.email({path:?}, {value})"),
                    _ => continue,
                };
                let check = match optional {
                    true => format!(
                        "match __value.{} {{\n{indent}    some(__v) => {check}\n{indent}    none => none\n{indent}}}",
                        field.name.name
                    ),
                    false => check,
                };
                out.push_str(&format!("{indent}{check},\n"));
            }
        }
        out
    }

    fn decode(&self, item: &ast::Item) -> String {
        let s = self.alias;
        let mut out = self.header("Decode");
        let target = self.target();
        out.push_str(&format!(
            "    fn decode[__S: {s}.Source](input: __S, first: {s}.Event): {target} ! {s}.Error {{\n"
        ));
        let name = &self.declared.name;
        match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) => {
                    out.push_str(&format!("        input.beginMap(first, {name:?})?\n"));
                    out.push_str(&self.read_fields(fields, "        "));
                    let checks = self.rules(fields, "            ");
                    if checks.is_empty() {
                        out.push_str(&format!(
                            "        {name} {{\n{}        }}\n",
                            self.fields(fields, "            ")
                        ));
                    } else {
                        out.push_str(&format!(
                            "        let __value = {name} {{\n{}        }}\n",
                            self.fields(fields, "            ")
                        ));
                        out.push_str(&format!("        let __violations = __rules.found([\n{checks}        ])\n"));
                        out.push_str("        if not __violations.isEmpty() {\n            fail input.invalid(__violations)\n        }\n");
                        out.push_str("        __value\n");
                    }
                }
                ast::TypeBody::Newtype(inner) => {
                    out.push_str(&format!("        {name}({s}.Decode.decode[{}](input, first)?)\n", self.text(inner)));
                }
                ast::TypeBody::Opaque => unreachable!("extern types are not derived"),
            },
            ast::Item::Enum(e) => {
                let unknown =
                    format!("fail input.failure({s}.ErrorKind.Type, \"unknown variant `{{__variant}}` of `{name}`\")");
                out.push_str("        match first {\n");
                out.push_str(&format!(
                    "            {s}.Event.Untyped(__untyped) => {s}.Decode.decode[{target}](input, {s}.Event.Text(__untyped))?\n"
                ));
                out.push_str(&format!("            {s}.Event.Text(__variant) => match __variant {{\n"));
                for variant in &e.variants {
                    if let ast::VariantPayload::Unit = variant.payload {
                        out.push_str(&format!(
                            "                {:?} => {name}.{}\n",
                            variant.name.name, variant.name.name
                        ));
                    }
                }
                out.push_str(&format!("                _ => {unknown}\n            }}\n"));
                let carrying = e.variants.iter().any(|v| !matches!(v.payload, ast::VariantPayload::Unit));
                if !carrying {
                    out.push_str(&format!(
                        "            _ => fail input.mismatch(first, \"a variant of `{name}`\")\n        }}\n"
                    ));
                    out.push_str("    }\n}\n\n");
                    return out;
                }
                out.push_str(&format!("            {s}.Event.BeginMap => {{\n"));
                out.push_str(&format!(
                    "                let __variant = match input.nextKey()? {{\n                    some(__key) => __key\n                    none => fail input.failure({s}.ErrorKind.Type, \"expected a variant of `{name}`\")\n                }}\n"
                ));
                out.push_str(
                    "                let __event = input.value()?\n                let __value = match __variant {\n",
                );
                for variant in &e.variants {
                    let path = format!("{name}.{}", variant.name.name);
                    let key = format!("{:?}", variant.name.name);
                    match &variant.payload {
                        ast::VariantPayload::Unit => {}
                        ast::VariantPayload::Tuple(types) if types.len() == 1 => {
                            out.push_str(&format!(
                                "                    {key} => {path}({s}.Decode.decode[{}](input, __event)?)\n",
                                self.text(&types[0])
                            ));
                        }
                        ast::VariantPayload::Tuple(types) => {
                            out.push_str(&format!(
                                "                    {key} => {{\n                        input.beginList(__event, {key})?\n"
                            ));
                            for (index, ty) in types.iter().enumerate() {
                                out.push_str(&format!(
                                    "                        let __{index} = {s}.Decode.decode[{}](input, input.value()?)?\n",
                                    self.text(ty)
                                ));
                            }
                            let names: Vec<String> = (0..types.len()).map(|i| format!("__{i}")).collect();
                            out.push_str(&format!(
                                "                        input.endList()?\n                        {path}({})\n                    }}\n",
                                names.join(", ")
                            ));
                        }
                        ast::VariantPayload::Record(fields) => {
                            out.push_str(&format!(
                                "                    {key} => {{\n                        input.beginMap(__event, {key})?\n"
                            ));
                            out.push_str(&self.read_fields(fields, "                        "));
                            out.push_str(&format!(
                                "                        {path} {{\n{}                        }}\n                    }}\n",
                                self.fields(fields, "                            ")
                            ));
                        }
                    }
                }
                out.push_str(&format!("                    _ => {unknown}\n                }}\n"));
                out.push_str(&format!(
                    "                match input.nextKey()? {{\n                    none => __value\n                    some(_) => fail input.failure({s}.ErrorKind.Type, \"a variant of `{name}` is a map with one key\")\n                }}\n            }}\n"
                ));
                out.push_str(&format!(
                    "            _ => fail input.mismatch(first, \"a variant of `{name}`\")\n        }}\n"
                ));
            }
            _ => {}
        }
        out.push_str("    }\n}\n\n");
        out
    }
}

impl Generator<'_> {
    /// `impl Reflect`: fields by name, the variant, display, conversion by
    /// shape and the `pub` methods.
    fn reflect(&self, item: &ast::Item, methods: &[Method]) -> String {
        let mut out = self.header("Reflect");
        match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) => out.push_str(&self.reflect_record(fields)),
                ast::TypeBody::Newtype(_) => {
                    let (r, name) = (self.alias, &self.declared.name);
                    out.push_str(&format!(
                        "    fn reflectShow(self, __nested: Bool): String {{\n        \"{name}({{{r}.Reflect.reflectShow(self.value, true)}})\"\n    }}\n\n"
                    ));
                }
                ast::TypeBody::Opaque => unreachable!("extern types are not derived"),
            },
            ast::Item::Enum(e) => out.push_str(&self.reflect_enum(e)),
            _ => {}
        }
        // A type with a `Display` shows as it displays.
        if self.displayed
            && let Some(start) = out.find("    fn reflectShow(")
            && let Some(length) = out[start..].find("\n    }\n\n")
        {
            let shown = "    fn reflectShow(self, __nested: Bool): String {\n        \"{self}\"\n    }\n\n";
            out.replace_range(start..start + length + 8, shown);
        }
        out.push_str(&self.reflect_methods(methods));
        out.push_str(&self.reflect_description(item, methods));
        out.push_str("}\n\n");
        out
    }

    /// `reflectDescription`: the kind, generic arguments, visible fields
    /// with their documentation, variants and methods.
    fn reflect_description(&self, item: &ast::Item, methods: &[Method]) -> String {
        let r = self.alias;
        let info = |ty: &str| format!("{r}.typeInfo[{ty}]()");
        // The program's annotations of a thing (27.13), which the checker
        // gives as their values: where it is, and a method by its name.
        let target = self.target();
        let notes = |place: u32, index: usize, part: usize, method: &str| {
            format!(".annotated({r}.annotationsOf[{target}]({place}, {index}, {part}, {method:?}))")
        };
        // A record's fields, or a record variant's (`variant`).
        let fields = |fields: &[ast::FieldDecl], variant: Option<usize>| -> String {
            let described: Vec<String> = fields
                .iter()
                .enumerate()
                .filter(|(_, f)| !f.private)
                .map(|(ordinal, f)| {
                    let docs = docs(self.sources, f.name.span)
                        .map_or("none".to_string(), |d| format!("some({})", literal(&d)));
                    let annotated = match (f.uses.is_empty(), variant) {
                        (true, _) => String::new(),
                        (false, None) => notes(1, ordinal, 0, ""),
                        (false, Some(variant)) => notes(3, variant, ordinal, ""),
                    };
                    format!(
                        "{r}.FieldInfo.of({:?}, {ordinal}, {docs}, {}){annotated}",
                        f.name.name,
                        info(&self.text(&f.ty))
                    )
                })
                .collect();
            format!("[{}]", described.join(", "))
        };
        let mut parts: Vec<String> = Vec::new();
        let kind = match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(declared) => {
                    parts.push(format!("fields: {}", fields(declared, None)));
                    "Record"
                }
                ast::TypeBody::Newtype(inner) => {
                    parts.push(format!("underlying: some({})", info(&self.text(inner))));
                    "Newtype"
                }
                ast::TypeBody::Opaque => "Opaque",
            },
            ast::Item::Enum(e) => {
                let variants: Vec<String> = e
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(ordinal, v)| {
                        let name = &v.name.name;
                        let (kind, elements, named) = match &v.payload {
                            ast::VariantPayload::Unit => ("Unit", "[]".to_string(), "[]".to_string()),
                            ast::VariantPayload::Tuple(types) => {
                                let elements: Vec<String> = types.iter().map(|t| info(&self.text(t))).collect();
                                ("Tuple", format!("[{}]", elements.join(", ")), "[]".to_string())
                            }
                            ast::VariantPayload::Record(declared) => {
                                ("Record", "[]".to_string(), fields(declared, Some(ordinal)))
                            }
                        };
                        let annotated = if v.uses.is_empty() { String::new() } else { notes(2, ordinal, 0, "") };
                        format!(
                            "{r}.VariantInfo.of({name:?}, {ordinal}, {r}.VariantPayloadKind.{kind}, {elements}, {named}){annotated}{}",
                            documented(docs(self.sources, v.name.span))
                        )
                    })
                    .collect();
                parts.push(format!("variants: [{}]", variants.join(", ")));
                "Enum"
            }
            _ => "Opaque",
        };
        let (annotated, name) = match item {
            ast::Item::Type(t) => (!t.uses.is_empty(), Some(t.name.span)),
            ast::Item::Enum(e) => (!e.uses.is_empty(), Some(e.name.span)),
            _ => (false, None),
        };
        if let Some(docs) = name.and_then(|span| docs(self.sources, span)) {
            parts.push(format!("docs: some({})", literal(&docs)));
        }
        if annotated {
            parts.push(format!("annotations: {r}.annotationsOf[{target}](0, 0, 0, \"\")"));
        }
        let generics: Vec<String> = self.declared.generics.iter().map(|g| info(g)).collect();
        if !generics.is_empty() {
            parts.push(format!("generics: [{}]", generics.join(", ")));
        }
        let described: Vec<String> = methods
            .iter()
            .map(|m| {
                let receiver = if m.mutating { "Mut" } else { "Ref" };
                let params: Vec<String> = m
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| {
                        // Its index among all the parameters counts the receiver.
                        let annotated = if m.annotated_params[i] { notes(5, i + 1, 0, &m.name) } else { String::new() };
                        format!(
                            "{r}.ParameterInfo.of({i}, {}, {r}.ParameterMode.Value, {:?}){annotated}",
                            info(ty),
                            m.names[i]
                        )
                    })
                    .collect();
                let annotated = if m.annotated { notes(4, 0, 0, &m.name) } else { String::new() };
                format!(
                    "{r}.MethodInfo.of({:?}, {r}.ParameterMode.{receiver}, {r}.FunctionInfo.of([{}], {}, false, false)){annotated}{}",
                    m.name,
                    params.join(", "),
                    info(&m.result),
                    documented(m.docs.clone())
                )
            })
            .collect();
        if !described.is_empty() {
            parts.push(format!("methods: [{}]", described.join(", ")));
        }
        let mut out = format!(
            "    fn reflectDescription(): {r}.TypeDescription {{\n        {r}.TypeDescription.of({r}.TypeKind.{kind})"
        );
        if !parts.is_empty() {
            out.push_str(" with {\n");
            for part in parts {
                out.push_str(&format!("            {part}\n"));
            }
            out.push_str("        }");
        }
        out.push_str("\n    }\n\n");
        out
    }

    /// `"a: {show(a)}, b: {show(b)}"`, for parts and the locals they are in.
    fn shown(&self, parts: &[(String, String)], labels: bool) -> String {
        let r = self.alias;
        let shown: Vec<String> = parts
            .iter()
            .map(|(label, local)| {
                let value = format!("{{{r}.Reflect.reflectShow({local}, true)}}");
                if labels { format!("{label}: {value}") } else { value }
            })
            .collect();
        shown.join(", ")
    }

    /// The fields of a record literal from the fields of `__value`.
    fn converted(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let mut out = String::new();
        for field in fields {
            let name = &field.name.name;
            if matches!(field.ty.kind, ast::TypeKind::Optional(_)) {
                out.push_str(&format!(
                    "{indent}{name}: match __value.optional({name:?})? {{\n{indent}    some(__found) => __found.to()?\n{indent}    none => none\n{indent}}}\n"
                ));
            } else {
                out.push_str(&format!("{indent}{name}: __value.get({name:?})?.to()?\n"));
            }
        }
        out
    }

    fn reflect_record(&self, fields: &[ast::FieldDecl]) -> String {
        let (r, name) = (self.alias, &self.declared.name);
        let visible: Vec<&str> = fields.iter().filter(|f| !f.private).map(|f| f.name.name.as_str()).collect();
        let listed: Vec<String> = visible.iter().map(|f| format!("{f:?}")).collect();
        let missing = format!("fail {r}.ReflectError.noField({name:?}, __name, [{}])", listed.join(", "));
        let mut out = format!(
            "    fn reflectGet(self, __name: String): {r}.Value ! {r}.ReflectError {{\n        match __name {{\n"
        );
        for f in &visible {
            out.push_str(&format!("            {f:?} => {r}.valueOf(self.{f})\n"));
        }
        out.push_str(&format!("            _ => {missing}\n        }}\n    }}\n\n"));
        out.push_str(&format!(
            "    fn reflectSet(mut self, __name: String, __value: {r}.Value): Unit ! {r}.ReflectError {{\n        match __name {{\n"
        ));
        for f in &visible {
            out.push_str(&format!(
                "            {f:?} => {{\n                self.{f} = __value.to()?\n            }}\n"
            ));
        }
        out.push_str(&format!("            _ => {missing}\n        }}\n    }}\n\n"));
        out.push_str(&format!("    fn reflectFields(self): Array[{r}.Field]? {{\n        some([\n"));
        for f in &visible {
            out.push_str(&format!("            {r}.Field {{ name: {f:?}, value: {r}.valueOf(self.{f}) }},\n"));
        }
        out.push_str("        ])\n    }\n\n");
        let parts: Vec<(String, String)> = visible.iter().map(|f| (f.to_string(), format!("self.{f}"))).collect();
        let text =
            if parts.is_empty() { name.clone() } else { format!("{name} {{{{ {} }}}}", self.shown(&parts, true)) };
        out.push_str(&format!("    fn reflectShow(self, __nested: Bool): String {{\n        \"{text}\"\n    }}\n\n"));
        out.push_str(&format!("    fn reflectFrom(__value: {r}.Value): {} ! {r}.ReflectError {{\n", self.target()));
        if fields.iter().any(|f| f.private) {
            out.push_str(&format!("        fail {r}.ReflectError.privateFields({name:?}, __value)\n"));
        } else {
            out.push_str(&format!("        {name} {{\n{}        }}\n", self.converted(fields, "            ")));
        }
        out.push_str("    }\n\n");
        out
    }

    fn reflect_enum(&self, e: &ast::EnumDecl) -> String {
        let (r, name) = (self.alias, &self.declared.name);
        // Each variant's pattern binding its payload to `__part` locals, and
        // the payload's labels with their locals.
        let variants: Vec<Arm> = e
            .variants
            .iter()
            .map(|v| {
                let variant = v.name.name.as_str();
                let (pattern, parts) = match &v.payload {
                    ast::VariantPayload::Unit => (format!("{name}.{variant}"), Vec::new()),
                    ast::VariantPayload::Tuple(types) => {
                        let parts: Vec<(String, String)> =
                            (0..types.len()).map(|i| (i.to_string(), format!("__{i}"))).collect();
                        let locals: Vec<&str> = parts.iter().map(|(_, l)| l.as_str()).collect();
                        (format!("{name}.{variant}({})", locals.join(", ")), parts)
                    }
                    ast::VariantPayload::Record(fields) => {
                        let parts: Vec<(String, String)> =
                            fields.iter().map(|f| (f.name.name.clone(), format!("__field_{}", f.name.name))).collect();
                        let bound: Vec<String> = parts.iter().map(|(f, l)| format!("{f}: {l}")).collect();
                        (format!("{name}.{variant} {{ {} }}", bound.join(", ")), parts)
                    }
                };
                Arm { variant, pattern, parts, payload: &v.payload }
            })
            .collect();
        let record = |payload: &ast::VariantPayload| matches!(payload, ast::VariantPayload::Record(_));
        let arms = |body: ArmBody| -> String {
            let mut out = String::from("        match self {\n");
            for arm in &variants {
                out.push_str(&format!(
                    "            {} => {}\n",
                    arm.pattern,
                    body(arm.variant, &arm.parts, arm.payload)
                ));
            }
            out.push_str("        }\n");
            out
        };
        let mut out =
            format!("    fn reflectVariant(self): String? {{\n{}    }}\n\n", arms(&|v, _, _| format!("some({v:?})")));
        out.push_str(&format!(
            "    fn reflectElements(self): Array[{r}.Value]? {{\n{}    }}\n\n",
            arms(&|_, parts, payload| {
                let values: Vec<String> = if record(payload) {
                    Vec::new()
                } else {
                    parts.iter().map(|(_, l)| format!("{r}.valueOf({l})")).collect()
                };
                format!("some([{}])", values.join(", "))
            })
        ));
        out.push_str(&format!(
            "    fn reflectFields(self): Array[{r}.Field]? {{\n{}    }}\n\n",
            arms(&|_, parts, payload| {
                let fields: Vec<String> = if record(payload) {
                    parts.iter().map(|(f, l)| format!("{r}.Field {{ name: {f:?}, value: {r}.valueOf({l}) }}")).collect()
                } else {
                    Vec::new()
                };
                format!("some([{}])", fields.join(", "))
            })
        ));
        let missing = |variant: &str, parts: &[(String, String)]| {
            let listed: Vec<String> = parts.iter().map(|(f, _)| format!("{f:?}")).collect();
            format!("fail {r}.ReflectError.noField(\"{name}.{variant}\", __name, [{}])", listed.join(", "))
        };
        out.push_str(&format!(
            "    fn reflectGet(self, __name: String): {r}.Value ! {r}.ReflectError {{\n{}    }}\n\n",
            arms(&|variant, parts, _| {
                let mut body = String::from("match __name {\n");
                for (label, local) in parts {
                    body.push_str(&format!("                {label:?} => {r}.valueOf({local})\n"));
                }
                body.push_str(&format!("                _ => {}\n            }}", missing(variant, parts)));
                body
            })
        ));
        out.push_str(&format!(
            "    fn reflectSet(mut self, __name: String, __value: {r}.Value): Unit ! {r}.ReflectError {{\n{}    }}\n\n",
            arms(&|variant, parts, payload| {
                let mut body = String::from("match __name {\n");
                for (index, (label, _)) in parts.iter().enumerate() {
                    let values: Vec<String> = parts
                        .iter()
                        .enumerate()
                        .map(|(i, (f, l))| {
                            let value = if i == index { "__value.to()?".to_string() } else { l.clone() };
                            if record(payload) { format!("{f}: {value}") } else { value }
                        })
                        .collect();
                    let rebuilt = if record(payload) {
                        format!("{name}.{variant} {{ {} }}", values.join(", "))
                    } else {
                        format!("{name}.{variant}({})", values.join(", "))
                    };
                    body.push_str(&format!(
                        "                {label:?} => {{\n                    self = {rebuilt}\n                }}\n"
                    ));
                }
                body.push_str(&format!("                _ => {}\n            }}", missing(variant, parts)));
                body
            })
        ));
        out.push_str(&format!(
            "    fn reflectShow(self, __nested: Bool): String {{\n{}    }}\n\n",
            arms(&|variant, parts, payload| match payload {
                ast::VariantPayload::Unit => format!("{variant:?}"),
                ast::VariantPayload::Tuple(_) => format!("\"{variant}({})\"", self.shown(parts, false)),
                ast::VariantPayload::Record(_) => format!("\"{variant} {{{{ {} }}}}\"", self.shown(parts, true)),
            })
        ));
        out.push_str(&format!(
            "    fn reflectFrom(__value: {r}.Value): {} ! {r}.ReflectError {{\n        match __value.variant() {{\n",
            self.target()
        ));
        let wrong = format!("fail {r}.ReflectError.wrongType({name:?}, __value)");
        for v in &e.variants {
            let variant = &v.name.name;
            let body = match &v.payload {
                ast::VariantPayload::Unit => format!("{name}.{variant}"),
                ast::VariantPayload::Tuple(types) => {
                    let items: Vec<String> = (0..types.len()).map(|i| format!("__items[{i}].to()?")).collect();
                    format!(
                        "{{\n                let __items = __value.elements()\n                if __items.length() != {} {{\n                    {wrong}\n                }}\n                {name}.{variant}({})\n            }}",
                        types.len(),
                        items.join(", ")
                    )
                }
                ast::VariantPayload::Record(fields) => {
                    format!("{name}.{variant} {{\n{}            }}", self.converted(fields, "                "))
                }
            };
            out.push_str(&format!("            some({variant:?}) => {body}\n"));
        }
        out.push_str(&format!("            _ => {wrong}\n        }}\n    }}\n\n"));
        out
    }

    /// `reflectCall` and `reflectCallMut`, when the type has methods to call.
    fn reflect_methods(&self, methods: &[Method]) -> String {
        if methods.is_empty() {
            return String::new();
        }
        let (r, name) = (self.alias, &self.declared.name);
        let listed: Vec<String> = methods.iter().map(|m| format!("{:?}", m.name)).collect();
        let mut out = String::new();
        for (function, receiver) in [("reflectCall", "self"), ("reflectCallMut", "mut self")] {
            out.push_str(&format!(
                "    fn {function}({receiver}, __method: String, __args: Array[{r}.Value]): {r}.Value ! {r}.ReflectError {{\n        match __method {{\n"
            ));
            for method in methods {
                let m = &method.name;
                if method.mutating && receiver == "self" {
                    out.push_str(&format!("            {m:?} => fail {r}.ReflectError.needsMut({name:?}, __method)\n"));
                    continue;
                }
                let count = method.params.len();
                let args: Vec<String> = (0..count).map(|i| format!("__args[{i}].to()?")).collect();
                out.push_str(&format!(
                    "            {m:?} => {{\n                if __args.length() != {count} {{\n                    fail {r}.ReflectError.arguments({name:?}, __method, {count}, __args.length())\n                }}\n                {r}.valueOf(self.{m}({}))\n            }}\n",
                    args.join(", ")
                ));
            }
            out.push_str(&format!(
                "            _ => fail {r}.ReflectError.noMethod({name:?}, __method, [{}])\n        }}\n    }}\n\n",
                listed.join(", ")
            ));
        }
        out
    }
}
