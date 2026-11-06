//! The implementations of `protobuf.Message` and `protobuf.Enum` that the
//! compiler writes (standard library 14.12): for each record whose fields
//! carry `@protobuf.Field`, code that writes and reads it field by field
//! with the exact protobuf type of each, and for each enum whose variants
//! carry `@protobuf.Value`, its numbers. A field or variant that protobuf
//! cannot write is `E1910`, with what to write instead.

use crate::diag::Diagnostics;
use crate::modules::ModuleSource;
use crate::source::{SourceMap, Span};
use crate::{ast, parser};
use std::fmt::Write;
use std::path::Path;

/// The alias a file imports a standard module under, if any.
fn alias_of(file: &ast::Module, module: &str) -> Option<String> {
    file.imports
        .iter()
        .find(|i| matches!(&i.path[..], [s, m] if s.name == "std" && m.name == module))
        .map(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name.clone())
}

/// A use of an annotation of `std.protobuf` among some: `@protobuf.Field(1)`.
fn annotation<'a>(uses: &'a [ast::AnnotationUse], alias: &str, name: &str) -> Option<&'a ast::AnnotationUse> {
    uses.iter().find(|u| matches!(&u.path[..], [module, annotation] if module.name == alias && annotation.name == name))
}

/// How an annotation says to write a field.
struct Annotated {
    number: i64,
    encoding: String,
    packed: bool,
    /// Its name in the `.proto`, when it is not the field's.
    name: Option<String>,
}

/// What a value of a field's type is to protobuf.
#[derive(Clone)]
enum Kind {
    /// A scalar, by its protobuf type: `int32`, `sint64`, `string`...
    Scalar(&'static str),
    Timestamp,
    Duration,
    /// A `json.Value` as a `Value`, a `Struct` or a `ListValue`.
    Json(&'static str),
    /// A message, by its type as written, and whether it is a group.
    Message(String, bool),
    /// A protobuf enum, by its type as written.
    Enum(String),
    /// A `oneof`: each variant's name, number, kind and name in the
    /// `.proto`.
    OneOf(Vec<(String, i64, Kind, String)>),
}

/// The shape of a field: its kind once, or optional, repeated or a map.
enum Shape {
    Single(Kind),
    Optional(Kind),
    /// An optional scalar as a wrapper of `google.protobuf`.
    Wrapped(Kind),
    Repeated(Kind),
    Map(Kind, Kind),
    /// The field that keeps the fields the message does not know.
    Unknown,
}

/// Where a type is declared: its module and file.
struct Found<'a> {
    module: usize,
    file: &'a ast::Module,
    item: &'a ast::Item,
}

/// Writes the implementations of every module whose files import
/// `std.protobuf`.
pub fn derive(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    let paths: Vec<Vec<String>> = modules.iter().map(|m| m.path.clone()).collect();
    let mut texts: Vec<Vec<String>> = modules.iter().map(|m| vec![String::new(); m.files.len()]).collect();
    for (module_index, module) in modules.iter().enumerate() {
        for (file_index, (file, _)) in module.files.iter().enumerate() {
            let Some(alias) = alias_of(file, "protobuf") else { continue };
            let generator = Generator { modules, paths: &paths, module: module_index, file, alias: &alias, sources };
            let mut text = String::new();
            for item in &file.items {
                match item {
                    ast::Item::Type(t) => {
                        // A message may have no numbered field: it is empty,
                        // or has only a oneof, as a `.proto` file's may.
                        if let ast::TypeBody::Record(fields) = &t.body
                            && (fields.iter().any(|f| {
                                annotation(&f.uses, &alias, "Field").is_some()
                                    || annotation(&f.uses, &alias, "Unknown").is_some()
                            }) || annotation(&t.uses, &alias, "Name").is_some())
                            && !generator.implemented(&t.name.name, "Message")
                            && let Some(code) = generator.message(t, fields, diags)
                        {
                            text.push_str(&code);
                        }
                    }
                    ast::Item::Enum(e)
                        if e.variants.iter().any(|v| annotation(&v.uses, &alias, "Value").is_some())
                            && !generator.implemented(&e.name.name, "Enum") =>
                    {
                        if let Some(code) = generator.enumeration(e, diags) {
                            text.push_str(&code);
                        }
                    }
                    _ => {}
                }
            }
            texts[module_index][file_index] = text;
        }
    }
    for (module_index, module) in modules.iter_mut().enumerate() {
        for (file_index, (file, _)) in module.files.iter_mut().enumerate() {
            let text = std::mem::take(&mut texts[module_index][file_index]);
            if text.is_empty() {
                continue;
            }
            let name = format!("{}.protobuf.derived.to", module.path.join("."));
            let id = sources.add(Path::new(&name), text);
            sources.set_identity(id, format!("<derived>/{name}"), module.path.join("."));
            let generated = parser::parse_file(sources.file(id), diags);
            file.items.extend(generated.items);
        }
    }
}

struct Generator<'a> {
    modules: &'a [ModuleSource],
    paths: &'a [Vec<String>],
    module: usize,
    file: &'a ast::Module,
    /// The alias of `std.protobuf` in the file.
    alias: &'a str,
    sources: &'a SourceMap,
}

