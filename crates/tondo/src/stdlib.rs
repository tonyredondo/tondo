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
];

/// The module written in Tondo an import brings: `std.testing` brings
/// `std.json`, which `testing.snapshot` encodes values with.
fn source(path: &[ast::Ident]) -> Option<(&'static str, &'static str)> {
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

// ---------------------------------------------------------------------------
// Automatic `json.Encode` and `json.Decode`
// ---------------------------------------------------------------------------

/// A record, enum or newtype of the program.
struct Declared {
    module: usize,
    file: usize,
    name: String,
    generics: Vec<String>,
}

/// The alias a file imports `std.json` under, if any.
fn json_alias(file: &ast::Module) -> Option<String> {
    file.imports
        .iter()
        .find(|i| matches!(&i.path[..], [s, j] if s.name == "std" && j.name == "json"))
        .map(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name.clone())
}

/// Implements `json.Encode` and `json.Decode` for every record, enum and
/// newtype whose parts implement them, unless the program implements them
/// itself, when the program imports `std.json` (standard library 14.6) or
/// `std.testing`.
/// The implementations are Tondo source in the type's module, which the
/// rest of the compiler checks like any other.
pub fn derive_json(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    let brings = |file: &ast::Module| file.imports.iter().any(|i| source(&i.path).is_some_and(|(n, _)| n == "json"));
    if !modules.iter().any(|m| m.files.iter().any(|(f, _)| brings(f))) {
        return;
    }
    let mut declared: Vec<Declared> = Vec::new();
    for (module, source) in modules.iter().enumerate() {
        for (file, (parsed, _)) in source.files.iter().enumerate() {
            for item in &parsed.items {
                let (name, generics) = match item {
                    // C structs cross to C, not to JSON.
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
    // Which types can be encoded: all their parts can, computed as a
    // greatest fixed point so that recursive types qualify.
    let mut encodable = vec![true; declared.len()];
    loop {
        let mut changed = false;
        for index in 0..declared.len() {
            if !encodable[index] {
                continue;
            }
            let d = &declared[index];
            let file = &modules[d.module].files[d.file].0;
            let context = Context { module: d.module, file, generics: &d.generics, paths: &paths };
            let ok = parts(file, &d.name)
                .iter()
                .all(|ty| context.encodable(ty, &|m, n| find(m, n).is_some_and(|i| encodable[i])))
                && fields(file, &d.name).iter().all(|f| !ignored(f) || matches!(f.ty.kind, ast::TypeKind::Optional(_)));
            if !ok {
                encodable[index] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (module_index, module) in modules.iter_mut().enumerate() {
        for (file_index, (file, _)) in module.files.iter_mut().enumerate() {
            let mut text = String::new();
            let alias = json_alias(file).unwrap_or_else(|| "__json".into());
            for (index, d) in declared.iter().enumerate() {
                if d.module != module_index || d.file != file_index || !encodable[index] {
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
                let generator = Generator { json: &alias, sources, declared: d };
                if !implemented("Encode") {
                    text.push_str(&generator.encode(item));
                }
                if !implemented("Decode") {
                    text.push_str(&generator.decode(item));
                }
            }
            if text.is_empty() {
                continue;
            }
            if json_alias(file).is_none() {
                let span = file.imports.first().map_or(crate::source::Span::new(0, 0), |i| i.span);
                let ident = |name: &str| ast::Ident { name: name.into(), span };
                file.imports.push(ast::Import {
                    path: vec![ident("std"), ident("json")],
                    alias: Some(ident("__json")),
                    span,
                });
            }
            let name = format!("{}.derived.to", module.path.join("."));
            let id = sources.add(Path::new(&name), text);
            sources.set_identity(id, format!("<derived>/{name}"), module.path.join("."));
            let generated = parser::parse_file(sources.file(id), diags);
            file.items.extend(generated.items);
        }
    }
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
    module: usize,
    file: &'a ast::Module,
    generics: &'a [String],
    paths: &'a [Vec<String>],
}

impl Context<'_> {
    fn encodable(&self, ty: &ast::TypeExpr, declared: &dyn Fn(usize, &str) -> bool) -> bool {
        match &ty.kind {
            ast::TypeKind::Optional(inner) => self.encodable(inner, declared),
            ast::TypeKind::Path { segments, args } => {
                let all = |args: &[ast::TypeExpr]| args.iter().all(|a| self.encodable(a, declared));
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                match (names.as_slice(), args.as_slice()) {
                    ([name], []) if self.generics.iter().any(|g| g == name) => true,
                    (["Int" | "Float" | "Bool" | "String"], []) => true,
                    (["Array" | "Option"], [element]) => self.encodable(element, declared),
                    (["Map"], [key, value]) => {
                        matches!(&key.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == "String")
                            && self.encodable(value, declared)
                    }
                    ([name], args) => {
                        if let Some(alias) = self.file.items.iter().find_map(|i| match i {
                            ast::Item::Alias(a) if a.name.name == *name => Some(a),
                            _ => None,
                        }) {
                            return args.is_empty() && self.encodable(&alias.target, declared);
                        }
                        declared(self.module, name) && all(args)
                    }
                    ([module, name], args) => {
                        let import = self
                            .file
                            .imports
                            .iter()
                            .find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == *module);
                        let Some(import) = import else { return false };
                        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
                        if path == ["std", "json"] {
                            return matches!(*name, "Value" | "Number" | "Raw") && args.is_empty();
                        }
                        match self.paths.iter().position(|p| p.iter().map(String::as_str).eq(path.iter().copied())) {
                            Some(target) => declared(target, name) && all(args),
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

/// The name a field has in JSON, `@name` or its own.
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

/// Writes the source of the implementations for one declaration.
struct Generator<'a> {
    json: &'a str,
    sources: &'a SourceMap,
    declared: &'a Declared,
}

impl Generator<'_> {
    fn text(&self, ty: &ast::TypeExpr) -> String {
        self.sources.slice(ty.span).to_string()
    }

    /// `impl[T: json.Trait] json.Trait for Name[T]`
    fn header(&self, name: &str) -> String {
        let j = self.json;
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
        let mut out = self.header("Encode");
        out.push_str(&format!("    fn encodeJson(self, out: {}.Writer) {{\n", self.json));
        match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) => {
                    out.push_str("        out.beginObject()\n");
                    for field in fields.iter().filter(|f| !ignored(f)) {
                        out.push_str(&format!("        out.key({:?})\n", wire(field)));
                        out.push_str(&format!(
                            "        {}.Encode.encodeJson(self.{}, out)\n",
                            self.json, field.name.name
                        ));
                    }
                    out.push_str("        out.endObject()\n");
                }
                ast::TypeBody::Newtype(_) => {
                    out.push_str(&format!("        {}.Encode.encodeJson(self.value, out)\n", self.json))
                }
                ast::TypeBody::Opaque => unreachable!("extern types are not derived"),
            },
            ast::Item::Enum(e) => {
                out.push_str("        match self {\n");
                for variant in &e.variants {
                    let path = format!("{}.{}", e.name.name, variant.name.name);
                    let key = format!("{:?}", variant.name.name);
                    match &variant.payload {
                        ast::VariantPayload::Unit => {
                            out.push_str(&format!("            {path} => out.string({key})\n"))
                        }
                        ast::VariantPayload::Tuple(types) => {
                            let names: Vec<String> = (0..types.len()).map(|i| format!("__{i}")).collect();
                            out.push_str(&format!("            {path}({}) => {{\n", names.join(", ")));
                            out.push_str(&format!(
                                "                out.beginObject()\n                out.key({key})\n"
                            ));
                            if types.len() > 1 {
                                out.push_str("                out.beginArray()\n");
                            }
                            for name in &names {
                                out.push_str(&format!(
                                    "                {}.Encode.encodeJson({name}, out)\n",
                                    self.json
                                ));
                            }
                            if types.len() > 1 {
                                out.push_str("                out.endArray()\n");
                            }
                            out.push_str("                out.endObject()\n            }\n");
                        }
                        ast::VariantPayload::Record(fields) => {
                            let names: Vec<String> =
                                fields.iter().map(|f| format!("{}: __{}", f.name.name, f.name.name)).collect();
                            out.push_str(&format!("            {path} {{ {} }} => {{\n", names.join(", ")));
                            out.push_str(&format!("                out.beginObject()\n                out.key({key})\n                out.beginObject()\n"));
                            for field in fields.iter().filter(|f| !ignored(f)) {
                                out.push_str(&format!(
                                    "                out.key({:?})\n                {}.Encode.encodeJson(__{}, out)\n",
                                    wire(field),
                                    self.json,
                                    field.name.name
                                ));
                            }
                            out.push_str(
                                "                out.endObject()\n                out.endObject()\n            }\n",
                            );
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

    /// The statements that read the members of an object into `__name`
    /// locals, one per field, until its end.
    fn read_fields(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let j = self.json;
        let mut out = String::new();
        for field in fields.iter().filter(|f| !ignored(f)) {
            let ty = self.text(&field.ty);
            let local = if ty.ends_with('?') { format!("({ty})?") } else { format!("{ty}?") };
            out.push_str(&format!("{indent}var __{}: {local} = none\n", field.name.name));
        }
        out.push_str(&format!(
            "{indent}for {{\n{indent}    match input.nextKey()? {{\n{indent}        some(__member) => {{\n"
        ));
        out.push_str(&format!(
            "{indent}            let __memberEvent = input.value()?\n{indent}            match __member {{\n"
        ));
        for field in fields.iter().filter(|f| !ignored(f)) {
            out.push_str(&format!(
                "{indent}                {:?} => {{\n{indent}                    __{} = some({j}.Decode.decodeJson[{}](input, __memberEvent)?)\n{indent}                }}\n",
                wire(field),
                field.name.name,
                self.text(&field.ty)
            ));
        }
        out.push_str(&format!(
            "{indent}                _ => input.unknownField(__member, __memberEvent)?\n{indent}            }}\n"
        ));
        out.push_str(&format!("{indent}        }}\n{indent}        none => break\n{indent}    }}\n{indent}}}\n"));
        out
    }

    /// The fields of a record literal from the `__name` locals.
    fn fields(&self, fields: &[ast::FieldDecl], indent: &str) -> String {
        let mut out = String::new();
        for field in fields {
            let name = &field.name.name;
            let value = if ignored(field) {
                "none".to_string()
            } else if self.text(&field.ty).ends_with('?') {
                format!("match __{name} {{\n{indent}    some(__value) => __value\n{indent}    none => none\n{indent}}}")
            } else {
                format!("input.required(__{name}, {:?})?", wire(field))
            };
            out.push_str(&format!("{indent}{name}: {value}\n"));
        }
        out
    }

    fn decode(&self, item: &ast::Item) -> String {
        let j = self.json;
        let mut out = self.header("Decode");
        let target = self.target();
        out.push_str(&format!("    fn decodeJson(input: {j}.Reader, first: {j}.Event): {target} ! {j}.JsonError {{\n"));
        let name = &self.declared.name;
        match item {
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) => {
                    out.push_str(&format!("        input.beginObject(first, {name:?})?\n"));
                    out.push_str(&self.read_fields(fields, "        "));
                    out.push_str(&format!("        {name} {{\n{}        }}\n", self.fields(fields, "            ")));
                }
                ast::TypeBody::Newtype(inner) => {
                    out.push_str(&format!(
                        "        {name}({j}.Decode.decodeJson[{}](input, first)?)\n",
                        self.text(inner)
                    ));
                }
                ast::TypeBody::Opaque => unreachable!("extern types are not derived"),
            },
            ast::Item::Enum(e) => {
                let unknown =
                    format!("fail input.failure({j}.ErrorKind.Type, \"unknown variant `{{__name}}` of `{name}`\")");
                out.push_str("        match first {\n");
                out.push_str(&format!("            {j}.Event.Text(__name) => match __name {{\n"));
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
                out.push_str(&format!("            {j}.Event.BeginObject => {{\n"));
                out.push_str(&format!(
                    "                let __name = match input.nextKey()? {{\n                    some(__key) => __key\n                    none => fail input.failure({j}.ErrorKind.Type, \"expected a variant of `{name}`\")\n                }}\n"
                ));
                out.push_str(
                    "                let __event = input.value()?\n                let __value = match __name {\n",
                );
                for variant in &e.variants {
                    let path = format!("{name}.{}", variant.name.name);
                    let key = format!("{:?}", variant.name.name);
                    match &variant.payload {
                        ast::VariantPayload::Unit => {}
                        ast::VariantPayload::Tuple(types) if types.len() == 1 => {
                            out.push_str(&format!(
                                "                    {key} => {path}({j}.Decode.decodeJson[{}](input, __event)?)\n",
                                self.text(&types[0])
                            ));
                        }
                        ast::VariantPayload::Tuple(types) => {
                            out.push_str(&format!("                    {key} => {{\n                        input.beginArray(__event, {key})?\n"));
                            for (index, ty) in types.iter().enumerate() {
                                out.push_str(&format!(
                                    "                        let __{index} = {j}.Decode.decodeJson[{}](input, input.value()?)?\n",
                                    self.text(ty)
                                ));
                            }
                            let names: Vec<String> = (0..types.len()).map(|i| format!("__{i}")).collect();
                            out.push_str(&format!(
                                "                        input.endArray()?\n                        {path}({})\n                    }}\n",
                                names.join(", ")
                            ));
                        }
                        ast::VariantPayload::Record(fields) => {
                            out.push_str(&format!("                    {key} => {{\n                        input.beginObject(__event, {key})?\n"));
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
                    "                match input.nextKey()? {{\n                    none => __value\n                    some(_) => fail input.failure({j}.ErrorKind.Type, \"a variant of `{name}` is an object with one key\")\n                }}\n            }}\n"
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
