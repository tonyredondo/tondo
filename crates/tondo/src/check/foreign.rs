//! Externs, exports, the types of C, their layouts and calls.

use super::*;

impl<'a> Checker<'a> {
    /// An `extern fn` (FFI): a module function without body whose
    /// parameters and result cross to C, called inside `unsafe`.
    pub(super) fn declare_extern(&mut self, f: &ast::FnDecl) {
        self.meta_denied(f.span, "`extern fn`");
        let module = std::mem::replace(&mut self.module, f.module);
        let foreign = f.foreign.clone().expect("an extern fn");
        let (params, result) = self.c_parts(f).unwrap_or_else(|| (vec![Ty::ERROR; f.params.len()], Ty::ERROR));
        let by_value = |this: &Self, ty: Ty| matches!(this.types.kind(this.types.repr(ty)), TyKind::Adt(_));
        if foreign.variadic && (params.iter().any(|&p| by_value(self, p)) || by_value(self, result)) {
            let message =
                "a C function with `...` takes and returns numbers and pointers; pass a `Pointer` to the struct";
            self.diags.error("E1704", f.name.span, message);
        }
        let id = self.externs.len() as u32;
        self.externs.push(hir::Extern {
            name: f.name.name.clone(),
            symbol: foreign.symbol,
            params,
            result,
            variadic: foreign.variadic,
        });
        self.extern_names.push(f.params.iter().map(|p| p.name.name.clone()).collect());
        self.declare_module_name(&f.name, ModuleName::Extern(id));
        self.module = module;
    }

    /// An `extern fn` with a body: a function of Tondo that C calls by its
    /// symbol (16.16), with the parameters and result of a C function.
    pub(super) fn declare_export(&mut self, f: &ast::FnDecl, id: FnId, paths: &[Vec<String>]) {
        self.meta_denied(f.span, "`extern fn`");
        let module = std::mem::replace(&mut self.module, f.module);
        let foreign = f.foreign.clone().expect("an extern fn");
        let parts = self.c_parts(f);
        self.module = module;
        if foreign.variadic {
            let message = "Tondo cannot read C's `...`: give the function fixed parameters";
            self.diags.error("E1704", f.name.span, message);
        }
        let symbol = foreign.symbol;
        let taken = match self.export_symbols.get(&symbol) {
            Some(&at) => Some(format!(
                "C already calls another function `{symbol}`, at {}: give one of them another `@symbol(\"...\")`",
                self.sources.location(at)
            )),
            None if symbol == "main" || symbol.starts_with("tondo_") => Some(format!(
                "`{symbol}` is a symbol of the program itself: give the function another name, or `@symbol(\"...\")`"
            )),
            None => None,
        };
        if let Some(message) = taken {
            self.diags.error("E1706", f.name.span, message);
            return;
        }
        self.export_symbols.insert(symbol.clone(), f.name.span);
        let Some((params, result)) = parts else { return };
        // A library publishes the `pub` ones of the package it builds.
        let published = f.public && paths[f.module as usize].first() == paths[0].first();
        if published {
            let named = f.params.iter().zip(&params);
            self.c_functions.push(hir::CFunction {
                symbol: symbol.clone(),
                params: named.map(|(p, &ty)| (p.name.name.clone(), self.c_type(p.ty.as_ref(), ty))).collect(),
                result: self.c_type(f.result.as_ref(), result),
                docs: crate::derive::docs(self.sources, f.span),
            });
        }
        let ty = self.types.intern(TyKind::CFn(params, result, false));
        self.exports.push(hir::Export { function: id, symbol, ty, published });
    }