impl Generator<'_> {
    fn text(&self, span: Span) -> String {
        self.sources.slice(span).to_string()
    }

    /// Whether the file implements a trait for a type itself.
    fn implemented(&self, name: &str, trait_name: &str) -> bool {
        self.file.items.iter().any(|item| match item {
            ast::Item::Impl(i) => {
                matches!(&i.trait_ref.kind, ast::TypeKind::Path { segments, .. } if segments.last().is_some_and(|s| s.name == trait_name))
                    && matches!(&i.target.kind, ast::TypeKind::Path { segments, .. } if segments.len() == 1 && segments[0].name == name)
            }
            _ => false,
        })
    }

    /// The number, encoding and packing an annotation gives.
    fn annotated(&self, uses: &[ast::AnnotationUse], name: &str) -> Option<Annotated> {
        let found = annotation(uses, self.alias, name)?;
        let mut annotated = Annotated { number: 0, encoding: "Default".into(), packed: true, name: None };
        for arg in &found.args {
            let text = self.text(arg.value.span);
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                None | Some("number") => annotated.number = text.replace('_', "").parse().unwrap_or(-1),
                Some("encoding") => annotated.encoding = text.rsplit('.').next().unwrap_or("").to_string(),
                Some("packed") => annotated.packed = text != "false",
                Some("name") => annotated.name = Some(text.trim_matches('"').to_string()),
                _ => {}
            }
        }
        Some(annotated)
    }

    /// The declaration a type names, in this module or one it imports.
    fn find(&self, ty: &ast::TypeExpr) -> Option<Found<'_>> {
        let ast::TypeKind::Path { segments, args } = &ty.kind else { return None };
        if !args.is_empty() {
            return None;
        }
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        let (module, name) = match names.as_slice() {
            [name] => (self.module, *name),
            [alias, name] => {
                let import = self
                    .file
                    .imports
                    .iter()
                    .find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == *alias)?;
                let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
                let target = self.paths.iter().position(|p| p.iter().map(String::as_str).eq(path.iter().copied()))?;
                (target, *name)
            }
            _ => return None,
        };
        self.modules[module].files.iter().find_map(|(file, _)| {
            let item = file.items.iter().find(|item| match item {
                ast::Item::Type(t) => t.name.name == name,
                ast::Item::Enum(e) => e.name.name == name,
                ast::Item::Alias(a) => a.name.name == name,
                _ => false,
            })?;
            Some(Found { module, file, item })
        })
    }

    /// The standard module and name a type names, as `("time", "DateTime")`.
    fn standard(&self, ty: &ast::TypeExpr) -> Option<(String, String)> {
        let ast::TypeKind::Path { segments, args } = &ty.kind else { return None };
        let [alias, name] = &segments[..] else { return None };
        if !args.is_empty() {
            return None;
        }
        let import =
            self.file.imports.iter().find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == alias.name)?;
        match &import.path[..] {
            // The alias `protobuf.JsonValue` is a `json.Value`.
            [std, module] if std.name == "std" && module.name == "protobuf" && name.name == "JsonValue" => {
                Some(("json".into(), "Value".into()))
            }
            [std, module] if std.name == "std" => Some((module.name.clone(), name.name.clone())),
            _ => None,
        }
    }

    /// The kind of a value of a type, written with an encoding; or why
    /// protobuf has none for it.
    fn kind(&self, ty: &ast::TypeExpr, encoding: &str) -> Result<Kind, String> {
        let plain = |name: &str| matches!(&ty.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == name);
        let bytes = matches!(&ty.kind, ast::TypeKind::Path { segments, args } if segments.len() == 1 && segments[0].name == "Array" && matches!(&args[..], [inner] if matches!(&inner.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == "Byte")));
        let scalar = |default: &'static str, zigzag: Option<&'static str>, fixed: Option<&'static str>| match encoding {
            "Default" | "Wrapper" => Ok(Kind::Scalar(default)),
            "ZigZag" => zigzag.map(Kind::Scalar).ok_or_else(|| format!("a `{default}` has no zigzag encoding")),
            "Fixed" => fixed.map(Kind::Scalar).ok_or_else(|| format!("a `{default}` has no fixed encoding")),
            other => Err(format!("`Encoding.{other}` does not go with a `{default}`")),
        };
        if plain("Int32") {
            return scalar("int32", Some("sint32"), Some("sfixed32"));
        }
        if plain("Int") || plain("Int64") {
            return scalar("int64", Some("sint64"), Some("sfixed64"));
        }
        if plain("UInt32") {
            return scalar("uint32", None, Some("fixed32"));
        }
        if plain("UInt") || plain("UInt64") {
            return scalar("uint64", None, Some("fixed64"));
        }
        for (name, protobuf) in
            [("Bool", "bool"), ("Float32", "float"), ("Float", "double"), ("Float64", "double"), ("String", "string")]
        {
            if plain(name) {
                return scalar(protobuf, None, None);
            }
        }
        if bytes {
            return scalar("bytes", None, None);
        }
        // An alias is its type, read where it is declared; a message or an
        // enum keeps the name written here.
        if let Some(Found { module, file, item: ast::Item::Alias(alias) }) = self.find(ty) {
            let protobuf = alias_of(file, "protobuf").unwrap_or_default();
            let declared = Generator { module, file, alias: &protobuf, ..*self };
            let written = self.text(ty.span);
            return match declared.kind(&alias.target, encoding)? {
                Kind::Message(_, group) => Ok(Kind::Message(written, group)),
                Kind::Enum(_) => Ok(Kind::Enum(written)),
                kind => Ok(kind),
            };
        }
        if let Some((module, name)) = self.standard(ty) {
            return match (module.as_str(), name.as_str(), encoding) {
                ("time", "DateTime", "Default") => Ok(Kind::Timestamp),
                ("time", "Duration", "Default") => Ok(Kind::Duration),
                ("json", "Value", "Default") => Ok(Kind::Json("Value")),
                ("json", "Value", "Struct") => Ok(Kind::Json("Struct")),
                ("json", "Value", "List") => Ok(Kind::Json("List")),
                ("protobuf", "Any" | "Empty" | "FieldMask", "Default") => Ok(Kind::Message(self.text(ty.span), false)),
                _ => Err(format!("protobuf has no type for `{}`: {FIELD_TYPES}", self.text(ty.span))),
            };
        }
        let written = self.text(ty.span);
        match self.find(ty).map(|found| found.item) {
            Some(ast::Item::Type(t)) if matches!(t.body, ast::TypeBody::Record(_)) => match encoding {
                "Default" => Ok(Kind::Message(written, false)),
                "Group" => Ok(Kind::Message(written, true)),
                other => Err(format!("`Encoding.{other}` does not go with a message")),
            },
            Some(ast::Item::Enum(e))
                if e.variants.iter().any(|v| annotation(&v.uses, self.alias, "Value").is_some()) =>
            {
                Ok(Kind::Enum(written))
            }
            Some(ast::Item::Enum(e))
                if e.variants.iter().any(|v| annotation(&v.uses, self.alias, "Field").is_some()) =>
            {
                let found = self.find(ty).map(|f| f.file);
                let mut variants = Vec::new();
                for variant in &e.variants {
                    let Some(annotated) = self.annotated(&variant.uses, "Field") else {
                        return Err(format!(
                            "the variant `{}` of the `oneof` `{written}` needs `@{}.Field(number)`",
                            variant.name.name, self.alias
                        ));
                    };
                    let ast::VariantPayload::Tuple(types) = &variant.payload else {
                        return Err(format!(
                            "the variant `{}` of the `oneof` `{written}` holds one value: `{}(T)`",
                            variant.name.name, variant.name.name
                        ));
                    };
                    let [inner] = &types[..] else {
                        return Err(format!(
                            "the variant `{}` of the `oneof` `{written}` holds one value",
                            variant.name.name
                        ));
                    };
                    // The variant's type is written in the enum's file.
                    let generator = Generator { file: found.unwrap_or(self.file), ..*self };
                    let kind = generator.kind(inner, &annotated.encoding)?;
                    let qualified = match kind {
                        Kind::Message(name, group) if !name.contains('.') && written.contains('.') => {
                            let module = written.rsplit_once('.').map_or("", |(m, _)| m);
                            Kind::Message(format!("{module}.{name}"), group)
                        }
                        Kind::Enum(name) if !name.contains('.') && written.contains('.') => {
                            let module = written.rsplit_once('.').map_or("", |(m, _)| m);
                            Kind::Enum(format!("{module}.{name}"))
                        }
                        kind => kind,
                    };
                    // A variant's name in the `.proto`: its own, or the
                    // variant's in lowercase from its first letter.
                    let vname = &variant.name.name;
                    let proto = annotated.name.clone().unwrap_or_else(|| {
                        let mut chars = vname.chars();
                        chars.next().map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
                    });
                    variants.push((vname.clone(), annotated.number, qualified, proto));
                }
                Ok(Kind::OneOf(variants))
            }
            _ => Err(format!(
                "protobuf has no type for `{written}`: {FIELD_TYPES}; an enum whose variants carry `@{0}.Value` is a protobuf enum, and one whose variants carry `@{0}.Field` a `oneof`",
                self.alias
            )),
        }
    }

    /// The shape of a field.
    fn shape(&self, field: &ast::FieldDecl, annotated: &Annotated) -> Result<Shape, String> {
        let encoding = annotated.encoding.as_str();
        let ty = &field.ty;
        if let Some((module, name)) = self.standard(ty)
            && module == "protobuf"
            && name == "UnknownFields"
        {
            return Ok(Shape::Unknown);
        }
        if encoding == "Wrapper" && !matches!(ty.kind, ast::TypeKind::Optional(_)) {
            return Err("`Encoding.Wrapper` goes with an optional value, `T?`".into());
        }
        match &ty.kind {
            ast::TypeKind::Optional(inner) => {
                if encoding == "Wrapper" {
                    return match self.kind(inner, "Default")? {
                        kind @ Kind::Scalar(_) => Ok(Shape::Wrapped(kind)),
                        _ => {
                            Err("`Encoding.Wrapper` goes with an optional number, `Bool`, `String` or `Array[Byte]`"
                                .into())
                        }
                    };
                }
                Ok(Shape::Optional(self.kind(inner, encoding)?))
            }
            ast::TypeKind::Path { segments, args } if segments.len() == 1 && segments[0].name == "Array" => {
                let [element] = &args[..] else { return Err("an `Array` has one type of element".into()) };
                if matches!(&element.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == "Byte")
                {
                    return Ok(Shape::Single(self.kind(ty, encoding)?));
                }
                match self.kind(element, encoding)? {
                    Kind::OneOf(..) => Err("a `oneof` is an optional field, `T?`, not an array".into()),
                    kind => Ok(Shape::Repeated(kind)),
                }
            }
            ast::TypeKind::Path { segments, args } if segments.len() == 1 && segments[0].name == "Map" => {
                let [key, value] = &args[..] else { return Err("a `Map` has a key and a value".into()) };
                let key = self.kind(key, "Default")?;
                if !matches!(key, Kind::Scalar(name) if !matches!(name, "float" | "double" | "bytes")) {
                    return Err("the key of a map is an integer, `Bool` or `String`".into());
                }
                match self.kind(value, encoding)? {
                    Kind::OneOf(..) => Err("the value of a map is not a `oneof`".into()),
                    value => Ok(Shape::Map(key, value)),
                }
            }
            _ => match self.kind(ty, encoding)? {
                Kind::OneOf(..) => Err("a `oneof` is an optional field: write its type as `T?`".into()),
                kind => Ok(Shape::Single(kind)),
            },
        }
    }

    fn message(&self, t: &ast::TypeDecl, fields: &[ast::FieldDecl], diags: &mut Diagnostics) -> Option<String> {
        let p = self.alias;
        let name = &t.name.name;
        if !t.generics.is_empty() {
            diags.error("E1910", t.name.span, "a protobuf message is a type without type parameters");
            return None;
        }
        let mut write = String::new();
        let mut locals = String::new();
        let mut arms = String::new();
        let mut checks = String::new();
        let mut parts = Vec::new();
        let mut unknown: Option<String> = None;
        let mut numbers: Vec<(i64, &str)> = Vec::new();
        // The JSON of the message, field by field, as the bytes are.
        let mut json_write = String::new();
        let mut json_arms = String::new();
        let mut aliases: Vec<(String, String)> = Vec::new();
        let mut failed = false;
        for field in fields {
            let fname = &field.name.name;
            let local = format!("__{fname}");
            // The field that keeps unknown fields needs no number, nor does
            // a `oneof`, whose variants have theirs.
            let plain = || Annotated { number: 0, encoding: "Default".into(), packed: true, name: None };
            let oneof = matches!(&field.ty.kind, ast::TypeKind::Optional(inner) if matches!(self.kind(inner, "Default"), Ok(Kind::OneOf(_))));
            let Some(annotated) = self
                .annotated(&field.uses, "Field")
                .or_else(|| annotation(&field.uses, p, "Unknown").map(|_| plain()))
                .or_else(|| oneof.then(plain))
            else {
                if field.annotations.iter().any(|a| a.name.name == "ignore") && field.default.is_some() {
                    continue;
                }
                diags.error(
                    "E1910",
                    field.name.span,
                    format!("the field `{fname}` of the protobuf message `{name}` needs a number: write `@{p}.Field(n)` before it, or `@ignore` with a default value"),
                );
                failed = true;
                continue;
            };
            let shape = match self.shape(field, &annotated) {
                Ok(shape) => shape,
                Err(why) => {
                    diags.error("E1910", field.ty.span, format!("the field `{fname}`: {why}"));
                    failed = true;
                    continue;
                }
            };
            if let Shape::Unknown = shape {
                if annotation(&field.uses, p, "Unknown").is_none() {
                    diags.error("E1910", field.name.span, format!("a `{p}.UnknownFields` field is written `@{p}.Unknown {fname}: {p}.UnknownFields = {p}.UnknownFields {{}}`"));
                    failed = true;
                    continue;
                }
                unknown = Some(fname.clone());
                let _ = writeln!(write, "        out.unknown(self.{fname})");
                let _ = writeln!(locals, "        var {local} = {p}.UnknownFields {{}}");
                parts.push(format!("{fname}: {local}"));
                continue;
            }
            // The numbers of the field, its own or its `oneof`'s.
            let owned: Vec<i64> = match &shape {
                Shape::Optional(Kind::OneOf(variants)) => variants.iter().map(|v| v.1).collect(),
                _ => vec![annotated.number],
            };
            for number in owned {
                if !(1..=536_870_911).contains(&number) || (19_000..=19_999).contains(&number) {
                    diags.error("E1910", field.name.span, format!("the field `{fname}` has the number {number}: a field number goes from 1 to 536870911, without 19000 to 19999, which protobuf keeps"));
                    failed = true;
                } else if let Some((_, other)) = numbers.iter().find(|(n, _)| *n == number) {
                    diags.error(
                        "E1910",
                        field.name.span,
                        format!("the field `{fname}` has the number {number}, which `{other}` has already"),
                    );
                    failed = true;
                }
                numbers.push((number, fname));
            }
            let default = field.default.as_ref().map(|d| self.text(d.span));
            let ty = self.text(field.ty.span);
            let number = annotated.number;
            // Its names in JSON: lowerCamel, or `@name`; and the `.proto`'s.
            let proto_name = annotated.name.clone().unwrap_or_else(|| fname.clone());
            let json = named(&field.annotations).unwrap_or_else(|| json_name(&proto_name));
            let field_keys = keys(&json, &mut aliases, &proto_name);
            let path = format!("$.{json}");
            let named_field = format!("out.field({json:?}, {proto_name:?})");
            match &shape {
                Shape::Single(kind) => {
                    let value = format!("self.{fname}");
                    match (&default, kind) {
                        // A field with its default value is not written, as
                        // proto3 does with its zeros.
                        (Some(default), Kind::Scalar(_) | Kind::Enum(_) | Kind::Json(_)) => {
                            let condition = match default.as_str() {
                                "\"\"" | "[]" => format!("not {value}.isEmpty()"),
                                _ => format!("{value} != ({default})"),
                            };
                            let _ = writeln!(
                                write,
                                "        if {condition} {{\n            {}\n        }}",
                                put(kind, number, &value)
                            );
                            let _ = writeln!(
                                json_write,
                                "        if {condition} {{\n            {named_field}\n            {}\n        }}",
                                json_put(kind, &value)
                            );
                        }
                        _ => {
                            let _ = writeln!(write, "        {}", put(kind, number, &value));
                            let _ = writeln!(json_write, "        {named_field}\n        {}", json_put(kind, &value));
                        }
                    }
                    json_arms.push_str(&json_arm(&field_keys, kind, &path, &local, default.is_none()));
                    match &default {
                        Some(default) => {
                            let _ = writeln!(locals, "        var {local}: {ty} = {default}");
                            arms.push_str(&self.arm(number, kind, fname, &local, false));
                            parts.push(format!("{fname}: {local}"));
                        }
                        None => {
                            let _ = writeln!(locals, "        var {local}: {ty}? = none");
                            arms.push_str(&self.arm(number, kind, fname, &local, true));
                            let _ = writeln!(
                                checks,
                                "        let {local}Read = match {local} {{\n            some(value) => value\n            none => fail input.missing({fname:?})\n        }}"
                            );
                            parts.push(format!("{fname}: {local}Read"));
                        }
                    }
                }
                Shape::Optional(Kind::OneOf(variants)) => {
                    let _ =
                        writeln!(write, "        match self.{fname} {{\n            some(chosen) => match chosen {{");
                    let _ = writeln!(
                        json_write,
                        "        match self.{fname} {{\n            some(chosen) => match chosen {{"
                    );
                    for (variant, number, kind, proto) in variants {
                        let enum_name = ty.trim_end_matches('?');
                        let _ = writeln!(
                            write,
                            "                {enum_name}.{variant}(value) => {}",
                            put(kind, *number, "value")
                        );
                        let json = json_name(proto);
                        let _ = writeln!(
                            json_write,
                            "                {enum_name}.{variant}(value) => {{\n                    out.field({json:?}, {proto:?})\n                    {}\n                }}",
                            json_put(kind, "value")
                        );
                        let read = json_read(kind, "field", &format!("$.{json}"));
                        let arm = match kind {
                            Kind::Enum(_) => format!(
                                "                {} => match {read} {{\n                    some(found) => {local} = some({enum_name}.{variant}(found))\n                    none => {{}}\n                }}\n",
                                keys(&json, &mut aliases, proto)
                            ),
                            _ => format!(
                                "                {} => {local} = some({enum_name}.{variant}({read}))\n",
                                keys(&json, &mut aliases, proto)
                            ),
                        };
                        json_arms.push_str(&arm);
                    }
                    let _ = writeln!(json_write, "            }}\n            none => {{}}\n        }}");
                    let _ = writeln!(write, "            }}\n            none => {{}}\n        }}");
                    let _ = writeln!(locals, "        var {local}: {ty} = none");
                    for (variant, number, kind, _) in variants {
                        let enum_name = ty.trim_end_matches('?');
                        let read = self.read(kind, fname);
                        arms.push_str(&format!(
                            "                {number} => {local} = some({enum_name}.{variant}({read}))\n"
                        ));
                        if let Kind::Enum(_) = kind {
                            // A closed enum's unknown number leaves the `oneof` as it was.
                            let last = arms.rfind(&format!("                {number} =>")).unwrap_or(0);
                            arms.truncate(last);
                            arms.push_str(&format!(
                                "                {number} => match {read} {{\n                    some(value) => {local} = some({enum_name}.{variant}(value))\n                    none => {{}}\n                }}\n"
                            ));
                        }
                    }
                    parts.push(format!("{fname}: {local}"));
                }
                Shape::Optional(kind) => {
                    let _ = writeln!(
                        write,
                        "        match self.{fname} {{\n            some(value) => {}\n            none => {{}}\n        }}",
                        put(kind, number, "value")
                    );
                    let _ = writeln!(locals, "        var {local}: {ty} = {}", default.as_deref().unwrap_or("none"));
                    arms.push_str(&self.arm(number, kind, fname, &local, true));
                    parts.push(format!("{fname}: {local}"));
                    let _ = writeln!(
                        json_write,
                        "        match self.{fname} {{\n            some(value) => {{\n                {named_field}\n                {}\n            }}\n            none => {{}}\n        }}",
                        json_put(kind, "value")
                    );
                    json_arms.push_str(&json_arm(&field_keys, kind, &path, &local, true));
                }
                Shape::Wrapped(kind) => {
                    let Kind::Scalar(scalar) = kind else { unreachable!("a wrapper holds a scalar") };
                    let zero = zero(scalar);
                    let condition = match *scalar {
                        "string" | "bytes" => "not value.isEmpty()".to_string(),
                        _ => format!("value != {zero}"),
                    };
                    let _ = writeln!(
                        write,
                        "        match self.{fname} {{\n            some(value) => {{\n                out.begin({number})\n                if {condition} {{\n                    {}\n                }}\n                out.end()\n            }}\n            none => {{}}\n        }}",
                        put(kind, 1, "value")
                    );
                    let inner = ty.trim_end_matches('?');
                    let _ = writeln!(locals, "        var {local}: {ty} = {}", default.as_deref().unwrap_or("none"));
                    arms.push_str(&format!(
                        "                {number} => {{\n                    input.enter({fname:?})?\n                    var wrapped: {inner} = {zero}\n                    for {{\n                        match input.next()? {{\n                            0 => break\n                            1 => wrapped = {}\n                            _ => input.skip()?\n                        }}\n                    }}\n                    {local} = some(wrapped)\n                }}\n",
                        self.read(kind, fname)
                    ));
                    parts.push(format!("{fname}: {local}"));
                    let _ = writeln!(
                        json_write,
                        "        match self.{fname} {{\n            some(value) => {{\n                {named_field}\n                {}\n            }}\n            none => {{}}\n        }}",
                        json_put(kind, "value")
                    );
                    json_arms.push_str(&json_arm(&field_keys, kind, &path, &local, true));
                }
                Shape::Repeated(kind) => {
                    let packable = matches!(kind, Kind::Enum(_))
                        || matches!(kind, Kind::Scalar(s) if !matches!(*s, "string" | "bytes"));
                    if packable && annotated.packed {
                        let _ = writeln!(
                            write,
                            "        if not self.{fname}.isEmpty() {{\n            out.begin({number})\n            for value in self.{fname} {{\n                {}\n            }}\n            out.end()\n        }}",
                            put(kind, 0, "value")
                        );
                    } else {
                        let _ = writeln!(
                            write,
                            "        for value in self.{fname} {{\n            {}\n        }}",
                            put(kind, number, "value")
                        );
                    }
                    let _ = writeln!(locals, "        var {local}: {ty} = {}", default.as_deref().unwrap_or("[]"));
                    let read = self.read(kind, fname);
                    let push = match kind {
                        Kind::Enum(_) => format!(
                            "match {read} {{\n                    some(value) => {local}.push(value)\n                    none => {{}}\n                }}"
                        ),
                        _ => format!("{local}.push({read})"),
                    };
                    if packable {
                        arms.push_str(&format!(
                            "                {number} => if input.isPacked() {{\n                    input.enterPacked({fname:?})?\n                    for input.more() {{\n                        {push}\n                    }}\n                }} else {{\n                    {push}\n                }}\n"
                        ));
                    } else {
                        arms.push_str(&format!("                {number} => {push}\n"));
                    }
                    parts.push(format!("{fname}: {local}"));
                    let _ = writeln!(
                        json_write,
                        "        if not self.{fname}.isEmpty() {{\n            {named_field}\n            out.beginArray()\n            for value in self.{fname} {{\n                {}\n            }}\n            out.endArray()\n        }}",
                        json_put(kind, "value")
                    );
                    let read = json_read(kind, "item", &path);
                    let push = match kind {
                        Kind::Enum(_) => format!(
                            "match {read} {{\n                        some(found) => {local}.push(found)\n                        none => {{}}\n                    }}"
                        ),
                        _ => format!("{local}.push({read})"),
                    };
                    let _ = writeln!(
                        json_arms,
                        "                {field_keys} => {{\n                    for item in input.array(field, {path:?})? {{\n                        {push}\n                    }}\n                }}"
                    );
                }
                Shape::Map(key, value) => {
                    let _ = writeln!(
                        write,
                        "        for (key, value) in {p}.entries(self.{fname}, out.deterministic()) {{\n            out.begin({number})\n            {}\n            {}\n            out.end()\n        }}",
                        put(key, 1, "key"),
                        put(value, 2, "value")
                    );
                    let ast::TypeKind::Path { args, .. } = &field.ty.kind else { unreachable!("a map") };
                    let (key_ty, value_ty) = (self.text(args[0].span), self.text(args[1].span));
                    let Kind::Scalar(key_scalar) = key else { unreachable!("a key is a scalar") };
                    let _ = writeln!(locals, "        var {local}: {ty} = {}", default.as_deref().unwrap_or("[:]"));
                    let read_value = match value {
                        Kind::Enum(_) => format!("entryValue = {}", self.read(value, fname)),
                        _ => format!("entryValue = some({})", self.read(value, fname)),
                    };
                    arms.push_str(&format!(
                        "                {number} => {{\n                    input.enter({fname:?})?\n                    var entryKey: {key_ty} = {}\n                    var entryValue: {value_ty}? = none\n                    for {{\n                        match input.next()? {{\n                            0 => break\n                            1 => entryKey = {}\n                            2 => {read_value}\n                            _ => input.skip()?\n                        }}\n                    }}\n                    {local}[entryKey] = match entryValue {{\n                        some(present) => present\n                        none => {}\n                    }}\n                }}\n",
                        zero(key_scalar),
                        self.read(key, fname),
                        self.missing_value(value)
                    ));
                    parts.push(format!("{fname}: {local}"));
                    let _ = writeln!(
                        json_write,
                        "        if not self.{fname}.isEmpty() {{\n            {named_field}\n            out.begin()\n            for (key, value) in {p}.entries(self.{fname}, true) {{\n                out.key(\"{{key}}\")\n                {}\n            }}\n            out.end()\n        }}",
                        json_put(value, "value")
                    );
                    let key_read = match *key_scalar {
                        "string" => "entryKey".to_string(),
                        "bool" => format!("input.boolKey(entryKey, {path:?})?"),
                        scalar => format!("input.{}(input.text(entryKey), {path:?})?", json_method(scalar)),
                    };
                    let read = json_read(value, "entryValue", &path);
                    let put_entry = match value {
                        Kind::Enum(_) => format!(
                            "match {read} {{\n                        some(found) => {local}[{key_read}] = found\n                        none => {{}}\n                    }}"
                        ),
                        _ => format!("{local}[{key_read}] = {read}"),
                    };
                    let _ = writeln!(
                        json_arms,
                        "                {field_keys} => {{\n                    for (entryKey, entryValue) in input.entries(field, {path:?})? {{\n                        {put_entry}\n                    }}\n                }}"
                    );
                }
                Shape::Unknown => unreachable!("handled above"),
            }
        }
        if failed {
            return None;
        }
        let fallback = match &unknown {
            Some(field) => format!("__{field} = input.keep(__{field})?"),
            None => "input.skip()?".to_string(),
        };
        let full = self.full_name(&t.uses, name);
        // A field's `.proto` name becomes its JSON name, which the arms match.
        let (renaming, matched) = if aliases.is_empty() {
            (String::new(), "key")
        } else {
            let mut renaming = String::from("            let __key = match key {\n");
            for (from, to) in &aliases {
                let _ = writeln!(renaming, "                {from:?} => {to:?}");
            }
            renaming.push_str("                _ => key\n            }\n");
            (renaming, "__key")
        };
        let mut out = format!("impl {p}.Message for {name} {{\n    fn writeTo(self, out: {p}.Writer) {{\n");
        // A message's own fields first, then those it did not know.
        let (own, kept): (Vec<&str>, Vec<&str>) =
            write.lines().partition(|line| !line.trim_start().starts_with("out.unknown("));
        for line in own.iter().chain(kept.iter()) {
            out.push_str(line);
            out.push('\n');
        }
        let _ = write!(
            out,
            "    }}\n\n    fn readFrom(input: {p}.Reader): {name} ! {p}.ProtobufError {{\n{locals}        for {{\n            match input.next()? {{\n                0 => break\n{arms}                _ => {fallback}\n            }}\n        }}\n{checks}        {name} {{ {} }}\n    }}\n\n    fn protobufName(): String {{\n        {full:?}\n    }}\n\n    fn writeJson(self, out: {p}.JsonWriter): !{p}.ProtobufError {{\n        out.begin()\n{json_write}        out.end()\n    }}\n\n    fn readJson(input: {p}.JsonReader, document: {p}.JsonValue): {name} ! {p}.ProtobufError {{\n{locals}        for (key, field) in input.fields(document, \"$\")? {{\n{renaming}            match {matched} {{\n{json_arms}                _ => input.unknown(key, \"$\")?\n            }}\n        }}\n{checks}        {name} {{ {} }}\n    }}\n}}\n\n",
            parts.join(", "),
            parts.join(", ")
        );
        Some(out)
    }

    /// The value of a map's entry that has none: the zero of its type.
    fn missing_value(&self, kind: &Kind) -> String {
        let p = self.alias;
        match kind {
            Kind::Scalar(scalar) => zero(scalar).to_string(),
            Kind::Message(name, _) => format!("{p}.Message.readFrom[{name}]({p}.emptyReader())?"),
            Kind::Enum(name) => format!("{p}.Enum.fromNumber[{name}](0).expect(\"a protobuf enum has a value 0\")"),
            Kind::Timestamp => format!("{p}.epoch()"),
            Kind::Duration => format!("{p}.zeroDuration()"),
            Kind::Json(_) => format!("{p}.nullValue()"),
            Kind::OneOf(..) => unreachable!("a map holds no oneof"),
        }
    }

    /// The match arm that reads field `number` into `local`, wrapped in
    /// `some` when the local is optional.
    fn arm(&self, number: i64, kind: &Kind, name: &str, local: &str, optional: bool) -> String {
        let read = self.read(kind, name);
        match (kind, optional) {
            // A closed enum's unknown number leaves the field as it was.
            (Kind::Enum(_), true) => format!(
                "                {number} => match {read} {{\n                    some(value) => {local} = some(value)\n                    none => {{}}\n                }}\n"
            ),
            (Kind::Enum(_), false) => format!(
                "                {number} => match {read} {{\n                    some(value) => {local} = value\n                    none => {{}}\n                }}\n"
            ),
            (_, true) => format!("                {number} => {local} = some({read})\n"),
            (_, false) => format!("                {number} => {local} = {read}\n"),
        }
    }

    /// The expression that reads a value of a kind, after its tag; an
    /// enum's gives an option.
    fn read(&self, kind: &Kind, name: &str) -> String {
        match kind {
            Kind::Scalar(scalar) => format!("input.{}({name:?})?", method(scalar)),
            Kind::Timestamp => format!("input.timestamp({name:?})?"),
            Kind::Duration => format!("input.duration({name:?})?"),
            Kind::Json(which) => format!("input.json{which}({name:?})?"),
            Kind::Message(ty, false) => format!("input.message[{ty}]({name:?})?"),
            Kind::Message(ty, true) => format!("input.group[{ty}]({name:?})?"),
            Kind::Enum(ty) => format!("input.enumValue[{ty}]({name:?})?"),
            Kind::OneOf(..) => unreachable!("a oneof is read by its variants"),
        }
    }

    /// The full name of a message: `@protobuf.Name`, or its own.
    fn full_name(&self, uses: &[ast::AnnotationUse], name: &str) -> String {
        annotation(uses, self.alias, "Name")
            .and_then(|found| found.args.first())
            .map(|arg| self.text(arg.value.span).trim_matches('"').to_string())
            .unwrap_or_else(|| name.to_string())
    }

    fn enumeration(&self, e: &ast::EnumDecl, diags: &mut Diagnostics) -> Option<String> {
        let p = self.alias;
        let name = &e.name.name;
        let mut numbers: Vec<(i64, &str)> = Vec::new();
        let mut open: Option<&str> = None;
        let mut failed = false;
        for variant in &e.variants {
            let vname = &variant.name.name;
            match (&variant.payload, self.annotated(&variant.uses, "Value")) {
                (ast::VariantPayload::Unit, Some(annotated)) => {
                    if !(-2_147_483_648..=2_147_483_647).contains(&annotated.number) {
                        diags.error("E1910", variant.name.span, format!("the value of `{vname}` is not an `Int32`"));
                        failed = true;
                    }
                    numbers.push((annotated.number, vname));
                }
                (ast::VariantPayload::Tuple(types), None)
                    if vname == "Unrecognized"
                        && matches!(&types[..], [ty] if matches!(&ty.kind, ast::TypeKind::Path { segments, args } if args.is_empty() && segments.len() == 1 && segments[0].name == "Int32")) =>
                {
                    open = Some(vname);
                }
                _ => {
                    diags.error(
                        "E1910",
                        variant.name.span,
                        format!("the variant `{vname}` of the protobuf enum `{name}` needs a number, `@{p}.Value(n)`; only `Unrecognized(Int32)`, which keeps the numbers the enum does not know, has none"),
                    );
                    failed = true;
                }
            }
        }
        if failed {
            return None;
        }
        let mut out = format!("impl {p}.Enum for {name} {{\n    fn number(self): Int32 {{\n        match self {{\n");
        for (number, variant) in &numbers {
            let _ = writeln!(out, "            {name}.{variant} => {number}");
        }
        if let Some(open) = open {
            let _ = writeln!(out, "            {name}.{open}(number) => number");
        }
        let _ = write!(
            out,
            "        }}\n    }}\n\n    fn fromNumber(number: Int32): {name}? {{\n        match number {{\n"
        );
        let mut seen = Vec::new();
        for (number, variant) in &numbers {
            if !seen.contains(number) {
                seen.push(*number);
                let _ = writeln!(out, "            {number} => some({name}.{variant})");
            }
        }
        match open {
            Some(open) => {
                let _ = writeln!(out, "            _ => some({name}.{open}(number))");
            }
            None => out.push_str("            _ => none\n"),
        }
        out.push_str("        }\n    }\n");
        // Its values' names in the `.proto`, `@name` or the variant's, which
        // JSON writes and reads.
        let proto = |variant: &str| {
            e.variants
                .iter()
                .find(|v| v.name.name == variant)
                .and_then(|v| named(&v.annotations))
                .unwrap_or_else(|| variant.to_string())
        };
        let _ = write!(out, "\n    fn protobufValueName(self): String {{\n        match self {{\n");
        for (_, variant) in &numbers {
            let _ = writeln!(out, "            {name}.{variant} => {:?}", proto(variant));
        }
        if let Some(open) = open {
            let _ = writeln!(out, "            {name}.{open}(number) => \"{{number}}\"");
        }
        let _ = write!(
            out,
            "        }}\n    }}\n\n    fn fromProtobufValueName(name: String): {name}? {{\n        match name {{\n"
        );
        for (_, variant) in &numbers {
            let _ = writeln!(out, "            {:?} => some({name}.{variant})", proto(variant));
        }
        out.push_str("            _ => none\n        }\n    }\n}\n\n");
        Some(out)
    }
}

