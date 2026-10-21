//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn};
use crate::parser::CONTEXTUAL_CONSTRUCTORS;
use crate::source::{SourceMap, Span};
use crate::types::{AdtId, AdtKind, Field, IntTy, Payload, StdType, Ty, TyKind, Types, Variant};
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
    };
    checker.program(module)
}

/// Prelude type names (26.3); they cannot be redeclared unqualified.
const PRELUDE_TYPES: [&str; 37] = [
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
}

/// Per-function checking state.
struct FnCx {
    locals: Vec<hir::Local>,
    mutable: Vec<bool>,
    scopes: Vec<Vec<(String, LocalId)>>,
    /// Success type that `return` produces.
    result: Ty,
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
        if matches!(self.types.kind(result), TyKind::Result(..)) {
            self.diags.unsupported(f.result.as_ref().unwrap().span, "fallible functions");
        }
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
                    self.diags.unsupported(t.span, "generic types");
                    return Ty::ERROR;
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
                        self.types.intern(TyKind::Std(StdType::ConsoleError))
                    }
                    [single] if ["Float", "Float32", "Float64", "Char", "Byte"].contains(single) => {
                        let what = format!("type `{single}`");
                        self.diags.unsupported(t.span, &what);
                        Ty::ERROR
                    }
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
            ast::TypeKind::Optional(_) => self.unsupported_type(t.span, "optional types"),
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

    fn new_cx(result: Ty) -> FnCx {
        FnCx {
            locals: Vec::new(),
            mutable: Vec::new(),
            scopes: vec![Vec::new()],
            result,
            loops: Vec::new(),
            diverged: false,
        }
    }

    fn function(&mut self, f: &ast::FnDecl, id: FnId) -> hir::Function {
        let result = self.signatures[id.0 as usize].result;
        let mut cx = Self::new_cx(result);
        let param_types = self.signatures[id.0 as usize].params.clone();
        let params =
            f.params.iter().zip(param_types).map(|(p, ty)| self.declare(&mut cx, &p.name, ty, false)).collect();
        let body = self.block(&mut cx, &f.body, Some(result));
        hir::Function { name: f.name.name.clone(), params, locals: cx.locals, result, body }
    }

    /// The implicit `main` of a root script (spec 20.3).
    fn script_main(&mut self, statements: &[ast::Stmt]) -> hir::Function {
        let mut cx = Self::new_cx(Ty::UNIT);
        let stmts = self.statements(&mut cx, statements);
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        let body = hir::Block { stmts, tail: None, ty };
        hir::Function { name: "main".into(), params: Vec::new(), locals: cx.locals, result: Ty::UNIT, body }
    }

    fn lookup(&self, cx: &FnCx, name: &str) -> Option<LocalId> {
        cx.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name).map(|&(_, id)| id)
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
            ast::StmtKind::Fail(_) => {
                self.diags.unsupported(stmt.span, "`fail`");
                None
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
    fn place(&mut self, cx: &FnCx, target: &ast::Expr) -> Option<(LocalId, Vec<u32>, Ty)> {
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
                    path.extend(index);
                    ty = field_ty;
                }
                return Some((local, path, ty));
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
        path.extend(index);
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
            ast::ForHeader::In { binding, source } => {
                let ast::ExprKind::Binary(op @ (BinOp::Range | BinOp::RangeInclusive), start, end) = &source.kind
                else {
                    self.diags.unsupported(source.span, "iteration over values other than integer ranges");
                    return None;
                };
                let (start, end) = self.operands(cx, start, end, None);
                self.require_int(&start);
                cx.scopes.push(Vec::new());
                let local = self.declare(cx, binding, start.ty, false);
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

    /// Reports a mismatch unless `value` already has (or diverges into) `expected`.
    fn coerce(&mut self, value: hir::Expr, expected: Ty) -> hir::Expr {
        if value.ty != expected && ![Ty::NEVER, Ty::ERROR].contains(&value.ty) && expected != Ty::ERROR {
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
            ast::ExprKind::Float { .. } => self.unsupported_expr(span, "floating-point values"),
            ast::ExprKind::Char(_) => self.unsupported_expr(span, "`Char` values"),
            ast::ExprKind::Field(base, name) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Name(&name.name), name.span, span)
            }
            ast::ExprKind::TupleSlot(base, slot) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Slot(*slot), span, span)
            }
            ast::ExprKind::Tuple(items) => self.tuple(cx, items, expected, span),
            ast::ExprKind::Match { scrutinee, arms } => self.match_expr(cx, scrutinee, arms, expected, span),
            ast::ExprKind::Record { path, fields } => self.record_literal(cx, path, fields, span),
            ast::ExprKind::With { base, updates } => self.with_expr(cx, base, updates, span),
            ast::ExprKind::Try(_) => self.unsupported_expr(span, "`?` propagation"),
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
        let [type_name, variant] = path else { return None };
        let ty = match self.type_names.get(&type_name.name)? {
            TypeName::Nominal(ty) => *ty,
            TypeName::Alias(index) => {
                let index = *index;
                self.resolve_alias(index)
            }
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
            ast::PatternKind::None => return self.unsupported_pattern(span, "`none` patterns"),
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
                    return self.unsupported_pattern(span, "`some`, `ok` and `err` patterns");
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
                    let displayable = matches!(
                        self.types.kind(value.ty),
                        TyKind::Int(_) | TyKind::Bool | TyKind::String | TyKind::Error | TyKind::Never
                    );
                    if !displayable {
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
        let ast::ExprKind::Path(segments) = &callee.kind else {
            return self.unsupported_expr(callee.span, "calling this expression");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if self.lookup(cx, names[0]).is_some() {
            return self.unsupported_expr(callee.span, "method calls and calls through values");
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
                let error = self.types.intern(TyKind::Std(StdType::ConsoleError));
                let result = self.types.intern(TyKind::Result(Ty::UNIT, error));
                self.call_with(cx, Callee::Std(function), &[Ty::STRING], result, args, span)
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
                let value = self.expr(cx, operand, expected);
                if self.types.as_int(value.ty).is_some_and(|i| !i.signed()) {
                    let message = format!("negation is not defined for {}", self.display(value.ty));
                    self.diags.error("E1102", span, message);
                } else {
                    self.require_int(&value);
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
                self.require_int(&value);
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
        }
    }

    fn is_untyped_literal(e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Int { suffix: None, .. } => true,
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
                self.require_int(&left);
                let right = self.expr(cx, right, None);
                self.require_int(&right);
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected);
                if self.types.as_int(left.ty).is_none() && ![Ty::ERROR, Ty::NEVER].contains(&left.ty) {
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
                let ordered =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::String | TyKind::Error | TyKind::Never);
                if !ordered {
                    operator_error(self, left.ty);
                }
                (left, right, Ty::BOOL)
            }
            BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
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
