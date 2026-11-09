//! Annotations, rules, messages and `embed`.

use super::*;

impl<'a> Checker<'a> {
    /// Each annotation the program declares (27.13): its fields of types
    /// that have constants, without `priv`, and the constant values of
    /// those a use may leave out.
    pub(super) fn annotation_declarations(&mut self) {
        let declared: Vec<(Ty, &'a ast::TypeDecl)> = self.annotation_decls.iter().map(|(&t, &d)| (t, d)).collect();
        for (ty, decl) in declared {
            let module = std::mem::replace(&mut self.module, decl.module);
            let ast::TypeBody::Record(declared) = &decl.body else { unreachable!("an annotation is a record") };
            let fields = self.record_fields(ty, decl.span).unwrap_or_default();
            let mut cx = self.new_cx(Ty::UNIT);
            let mut defaults = Vec::new();
            for (field, (name, field_ty)) in declared.iter().zip(fields) {
                if field.private {
                    let message = format!(
                        "the field `{name}` of the annotation `{}` is `priv`, but an annotation is data its readers see: remove `priv`",
                        decl.name.name
                    );
                    self.diags.error("E1618", field.name.span, message);
                }
                if !self.annotation_type(field_ty) {
                    let message = format!(
                        "the field `{name}` of the annotation `{}` is {}, which has no constants: an annotation holds numbers, `Bool`, `Char`, `Byte`, `String`, enums without payloads, and options and arrays of them",
                        decl.name.name,
                        self.display(field_ty)
                    );
                    self.diags.error("E1618", field.ty.span, message);
                }
                let default = field.default.as_ref().map(|default| {
                    let value = self.expr(&mut cx, default, Some(field_ty));
                    let mut value = self.coerce(value, field_ty);
                    if !self.types.has_error(value.ty) && !constant(&mut value) {
                        let message = format!(
                            "the value of the field `{name}` of the annotation `{}` is not a constant: write a literal, a `const`, a variant or an array of them",
                            decl.name.name
                        );
                        self.diags.error("E1618", default.span, message);
                    }
                    value
                });
                defaults.push(default);
            }
            self.annotation_defaults.insert(ty, defaults);
            self.module = module;
        }
    }

    /// Whether an annotation's field may be of `ty` (27.13): a type that has
    /// constants.
    pub(super) fn annotation_type(&self, ty: Ty) -> bool {
        match self.types.kind(ty) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Byte | TyKind::String => true,
            TyKind::Error => true,
            TyKind::Option(inner) | TyKind::Array(inner) => self.annotation_type(*inner),
            TyKind::Adt(_) => {
                self.types.variants(ty).is_some_and(|variants| variants.iter().all(|parts| parts.is_empty()))
            }
            _ => false,
        }
    }