/// What a field may hold, for the errors that say it.
const FIELD_TYPES: &str = "a field holds an `Int32`, `Int`, `UInt32`, `UInt`, `Float32`, `Float`, `Bool`, `String` or `Array[Byte]`, a message, a protobuf enum, `time.DateTime`, `time.Duration` or `json.Value`";

/// The statement that writes a value of a kind as field `number`.
fn put(kind: &Kind, number: i64, value: &str) -> String {
    match kind {
        Kind::Scalar(scalar) => format!("out.{}({number}, {value})", method(scalar)),
        Kind::Timestamp => format!("out.timestamp({number}, {value})"),
        Kind::Duration => format!("out.duration({number}, {value})"),
        Kind::Json(which) => format!("out.json{which}({number}, {value})"),
        Kind::Message(_, false) => format!("out.message({number}, {value})"),
        Kind::Message(_, true) => format!("out.group({number}, {value})"),
        Kind::Enum(_) => format!("out.enumValue({number}, {value})"),
        Kind::OneOf(..) => unreachable!("a oneof is written by its variants"),
    }
}

/// The method of `Writer` and `Reader` for a scalar.
fn method(scalar: &str) -> &'static str {
    match scalar {
        "int32" => "int32",
        "int64" => "int64",
        "uint32" => "uint32",
        "uint64" => "uint64",
        "sint32" => "sint32",
        "sint64" => "sint64",
        "fixed32" => "fixed32",
        "fixed64" => "fixed64",
        "sfixed32" => "sfixed32",
        "sfixed64" => "sfixed64",
        "bool" => "bool",
        "float" => "float",
        "double" => "double",
        "string" => "string",
        _ => "bytes",
    }
}