    /// How a header spells a type that crosses to C, from how it is
    /// written: `std.ffi`'s aliases keep their C names.
    pub(super) fn c_type(&self, written: Option<&ast::TypeExpr>, ty: Ty) -> hir::CType {
        use crate::types::{FloatTy, IntTy};
        use hir::CType;
        if ty == self.c_string {
            return CType::Pointer(Box::new(CType::Named("char")));
        }
        let repr = self.types.repr(ty);
        let kind = self.types.kind(repr).clone();
        // The last name of the path: `CInt` of `ffi.CInt`.
        let (name, args) = match written.map(|w| &w.kind) {
            Some(ast::TypeKind::Path { segments, args }) => {
                (segments.last().and_then(|s| s.name.rsplit('.').next()), &args[..])
            }
            _ => (None, &[][..]),
        };
        let alias = match name {
            Some("CChar") => Some(("char", TyKind::Byte)),
            Some("CShort") => Some(("short", TyKind::Int(IntTy::I16))),
            Some("CUShort") => Some(("unsigned short", TyKind::Int(IntTy::U16))),
            Some("CInt") => Some(("int", TyKind::Int(IntTy::I32))),
            Some("CUInt") => Some(("unsigned int", TyKind::Int(IntTy::U32))),
            Some("CLong") => Some(("long", TyKind::Int(IntTy::I64))),
            Some("CULong") => Some(("unsigned long", TyKind::Int(IntTy::U64))),
            Some("CLongLong") => Some(("long long", TyKind::Int(IntTy::I64))),
            Some("CULongLong") => Some(("unsigned long long", TyKind::Int(IntTy::U64))),
            Some("CSize") => Some(("size_t", TyKind::Int(IntTy::U64))),
            Some("CSSize") => Some(("ssize_t", TyKind::Int(IntTy::I64))),
            Some("CFloat") => Some(("float", TyKind::Float(FloatTy::F32))),
            Some("CDouble") => Some(("double", TyKind::Float(FloatTy::F64))),
            Some("CBool") => Some(("bool", TyKind::Bool)),
            _ => None,
        };
        if let Some((c, aliased)) = alias
            && kind == aliased
        {
            return CType::Named(c);
        }
        match kind {
            TyKind::Int(int) => CType::Named(match int {
                IntTy::I128 => "__int128",
                IntTy::U128 => "unsigned __int128",
                IntTy::I8 => "int8_t",
                IntTy::I16 => "int16_t",
                IntTy::I32 => "int32_t",
                IntTy::I64 => "int64_t",
                IntTy::U8 => "uint8_t",
                IntTy::U16 => "uint16_t",
                IntTy::U32 => "uint32_t",
                IntTy::U64 => "uint64_t",
            }),
            TyKind::Byte => CType::Named("uint8_t"),
            TyKind::Float(FloatTy::F32) => CType::Named("float"),
            TyKind::Float(FloatTy::F64) => CType::Named("double"),
            TyKind::Bool => CType::Named("bool"),
            TyKind::Pointer(inner) => CType::Pointer(Box::new(self.c_type(args.first(), inner))),
            TyKind::Option(inner) => {
                let written = match written.map(|w| &w.kind) {
                    Some(ast::TypeKind::Optional(inner)) => Some(&**inner),
                    _ => None,
                };
                self.c_type(written, inner)
            }
            TyKind::CFn(params, result, _) => {
                let (written_params, written_result) = match written.map(|w| &w.kind) {
                    Some(ast::TypeKind::Fn { params, result, .. }) => (&params[..], result.as_deref()),
                    _ => (&[][..], None),
                };
                let params = params.iter().enumerate().map(|(i, &p)| self.c_type(written_params.get(i), p)).collect();
                CType::Function(params, Box::new(self.c_type(written_result, result)))
            }
            TyKind::CArray(inner, count) => CType::Array(Box::new(self.c_type(written, inner)), count),
            TyKind::Adt(_) if repr == self.void => CType::Named("void"),
            TyKind::Adt(id) => CType::Struct(c_name(&self.types.adt(id).name)),
            _ => CType::Named("void"),
        }
    }

