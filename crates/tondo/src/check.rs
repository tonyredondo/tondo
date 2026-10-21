//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn};
use crate::parser::CONTEXTUAL_CONSTRUCTORS;
use crate::source::{SourceMap, Span};
use crate::types::{IntTy, StdType, Ty, TyKind, Types};
use std::collections::HashMap;

pub fn check(module: &ast::Module, sources: &SourceMap, diags: &mut Diagnostics) -> Option<hir::Program> {
    let mut checker =
        Checker { types: Types::default(), diags, sources, names: HashMap::new(), signatures: Vec::new() };
    checker.program(module)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum StdModule {
    Console,
}

#[derive(Copy, Clone)]
enum ModuleName {
    Fn(FnId),
    Module(StdModule),
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

        let fns: Vec<&ast::FnDecl> = module.items.iter().map(|ast::Item::Fn(f)| f).collect();
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
            ast::TypeKind::Tuple(_) => self.unsupported_type(t.span, "tuple types"),
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
            ast::StmtKind::Let { mutable, name, ty, value } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let Some(value) = value else {
                    let message = format!("`{}` must be initialized where it is declared", name.name);
                    self.diags.error("E1109", stmt.span, message);
                    return None;
                };
                let value = self.expr(cx, value, declared);
                let value = match declared {
                    Some(declared) => self.coerce(value, declared),
                    None => value,
                };
                cx.diverged |= value.ty == Ty::NEVER;
                if name.name == "_" {
                    return Some(hir::Stmt::Expr(value));
                }
                let ty = declared.unwrap_or(value.ty);
                let local = self.declare(cx, name, ty, *mutable);
                Some(hir::Stmt::Let(local, value))
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
        let ast::ExprKind::Path(segments) = &target.kind else {
            self.diags.unsupported(target.span, "this assignment target");
            return None;
        };
        if segments.len() != 1 {
            self.diags.unsupported(target.span, "field assignment");
            return None;
        }
        let name = &segments[0];
        let Some(local) = self.lookup(cx, &name.name) else {
            let message = if self.names.contains_key(&name.name) {
                format!("`{}` is not a mutable binding", name.name)
            } else {
                format!("unknown name `{}`", name.name)
            };
            let code = if self.names.contains_key(&name.name) { "E1411" } else { "E1001" };
            self.diags.error(code, name.span, message);
            return None;
        };
        if !cx.mutable[local.0 as usize] {
            let message = format!("cannot assign to `{}`: it is not declared with `var`", name.name);
            self.diags.error("E1411", name.span, message);
        }
        let ty = cx.locals[local.0 as usize].ty;
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
        Some(hir::Stmt::Assign { local, op, value, span })
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
            ast::ExprKind::Field(..) => self.unsupported_expr(span, "field access"),
            ast::ExprKind::Tuple(_) => self.unsupported_expr(span, "tuples"),
            ast::ExprKind::Try(_) => self.unsupported_expr(span, "`?` propagation"),
            ast::ExprKind::Error => Self::error_expr(span),
        }
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
            if segments.len() > 1 {
                return self.unsupported_expr(span, "field access");
            }
            let ty = cx.locals[local.0 as usize].ty;
            return Self::mk(hir::ExprKind::Local(local), ty, span);
        }
        match self.names.get(&first.name) {
            Some(ModuleName::Fn(_)) => self.unsupported_expr(span, "function values"),
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
                let comparable = matches!(
                    self.types.kind(left.ty),
                    TyKind::Int(_) | TyKind::Bool | TyKind::String | TyKind::Unit | TyKind::Error | TyKind::Never
                );
                if !comparable {
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