    /// The program's annotations of every type, enum, variant, field,
    /// function and parameter (27.13), checked and kept as constants.
    pub(super) fn annotation_uses(&mut self, items: &'a [ast::Item], bodies: &[&'a ast::FnDecl]) {
        // A type by its name, a generic one with its own parameters.
        let declared = |checker: &mut Self, name: &str| match checker.type_names.get(name) {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            Some(&TypeName::Template(template)) => {
                let params = checker.types.template(template).params.clone();
                Some(checker.types.instantiate(template, params))
            }
            _ => None,
        };
        for item in items {
            match item {
                ast::Item::Type(decl) => {
                    let Some(ty) = declared(self, &decl.name.name) else { continue };
                    self.annotate(Annotated::Type(ty), &decl.uses, decl.module);
                    if let ast::TypeBody::Record(fields) = &decl.body {
                        for (index, field) in fields.iter().enumerate() {
                            self.annotate(Annotated::Field(ty, index as u32), &field.uses, decl.module);
                        }
                    }
                }
                ast::Item::Enum(decl) => {
                    let Some(ty) = declared(self, &decl.name.name) else { continue };
                    self.annotate(Annotated::Type(ty), &decl.uses, decl.module);
                    for (index, variant) in decl.variants.iter().enumerate() {
                        let index = index as u32;
                        self.annotate(Annotated::Variant(ty, index), &variant.uses, decl.module);
                        if let ast::VariantPayload::Record(fields) = &variant.payload {
                            for (at, field) in fields.iter().enumerate() {
                                let annotated = Annotated::VariantField(ty, index, at as u32);
                                self.annotate(annotated, &field.uses, decl.module);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (index, f) in bodies.iter().enumerate() {
            let id = FnId(index as u32);
            if f.foreign.is_none() {
                for annotation in f.annotations.iter().filter(|a| deprecation(a).is_none()) {
                    let (code, message) = match annotation.name.name.as_str() {
                        "deprecated" => ("E1102", DEPRECATED_TEXT.to_string()),
                        name => ("E1619", misplaced_annotation(name, "a function")),
                    };
                    self.diags.error(code, annotation.span, message);
                }
            }
            self.annotate(Annotated::Function(id), &f.uses, f.module);
            for (at, param) in f.params.iter().enumerate() {
                self.annotate(Annotated::Parameter(id, at as u32), &param.uses, f.module);
            }
        }
    }

    /// The annotations of one thing: each an annotation the program
    /// declares, once, with constant arguments, checked as a literal of its
    /// record with its fields in order or by name.
    pub(super) fn annotate(&mut self, annotated: Annotated, uses: &[ast::AnnotationUse], module: u32) {
        if uses.is_empty() {
            return;
        }
        let outer = std::mem::replace(&mut self.module, module);
        let mut cx = self.new_cx(Ty::UNIT);
        let mut values: Vec<hir::Expr> = Vec::new();
        for annotation in uses {
            let shown = annotation.path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".");
            let declared = match annotation.path.as_slice() {
                [name] => match self.type_names.get(&name.name) {
                    Some(&TypeName::Nominal(ty)) => Some(ty),
                    _ => None,
                },
                _ => None,
            };
            let Some((ty, decl)) = declared.and_then(|ty| self.annotation_decls.get(&ty).map(|&d| (ty, d))) else {
                // The modules that declare an annotation of that name.
                let mut modules: Vec<&str> = self
                    .annotation_decls
                    .values()
                    .filter_map(|d| d.name.name.rsplit_once('.').filter(|(_, name)| *name == shown).map(|(m, _)| m))
                    .collect();
                modules.sort();
                let message = match (declared, modules.first()) {
                    (Some(_), _) => format!(
                        "`{shown}` is a type, not an annotation: an annotation is declared with `annotation {shown} = {{ ... }}`"
                    ),
                    (None, Some(module)) => format!(
                        "there is no annotation `{shown}` here, but the module `{module}` declares one: write `@{}.{shown}`, and import that module if this file does not",
                        module.rsplit('.').next().unwrap_or(module)
                    ),
                    (None, None) => format!(
                        "there is no annotation `{shown}`: declare it with `annotation {shown} = {{ ... }}`, or import the module that does and write `@module.{shown}`"
                    ),
                };
                self.diags.error("E1619", annotation.span, message);
                continue;
            };
            if values.iter().any(|v| v.ty == ty) {
                let message = format!(
                    "`@{shown}` is written twice here: an annotation goes once on each thing, with a list in it for what repeats"
                );
                self.diags.error("E1619", annotation.span, message);
                continue;
            }
            // Its arguments, as the fields of a literal of its record.
            let ast::TypeBody::Record(fields) = &decl.body else { continue };
            let mut inits = Vec::new();
            let mut positional = 0;
            for arg in &annotation.args {
                if arg.mode != ast::Mode::Value || arg.spread {
                    let message = "an annotation's argument is a constant, without `ref`, `mut`, `var` or `...`";
                    self.diags.error("E1619", arg.span, message);
                    continue;
                }
                let name = match &arg.name {
                    Some(name) => name.clone(),
                    None => {
                        let Some(field) = fields.get(positional) else {
                            let message = format!("`@{shown}` takes {} fields, and this is one more", fields.len());
                            self.diags.error("E1102", arg.span, message);
                            continue;
                        };
                        positional += 1;
                        ast::Ident { name: field.name.name.clone(), span: arg.span }
                    }
                };
                // An integer literal is a float where a field is one, as
                // `@validate.Min(18)` (27.13).
                let mut written = arg.value.clone();
                let float = fields.iter().find(|f| f.name.name == name.name).is_some_and(|f| {
                    let ty = match &f.ty.kind {
                        ast::TypeKind::Optional(inner) => inner.as_ref(),
                        _ => &f.ty,
                    };
                    matches!(&ty.kind, ast::TypeKind::Path { segments, .. }
                        if segments.len() == 1 && matches!(segments[0].name.as_str(), "Float" | "Float64" | "Float32"))
                });
                let literal = match &mut written.kind {
                    ast::ExprKind::Unary(ast::UnaryOp::Neg, inner) => inner.as_mut(),
                    _ => &mut written,
                };
                if float && let ast::ExprKind::Int { value, suffix: None } = literal.kind {
                    literal.kind = ast::ExprKind::Float { text: format!("{value}.0"), suffix: None };
                }
                inits.push(ast::FieldInit { name, value: Some(written) });
            }
            let mut value = self.record_literal(&mut cx, &annotation.path, &[], &inits, Some(ty), annotation.span);
            if !self.types.has_error(value.ty) && !constant(&mut value) {
                let message = format!(
                    "the arguments of `@{shown}` are constants: write literals, `const`s, variants or arrays of them"
                );
                self.diags.error("E1619", annotation.span, message);
            }
            if self.module_paths.get(decl.module as usize).is_some_and(|p| p == &["std", "validate"])
                && let Some(field) = self.annotated_field(annotated)
            {
                self.check_rule(&decl.name.name, annotation, field);
            }
            values.push(value);
        }
        self.module = outer;
        self.annotation_values.push((annotated, values));
    }

    /// The type of the field an annotation is on, if it is on a field.
    pub(super) fn annotated_field(&mut self, annotated: Annotated) -> Option<Ty> {
        let fields = match annotated {
            Annotated::Field(ty, index) => (ty, None, index),
            Annotated::VariantField(ty, variant, index) => (ty, Some(variant), index),
            _ => return None,
        };
        match fields {
            (ty, None, index) => {
                self.record_fields(ty, Span::default()).and_then(|f| f.get(index as usize).map(|f| f.1))
            }
            (ty, Some(variant), index) => match self.payload(ty, variant) {
                Payload::Record(fields) => fields.get(index as usize).map(|f| f.ty),
                _ => None,
            },
        }
    }

    /// A rule of `std.validate` on a field it does not go with: on a type
    /// it does not apply to, with a `min` above its `max`, a fraction as the
    /// bound of an integer, or a pattern that does not compile (standard
    /// library 14.35).
    pub(super) fn check_rule(&mut self, rule: &str, annotation: &ast::AnnotationUse, field: Ty) {
        let rule = rule.rsplit('.').next().unwrap_or(rule);
        let inner = match *self.types.kind(field) {
            TyKind::Option(inner) => inner,
            _ => field,
        };
        let text = inner == Ty::STRING;
        let counted = text || matches!(self.types.kind(inner), TyKind::Array(_) | TyKind::Map(..) | TyKind::Set(_));
        let integer = self.types.as_int(inner).is_some() || inner == Ty::BYTE;
        let float = matches!(self.types.kind(inner), TyKind::Float(_));
        let number = integer || float || self.number.is_some_and(|n| self.satisfies(inner, n, &[]));
        let argument = |name: &str, position: usize| {
            let named = annotation.args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == name));
            named.or_else(|| annotation.args.iter().filter(|a| a.name.is_none()).nth(position))
        };
        let integer_of = |arg: Option<&ast::Arg>| match arg.map(|a| &a.value.kind) {
            Some(ast::ExprKind::Int { value, .. }) => Some(*value),
            _ => None,
        };
        let shown = self.display(field);
        let problem = match rule {
            "Length" if !counted => Some(format!(
                "`@validate.Length` counts the characters of a `String` or the elements of an `Array`, a `Map` or a `Set`, not {shown}: bound a number with `@validate.Min` and `@validate.Max`"
            )),
            "Length" => match (integer_of(argument("min", 0)), integer_of(argument("max", 1))) {
                (Some(min), Some(max)) if min > max => Some(format!(
                    "`@validate.Length(min: {min}, max: {max})` allows no length: its `min` is above its `max`"
                )),
                _ => None,
            },
            "Min" | "Max" if !number => Some(format!(
                "`@validate.{rule}` bounds a number, not {shown}: count a text or a collection with `@validate.Length(min: ..., max: ...)`"
            )),
            "Min" | "Max" if integer => match argument("value", 0).map(|a| self.sources.slice(a.value.span)) {
                Some(bound) if bound.contains(['.', 'e', 'E']) => Some(format!(
                    "the bound of {shown} is an integer, and `{bound}` is not: write `@validate.{rule}({})`",
                    bound.split(['.', 'e', 'E']).next().unwrap_or("0")
                )),
                _ => None,
            },
            "Matches" | "Email" if !text => Some(format!("`@validate.{rule}` checks a `String`, not {shown}")),
            "Matches" => match argument("pattern", 0).map(|a| &a.value.kind) {
                Some(ast::ExprKind::Str(segments)) => {
                    let pattern: String = segments
                        .iter()
                        .map(|s| match s {
                            ast::StrSegment::Text(text) => text.as_str(),
                            ast::StrSegment::Expr(..) => "",
                        })
                        .collect();
                    regex_syntax::Parser::new().parse(&pattern).err().map(|error| {
                        let text = error.to_string();
                        let why = text.lines().last().unwrap_or_default().trim_start_matches("error: ").to_string();
                        format!("the pattern of `@validate.Matches` does not compile: {why}")
                    })
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(problem) = problem {
            self.diags.error("E1908", annotation.span, problem);
        }
    }

    /// `reflect.annotationsOf[T](place, index, part, method)`, which the
    /// reflection `derive` writes for an annotated thing of `T`: the values
    /// of its annotations (27.13). `place` is the type (0), a field (1), a
    /// variant (2), a field of a variant (3), a method (4) or one of its
    /// parameters (5), counting the receiver.
    pub(super) fn annotations_of(&mut self, ty: Ty, args: &[ast::Arg], result: Ty, span: Span) -> hir::Expr {
        let number = |at: usize| match args.get(at).map(|a| &a.value.kind) {
            Some(ast::ExprKind::Int { value, .. }) => *value as u32,
            _ => 0,
        };
        let method = match args.get(3).map(|a| &a.value.kind) {
            Some(ast::ExprKind::Str(segments)) => match segments.as_slice() {
                [ast::StrSegment::Text(text)] => text.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let ty = self.owner_key(ty);
        let function = self.methods.get(&(ty, method)).copied();
        let (index, part) = (number(1), number(2));
        let annotated = match number(0) {
            0 => Some(Annotated::Type(ty)),
            1 => Some(Annotated::Field(ty, index)),
            2 => Some(Annotated::Variant(ty, index)),
            3 => Some(Annotated::VariantField(ty, index, part)),
            4 => function.map(Annotated::Function),
            _ => function.map(|id| Annotated::Parameter(id, index)),
        };
        let values = annotated
            .and_then(|annotated| self.annotation_values.iter().find(|(at, _)| *at == annotated))
            .map(|(_, values)| values.clone())
            .unwrap_or_default();
        let items = values.into_iter().map(|value| self.reflect_value(value)).collect();
        Self::mk(hir::ExprKind::Array(items), result, span)
    }

    /// `knownMessages()` of `std.protobuf`, which an `Any` in JSON reads:
    /// `entryOf[T]()` for every message type of the program, each the
    /// type of an implementation of `Message` without type parameters.
    pub(super) fn known_messages(&mut self, result: Ty, span: Span) -> hir::Expr {
        // By their names in `std.protobuf`, whose names the body does not
        // see as the module's.
        let protobuf = self.module_paths.iter().position(|p| p == &["std", "protobuf"]).map(|m| m as u32);
        let message = (0..self.traits.len())
            .find(|&i| self.traits[i].name == "protobuf.Message" || self.traits[i].name == "Message")
            .map(|i| TraitId(i as u32))
            .filter(|id| self.trait_modules.get(id).copied() == protobuf);
        let entry = (0..self.signatures.len())
            .find(|&i| {
                self.signatures[i].name == "protobuf.entryOf"
                    && self.fn_scopes.get(i).is_some_and(|scope| Some(scope.module) == protobuf)
            })
            .map(|i| FnId(i as u32));
        let (Some(message), Some(entry), TyKind::Array(element)) = (message, entry, self.types.kind(result).clone())
        else {
            return Self::mk(hir::ExprKind::Array(Vec::new()), result, span);
        };
        let mut types: Vec<Ty> = Vec::new();
        for implementation in &self.impls {
            if implementation.trait_id == message
                && implementation.generics.is_empty()
                && !self.types.has_error(implementation.target)
                && !types.contains(&implementation.target)
            {
                types.push(implementation.target);
            }
        }
        let items = types
            .into_iter()
            .map(|ty| Self::mk(hir::ExprKind::Call(Callee::Fn(entry), vec![ty], Vec::new()), element, span))
            .collect();
        Self::mk(hir::ExprKind::Array(items), result, span)
    }

    /// `embed.text`, `embed.bytes` and `embed.files` (standard library
    /// 14.27): what they name in the package, read now and kept as a
    /// constant.
    pub(super) fn embedded(
        &mut self,
        kind: crate::embed::Kind,
        args: &[ast::Arg],
        result: Ty,
        span: Span,
    ) -> hir::Expr {
        use crate::embed::{Kind, Read};
        let name = match kind {
            Kind::Text => "embed.text",
            Kind::Bytes => "embed.bytes",
            Kind::Files => "embed.files",
        };
        if self.meta_denied(span, &format!("`{name}`, which reads the package's files,")) {
            return Self::error_expr(span);
        }
        let written = match args {
            [arg] => match &arg.value.kind {
                ast::ExprKind::Str(segments) => segments
                    .iter()
                    .map(|s| match s {
                        ast::StrSegment::Text(text) => Some(text.as_str()),
                        ast::StrSegment::Expr(..) => None,
                    })
                    .collect::<Option<String>>(),
                _ => None,
            },
            _ => None,
        };
        let Some(written) = written else {
            let message = format!(
                "`{name}` reads its file when the program compiles, so its path is a string literal: `{name}(\"static/index.html\")`"
            );
            self.diags.error("E1904", span, message);
            return Self::error_expr(span);
        };
        let source = self.sources.file_of(span.start).path.clone();
        let read = match crate::embed::read(&source, &written, kind) {
            Ok(read) => read,
            Err(why) => {
                self.diags.error("E1904", args[0].value.span, why);
                return Self::error_expr(span);
            }
        };
        let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
        match read {
            Read::Text(text) => Self::mk(hir::ExprKind::Str(text), result, span),
            Read::Bytes(contents) => Self::mk(hir::ExprKind::Bytes(contents.into()), result, span),
            Read::Files(files) => {
                // `embed.Files`: the paths in order, the contents and a digest of each.
                let strings = self.types.intern(TyKind::Array(Ty::STRING));
                let arrays = self.types.intern(TyKind::Array(bytes));
                let text = |text: String| Self::mk(hir::ExprKind::Str(text), Ty::STRING, span);
                let names = files.iter().map(|(name, _)| text(name.clone())).collect();
                let tags = files.iter().map(|(_, contents)| text(crate::hash::sha256_hex(contents)[..16].to_string()));
                let tags = tags.collect();
                let contents = files
                    .into_iter()
                    .map(|(_, contents)| Self::mk(hir::ExprKind::Bytes(contents.into()), bytes, span))
                    .collect();
                // A tree of `std.fs` that only reads: `FileSystem { tree:
                // Tree.Kept(names, contents, tags) }` (`std/files.to`).
                let parts = vec![
                    (0, Self::mk(hir::ExprKind::Array(names), strings, span)),
                    (1, Self::mk(hir::ExprKind::Array(contents), arrays, span)),
                    (2, Self::mk(hir::ExprKind::Array(tags), strings, span)),
                ];
                let Some(tree) = self.record_fields(result, span).and_then(|fields| fields.first().map(|f| f.1)) else {
                    return Self::error_expr(span);
                };
                let kept = Self::mk(hir::ExprKind::Variant(KEPT_TREE, parts), tree, span);
                Self::mk(hir::ExprKind::Record(vec![(0, kept)]), result, span)
            }
        }
    }
}