    /// The `extern type`s the published functions reach, each after those
    /// it holds by value.
    pub(super) fn c_structs(&self) -> Vec<hir::CStruct> {
        fn reach(this: &Checker, ty: &hir::CType, seen: &mut HashSet<String>, out: &mut Vec<hir::CStruct>) {
            match ty {
                hir::CType::Named(_) => {}
                hir::CType::Pointer(inner) | hir::CType::Array(inner, _) => reach(this, inner, seen, out),
                hir::CType::Function(params, result) => {
                    for ty in params.iter().chain([&**result]) {
                        reach(this, ty, seen, out);
                    }
                }
                hir::CType::Struct(name) => {
                    if !seen.insert(name.clone()) {
                        return;
                    }
                    let found = this.c_decls.iter().find(|(id, _)| c_name(&this.types.adt(**id).name) == *name);
                    let Some((&id, decl)) = found else { return };
                    let written: Vec<&ast::FieldDecl> = match &decl.body {
                        ast::TypeBody::Record(fields) => fields.iter().collect(),
                        _ => Vec::new(),
                    };
                    let fields: Vec<(String, hir::CType)> = match &this.types.adt(id).kind {
                        crate::types::AdtKind::Record(fields) => fields
                            .iter()
                            .zip(&written)
                            .map(|(field, decl)| (field.name.clone(), this.c_type(Some(&decl.ty), field.ty)))
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (_, field) in &fields {
                        reach(this, field, seen, out);
                    }
                    out.push(hir::CStruct {
                        name: name.clone(),
                        fields,
                        layout: this.types.foreign.get(&id).cloned().unwrap_or_default(),
                        docs: crate::derive::docs(this.sources, decl.span),
                    });
                }
            }
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for function in &self.c_functions {
            for ty in function.params.iter().map(|(_, ty)| ty).chain([&function.result]) {
                reach(self, ty, &mut seen, &mut out);
            }
        }
        out
    }

    /// The parameters and result of a C function's declaration, which
    /// cross to C; none for one with type parameters.
    pub(super) fn c_parts(&mut self, f: &ast::FnDecl) -> Option<(Vec<Ty>, Ty)> {
        if !f.generics.is_empty() {
            self.diags.error("E1704", f.generics[0].name.span, "an `extern fn` takes no type parameters: C has none");
            return None;
        }
        if let Some(owner) = &f.owner {
            let message = "an `extern fn` is a module function, not a method: remove the type before its name";
            self.diags.error("E1704", owner.span, message);
        }
        for annotation in &f.annotations {
            match (annotation.name.name.as_str(), &annotation.argument) {
                ("symbol", Some(ast::AnnotationArg::Text(symbol))) if !symbol.is_empty() => {}
                ("symbol", _) => {
                    let message = "`@symbol` takes the C function's name: `@symbol(\"sqlite3_open\")`";
                    self.diags.error("E1704", annotation.span, message);
                }
                (name, _) => {
                    let message = format!(
                        "unknown annotation `@{name}`; an `extern fn` takes `@symbol(\"name\")`, the name C knows it by"
                    );
                    self.diags.error("E1001", annotation.span, message);
                }
            }
        }
        let mut params = Vec::new();
        for param in &f.params {
            let Some(written) = &param.ty else {
                self.diags.error("E1704", param.span, "an `extern fn` has no receiver: C has no methods");
                params.push(Ty::ERROR);
                continue;
            };
            let ty = self.resolve_type(written);
            if param.mode != ast::Mode::Value {
                let message = format!(
                    "C takes its arguments by value: remove `{}`, or pass a `Pointer[...]` to what it changes",
                    param.mode.keyword()
                );
                self.diags.error("E1704", param.span, message);
            } else if param.variadic {
                let message = "C's variable arguments are a last `...`, without name or type";
                self.diags.error("E1704", param.span, message);
            } else if let Some(default) = &param.default {
                self.diags.error("E1704", default.span, "a parameter of an `extern fn` has no default value");
            } else {
                self.require_c_type(ty, written.span);
            }
            params.push(ty);
        }
        let result = match &f.result {
            Some(written) => {
                let ty = self.resolve_type(written);
                if ty != Ty::UNIT && ty != Ty::NEVER {
                    self.require_c_type(ty, written.span);
                }
                ty
            }
            None => Ty::UNIT,
        };
        Some((params, result))
    }

    /// Reports a type that cannot cross to C (`E1703`): C's numbers,
    /// `Bool`, pointers, `extern fn(...)` and `extern type`s can.
    pub(super) fn require_c_type(&mut self, ty: Ty, span: Span) -> bool {
        let Some(reason) = self.c_type_error(ty) else { return true };
        let message = format!("{} cannot cross to C: {reason}", self.display(ty));
        self.diags.error("E1703", span, message);
        false
    }

    /// Why a value of `ty` cannot cross to C, if it cannot.
    pub(super) fn c_type_error(&self, ty: Ty) -> Option<String> {
        let repr = self.types.repr(ty);
        Some(match self.types.kind(repr) {
            TyKind::Int(_)
            | TyKind::Float(_)
            | TyKind::Bool
            | TyKind::Byte
            | TyKind::Pointer(_)
            | TyKind::CFn(..)
            | TyKind::Error => return None,
            TyKind::CArray(element, _) => return self.c_type_error(*element),
            TyKind::Option(inner) if self.types.is_address(*inner) => return None,
            TyKind::Adt(id) => match self.types.foreign.get(id) {
                Some(layout) if !layout.opaque => return None,
                Some(_) => "an opaque C type has no size; pass a `Pointer` to it".into(),
                None => "only an `extern type` is laid out as C lays out a struct".into(),
            },
            TyKind::String => "text crosses as `ffi.CString`".into(),
            TyKind::Array(element) => format!("pass a `Pointer[{}]` to its elements", self.types.display(*element)),
            TyKind::Char => "a character crosses as `UInt32`".into(),
            TyKind::Fn(..) => "a function crosses as `extern fn(...)`".into(),
            TyKind::Option(_) => {
                "only an option of a pointer or of an `extern fn(...)` crosses, as a null address for `none`".into()
            }
            _ => "only numbers, `Bool`, `Byte`, pointers, `extern fn(...)` and `extern type`s do".into(),
        })
    }

    /// Whether a type declaration is well formed as to C: an `extern type`
    /// is a struct or opaque, without type parameters, and only it takes
    /// `@packed` or `@align(n)`.
    pub(super) fn c_type_declaration(&mut self, decl: &ast::TypeDecl) -> bool {
        if !decl.foreign {
            // `@omitNone` leaves out of every format the record's fields that
            // are `none`; `@secret` hides the type's values from the tools.
            let record = matches!(decl.body, ast::TypeBody::Record(_));
            let fits = |a: &&ast::Annotation| {
                deprecation(a).is_some()
                    || a.argument.is_none() && (a.name.name == "secret" || record && a.name.name == "omitNone")
            };
            if let Some(annotation) = decl.annotations.iter().find(|a| !fits(a)) {
                let (code, message) = match annotation.name.name.as_str() {
                    "packed" | "align" | "union" => (
                        "E1704",
                        format!(
                            "`@{}` lays out a C struct: declare it as `extern type {} = {{ ... }}`",
                            annotation.name.name, decl.name.name
                        ),
                    ),
                    "secret" => ("E1102", "`@secret` takes nothing: write `@secret`".to_string()),
                    "deprecated" => ("E1102", DEPRECATED_TEXT.to_string()),
                    name => ("E1619", misplaced_annotation(name, "a type")),
                };
                self.diags.error(code, annotation.span, message);
            }
            return true;
        }
        if let Some(generic) = decl.generics.first() {
            self.diags.error("E1704", generic.name.span, "an `extern type` takes no type parameters: C has none");
            return false;
        }
        if let ast::TypeBody::Newtype(inner) = &decl.body {
            let message = "an `extern type` is a C struct, `= { field: Type, ... }`, or opaque, without `=`; C's other types are aliases in `std.ffi`";
            self.diags.error("E1704", inner.span, message);
            return false;
        }
        true
    }

    /// The layout of an `extern type` from `@packed` and `@align(n)`.
    pub(super) fn c_layout(&mut self, decl: &ast::TypeDecl) -> crate::types::CLayout {
        let mut layout =
            crate::types::CLayout { opaque: matches!(decl.body, ast::TypeBody::Opaque), ..Default::default() };
        for annotation in &decl.annotations {
            match (annotation.name.name.as_str(), &annotation.argument) {
                _ if layout.opaque => {
                    let message = "an opaque C type has no layout: C alone knows it";
                    self.diags.error("E1704", annotation.span, message);
                }
                ("packed", None) => layout.packed = true,
                ("union", None) => layout.union = true,
                ("align", Some(ast::AnnotationArg::Int(align))) if align.is_power_of_two() && *align <= 4096 => {
                    layout.align = *align as u32
                }
                ("align", _) => {
                    let message = "`@align` takes a power of two up to 4096: `@align(16)`";
                    self.diags.error("E1704", annotation.span, message);
                }
                (name, _) => {
                    let message = format!(
                        "unknown annotation `@{name}`; an `extern type` takes `@packed`, `@align(n)` and `@union`"
                    );
                    self.diags.error("E1001", annotation.span, message);
                }
            }
        }
        layout
    }

    /// The fields of a C struct: types that cross to C, and `@count(n)`
    /// arrays of them stored in place.
    pub(super) fn c_fields(&mut self, fields: &[ast::FieldDecl]) -> Vec<Field> {
        let mut seen = HashMap::new();
        let mut resolved = Vec::new();
        for field in fields {
            if let Some(first) = seen.insert(field.name.name.as_str(), field.name.span) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1505", field.name.span, message);
                self.diags.related(first, format!("`{}` is first declared here", field.name.name));
            }
            let mut ty = self.resolve_type(&field.ty);
            if !self.require_c_type(ty, field.ty.span) {
                ty = Ty::ERROR;
            }
            for annotation in &field.annotations {
                match (annotation.name.name.as_str(), &annotation.argument) {
                    ("count", Some(ast::AnnotationArg::Int(count))) if *count > 0 && *count <= u32::MAX as u64 => {
                        ty = self.types.intern(TyKind::CArray(ty, *count as u32));
                    }
                    ("count", _) => {
                        let message = "`@count` takes how many elements the C array has: `@count(16)`";
                        self.diags.error("E1704", annotation.span, message);
                    }
                    (name, _) => {
                        let message = format!(
                            "unknown annotation `@{name}`; a field of an `extern type` takes `@count(n)`, for a C array"
                        );
                        self.diags.error("E1001", annotation.span, message);
                    }
                }
            }
            resolved.push(Field { name: field.name.name.clone(), ty, private: false, secret: false });
        }
        resolved
    }

    /// A call of a C function (FFI), inside `unsafe`: its arguments as an
    /// ordinary call takes them, and after them those of C's `...`.
    pub(super) fn extern_call(
        &mut self,
        cx: &mut FnCx,
        id: u32,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let declared = self.externs[id as usize].clone();
        let fixed = declared.params.len();
        let (args, extra) =
            if declared.variadic && args.len() > fixed { args.split_at(fixed) } else { (args, &[][..]) };
        if declared.variadic
            && let Some(arg) = args.iter().chain(extra).find(|a| a.name.is_some())
        {
            let message = "a C function with `...` takes its arguments by position: remove the name";
            self.diags.error("E1102", arg.span, message);
            return Self::error_expr(span);
        }
        let signature = Signature {
            modes: vec![ast::Mode::Value; fixed],
            names: self.extern_names[id as usize].clone(),
            is_unsafe: true,
            foreign: true,
            name: declared.name.clone(),
            ..Signature::simple(declared.params, declared.result)
        };
        let target = CallTarget::new(Callee::Extern(id), signature, expected);
        let mut call = self.call_signature(cx, target, None, args, span);
        let values: Vec<hir::Expr> = extra.iter().map(|arg| self.c_variadic_argument(cx, arg)).collect();
        if let hir::ExprKind::Call(_, _, args) = &mut call.kind {
            args.extend(values);
        }
        call
    }

    /// An argument of C's `...`: a number, `Bool`, `Byte`, pointer or
    /// text, which passes as a `ffi.CString`.
    pub(super) fn c_variadic_argument(&mut self, cx: &mut FnCx, arg: &ast::Arg) -> hir::Expr {
        let value = self.expr(cx, &arg.value, None);
        if arg.spread || arg.mode != ast::Mode::Value {
            self.diags.error("E1102", arg.span, "C's `...` takes values: remove the keyword or the spread");
            return value;
        }
        let repr = self.types.repr(value.ty);
        if repr == Ty::STRING {
            return value;
        }
        let problem = match self.types.kind(repr) {
            TyKind::Adt(id) if self.types.foreign.contains_key(id) => {
                Some("C's `...` takes no struct by value; pass a `Pointer` to it".to_string())
            }
            _ => self.c_type_error(value.ty),
        };
        if let Some(reason) = problem {
            let message = format!("{} cannot cross to C: {reason}", self.display(value.ty));
            self.diags.error("E1703", value.span, message);
        }
        value
    }

    /// A call through an `extern fn(...)` value, inside `unsafe`.
    pub(super) fn c_value_call(
        &mut self,
        cx: &mut FnCx,
        callee: hir::Expr,
        (params, result, variadic): (Vec<Ty>, Ty, bool),
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if variadic {
            let message =
                "call a C function with `...` by its `extern fn` declaration, which passes its extra arguments";
            self.diags.error("E1704", span, message);
            return Self::error_expr(span);
        }
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            self.diags.error("E1102", arg.span, "a call through a function value takes positional arguments");
            return Self::error_expr(span);
        }
        let signature = Signature {
            names: (0..params.len()).map(|i| format!("argument{}", i + 1)).collect(),
            is_unsafe: true,
            foreign: true,
            name: "this C function".into(),
            ..Signature::simple(params, result)
        };
        let target = CallTarget::new(Callee::Fn(FnId(u32::MAX)), signature, expected);
        let call = self.call_signature(cx, target, None, args, span);
        let hir::ExprKind::Call(_, _, values) = call.kind else { return call };
        Self::mk(hir::ExprKind::CallValue(Box::new(callee), values), call.ty, span)
    }
}

/// The C name of an `extern type`: its name as declared, without the
/// module a project with several qualifies it with, which C cannot have.
fn c_name(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_string()
}
