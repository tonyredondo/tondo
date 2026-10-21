//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn};
use crate::parser::CONTEXTUAL_CONSTRUCTORS;
use crate::source::{SourceMap, Span};
use crate::types::{AdtId, AdtKind, ERR, Field, IntTy, NONE, OK, Payload, SOME, Ty, TyKind, Types, Variant};
use std::collections::{HashMap, HashSet};

pub fn check(module: &ast::Module, sources: &SourceMap, diags: &mut Diagnostics) -> Option<hir::Program> {
    let mut checker = Checker {
        types: Types::default(),
        diags,
        sources,
        names: HashMap::new(),
        type_names: HashMap::new(),
        aliases: Vec::new(),
        signatures: Vec::new(),
        console_error: None,
        conversion_error: Ty::ERROR,
        collection_error: Ty::ERROR,
    };
    checker.declare_prelude();
    checker.program(module)
}

/// Prelude type names (26.3); they cannot be redeclared unqualified.
const PRELUDE_TYPES: [&str; 38] = [
    "NumericConversionError",
    "Bool",
    "Int",
    "Float",
    "Byte",
    "Char",
    "String",
    "Unit",
    "Never",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "Float32",
    "Float64",
    "Option",
    "Result",
    "Array",
    "Map",
    "Set",
    "Range",
    "Iterator",
    "Ref",
    "Pointer",
    "Join",
    "Command",
    "Pipeline",
    "Copy",
    "Discard",
    "Equatable",
    "Key",
    "Send",
    "Share",
    "Display",
];

enum TypeName {
    Nominal(Ty),
    Alias(usize),
}

struct AliasEntry {
    target: ast::TypeExpr,
    span: Span,
    resolved: Option<Ty>,
    resolving: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum StdModule {
    Console,
}

#[derive(Copy, Clone)]
enum ModuleName {
    Fn(FnId),
    Module(StdModule),
    /// The constructor `Name(value)` of a newtype (8.8).
    Newtype(Ty),
}

#[derive(Copy, Clone)]
struct PatternContext {
    /// In a `match` arm, where a bare `ok`/`err`/`some` binding is rejected.
    in_match: bool,
    /// Bindings introduced by `var`.
    mutable: bool,
}

#[derive(Copy, Clone)]
enum FieldRef<'a> {
    Name(&'a str),
    Slot(u32),
}

struct Signature {
    params: Vec<Ty>,
    result: Ty,
}

const INTRINSICS: [&str; 2] = ["panic", "assert"];

struct Checker<'a> {
    types: Types,
    diags: &'a mut Diagnostics,
    sources: &'a SourceMap,
    names: HashMap<String, ModuleName>,
    type_names: HashMap<String, TypeName>,
    aliases: Vec<AliasEntry>,
    signatures: Vec<Signature>,
    /// `console.ConsoleError`, once `std.console` is imported.
    console_error: Option<Ty>,
    /// The prelude's `NumericConversionError`.
    conversion_error: Ty,
    /// `std.collections.CollectionError`.
    collection_error: Ty,
}

/// Per-function checking state.
struct FnCx {
    locals: Vec<hir::Local>,
    mutable: Vec<bool>,
    scopes: Vec<Vec<(String, LocalId)>>,
    /// Success type that `return` produces.
    result: Ty,
    /// Error type of a fallible function (`T ! E`), or of a script whose
    /// errors are still being inferred.
    error: Option<Ty>,
    /// A root script infers its error type from `?` and `fail` (20.3).
    infer_error: bool,
    /// One entry per enclosing loop: whether a `break` targets it.
    loops: Vec<bool>,
    /// The current path has unconditionally left the enclosing block.
    diverged: bool,
}