/// The zero of a scalar's type.
fn zero(scalar: &str) -> &'static str {
    match scalar {
        "bool" => "false",
        "float" | "double" => "0.0",
        "string" => "\"\"",
        "bytes" => "[]",
        _ => "0",
    }
}

/// The text of `@name("text")` among a field's annotations.
fn named(annotations: &[ast::Annotation]) -> Option<String> {
    annotations.iter().find(|a| a.name.name == "name").and_then(|a| match &a.argument {
        Some(ast::AnnotationArg::Text(text)) => Some(text.clone()),
        _ => None,
    })
}

/// A field's name in JSON, as protoc gives it: `user_id` is `userId`.
fn json_name(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The pattern of a field's key in JSON, its JSON name; its `.proto` name,
/// which reading takes too, becomes it first.
fn keys(json: &str, aliases: &mut Vec<(String, String)>, proto: &str) -> String {
    if json != proto && !aliases.iter().any(|(from, _)| from == proto) {
        aliases.push((proto.to_string(), json.to_string()));
    }
    format!("{json:?}")
}

/// The method of `JsonWriter` and `JsonReader` for a scalar.
fn json_method(scalar: &str) -> &'static str {
    match scalar {
        "int32" | "sint32" | "sfixed32" => "int32",
        "uint32" | "fixed32" => "uint32",
        "int64" | "sint64" | "sfixed64" => "int64",
        "uint64" | "fixed64" => "uint64",
        "bool" => "bool",
        "float" => "float",
        "double" => "double",
        "string" => "string",
        _ => "bytes",
    }
}