impl Checker<'_> {
    fn program(&mut self, module: &ast::Module) -> Option<hir::Program> {
        for import in &module.imports {
            self.import(import);
        }

        self.declare_types(&module.items);
        let fns: Vec<&ast::FnDecl> = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Fn(f) => Some(f),
                _ => None,
            })
            .collect();
        for (index, f) in fns.iter().enumerate() {
            let signature = self.signature(f);
            self.signatures.push(signature);
            self.declare_module_name(&f.name, ModuleName::Fn(FnId(index as u32)));
        }

        let mut functions: Vec<hir::Function> = fns.iter().zip(0..).map(|(f, i)| self.function(f, FnId(i))).collect();

        let explicit_main = match self.names.get("main") {
            Some(ModuleName::Fn(id)) => Some(*id),
            _ => None,
        };
        let main = if !module.statements.is_empty() {
            if let Some(id) = explicit_main {
                let span = fns[id.0 as usize].name.span;
                self.diags.error("E1802", span, "a script with top-level statements cannot also declare `main`");
            }
            functions.push(self.script_main(&module.statements));
            FnId(functions.len() as u32 - 1)
        } else if let Some(id) = explicit_main {
            self.validate_main(fns[id.0 as usize], id);
            id
        } else {
            self.diags.error_without_span("E1806", "the program has no `main` function and no top-level statements");
            return None;
        };

        if self.diags.has_errors() {
            return None;
        }
        Some(hir::Program { types: std::mem::take(&mut self.types), functions, main })
    }

    fn declare_prelude(&mut self) {
        let (id, ty) = self.types.declare_adt("NumericConversionError".into());
        let variants = ["OutOfRange", "NotFinite", "NotIntegral"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.type_names.insert("NumericConversionError".into(), TypeName::Nominal(ty));
        self.conversion_error = ty;
        let (id, ty) = self.types.declare_adt("collections.CollectionError".into());
        let variants = ["InvalidCapacity", "InvalidIndex", "InvalidStep", "ResourceLimit"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.collection_error = ty;
    }

    /// The type a numeric constructor such as `Int8(value)` names (18.6).
    fn numeric_type(name: &str) -> Option<Ty> {
        Some(match name {
            "Int" | "Int64" => Ty::INT,
            "Int8" => Ty::int(IntTy::I8),
            "Int16" => Ty::int(IntTy::I16),
            "Int32" => Ty::int(IntTy::I32),
            "UInt8" => Ty::int(IntTy::U8),
            "UInt16" => Ty::int(IntTy::U16),
            "UInt32" => Ty::int(IntTy::U32),
            "UInt64" => Ty::int(IntTy::U64),
            "Float" | "Float64" => Ty::F64,
            "Float32" => Ty::F32,
            "Byte" => Ty::BYTE,
            _ => return None,
        })
    }

    /// Whether converting `from` to `to` can fail, or `None` when the table
    /// of 18.6 has no such conversion.
    fn conversion_checked(&self, from: Ty, to: Ty) -> Option<bool> {
        let range = |ty: Ty| match self.types.kind(ty) {
            TyKind::Int(int) => Some((int.min(), int.max())),
            TyKind::Byte => Some((0, 255)),
            _ => None,
        };
        let float = |ty: Ty| matches!(self.types.kind(ty), TyKind::Float(_));
        match (range(from), range(to)) {
            (Some((from_min, from_max)), Some((to_min, to_max))) => Some(from_min < to_min || from_max > to_max),
            (Some(_), None) if float(to) => Some(false),
            (None, Some(_)) if float(from) => Some(true),
            (None, None) if float(from) && float(to) => Some(from == Ty::F64 && to == Ty::F32),
            _ => None,
        }
    }

    fn import(&mut self, import: &ast::Import) {
        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        let module = match path.as_slice() {
            ["std", "console"] => StdModule::Console,
            ["std", ..] => {
                let what = format!("module `{}`", path.join("."));
                self.diags.unsupported(import.span, &what);
                return;
            }
            _ => {
                let message = format!("module `{}` is not available", path.join("."));
                self.diags.error("E1008", import.span, message);
                return;
            }
        };
        let name = import.alias.as_ref().unwrap_or(import.path.last().unwrap());
        self.declare_module_name(name, ModuleName::Module(module));
        if module == StdModule::Console && self.console_error.is_none() {
            let unit = |name: &str| Variant { name: name.into(), payload: Payload::Unit };
            let (io_id, io) = self.types.declare_adt("io.IoError".into());
            let io_variants = ["Closed", "Cancelled", "InvalidData", "ResourceLimit", "Host"].map(unit).to_vec();
            self.types.define_adt(io_id, AdtKind::Enum(io_variants));
            let (id, ty) = self.types.declare_adt(format!("{}.ConsoleError", name.name));
            let mut variants = ["Unavailable", "Closed", "Cancelled"].map(unit).to_vec();
            variants.push(Variant { name: "Io".into(), payload: Payload::Tuple(vec![io]) });
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.console_error = Some(ty);
        }
    }

    fn declare_module_name(&mut self, name: &ast::Ident, value: ModuleName) {
        if matches!(value, ModuleName::Fn(_)) && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str()) {
            let message = format!(
                "a function cannot be named `{}`: `{}(...)` always constructs a `Result` or `Option`",
                name.name, name.name
            );
            self.diags.error("E1005", name.span, message);
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        } else if self.names.insert(name.name.clone(), value).is_some() {
            let message = format!("`{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
        }
    }

    fn declare_type_name(&mut self, name: &ast::Ident, value: TypeName) {
        if PRELUDE_TYPES.contains(&name.name.as_str()) {
            let message = format!("`{}` is a prelude type and cannot be redeclared", name.name);
            self.diags.error("E1005", name.span, message);
        } else if self.type_names.insert(name.name.clone(), value).is_some() {
            let message = format!("type `{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
        }
    }

    /// Declares every nominal type and alias, then resolves their definitions,
    /// so declarations may refer to each other in any order (7.11).
    fn declare_types(&mut self, items: &[ast::Item]) {
        let mut declared = Vec::new();
        for item in items {
            let name = match item {
                ast::Item::Type(decl) => &decl.name,
                ast::Item::Enum(decl) => &decl.name,
                ast::Item::Alias(decl) => {
                    let entry = AliasEntry {
                        target: decl.target.clone(),
                        span: decl.name.span,
                        resolved: None,
                        resolving: false,
                    };
                    self.aliases.push(entry);
                    self.declare_type_name(&decl.name, TypeName::Alias(self.aliases.len() - 1));
                    continue;
                }
                ast::Item::Fn(_) => continue,
            };
            let (id, ty) = self.types.declare_adt(name.name.clone());
            self.declare_type_name(name, TypeName::Nominal(ty));
            if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                self.declare_module_name(name, ModuleName::Newtype(ty));
            }
            declared.push((item, name, id));
        }
        for &(item, _, id) in &declared {
            let kind = match item {
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(inner), .. }) => {
                    AdtKind::Newtype(self.resolve_type(inner))
                }
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Record(fields), .. }) => {
                    AdtKind::Record(self.field_decls(fields))
                }
                ast::Item::Enum(decl) => {
                    let mut seen = HashSet::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if !seen.insert(variant.name.name.as_str()) {
                            let message = format!("variant `{}` is declared more than once", variant.name.name);
                            self.diags.error("E1002", variant.name.span, message);
                        }
                        let payload = match &variant.payload {
                            ast::VariantPayload::Unit => Payload::Unit,
                            ast::VariantPayload::Tuple(items) => {
                                Payload::Tuple(items.iter().map(|t| self.resolve_type(t)).collect())
                            }
                            ast::VariantPayload::Record(fields) => Payload::Record(self.field_decls(fields)),
                        };
                        variants.push(Variant { name: variant.name.name.clone(), payload });
                    }
                    AdtKind::Enum(variants)
                }
                _ => unreachable!(),
            };
            self.types.define_adt(id, kind);
        }
        for index in 0..self.aliases.len() {
            self.resolve_alias(index);
        }
        self.check_productivity(&declared);
    }

    fn field_decls(&mut self, fields: &[ast::FieldDecl]) -> Vec<Field> {
        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        for field in fields {
            if !seen.insert(field.name.name.as_str()) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1002", field.name.span, message);
            }
            resolved.push(Field { name: field.name.name.clone(), ty: self.resolve_type(&field.ty) });
        }
        resolved
    }

    /// Productivity (9.10) as a least fixed point: a nominal type is
    /// productive once some finite value of it can be built from productive
    /// components. A recursive type that never becomes productive is an error.
    fn check_productivity(&mut self, declared: &[(&ast::Item, &ast::Ident, AdtId)]) {
        let mut productive: HashSet<AdtId> = HashSet::new();
        loop {
            let before = productive.len();
            for &(_, _, id) in declared {
                if !productive.contains(&id) && self.adt_productive(id, &productive) {
                    productive.insert(id);
                }
            }
            if productive.len() == before {
                break;
            }
        }
        for &(_, name, id) in declared {
            let ty = self.types.intern(TyKind::Adt(id));
            if !productive.contains(&id) && self.types.reaches(ty, ty) {
                let message = format!("type `{}` contains itself and has no finite value", name.name);
                self.diags.error("E1107", name.span, message);
                self.types.define_adt(id, AdtKind::Newtype(Ty::ERROR));
            }
        }
    }

    fn adt_productive(&self, id: AdtId, productive: &HashSet<AdtId>) -> bool {
        let all = |types: Vec<Ty>| types.iter().all(|&t| self.ty_productive(t, productive));
        match &self.types.adt(id).kind {
            AdtKind::Newtype(inner) => self.ty_productive(*inner, productive),
            AdtKind::Record(fields) => all(fields.iter().map(|f| f.ty).collect()),
            AdtKind::Enum(variants) => variants.iter().any(|v| all(v.payload.types())),
        }
    }

    fn ty_productive(&self, ty: Ty, productive: &HashSet<AdtId>) -> bool {
        match self.types.kind(ty) {
            TyKind::Adt(id) => productive.contains(id),
            TyKind::Tuple(items) => items.iter().all(|&t| self.ty_productive(t, productive)),
            TyKind::Never => false,
            _ => true,
        }
    }

    fn resolve_alias(&mut self, index: usize) -> Ty {
        let entry = &self.aliases[index];
        if let Some(ty) = entry.resolved {
            return ty;
        }
        if entry.resolving {
            let span = entry.span;
            self.diags.error("E1106", span, "transparent aliases form a cycle");
            self.aliases[index].resolved = Some(Ty::ERROR);
            return Ty::ERROR;
        }
        self.aliases[index].resolving = true;
        let target = self.aliases[index].target.clone();
        let ty = self.resolve_type(&target);
        self.aliases[index].resolved.get_or_insert(ty);
        self.aliases[index].resolved.unwrap()
    }

    fn signature(&mut self, f: &ast::FnDecl) -> Signature {
        if let Some(owner) = &f.owner {
            self.diags.unsupported(owner.span, "methods");
        }
        if f.suspends {
            self.diags.unsupported(f.span, "`suspends` functions");
        }
        let params = f.params.iter().map(|p| self.resolve_type(&p.ty)).collect();
        let result = f.result.as_ref().map_or(Ty::UNIT, |t| self.resolve_type(t));
        Signature { params, result }
    }

    fn validate_main(&mut self, f: &ast::FnDecl, id: FnId) {
        let signature = &self.signatures[id.0 as usize];
        let problem = if f.public {
            Some("`main` cannot be `pub`")
        } else if !f.params.is_empty() {
            Some("`main` takes no parameters")
        } else if signature.result != Ty::UNIT
            && !matches!(self.types.kind(signature.result), TyKind::Result(ok, _) if *ok == Ty::UNIT)
        {
            Some("`main` returns `Unit` or `!E`")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.diags.error("E1803", f.name.span, problem);
        }
    }

    fn resolve_type(&mut self, t: &ast::TypeExpr) -> Ty {
        match &t.kind {
            ast::TypeKind::Path { segments, args } => {
                if !args.is_empty() {
                    let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
                    return match (segments[0].name.as_str(), args.as_slice()) {
                        ("Option", [inner]) if segments.len() == 1 => self.types.intern(TyKind::Option(*inner)),
                        ("Result", [ok, err]) if segments.len() == 1 => self.types.intern(TyKind::Result(*ok, *err)),
                        ("Array", [element]) if segments.len() == 1 => self.types.intern(TyKind::Array(*element)),
                        ("Map", [key, value]) if segments.len() == 1 => {
                            self.require_key(*key, t.span);
                            self.map_type(*key, *value)
                        }
                        ("Set", [element]) if segments.len() == 1 => {
                            self.require_key(*element, t.span);
                            self.types.intern(TyKind::Set(*element))
                        }
                        ("Option" | "Result" | "Array" | "Map" | "Set", _) if segments.len() == 1 => {
                            self.diags.error("E1104", t.span, "wrong number of type arguments");
                            Ty::ERROR
                        }
                        _ => self.unsupported_type(t.span, "generic types"),
                    };
                }
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                match names.as_slice() {
                    ["Int" | "Int64"] => Ty::INT,
                    ["Int8"] => Ty::int(IntTy::I8),
                    ["Int16"] => Ty::int(IntTy::I16),
                    ["Int32"] => Ty::int(IntTy::I32),
                    ["UInt8"] => Ty::int(IntTy::U8),
                    ["UInt16"] => Ty::int(IntTy::U16),
                    ["UInt32"] => Ty::int(IntTy::U32),
                    ["UInt64"] => Ty::int(IntTy::U64),
                    ["Bool"] => Ty::BOOL,
                    ["String"] => Ty::STRING,
                    ["Unit"] => Ty::UNIT,
                    ["Never"] => Ty::NEVER,
                    [module, "ConsoleError"]
                        if matches!(self.names.get(*module), Some(ModuleName::Module(StdModule::Console))) =>
                    {
                        self.console_error.unwrap()
                    }
                    ["Float" | "Float64"] => Ty::F64,
                    ["Float32"] => Ty::F32,
                    ["Char"] => Ty::CHAR,
                    ["Byte"] => Ty::BYTE,
                    [single] if self.type_names.contains_key(*single) => match self.type_names[*single] {
                        TypeName::Nominal(ty) => ty,
                        TypeName::Alias(index) => self.resolve_alias(index),
                    },
                    _ => {
                        let message = format!("unknown type `{}`", names.join("."));
                        self.diags.error("E1001", t.span, message);
                        Ty::ERROR
                    }
                }
            }
            ast::TypeKind::Result { ok, err } => {
                let ok = ok.as_ref().map_or(Ty::UNIT, |ok| self.resolve_type(ok));
                let err = self.resolve_type(err);
                self.types.intern(TyKind::Result(ok, err))
            }
            ast::TypeKind::Optional(inner) => {
                let inner = self.resolve_type(inner);
                self.types.intern(TyKind::Option(inner))
            }
            ast::TypeKind::Union(_) => self.unsupported_type(t.span, "union types"),
            ast::TypeKind::Tuple(items) => {
                let items = items.iter().map(|item| self.resolve_type(item)).collect();
                self.types.intern(TyKind::Tuple(items))
            }
            ast::TypeKind::Fn => self.unsupported_type(t.span, "function types"),
        }
    }

    fn unsupported_type(&mut self, span: Span, what: &str) -> Ty {
        self.diags.unsupported(span, what);
        Ty::ERROR
    }

    fn display(&self, ty: Ty) -> String {
        format!("`{}`", self.types.display(ty))
    }

    // ---- functions -------------------------------------------------------

    /// A context for a body whose declared result is `result`. A `T ! E`
    /// result makes the body fallible: it is written in terms of `T` (9.8).
    fn new_cx(&self, result: Ty) -> FnCx {
        let (success, error) = match self.types.kind(result) {
            TyKind::Result(ok, err) => (*ok, Some(*err)),
            _ => (result, None),
        };
        FnCx {
            locals: Vec::new(),
            mutable: Vec::new(),
            scopes: vec![Vec::new()],
            result: success,
            error,
            infer_error: false,
            loops: Vec::new(),
            diverged: false,
        }
    }

    fn function(&mut self, f: &ast::FnDecl, id: FnId) -> hir::Function {
        let result = self.signatures[id.0 as usize].result;
        let mut cx = self.new_cx(result);
        let param_types = self.signatures[id.0 as usize].params.clone();
        let params =
            f.params.iter().zip(param_types).map(|(p, ty)| self.declare(&mut cx, &p.name, ty, false)).collect();
        let success = cx.result;
        let body = self.block(&mut cx, &f.body, Some(success));
        hir::Function { name: f.name.name.clone(), params, locals: cx.locals, result, body }
    }

    /// The implicit `main` of a root script (spec 20.3). Its error type is
    /// inferred from the `?` and `fail` in its statements.
    fn script_main(&mut self, statements: &[ast::Stmt]) -> hir::Function {
        let mut cx = self.new_cx(Ty::UNIT);
        cx.infer_error = true;
        let stmts = self.statements(&mut cx, statements);
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        let body = hir::Block { stmts, tail: None, ty };
        let result = match cx.error {
            Some(error) => self.types.intern(TyKind::Result(Ty::UNIT, error)),
            None => Ty::UNIT,
        };
        hir::Function { name: "main".into(), params: Vec::new(), locals: cx.locals, result, body }
    }

    /// The error type a `fail` or `?` may produce here, inferring it in a
    /// root script; `None` in an infallible function.
    fn error_channel(&mut self, cx: &mut FnCx, produced: Ty, span: Span, code: &'static str) -> Option<Ty> {
        match cx.error {
            Some(error) => Some(error),
            None if cx.infer_error => {
                cx.error = Some(produced);
                Some(produced)
            }
            None => {
                self.diags.error(code, span, "this function cannot fail; declare its result as `T ! E`");
                None
            }
        }
    }

    fn lookup(&self, cx: &FnCx, name: &str) -> Option<LocalId> {
        cx.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name).map(|&(_, id)| id)
    }

    /// A local that no name refers to, such as the value of `for _ in ...`.
    fn hidden_local(&mut self, cx: &mut FnCx, ty: Ty) -> LocalId {
        cx.locals.push(hir::Local { name: "_".into(), ty });
        cx.mutable.push(false);
        LocalId(cx.locals.len() as u32 - 1)
    }

    fn declare(&mut self, cx: &mut FnCx, name: &ast::Ident, ty: Ty, mutable: bool) -> LocalId {
        if self.lookup(cx, &name.name).is_some() || self.names.contains_key(&name.name) {
            let message = format!("`{}` would shadow a visible binding with the same name", name.name);
            self.diags.error("E1003", name.span, message);
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        }
        let id = LocalId(cx.locals.len() as u32);
        cx.locals.push(hir::Local { name: name.name.clone(), ty });
        cx.mutable.push(mutable);
        cx.scopes.last_mut().unwrap().push((name.name.clone(), id));
        id
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self, cx: &mut FnCx, b: &ast::Block, expected: Option<Ty>) -> hir::Block {
        cx.scopes.push(Vec::new());
        let outer = std::mem::replace(&mut cx.diverged, false);
        let stmts = self.statements(cx, &b.stmts);
        let tail = b.tail.as_ref().map(|t| {
            if cx.diverged {
                self.diags.warning("W1006", t.span, "unreachable expression");
            }
            let value = self.expr(cx, t, expected);
            Box::new(match expected {
                Some(expected) => self.coerce(value, expected),
                None => value,
            })
        });
        let ty = match &tail {
            _ if cx.diverged => Ty::NEVER,
            Some(tail) => tail.ty,
            None => {
                if let Some(expected) = expected.filter(|&t| t != Ty::UNIT && t != Ty::ERROR) {
                    let message = format!("expected {}, found `Unit`", self.display(expected));
                    let end = Span::new(b.span.end - 1, b.span.end);
                    self.diags.error("E1102", end, message);
                }
                Ty::UNIT
            }
        };
        cx.diverged = outer;
        cx.scopes.pop();
        hir::Block { stmts, tail, ty }
    }

    fn statements(&mut self, cx: &mut FnCx, stmts: &[ast::Stmt]) -> Vec<hir::Stmt> {
        let mut out = Vec::new();
        let mut warned = false;
        for stmt in stmts {
            let reachable = !cx.diverged;
            if !reachable && !warned {
                self.diags.warning("W1006", stmt.span, "unreachable statement");
                warned = true;
            }
            if let Some(stmt) = self.statement(cx, stmt)
                && reachable
            {
                out.push(stmt);
            }
        }
        out
    }

    fn statement(&mut self, cx: &mut FnCx, stmt: &ast::Stmt) -> Option<hir::Stmt> {
        match &stmt.kind {
            ast::StmtKind::Let { mutable, pattern, ty, value } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let Some(value) = value else {
                    self.diags.error("E1109", stmt.span, "a binding must be initialized where it is declared");
                    return None;
                };
                let value = self.expr(cx, value, declared);
                let value = match declared {
                    Some(declared) => self.coerce(value, declared),
                    None => value,
                };
                cx.diverged |= value.ty == Ty::NEVER;
                let ty = declared.unwrap_or(value.ty);
                match &pattern.kind {
                    ast::PatternKind::Wildcard => Some(hir::Stmt::Expr(value)),
                    ast::PatternKind::Binding(name) => {
                        let local = self.declare(cx, name, ty, *mutable);
                        Some(hir::Stmt::Let(local, value))
                    }
                    _ => {
                        let pattern =
                            self.pattern(cx, pattern, ty, PatternContext { in_match: false, mutable: *mutable });
                        if let Some(missing) = crate::exhaust::refutation(&self.types, ty, &pattern) {
                            let message = format!("this pattern can fail, for example on `{missing}`; use `match`");
                            self.diags.error("E1201", pattern.span, message);
                        }
                        Some(hir::Stmt::Destructure(pattern, value))
                    }
                }
            }
            ast::StmtKind::Assign { target, op, value } => self.assign(cx, target, *op, value, stmt.span),
            ast::StmtKind::Discard(value) => {
                let value = self.expr(cx, value, None);
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Expr(value) => {
                let value = self.expr(cx, value, None);
                if ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&value.ty) {
                    let message =
                        format!("a value of type {} is not used; discard it with `_ = ...`", self.display(value.ty));
                    self.diags.error("E1303", value.span, message);
                }
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Return(value) => {
                let result = cx.result;
                let value = match value {
                    Some(value) => {
                        let value = self.expr(cx, value, Some(result));
                        Some(self.coerce(value, result))
                    }
                    None => {
                        if result != Ty::UNIT && result != Ty::ERROR {
                            let message =
                                format!("`return` without a value in a function returning {}", self.display(result));
                            self.diags.error("E1102", stmt.span, message);
                        }
                        None
                    }
                };
                cx.diverged = true;
                Some(hir::Stmt::Return(value))
            }
            ast::StmtKind::Fail(value) => {
                let declared = cx.error;
                let value = self.expr(cx, value, declared);
                let error = self.error_channel(cx, value.ty, stmt.span, "E1302")?;
                let value = self.coerce(value, error);
                cx.diverged = true;
                Some(hir::Stmt::Fail(value))
            }
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                let is_break = matches!(stmt.kind, ast::StmtKind::Break);
                let Some(saw_break) = cx.loops.last_mut() else {
                    let keyword = if is_break { "break" } else { "continue" };
                    let message = format!("`{keyword}` outside of a loop");
                    self.diags.error("E1205", stmt.span, message);
                    return None;
                };
                *saw_break |= is_break;
                cx.diverged = true;
                Some(if is_break { hir::Stmt::Break } else { hir::Stmt::Continue })
            }
            ast::StmtKind::For { header, body } => self.for_loop(cx, header, body),
        }
    }

    fn assign(
        &mut self,
        cx: &mut FnCx,
        target: &ast::Expr,
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        if let ast::ExprKind::Index { base, index } = &target.kind {
            let (local, path, ty) = self.place(cx, base)?;
            if let TyKind::Map(key, entry) = *self.types.kind(ty) {
                // `map[key] = value` inserts or replaces (10.11); there is no
                // compound form because the key may be absent.
                if op.is_some() {
                    self.diags.error("E1411", span, "a map entry has no compound assignment; read it with `get` first");
                    return None;
                }
                let key_value = self.expr(cx, index, Some(key));
                let key_value = self.coerce(key_value, key);
                let new = self.expr(cx, value, Some(entry));
                let new = self.coerce(new, entry);
                let result = self.types.intern(TyKind::Option(entry));
                let place = hir::Place { local, path };
                let insert = hir::ExprKind::MapMutate(hir::MapMutation::Insert, place, vec![key_value, new]);
                return Some(hir::Stmt::Expr(Self::mk(insert, result, span)));
            }
        }
        let (local, path, ty) = self.place(cx, target)?;
        let value = match op {
            Some(BinOp::Shl | BinOp::Shr) => {
                if self.types.as_int(ty).is_none() && ty != Ty::ERROR {
                    let message =
                        format!("`{}=` requires an integer target, found {}", op.unwrap().as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, None);
                self.require_int(&value);
                value
            }
            Some(op) => {
                if self.types.as_int(ty).is_none() && ty != Ty::ERROR {
                    let message = format!("`{}=` cannot be applied to {}", op.as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
            None => {
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
        };
        cx.diverged |= value.ty == Ty::NEVER;
        Some(hir::Stmt::Assign { local, path, op, value, span })
    }

    /// Resolves an assignment target to a `var` binding and a field path.
    fn place(&mut self, cx: &mut FnCx, target: &ast::Expr) -> Option<(LocalId, Vec<hir::PlaceStep>, Ty)> {
        let (base, field) = match &target.kind {
            ast::ExprKind::Path(segments) => {
                let root = &segments[0];
                let Some(local) = self.lookup(cx, &root.name) else {
                    let known = self.names.contains_key(&root.name);
                    let (code, message) = if known {
                        ("E1411", format!("`{}` is not a mutable binding", root.name))
                    } else {
                        ("E1001", format!("unknown name `{}`", root.name))
                    };
                    self.diags.error(code, root.span, message);
                    return None;
                };
                if !cx.mutable[local.0 as usize] {
                    let message = format!("cannot assign to `{}`: it is not declared with `var`", root.name);
                    self.diags.error("E1411", root.span, message);
                }
                let mut ty = cx.locals[local.0 as usize].ty;
                let mut path = Vec::new();
                for segment in &segments[1..] {
                    let (index, field_ty) = self.field_of(ty, FieldRef::Name(&segment.name), segment.span)?;
                    path.extend(index.map(hir::PlaceStep::Field));
                    ty = field_ty;
                }
                return Some((local, path, ty));
            }
            ast::ExprKind::Index { base, index } => {
                let (local, mut path, ty) = self.place(cx, base)?;
                let TyKind::Array(element) = *self.types.kind(ty) else {
                    let message = format!("{} cannot be indexed", self.display(ty));
                    self.diags.error("E1102", base.span, message);
                    return None;
                };
                let index = self.expr(cx, index, Some(Ty::INT));
                path.push(hir::PlaceStep::Index(self.coerce(index, Ty::INT)));
                return Some((local, path, element));
            }
            ast::ExprKind::Field(base, name) => (base, (FieldRef::Name(&name.name), name.span)),
            ast::ExprKind::TupleSlot(base, slot) => (base, (FieldRef::Slot(*slot), target.span)),
            _ => {
                self.diags.error("E1411", target.span, "this expression cannot be assigned to");
                return None;
            }
        };
        let (local, mut path, ty) = self.place(cx, base)?;
        let (index, field_ty) = self.field_of(ty, field.0, field.1)?;
        path.extend(index.map(hir::PlaceStep::Field));
        Some((local, path, field_ty))
    }

    /// The position and type of a field. The position is `None` for a
    /// newtype's `.value`, which shares the newtype's representation.
    fn field_of(&mut self, ty: Ty, field: FieldRef, span: Span) -> Option<(Option<u32>, Ty)> {
        let found = match (self.types.kind(ty), field) {
            (TyKind::Error, _) => return None,
            (TyKind::Tuple(items), FieldRef::Slot(slot)) => items.get(slot as usize).map(|&t| (Some(slot), t)),
            (TyKind::Adt(id), FieldRef::Name(name)) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => {
                    fields.iter().position(|f| f.name == name).map(|i| (Some(i as u32), fields[i].ty))
                }
                AdtKind::Newtype(inner) => (name == "value").then_some((None, *inner)),
                AdtKind::Enum(_) => None,
            },
            _ => None,
        };
        if found.is_none() {
            let what = match field {
                FieldRef::Name(name) => format!("field `{name}`"),
                FieldRef::Slot(slot) => format!("element {slot}"),
            };
            let message = format!("{} has no {what}", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        found
    }

    fn for_loop(&mut self, cx: &mut FnCx, header: &ast::ForHeader, body: &ast::Block) -> Option<hir::Stmt> {
        match header {
            ast::ForHeader::Infinite => {
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                let saw_break = cx.loops.pop().unwrap();
                cx.diverged |= !saw_break;
                Some(hir::Stmt::Loop { condition: None, body })
            }
            ast::ForHeader::Condition(condition) => {
                let condition = self.expr(cx, condition, Some(Ty::BOOL));
                let condition = self.coerce(condition, Ty::BOOL);
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                Some(hir::Stmt::Loop { condition: Some(condition), body })
            }
            ast::ForHeader::In { pattern, source } => {
                let ast::ExprKind::Binary(op @ (BinOp::Range | BinOp::RangeInclusive), start, end) = &source.kind
                else {
                    let source = self.expr(cx, source, None);
                    let element = match *self.types.kind(source.ty) {
                        TyKind::Array(element) | TyKind::Set(element) => element,
                        // A map yields its entries as `(key, value)` (10.13).
                        TyKind::Map(key, value) => self.types.intern(TyKind::Tuple(vec![key, value])),
                        TyKind::Error => Ty::ERROR,
                        _ => {
                            let message = format!("{} cannot be iterated", self.display(source.ty));
                            self.diags.error("E1206", source.span, message);
                            return None;
                        }
                    };
                    cx.scopes.push(Vec::new());
                    let pattern =
                        self.pattern(cx, pattern, element, PatternContext { in_match: false, mutable: false });
                    if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
                        let message = format!("this pattern can fail, for example on `{missing}`");
                        self.diags.error("E1201", pattern.span, message);
                    }
                    cx.loops.push(false);
                    let body = self.block(cx, body, Some(Ty::UNIT));
                    cx.loops.pop();
                    cx.scopes.pop();
                    return Some(hir::Stmt::ForEach { pattern, source, body });
                };
                let (start, end) = self.operands(cx, start, end, None);
                self.require_int(&start);
                cx.scopes.push(Vec::new());
                let local = match &pattern.kind {
                    ast::PatternKind::Binding(binding) => self.declare(cx, binding, start.ty, false),
                    ast::PatternKind::Wildcard => self.hidden_local(cx, start.ty),
                    _ => {
                        self.diags.error("E1206", pattern.span, "a range yields integers; bind them to a name");
                        self.hidden_local(cx, start.ty)
                    }
                };
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                cx.scopes.pop();
                Some(hir::Stmt::ForRange { local, start, end, inclusive: *op == BinOp::RangeInclusive, body })
            }
        }
    }

    // ---- expressions -----------------------------------------------------

    fn mk(kind: hir::ExprKind, ty: Ty, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
    }

    fn error_expr(span: Span) -> hir::Expr {
        Self::mk(hir::ExprKind::Error, Ty::ERROR, span)
    }

    /// Reports a mismatch unless `value` already has (or diverges into)
    /// `expected`. A `T` where `T?` is expected becomes `some(T)` (8.9).
    fn coerce(&mut self, value: hir::Expr, expected: Ty) -> hir::Expr {
        if let TyKind::Option(inner) = self.types.kind(expected)
            && value.ty == *inner
        {
            let span = value.span;
            return Self::mk(hir::ExprKind::Variant(SOME, vec![(0, value)]), expected, span);
        }
        if value.ty != expected
            && value.ty != Ty::NEVER
            && !self.types.has_error(value.ty)
            && !self.types.has_error(expected)
        {
            let message = format!("expected {}, found {}", self.display(expected), self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
        value
    }

    fn require_int(&mut self, value: &hir::Expr) {
        if self.types.as_int(value.ty).is_none() && ![Ty::NEVER, Ty::ERROR].contains(&value.ty) {
            let message = format!("expected an integer, found {}", self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
    }

    fn expr(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::Int { value, suffix } => self.int_literal(*value as i128, *suffix, expected, span),
            ast::ExprKind::Bool(b) => Self::mk(hir::ExprKind::Bool(*b), Ty::BOOL, span),
            ast::ExprKind::Unit => Self::mk(hir::ExprKind::Unit, Ty::UNIT, span),
            ast::ExprKind::Str(segments) => self.string(cx, segments, span),
            ast::ExprKind::Path(segments) => self.path(cx, segments, span),
            ast::ExprKind::Call { callee, args } => self.call(cx, callee, args, span),
            ast::ExprKind::Unary(op, operand) => self.unary(cx, *op, operand, expected, span),
            ast::ExprKind::Binary(op, left, right) => self.binary(cx, *op, left, right, expected, span),
            ast::ExprKind::If { condition, then, otherwise } => {
                self.if_expr(cx, condition, then, otherwise.as_deref(), expected, span)
            }
            ast::ExprKind::Block(block) => {
                let block = self.block(cx, block, expected);
                let ty = block.ty;
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            ast::ExprKind::Float { text, suffix } => self.float_literal(text, *suffix, false, expected, span),
            ast::ExprKind::Char(c) => Self::mk(hir::ExprKind::Char(*c), Ty::CHAR, span),
            ast::ExprKind::Field(base, name) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Name(&name.name), name.span, span)
            }
            ast::ExprKind::TupleSlot(base, slot) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Slot(*slot), span, span)
            }
            ast::ExprKind::Tuple(items) => self.tuple(cx, items, expected, span),
            ast::ExprKind::Array(items) => self.array_literal(cx, items, expected, span),
            ast::ExprKind::Map(pairs) => self.map_literal(cx, pairs, expected, span),
            ast::ExprKind::Set(items) => self.set_literal(cx, items, expected, span),
            ast::ExprKind::Index { base, index } => {
                let base = self.expr(cx, base, None);
                if let TyKind::Map(key, value) = *self.types.kind(base.ty) {
                    // Reading a map entry yields `V?`: absence is normal (10.10).
                    let index = self.expr(cx, index, Some(key));
                    let index = self.coerce(index, key);
                    let ty = self.types.intern(TyKind::Option(value));
                    return Self::mk(hir::ExprKind::MapOp(hir::MapOp::Get, vec![base, index]), ty, span);
                }
                let TyKind::Array(element) = *self.types.kind(base.ty) else {
                    return self.not_indexable(base, span);
                };
                let index = self.expr(cx, index, Some(Ty::INT));
                let index = self.coerce(index, Ty::INT);
                Self::mk(hir::ExprKind::Index(Box::new(base), Box::new(index)), element, span)
            }
            ast::ExprKind::Slice { base, start, end, step } => {
                let base = self.expr(cx, base, None);
                if !matches!(self.types.kind(base.ty), TyKind::Array(_)) {
                    return self.not_indexable(base, span);
                }
                let mut bound = |this: &mut Self, part: &Option<Box<ast::Expr>>| {
                    part.as_ref().map(|p| {
                        let value = this.expr(cx, p, Some(Ty::INT));
                        Box::new(this.coerce(value, Ty::INT))
                    })
                };
                let parts = [bound(self, start), bound(self, end), bound(self, step)];
                let ty = base.ty;
                Self::mk(hir::ExprKind::Slice(Box::new(base), parts), ty, span)
            }
            ast::ExprKind::Generic { .. } => {
                self.diags.error("E1102", span, "type arguments must be followed by a call");
                Self::error_expr(span)
            }
            ast::ExprKind::Match { scrutinee, arms } => self.match_expr(cx, scrutinee, arms, expected, span),
            ast::ExprKind::Record { path, fields } => self.record_literal(cx, path, fields, span),
            ast::ExprKind::With { base, updates } => self.with_expr(cx, base, updates, span),
            ast::ExprKind::Try(inner) => self.try_expr(cx, inner, span),
            ast::ExprKind::None => match expected.map(|t| self.types.kind(t)) {
                Some(TyKind::Option(_)) => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), expected.unwrap(), span),
                _ => {
                    self.diags.error("E1304", span, "`none` needs an expected `T?` type");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Construct(constructor, value) => self.construct(cx, constructor, value, expected, span),
            ast::ExprKind::Error => Self::error_expr(span),
        }
    }

    fn field_access(&mut self, base: hir::Expr, field: FieldRef, field_span: Span, span: Span) -> hir::Expr {
        let Some((index, ty)) = self.field_of(base.ty, field, field_span) else {
            return Self::error_expr(span);
        };
        let kind = match index {
            Some(index) => hir::ExprKind::Field(Box::new(base), index),
            None => hir::ExprKind::Retype(Box::new(base)),
        };
        Self::mk(kind, ty, span)
    }

    fn tuple(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let expected_items = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Tuple(types)) if types.len() == items.len() => types.clone(),
            _ => Vec::new(),
        };
        let items: Vec<hir::Expr> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let expected = expected_items.get(i).copied();
                let value = self.expr(cx, item, expected);
                match expected {
                    Some(expected) => self.coerce(value, expected),
                    None => value,
                }
            })
            .collect();
        let ty = self.types.intern(TyKind::Tuple(items.iter().map(|i| i.ty).collect()));
        Self::mk(hir::ExprKind::Aggregate(items), ty, span)
    }

    /// The fields of a record type, or an error at `span`.
    fn record_fields(&mut self, ty: Ty, span: Span) -> Option<Vec<(String, Ty)>> {
        if let TyKind::Adt(id) = self.types.kind(ty)
            && let AdtKind::Record(fields) = &self.types.adt(*id).kind
        {
            return Some(fields.iter().map(|f| (f.name.clone(), f.ty)).collect());
        }
        if ty != Ty::ERROR {
            let message = format!("{} is not a record type", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        None
    }

    /// Checks field initializers against `fields`, in source order.
    fn field_values(
        &mut self,
        cx: &mut FnCx,
        record: Ty,
        fields: &[(String, Ty)],
        inits: &[ast::FieldInit],
        allow_shorthand: bool,
    ) -> (Vec<(u32, hir::Expr)>, Vec<bool>) {
        let mut seen = vec![false; fields.len()];
        let mut values = Vec::new();
        for init in inits {
            let index = fields.iter().position(|(name, _)| *name == init.name.name);
            let expected = index.map(|i| fields[i].1);
            let value = match &init.value {
                Some(value) => self.expr(cx, value, expected),
                None if allow_shorthand && crate::lexer::Kw::lookup(&init.name.name).is_none() => {
                    self.path(cx, std::slice::from_ref(&init.name), init.name.span)
                }
                None => {
                    let message =
                        format!("field `{}` needs an explicit value: `{}: value`", init.name.name, init.name.name);
                    self.diags.error("E1102", init.name.span, message);
                    Self::error_expr(init.name.span)
                }
            };
            let Some(index) = index else {
                let message = format!("{} has no field `{}`", self.display(record), init.name.name);
                self.diags.error("E1102", init.name.span, message);
                continue;
            };
            if std::mem::replace(&mut seen[index], true) {
                let message = format!("field `{}` is initialized more than once", init.name.name);
                self.diags.error("E1102", init.name.span, message);
            }
            let value = self.coerce(value, fields[index].1);
            values.push((index as u32, value));
        }
        (values, seen)
    }

    fn record_literal(
        &mut self,
        cx: &mut FnCx,
        path: &[ast::Ident],
        inits: &[ast::FieldInit],
        span: Span,
    ) -> hir::Expr {
        if let Some(resolved) = self.variant_path(path) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            let Payload::Record(fields) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take named fields", Self::path_text(path));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            let fields: Vec<(String, Ty)> = fields.into_iter().map(|f| (f.name, f.ty)).collect();
            let (values, seen) = self.field_values(cx, ty, &fields, inits, true);
            self.report_missing(&fields, &seen, span);
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        let [name] = path else {
            return self.unsupported_expr(span, "qualified record literals");
        };
        let ty = match self.type_names.get(&name.name) {
            Some(TypeName::Nominal(ty)) => *ty,
            Some(TypeName::Alias(index)) => {
                let index = *index;
                self.resolve_alias(index)
            }
            None => {
                let message = format!("unknown type `{}`", name.name);
                self.diags.error("E1001", name.span, message);
                return Self::error_expr(span);
            }
        };
        let Some(fields) = self.record_fields(ty, name.span) else {
            return Self::error_expr(span);
        };
        let (values, seen) = self.field_values(cx, ty, &fields, inits, true);
        self.report_missing(&fields, &seen, span);
        Self::mk(hir::ExprKind::Record(values), ty, span)
    }

    fn report_missing(&mut self, fields: &[(String, Ty)], seen: &[bool], span: Span) {
        let missing: Vec<&str> = fields.iter().zip(seen).filter(|(_, s)| !**s).map(|((n, _), _)| n.as_str()).collect();
        if !missing.is_empty() {
            let message = format!("construction is missing fields: {}", missing.join(", "));
            self.diags.error("E1102", span, message);
        }
    }

    fn path_text(path: &[ast::Ident]) -> String {
        path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
    }

    /// Resolves `Enum.Variant`. Returns `None` when the path does not name a
    /// type, and `Some(None)` after reporting a bad variant.
    fn variant_path(&mut self, path: &[ast::Ident]) -> Option<Option<(Ty, u32)>> {
        let (variant, type_path) = path.split_last()?;
        let type_name = type_path.last()?;
        let ty = match type_path {
            [name] => match self.type_names.get(&name.name)? {
                TypeName::Nominal(ty) => *ty,
                TypeName::Alias(index) => {
                    let index = *index;
                    self.resolve_alias(index)
                }
            },
            [module, name]
                if name.name == "ConsoleError"
                    && matches!(self.names.get(&module.name), Some(ModuleName::Module(StdModule::Console))) =>
            {
                self.console_error?
            }
            _ => return None,
        };
        let found = match self.types.kind(ty) {
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Enum(variants) => Some(variants.iter().position(|v| v.name == variant.name)),
                _ => None,
            },
            _ => None,
        };
        match found {
            Some(Some(index)) => Some(Some((ty, index as u32))),
            Some(None) => {
                let message = format!("enum `{}` has no variant `{}`", type_name.name, variant.name);
                self.diags.error("E1001", variant.span, message);
                Some(None)
            }
            None if ty == Ty::ERROR => Some(None),
            None => {
                let message = format!("`{}` is not an enum", type_name.name);
                self.diags.error("E1102", type_name.span, message);
                Some(None)
            }
        }
    }

    fn payload(&self, ty: Ty, variant: u32) -> Payload {
        let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
        let AdtKind::Enum(variants) = &self.types.adt(*id).kind else { unreachable!() };
        variants[variant as usize].payload.clone()
    }

    fn match_expr(
        &mut self,
        cx: &mut FnCx,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let scrutinee = self.expr(cx, scrutinee, None);
        let mut result = expected;
        let mut out = Vec::new();
        for arm in arms {
            cx.scopes.push(Vec::new());
            let outer = std::mem::replace(&mut cx.diverged, false);
            let pattern =
                self.pattern(cx, &arm.pattern, scrutinee.ty, PatternContext { in_match: true, mutable: false });
            let guard = arm.guard.as_ref().map(|guard| {
                let guard = self.expr(cx, guard, Some(Ty::BOOL));
                self.coerce(guard, Ty::BOOL)
            });
            let body = match &arm.body {
                ast::ArmBody::Expr(body) => {
                    let value = self.expr(cx, body, result);
                    match result {
                        Some(expected) => self.coerce(value, expected),
                        None => value,
                    }
                }
                ast::ArmBody::Transfer(stmt) => {
                    let stmts = self.statement(cx, stmt).into_iter().collect();
                    let block = hir::Block { stmts, tail: None, ty: Ty::NEVER };
                    Self::mk(hir::ExprKind::Block(block), Ty::NEVER, stmt.span)
                }
            };
            if result.is_none() && body.ty != Ty::NEVER && body.ty != Ty::ERROR {
                result = Some(body.ty);
            }
            cx.diverged = outer;
            cx.scopes.pop();
            out.push(hir::Arm { pattern, guard, body });
        }
        let checked: Vec<(&hir::Pattern, bool)> = out.iter().map(|a| (&a.pattern, a.guard.is_some())).collect();
        if scrutinee.ty != Ty::ERROR && !checked.iter().any(|(p, _)| p.ty == Ty::ERROR) {
            let report = crate::exhaust::check_match(&self.types, scrutinee.ty, &checked);
            for index in report.unreachable {
                self.diags.error("E1203", out[index].pattern.span, "this arm is covered by earlier arms");
            }
            if let Some(missing) = report.missing {
                let message = format!("`match` is not exhaustive: `{missing}` is not covered");
                self.diags.error("E1204", span, message);
            }
        }
        let all_diverge = out.iter().all(|a| a.body.ty == Ty::NEVER);
        let ty = if all_diverge { Ty::NEVER } else { result.unwrap_or(Ty::UNIT) };
        Self::mk(hir::ExprKind::Match(Box::new(scrutinee), out), ty, span)
    }

    fn pattern_error(&mut self, span: Span, message: String) -> hir::Pattern {
        self.diags.error("E1202", span, message);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    /// Checks a pattern against a value of type `ty`, declaring its bindings
    /// in the current scope.
    fn pattern(&mut self, cx: &mut FnCx, p: &ast::Pattern, ty: Ty, context: PatternContext) -> hir::Pattern {
        let span = p.span;
        let mismatch = |this: &Self, what: &str| format!("{what} cannot match a value of type {}", this.display(ty));
        let kind = match &p.kind {
            ast::PatternKind::Wildcard => hir::PatKind::Wildcard,
            ast::PatternKind::Binding(name) => {
                let message = match name.name.as_str() {
                    "None" => Some("`None` is not a Tondo pattern; write `none`".to_string()),
                    "Some" | "Ok" | "Err" => Some(format!(
                        "`{}` is not a Tondo pattern; write `{}(...)`",
                        name.name,
                        name.name.to_lowercase()
                    )),
                    n if context.in_match && CONTEXTUAL_CONSTRUCTORS.contains(&n) => {
                        Some(format!("`{n}` without parentheses would match any value; write `{n}(...)` or rename it"))
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    return self.pattern_error(name.span, message);
                }
                hir::PatKind::Binding(self.declare(cx, name, ty, context.mutable))
            }
            ast::PatternKind::Literal(expr) => {
                let value = self.expr(cx, expr, Some(ty));
                let literal = match value.kind {
                    hir::ExprKind::Int(v) => hir::Literal::Int(v),
                    hir::ExprKind::Float(v) => hir::Literal::Float(v.to_bits()),
                    hir::ExprKind::Char(c) => hir::Literal::Char(c),
                    hir::ExprKind::Bool(b) => hir::Literal::Bool(b),
                    hir::ExprKind::Str(s) => hir::Literal::Str(s),
                    hir::ExprKind::Unit => hir::Literal::Unit,
                    _ => return self.unsupported_pattern(span, "this literal pattern"),
                };
                if value.ty != ty && ty != Ty::ERROR && value.ty != Ty::ERROR {
                    let message = mismatch(self, &format!("a {} literal", self.display(value.ty)));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Literal(literal)
            }
            ast::PatternKind::None if !context.in_match => {
                return self.pattern_error(span, "`none` is a reserved keyword and cannot name a binding".into());
            }
            ast::PatternKind::None => match self.types.kind(ty) {
                TyKind::Option(_) => hir::PatKind::Variant(NONE, Vec::new()),
                TyKind::Error => hir::PatKind::Wildcard,
                _ => return self.pattern_error(span, mismatch(self, "`none`")),
            },
            ast::PatternKind::Tuple(items) => match self.types.kind(ty) {
                TyKind::Tuple(types) if types.len() == items.len() => {
                    let types = types.clone();
                    let fields = items.iter().zip(types).map(|(item, t)| self.pattern(cx, item, t, context)).collect();
                    hir::PatKind::Fields(fields)
                }
                TyKind::Error => hir::PatKind::Wildcard,
                _ => {
                    let message = mismatch(self, &format!("a tuple pattern of {} elements", items.len()));
                    return self.pattern_error(span, message);
                }
            },
            ast::PatternKind::Constructor { path, args } => {
                if let [name] = path.as_slice()
                    && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str())
                {
                    let (variant, payload) = match (name.name.as_str(), self.types.kind(ty)) {
                        ("some", TyKind::Option(inner)) => (SOME, *inner),
                        ("ok", TyKind::Result(ok, _)) => (OK, *ok),
                        ("err", TyKind::Result(_, err)) => (ERR, *err),
                        (_, TyKind::Error) => return Self::wild(ty, span),
                        _ => return self.pattern_error(span, mismatch(self, &format!("`{}(...)`", name.name))),
                    };
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let inner = self.pattern(cx, arg, payload, context);
                    return hir::Pattern { kind: hir::PatKind::Variant(variant, vec![inner]), ty, span };
                }
                if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    if enum_ty != ty {
                        let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                        return self.pattern_error(span, message);
                    }
                    let Payload::Tuple(types) = self.payload(ty, variant) else {
                        let message = format!("`{}` has no positional payload", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    if types.len() != args.len() {
                        let message = format!("`{}` has {} payload values", Self::path_text(path), types.len());
                        return self.pattern_error(span, message);
                    }
                    let fields = args.iter().zip(types).map(|(arg, t)| self.pattern(cx, arg, t, context)).collect();
                    hir::PatKind::Variant(variant, fields)
                } else if let [name] = path.as_slice()
                    && let Some(&ModuleName::Newtype(newtype)) = self.names.get(&name.name)
                {
                    if newtype != ty {
                        let message = mismatch(self, &format!("a `{}` pattern", name.name));
                        return self.pattern_error(span, message);
                    }
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                    let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                    hir::PatKind::Newtype(Box::new(self.pattern(cx, arg, inner, context)))
                } else {
                    let message = format!("`{}` is not a constructor", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
            }
            ast::PatternKind::Record { path, fields, rest } => {
                let (record_ty, declared, variant) = if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    let Payload::Record(fields) = self.payload(enum_ty, variant) else {
                        let message = format!("`{}` has no named fields", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    (enum_ty, fields.into_iter().map(|f| (f.name, f.ty)).collect(), Some(variant))
                } else if let [name] = path.as_slice()
                    && let Some(TypeName::Nominal(record)) = self.type_names.get(&name.name)
                {
                    let record = *record;
                    let Some(fields) = self.record_fields(record, name.span) else { return Self::wild(ty, span) };
                    (record, fields, None)
                } else {
                    let message = format!("`{}` is not a record type or variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                if record_ty != ty {
                    let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                let patterns = self.field_patterns(cx, &declared, fields, *rest, context, span);
                match variant {
                    Some(variant) => hir::PatKind::Variant(variant, patterns),
                    None => hir::PatKind::Fields(patterns),
                }
            }
            ast::PatternKind::Path(path) => {
                let Some(resolved) = self.variant_path(path) else {
                    let message = format!("`{}` is not a variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                if enum_ty != ty {
                    let message = mismatch(self, &format!("`{}`", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                if !matches!(self.payload(ty, variant), Payload::Unit) {
                    let message = format!("`{}` has a payload; match it explicitly", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Variant(variant, Vec::new())
            }
        };
        hir::Pattern { kind, ty, span }
    }

    fn wild(ty: Ty, span: Span) -> hir::Pattern {
        hir::Pattern { kind: hir::PatKind::Wildcard, ty, span }
    }

    fn unsupported_pattern(&mut self, span: Span, what: &str) -> hir::Pattern {
        self.diags.unsupported(span, what);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    fn field_patterns(
        &mut self,
        cx: &mut FnCx,
        declared: &[(String, Ty)],
        fields: &[ast::FieldPattern],
        rest: bool,
        context: PatternContext,
        span: Span,
    ) -> Vec<hir::Pattern> {
        let mut patterns: Vec<Option<hir::Pattern>> = (0..declared.len()).map(|_| None).collect();
        for field in fields {
            let Some(index) = declared.iter().position(|(n, _)| *n == field.name.name) else {
                let message = format!("there is no field `{}`", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            };
            if patterns[index].is_some() {
                let message = format!("field `{}` appears more than once", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            }
            let ty = declared[index].1;
            let pattern = match &field.pattern {
                Some(pattern) => self.pattern(cx, pattern, ty, context),
                None => {
                    let binding =
                        ast::Pattern { kind: ast::PatternKind::Binding(field.name.clone()), span: field.name.span };
                    self.pattern(cx, &binding, ty, context)
                }
            };
            patterns[index] = Some(pattern);
        }
        let missing: Vec<&str> =
            declared.iter().zip(&patterns).filter(|(_, p)| p.is_none()).map(|((n, _), _)| n.as_str()).collect();
        if !rest && !missing.is_empty() {
            let message = format!("pattern does not mention {}; add `..` to ignore them", missing.join(", "));
            self.diags.error("E1202", span, message);
        }
        patterns
            .into_iter()
            .zip(declared)
            .map(|(p, (_, ty))| p.unwrap_or(hir::Pattern { kind: hir::PatKind::Wildcard, ty: *ty, span }))
            .collect()
    }

    fn with_expr(&mut self, cx: &mut FnCx, base: &ast::Expr, updates: &[ast::FieldInit], span: Span) -> hir::Expr {
        let base = self.expr(cx, base, None);
        let ty = base.ty;
        let Some(fields) = self.record_fields(ty, base.span) else {
            return Self::error_expr(span);
        };
        if updates.is_empty() {
            self.diags.error("E1102", span, "`with` must update at least one field");
        }
        let (values, _) = self.field_values(cx, ty, &fields, updates, false);
        Self::mk(hir::ExprKind::With(Box::new(base), values), ty, span)
    }

    /// `some(value)`, `ok(value)` or `err(value)`.
    fn construct(
        &mut self,
        cx: &mut FnCx,
        constructor: &str,
        value: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let expected_kind = expected.map(|t| self.types.kind(t).clone());
        let (variant, payload_ty, ty) = match (constructor, expected_kind) {
            ("some", Some(TyKind::Option(inner))) => (SOME, Some(inner), expected),
            ("some", _) => (SOME, None, None),
            ("ok", Some(TyKind::Result(ok, _))) => (OK, Some(ok), expected),
            ("err", Some(TyKind::Result(_, err))) => (ERR, Some(err), expected),
            _ => {
                let message = format!("`{constructor}(...)` needs an expected `T ! E` type");
                self.diags.error("E1304", span, message);
                self.expr(cx, value, None);
                return Self::error_expr(span);
            }
        };
        let value = self.expr(cx, value, payload_ty);
        let value = match payload_ty {
            Some(payload) => self.coerce(value, payload),
            None => value,
        };
        let ty = ty.unwrap_or_else(|| self.types.intern(TyKind::Option(value.ty)));
        Self::mk(hir::ExprKind::Variant(variant, vec![(0, value)]), ty, span)
    }

    /// `value?` propagates `none` or an error out of the enclosing function
    /// and otherwise yields the payload (15.2).
    fn try_expr(&mut self, cx: &mut FnCx, inner: &ast::Expr, span: Span) -> hir::Expr {
        let value = self.expr(cx, inner, None);
        match self.types.kind(value.ty).clone() {
            TyKind::Option(payload) => {
                if !matches!(self.types.kind(cx.result), TyKind::Option(_)) {
                    let message = format!(
                        "`?` on an option needs the function to return an option, but it returns {}",
                        self.display(cx.result)
                    );
                    self.diags.error("E1301", span, message);
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), payload, span)
            }
            TyKind::Result(ok, error) => {
                if let Some(channel) = self.error_channel(cx, error, span, "E1301")
                    && channel != error
                    && channel != Ty::ERROR
                {
                    let message = format!(
                        "`?` would propagate {} but the function fails with {}",
                        self.display(error),
                        self.display(channel)
                    );
                    self.diags.error("E1301", span, message);
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), ok, span)
            }
            TyKind::Error => Self::error_expr(span),
            _ => {
                let message = format!("`?` applies to `T?` or `T ! E`, not {}", self.display(value.ty));
                self.diags.error("E1301", span, message);
                Self::error_expr(span)
            }
        }
    }

    fn displayable(&self, ty: Ty) -> bool {
        match self.types.kind(ty) {
            TyKind::Int(_)
            | TyKind::Float(_)
            | TyKind::Char
            | TyKind::Byte
            | TyKind::Bool
            | TyKind::String
            | TyKind::Error
            | TyKind::Never => true,
            TyKind::Array(element) | TyKind::Set(element) => self.displayable(*element),
            TyKind::Map(key, value) => self.displayable(*key) && self.displayable(*value),
            _ => false,
        }
    }

    /// A map type; its entry tuple `(K, V)` is interned too, for iteration.
    fn map_type(&mut self, key: Ty, value: Ty) -> Ty {
        self.types.intern(TyKind::Tuple(vec![key, value]));
        self.types.intern(TyKind::Option(value));
        self.types.intern(TyKind::Map(key, value))
    }

    fn require_key(&mut self, ty: Ty, span: Span) {
        if !self.types.is_key(ty) {
            let message = format!("{} cannot be a map key or set element: it is not `Key`", self.display(ty));
            self.diags.error("E1105", span, message);
        }
    }

    fn map_literal(
        &mut self,
        cx: &mut FnCx,
        pairs: &[(ast::Expr, ast::Expr)],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let (mut key, mut value) = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Map(key, value)) => (Some(*key), Some(*value)),
            _ => (None, None),
        };
        let mut entries = Vec::new();
        let mut constants: Vec<(hir::Literal, Span)> = Vec::new();
        for (k, v) in pairs {
            let k = self.expr(cx, k, key);
            let k = match key {
                Some(key) => self.coerce(k, key),
                None => {
                    key = Some(k.ty);
                    k
                }
            };
            let constant = match &k.kind {
                hir::ExprKind::Int(v) => Some(hir::Literal::Int(*v)),
                hir::ExprKind::Str(s) => Some(hir::Literal::Str(s.clone())),
                hir::ExprKind::Bool(b) => Some(hir::Literal::Bool(*b)),
                hir::ExprKind::Char(c) => Some(hir::Literal::Char(*c)),
                _ => None,
            };
            if let Some(constant) = constant {
                if constants.iter().any(|(c, _)| *c == constant) {
                    self.diags.error("E1116", k.span, "this key appears more than once in the literal");
                }
                constants.push((constant, k.span));
            }
            let v = self.expr(cx, v, value);
            let v = match value {
                Some(value) => self.coerce(v, value),
                None => {
                    value = Some(v.ty);
                    v
                }
            };
            entries.push((k, v));
        }
        let (Some(key), Some(value)) = (key, value) else {
            self.diags.error("E1101", span, "an empty map literal needs an expected `Map[K, V]` type");
            return Self::error_expr(span);
        };
        self.require_key(key, span);
        let ty = self.map_type(key, value);
        Self::mk(hir::ExprKind::Map(entries), ty, span)
    }

    fn set_literal(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Set(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty set literal needs an expected `Set[T]` type");
            return Self::error_expr(span);
        };
        self.require_key(element, span);
        let ty = self.types.intern(TyKind::Set(element));
        Self::mk(hir::ExprKind::Set(values), ty, span)
    }

    fn not_indexable(&mut self, base: hir::Expr, span: Span) -> hir::Expr {
        if base.ty != Ty::ERROR {
            let message = format!("{} cannot be indexed", self.display(base.ty));
            self.diags.error("E1102", base.span, message);
        }
        Self::error_expr(span)
    }

    fn array_literal(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Array(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty array literal needs an expected `Array[T]` type");
            return Self::error_expr(span);
        };
        let ty = self.types.intern(TyKind::Array(element));
        Self::mk(hir::ExprKind::Array(values), ty, span)
    }

    /// `Array.new[T]()` and `Array.withCapacity[T](capacity)`.
    fn generic_call(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let ast::ExprKind::Path(segments) = &base.kind else {
            return self.unsupported_expr(span, "generic calls");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if let ["Map" | "Set", "new"] = names.as_slice() {
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let ty = match (names[0], types.as_slice()) {
                ("Map", [key, value]) => {
                    self.require_key(*key, span);
                    self.map_type(*key, *value)
                }
                ("Set", [element]) => {
                    self.require_key(*element, span);
                    self.types.intern(TyKind::Set(*element))
                }
                _ => {
                    self.diags.error("E1104", span, "wrong number of type arguments");
                    return Self::error_expr(span);
                }
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            return Self::mk(hir::ExprKind::MapOp(hir::MapOp::New, Vec::new()), ty, span);
        }
        let (op, params) = match names.as_slice() {
            ["Array", "new"] => (hir::ArrayOp::New, Vec::new()),
            ["Array", "withCapacity"] => (hir::ArrayOp::WithCapacity, vec![Ty::INT]),
            _ => return self.unsupported_expr(span, "generic functions"),
        };
        let [element] = type_args else {
            self.diags.error("E1104", span, "`Array` takes one type argument");
            return Self::error_expr(span);
        };
        let element = self.resolve_type(element);
        let array = self.types.intern(TyKind::Array(element));
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let values = args
            .iter()
            .zip(params)
            .map(|(a, t)| {
                let v = self.expr(cx, &a.value, Some(t));
                self.coerce(v, t)
            })
            .collect();
        let ty = match op {
            hir::ArrayOp::New => array,
            _ => self.types.intern(TyKind::Result(array, self.collection_error)),
        };
        Self::mk(hir::ExprKind::ArrayOp(op, values), ty, span)
    }

    /// `receiver.method(args)` on an intrinsic collection (std.collections).
    fn method_call(
        &mut self,
        cx: &mut FnCx,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if matches!(method.name.as_str(), "insert" | "remove") {
            // These change the collection's extent and need a `var` receiver.
            let Some((local, path, ty)) = self.place(cx, receiver) else { return Self::error_expr(span) };
            let place = hir::Place { local, path };
            let mutation = if method.name == "insert" { hir::MapMutation::Insert } else { hir::MapMutation::Remove };
            let (params, result) = match (self.types.kind(ty).clone(), mutation) {
                (TyKind::Map(key, value), hir::MapMutation::Insert) => {
                    (vec![key, value], self.types.intern(TyKind::Option(value)))
                }
                (TyKind::Map(key, value), hir::MapMutation::Remove) => {
                    (vec![key], self.types.intern(TyKind::Option(value)))
                }
                (TyKind::Set(element), _) => (vec![element], Ty::BOOL),
                _ => return self.no_method(ty, method, span),
            };
            if args.len() != params.len() {
                return self.arity_error(span, params.len(), args.len());
            }
            let values = args
                .iter()
                .zip(params)
                .map(|(a, t)| {
                    let v = self.expr(cx, &a.value, Some(t));
                    self.coerce(v, t)
                })
                .collect();
            return Self::mk(hir::ExprKind::MapMutate(mutation, place, values), result, span);
        }
        if matches!(method.name.as_str(), "push" | "pop") {
            // These change the array's length and need a `var` receiver (10.5).
            let Some((local, path, ty)) = self.place(cx, receiver) else { return Self::error_expr(span) };
            let TyKind::Array(element) = *self.types.kind(ty) else {
                return self.no_method(ty, method, span);
            };
            let place = hir::Place { local, path };
            return if method.name == "push" {
                let [value] = args else { return self.arity_error(span, 1, args.len()) };
                let value = self.expr(cx, &value.value, Some(element));
                let value = self.coerce(value, element);
                let ty = self.types.intern(TyKind::Result(Ty::UNIT, self.collection_error));
                Self::mk(hir::ExprKind::Push(place, Box::new(value)), ty, span)
            } else {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                let ty = self.types.intern(TyKind::Option(element));
                Self::mk(hir::ExprKind::Pop(place), ty, span)
            };
        }
        let value = self.expr(cx, receiver, None);
        if let TyKind::Map(key, _) | TyKind::Set(key) = *self.types.kind(value.ty) {
            let (op, result) = match (method.name.as_str(), self.types.kind(value.ty).clone()) {
                ("get", TyKind::Map(_, entry)) => (hir::MapOp::Get, self.types.intern(TyKind::Option(entry))),
                ("contains", _) => (hir::MapOp::Contains, Ty::BOOL),
                _ => return self.no_method(value.ty, method, span),
            };
            let [arg] = args else { return self.arity_error(span, 1, args.len()) };
            let arg = self.expr(cx, &arg.value, Some(key));
            let arg = self.coerce(arg, key);
            return Self::mk(hir::ExprKind::MapOp(op, vec![value, arg]), result, span);
        }
        let TyKind::Array(element) = *self.types.kind(value.ty) else {
            if value.ty == Ty::ERROR {
                return Self::error_expr(span);
            }
            return self.no_method(value.ty, method, span);
        };
        let array = value.ty;
        let (op, params, result) = match method.name.as_str() {
            "length" => (hir::ArrayOp::Length, vec![], Ty::INT),
            "get" => (hir::ArrayOp::Get, vec![Ty::INT], self.types.intern(TyKind::Option(element))),
            "slice" => {
                let result = self.types.intern(TyKind::Result(array, self.collection_error));
                (hir::ArrayOp::SliceChecked, vec![Ty::INT, Ty::INT], result)
            }
            "concat" => (hir::ArrayOp::Concat, vec![array], array),
            "repeat" => (hir::ArrayOp::Repeat, vec![Ty::INT], array),
            _ => return self.no_method(array, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![value];
        for (arg, ty) in args.iter().zip(params) {
            let arg = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(arg, ty));
        }
        Self::mk(hir::ExprKind::ArrayOp(op, values), result, span)
    }

    fn no_method(&mut self, ty: Ty, method: &ast::Ident, span: Span) -> hir::Expr {
        let message = format!("{} has no method `{}`", self.display(ty), method.name);
        self.diags.error("E1001", method.span, message);
        Self::error_expr(span)
    }

    fn unsupported_expr(&mut self, span: Span, what: &str) -> hir::Expr {
        self.diags.unsupported(span, what);
        Self::error_expr(span)
    }

    fn int_literal(&mut self, value: i128, suffix: Option<&str>, expected: Option<Ty>, span: Span) -> hir::Expr {
        let int = match suffix {
            Some(suffix) => IntTy::from_suffix(suffix),
            None => expected.and_then(|t| self.types.as_int(t)).unwrap_or(IntTy::I64),
        };
        if value < int.min() || value > int.max() {
            let message = format!("literal `{value}` does not fit in `{}`", int.name());
            self.diags.error("E1102", span, message);
        }
        Self::mk(hir::ExprKind::Int(value), Ty::int(int), span)
    }

    fn float_literal(
        &mut self,
        text: &str,
        suffix: Option<&str>,
        negative: bool,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = match suffix {
            Some("f32") => Ty::F32,
            Some(_) => Ty::F64,
            None if expected == Some(Ty::F32) => Ty::F32,
            None => Ty::F64,
        };
        // Parse in the target format so rounding happens once (5.8).
        let value = if ty == Ty::F32 { text.parse::<f32>().map(f64::from) } else { text.parse::<f64>() };
        let value = value.unwrap_or(f64::INFINITY);
        if value.is_infinite() {
            let message = format!("literal `{text}` does not fit in {}", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        Self::mk(hir::ExprKind::Float(if negative { -value } else { value }), ty, span)
    }

    fn string(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], span: Span) -> hir::Expr {
        if let [ast::StrSegment::Text(text)] = segments {
            return Self::mk(hir::ExprKind::Str(text.clone()), Ty::STRING, span);
        }
        let mut out = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(text) => out.push(hir::Segment::Text(text.clone())),
                ast::StrSegment::Expr(e) => {
                    let value = self.expr(cx, e, None);
                    if !self.displayable(value.ty) {
                        let message = format!("{} does not implement `Display`", self.display(value.ty));
                        self.diags.error("E1105", value.span, message);
                    }
                    out.push(hir::Segment::Value(value));
                }
            }
        }
        Self::mk(hir::ExprKind::Interpolate(out), Ty::STRING, span)
    }

    fn path(&mut self, cx: &mut FnCx, segments: &[ast::Ident], span: Span) -> hir::Expr {
        let first = &segments[0];
        if let Some(local) = self.lookup(cx, &first.name) {
            let ty = cx.locals[local.0 as usize].ty;
            let mut value = Self::mk(hir::ExprKind::Local(local), ty, first.span);
            for segment in &segments[1..] {
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        if let Some(resolved) = self.variant_path(segments) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            if let Payload::Unit = self.payload(ty, variant) {
                return Self::mk(hir::ExprKind::Variant(variant, Vec::new()), ty, span);
            }
            let message = format!("`{}` needs a payload", Self::path_text(segments));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        match self.names.get(&first.name) {
            Some(ModuleName::Fn(_)) => self.unsupported_expr(span, "function values"),
            Some(ModuleName::Newtype(_)) => {
                let message = format!("`{}` constructs a value: write `{}(value)`", first.name, first.name);
                self.diags.error("E1102", span, message);
                Self::error_expr(span)
            }
            Some(ModuleName::Module(_)) => {
                let message = format!(
                    "`{}` is a module, not a value",
                    segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
                );
                self.diags.error("E1102", span, message);
                Self::error_expr(span)
            }
            None => {
                self.unknown_name(&first.name, first.span);
                Self::error_expr(span)
            }
        }
    }

    /// Reports `E1001`, pointing habits from other languages at the canonical form.
    fn unknown_name(&mut self, name: &str, span: Span) {
        let message = match name {
            "Some" | "Ok" | "Err" => format!("unknown name `{name}`; Tondo writes `{}(...)`", name.to_lowercase()),
            "None" => "unknown name `None`; Tondo writes `none`".to_string(),
            _ => format!("unknown name `{name}`"),
        };
        self.diags.error("E1001", span, message);
    }

    fn call(&mut self, cx: &mut FnCx, callee: &ast::Expr, args: &[ast::Arg], span: Span) -> hir::Expr {
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            return self.unsupported_expr(arg.span, "named arguments");
        }
        match &callee.kind {
            ast::ExprKind::Field(receiver, method) => return self.method_call(cx, receiver, method, args, span),
            ast::ExprKind::Path(segments) if segments.len() > 1 && self.lookup(cx, &segments[0].name).is_some() => {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, span);
            }
            ast::ExprKind::Generic { base, args: type_args } => {
                return self.generic_call(cx, base, type_args, args, span);
            }
            _ => {}
        }
        let ast::ExprKind::Path(segments) = &callee.kind else {
            return self.unsupported_expr(callee.span, "calling this expression");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if self.lookup(cx, names[0]).is_some() {
            return self.unsupported_expr(callee.span, "calls through values");
        }
        if let ["Array", "new" | "withCapacity"] = names.as_slice() {
            let message = format!("`Array.{}` needs its element type: `Array.{}[T](...)`", names[1], names[1]);
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        if let Some(resolved) = self.variant_path(segments) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            let Payload::Tuple(types) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take positional values", Self::path_text(segments));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            if types.len() != args.len() {
                return self.arity_error(span, types.len(), args.len());
            }
            let values = args
                .iter()
                .zip(types)
                .enumerate()
                .map(|(index, (arg, ty))| {
                    let value = self.expr(cx, &arg.value, Some(ty));
                    (index as u32, self.coerce(value, ty))
                })
                .collect();
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        match (names.as_slice(), self.names.get(names[0]).copied()) {
            (["panic"], _) => {
                let [message] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let message = self.expr(cx, &message.value, Some(Ty::STRING));
                let message = self.coerce(message, Ty::STRING);
                Self::mk(hir::ExprKind::Panic(Box::new(message)), Ty::NEVER, span)
            }
            (["assert"], _) => {
                let Some((condition, parts)) = args.split_first() else {
                    return self.arity_error(span, 1, 0);
                };
                let source = self.sources.slice(condition.value.span).to_string();
                let condition = self.expr(cx, &condition.value, Some(Ty::BOOL));
                let condition = Box::new(self.coerce(condition, Ty::BOOL));
                let parts = parts
                    .iter()
                    .map(|p| {
                        let part = self.expr(cx, &p.value, Some(Ty::STRING));
                        self.coerce(part, Ty::STRING)
                    })
                    .collect();
                Self::mk(hir::ExprKind::Assert { condition, parts, source }, Ty::UNIT, span)
            }
            ([_], Some(ModuleName::Newtype(ty))) => {
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                let value = self.expr(cx, &arg.value, Some(inner));
                let value = self.coerce(value, inner);
                Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span)
            }
            ([_], Some(ModuleName::Fn(id))) => {
                let params = self.signatures[id.0 as usize].params.clone();
                let result = self.signatures[id.0 as usize].result;
                self.call_with(cx, Callee::Fn(id), &params, result, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Console))) => {
                let function = match *member {
                    "print" => StdFn::ConsolePrint,
                    "println" => StdFn::ConsolePrintln,
                    "stdin" | "stdout" | "stderr" | "readLine" | "flush" => {
                        let what = format!("`console.{member}`");
                        return self.unsupported_expr(span, &what);
                    }
                    _ => {
                        let message = format!("module `console` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let error = self.console_error.unwrap();
                let result = self.types.intern(TyKind::Result(Ty::UNIT, error));
                self.call_with(cx, Callee::Std(function), &[Ty::STRING], result, args, span)
            }
            ([name], None) if Self::numeric_type(name).is_some() => {
                let to = Self::numeric_type(name).unwrap();
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let value = self.expr(cx, &arg.value, None);
                match self.conversion_checked(value.ty, to) {
                    _ if value.ty == Ty::ERROR => Self::error_expr(span),
                    Some(false) => Self::mk(hir::ExprKind::Convert(Box::new(value)), to, span),
                    Some(true) => {
                        let ty = self.types.intern(TyKind::Result(to, self.conversion_error));
                        Self::mk(hir::ExprKind::Convert(Box::new(value)), ty, span)
                    }
                    None => {
                        let message = format!("there is no conversion from {} to `{name}`", self.display(value.ty));
                        self.diags.error("E1103", span, message);
                        Self::error_expr(span)
                    }
                }
            }
            (_, None) => {
                self.unknown_name(names[0], segments[0].span);
                Self::error_expr(span)
            }
            _ => self.unsupported_expr(callee.span, "calling this expression"),
        }
    }

    fn arity_error(&mut self, span: Span, expected: usize, found: usize) -> hir::Expr {
        let message = format!("expected {expected} argument(s), found {found}");
        self.diags.error("E1102", span, message);
        Self::error_expr(span)
    }

    fn call_with(
        &mut self,
        cx: &mut FnCx,
        callee: Callee,
        params: &[Ty],
        result: Ty,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if params.len() != args.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let args = args
            .iter()
            .zip(params)
            .map(|(arg, &ty)| {
                let value = self.expr(cx, &arg.value, Some(ty));
                self.coerce(value, ty)
            })
            .collect();
        Self::mk(hir::ExprKind::Call(callee, args), result, span)
    }

    fn unary(
        &mut self,
        cx: &mut FnCx,
        op: UnaryOp,
        operand: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        match op {
            UnaryOp::Neg => {
                if let ast::ExprKind::Int { value, suffix } = operand.kind {
                    let literal = self.int_literal(-(value as i128), suffix, expected, span);
                    if self.types.as_int(literal.ty).is_some_and(|i| !i.signed()) {
                        let message = format!("negation is not defined for {}", self.display(literal.ty));
                        self.diags.error("E1102", span, message);
                    }
                    return literal;
                }
                if let ast::ExprKind::Float { text, suffix } = &operand.kind {
                    return self.float_literal(text, *suffix, true, expected, span);
                }
                let value = self.expr(cx, operand, expected);
                let negatable = match self.types.kind(value.ty) {
                    TyKind::Int(int) => int.signed(),
                    TyKind::Float(_) | TyKind::Error | TyKind::Never => true,
                    _ => false,
                };
                if !negatable {
                    let message = format!("negation is not defined for {}", self.display(value.ty));
                    self.diags.error("E1102", span, message);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
            UnaryOp::Not => {
                let value = self.expr(cx, operand, Some(Ty::BOOL));
                let value = self.coerce(value, Ty::BOOL);
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), Ty::BOOL, span)
            }
            UnaryOp::BitNot => {
                let value = self.expr(cx, operand, expected);
                if value.ty != Ty::BYTE {
                    self.require_int(&value);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
        }
    }

    fn is_untyped_literal(e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Int { suffix: None, .. } | ast::ExprKind::Float { suffix: None, .. } => true,
            ast::ExprKind::Unary(UnaryOp::Neg, inner) => Self::is_untyped_literal(inner),
            _ => false,
        }
    }

    /// Types two operands that must share a type. An unsuffixed literal on
    /// the left adopts the right operand's type; evaluation order is unchanged.
    fn operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
    ) -> (hir::Expr, hir::Expr) {
        if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let left = self.expr(cx, left, Some(right.ty));
            let left = self.coerce(left, right.ty);
            (left, right)
        } else {
            let left = self.expr(cx, left, expected);
            let right = self.expr(cx, right, Some(left.ty));
            let right = if left.ty == Ty::NEVER { right } else { self.coerce(right, left.ty) };
            (left, right)
        }
    }

    fn binary(
        &mut self,
        cx: &mut FnCx,
        op: BinOp,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let operator_error = |this: &mut Self, ty: Ty| {
            let message = format!("operator `{}` cannot be applied to {}", op.as_str(), this.display(ty));
            this.diags.error("E1102", span, message);
        };
        let (left, right, ty) = match op {
            BinOp::And | BinOp::Or => {
                let left = self.expr(cx, left, Some(Ty::BOOL));
                let left = self.coerce(left, Ty::BOOL);
                let right = self.expr(cx, right, Some(Ty::BOOL));
                let right = self.coerce(right, Ty::BOOL);
                (left, right, Ty::BOOL)
            }
            BinOp::Shl | BinOp::Shr => {
                let left = self.expr(cx, left, expected);
                if left.ty != Ty::BYTE {
                    self.require_int(&left);
                }
                let right = self.expr(cx, right, None);
                self.require_int(&right);
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let (left, right) = self.operands(cx, left, right, expected);
                // Floats have no `%` operator; their remainder is a named operation (17.4).
                let allowed = match self.types.kind(left.ty) {
                    TyKind::Int(_) | TyKind::Error | TyKind::Never => true,
                    TyKind::Float(_) => op != BinOp::Rem,
                    _ => false,
                };
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected);
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.operands(cx, left, right, None);
                if !self.types.is_equatable(left.ty) {
                    operator_error(self, left.ty);
                }
                (left, right, Ty::BOOL)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.operands(cx, left, right, None);
                let ordered = matches!(
                    self.types.kind(left.ty),
                    TyKind::Int(_)
                        | TyKind::Float(_)
                        | TyKind::Char
                        | TyKind::Byte
                        | TyKind::String
                        | TyKind::Error
                        | TyKind::Never
                );
                if !ordered {
                    operator_error(self, left.ty);
                }
                (left, right, Ty::BOOL)
            }
            BinOp::In => {
                let container = self.expr(cx, right, None);
                if let TyKind::Map(element, _) | TyKind::Set(element) = *self.types.kind(container.ty) {
                    let item = self.expr(cx, left, Some(element));
                    let item = self.coerce(item, element);
                    let op = hir::MapOp::Contains;
                    return Self::mk(hir::ExprKind::MapOp(op, vec![container, item]), Ty::BOOL, span);
                }
                let TyKind::Array(element) = *self.types.kind(container.ty) else {
                    if container.ty != Ty::ERROR {
                        let message = format!("`in` is not defined for {}", self.display(container.ty));
                        self.diags.error("E1102", container.span, message);
                    }
                    return Self::error_expr(span);
                };
                if !self.types.is_equatable(element) {
                    operator_error(self, element);
                }
                // Membership observes the element first (5.12 orders operands
                // textually; the left operand is the element).
                let item = self.expr(cx, left, Some(element));
                let item = self.coerce(item, element);
                return Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Contains, vec![item, container]), Ty::BOOL, span);
            }
            BinOp::Range | BinOp::RangeInclusive => {
                return self.unsupported_expr(span, &format!("`{}` outside a `for` header", op.as_str()));
            }
        };
        Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span)
    }

    fn if_expr(
        &mut self,
        cx: &mut FnCx,
        condition: &ast::Expr,
        then: &ast::Block,
        otherwise: Option<&ast::Expr>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let condition = self.expr(cx, condition, Some(Ty::BOOL));
        let condition = Box::new(self.coerce(condition, Ty::BOOL));
        let Some(otherwise) = otherwise else {
            let then = self.block(cx, then, Some(Ty::UNIT));
            return Self::mk(hir::ExprKind::If(condition, then, None), Ty::UNIT, span);
        };
        let then = self.block(cx, then, expected);
        let other_expected = expected.or((then.ty != Ty::NEVER).then_some(then.ty));
        let other = self.expr(cx, otherwise, other_expected);
        let ty = if then.ty == Ty::NEVER {
            other.ty
        } else if other.ty == Ty::NEVER {
            then.ty
        } else {
            if expected.is_none() && other.ty != then.ty && ![then.ty, other.ty].contains(&Ty::ERROR) {
                let message = format!(
                    "`if` branches have different types: {} and {}",
                    self.display(then.ty),
                    self.display(other.ty)
                );
                self.diags.error("E1102", other.span, message);
            }
            then.ty
        };
        Self::mk(hir::ExprKind::If(condition, then, Some(Box::new(other))), ty, span)
    }
}