/// The statement that writes a value of a kind as JSON.
fn json_put(kind: &Kind, value: &str) -> String {
    match kind {
        Kind::Scalar(scalar) => format!("out.{}({value})", json_method(scalar)),
        Kind::Timestamp => format!("out.timestamp({value})"),
        Kind::Duration => format!("out.duration({value})"),
        Kind::Json(_) => format!("out.jsonValue({value})"),
        Kind::Message(..) => format!("out.message({value})?"),
        Kind::Enum(_) => format!("out.enumValue({value})"),
        Kind::OneOf(..) => unreachable!("a oneof is written by its variants"),
    }
}

/// The expression that reads a value of a kind from the JSON in `value`;
/// an enum's gives an option.
fn json_read(kind: &Kind, value: &str, path: &str) -> String {
    match kind {
        Kind::Scalar(scalar) => format!("input.{}({value}, {path:?})?", json_method(scalar)),
        Kind::Timestamp => format!("input.timestamp({value}, {path:?})?"),
        Kind::Duration => format!("input.duration({value}, {path:?})?"),
        Kind::Json(_) => value.to_string(),
        Kind::Message(ty, _) => format!("input.message[{ty}]({value}, {path:?})?"),
        Kind::Enum(ty) => format!("input.enumValue[{ty}]({value}, {path:?})?"),
        Kind::OneOf(..) => unreachable!("a oneof is read by its variants"),
    }
}

/// The match arm that reads a field of JSON into `local`, wrapped in
/// `some` when the local is optional.
fn json_arm(keys: &str, kind: &Kind, path: &str, local: &str, optional: bool) -> String {
    let read = json_read(kind, "field", path);
    match (kind, optional) {
        (Kind::Enum(_), true) => format!(
            "                {keys} => match {read} {{\n                    some(found) => {local} = some(found)\n                    none => {{}}\n                }}\n"
        ),
        (Kind::Enum(_), false) => format!(
            "                {keys} => match {read} {{\n                    some(found) => {local} = found\n                    none => {{}}\n                }}\n"
        ),
        (_, true) => format!("                {keys} => {local} = some({read})\n"),
        (_, false) => format!("                {keys} => {local} = {read}\n"),
    }
}
