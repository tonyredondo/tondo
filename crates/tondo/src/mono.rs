//! Monomorphization: one copy of each function per combination of type
//! arguments reachable from `main`, with every type parameter substituted.
//! Generic code is checked once (12.1); lowering and code generation only
//! see concrete types.

use crate::diag::Diagnostics;
use crate::hir::{Block, Callee, ExprKind, FnId, Function, PatKind, Pattern, Place, PlaceStep, Program, Segment, Stmt};
use crate::types::{Ty, Types};
use std::collections::{HashMap, VecDeque};

/// Instantiation depth beyond which a generic function is considered to
/// call itself with ever larger type arguments.
const MAX_DEPTH: usize = 32;

pub fn monomorphize(program: Program, diags: &mut Diagnostics) -> Option<Program> {
    let Program { mut types, functions, main } = program;
    let mut mono = Mono {
        types: &mut types,
        functions: &functions,
        instances: HashMap::new(),
        queue: VecDeque::new(),
        out: Vec::new(),
        runaway: None,
    };
    let main = mono.instance(main, Vec::new());
    while let Some((id, args, instance)) = mono.queue.pop_front() {
        let generic = &functions[id.0 as usize];
        let map: HashMap<Ty, Ty> = generic.generics.iter().copied().zip(args).collect();
        let mut function = generic.clone();
        mono.function(&mut function, &map);
        mono.out[instance.0 as usize] = Some(function);
    }
    if let Some(name) = mono.runaway.take().or_else(|| mono.types.runaway.take()) {
        let message = format!("`{name}` is instantiated with ever larger type arguments and never terminates");
        diags.error_without_span("E1112", message);
        return None;
    }
    let functions = mono.out.into_iter().map(|f| f.expect("every instance is generated")).collect();
    Some(Program { types, functions, main })
}

struct Mono<'a> {
    types: &'a mut Types,
    functions: &'a [Function],
    instances: HashMap<(FnId, Vec<Ty>), FnId>,
    queue: VecDeque<(FnId, Vec<Ty>, FnId)>,
    out: Vec<Option<Function>>,
    runaway: Option<String>,
}

impl Mono<'_> {
    /// The concrete function for `id` applied to `args`, queued on first use.
    fn instance(&mut self, id: FnId, args: Vec<Ty>) -> FnId {
        if let Some(&instance) = self.instances.get(&(id, args.clone())) {
            return instance;
        }
        let instance = FnId(self.out.len() as u32);
        if args.iter().any(|&a| self.types.depth(a) > MAX_DEPTH) {
            self.runaway.get_or_insert_with(|| self.functions[id.0 as usize].name.clone());
            return instance;
        }
        let name = &self.functions[id.0 as usize].name;
        let mut function = Function {
            name: name.clone(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            locals: Vec::new(),
            result: Ty::UNIT,
            body: Block { stmts: Vec::new(), tail: None, ty: Ty::UNIT },
        };
        if !args.is_empty() {
            let shown: Vec<String> = args.iter().map(|&a| self.types.display(a)).collect();
            function.name = format!("{name}[{}]", shown.join(", "));
        }
        self.out.push(None);
        self.instances.insert((id, args.clone()), instance);
        self.queue.push_back((id, args, instance));
        instance
    }

    fn ty(&mut self, ty: Ty, map: &HashMap<Ty, Ty>) -> Ty {
        self.types.subst(ty, map)
    }

    fn function(&mut self, f: &mut Function, map: &HashMap<Ty, Ty>) {
        if !f.generics.is_empty() {
            let shown: Vec<String> = f.generics.iter().map(|g| self.types.display(map[g])).collect();
            f.name = format!("{}[{}]", f.name, shown.join(", "));
        }
        f.generics.clear();
        for local in &mut f.locals {
            local.ty = self.ty(local.ty, map);
        }
        f.result = self.ty(f.result, map);
        self.block(&mut f.body, map);
    }

    fn block(&mut self, block: &mut Block, map: &HashMap<Ty, Ty>) {
        block.ty = self.ty(block.ty, map);
        for stmt in &mut block.stmts {
            self.stmt(stmt, map);
        }
        if let Some(tail) = &mut block.tail {
            self.expr(tail, map);
        }
    }

    fn stmt(&mut self, stmt: &mut Stmt, map: &HashMap<Ty, Ty>) {
        match stmt {
            Stmt::Let(_, value) | Stmt::Expr(value) | Stmt::Fail(value) | Stmt::Return(Some(value)) => {
                self.expr(value, map)
            }
            Stmt::Destructure(pattern, value) => {
                self.pattern(pattern, map);
                self.expr(value, map);
            }
            Stmt::Assign { path, value, .. } => {
                self.steps(path, map);
                self.expr(value, map);
            }
            Stmt::AssignSlice { path, bounds, value, .. } => {
                self.steps(path, map);
                for bound in bounds.iter_mut().flatten() {
                    self.expr(bound, map);
                }
                self.expr(value, map);
            }
            Stmt::Loop { condition, body } => {
                if let Some(condition) = condition {
                    self.expr(condition, map);
                }
                self.block(body, map);
            }
            Stmt::ForEach { pattern, source, body } => {
                self.pattern(pattern, map);
                self.expr(source, map);
                self.block(body, map);
            }
            Stmt::ForRange { start, end, body, .. } => {
                self.expr(start, map);
                self.expr(end, map);
                self.block(body, map);
            }
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }

    fn steps(&mut self, steps: &mut [PlaceStep], map: &HashMap<Ty, Ty>) {
        for step in steps {
            if let PlaceStep::Index(index) = step {
                self.expr(index, map);
            }
        }
    }

    fn place(&mut self, place: &mut Place, map: &HashMap<Ty, Ty>) {
        self.steps(&mut place.path, map);
    }

    fn exprs(&mut self, exprs: &mut [crate::hir::Expr], map: &HashMap<Ty, Ty>) {
        for e in exprs {
            self.expr(e, map);
        }
    }

    fn expr(&mut self, e: &mut crate::hir::Expr, map: &HashMap<Ty, Ty>) {
        e.ty = self.ty(e.ty, map);
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::Error => {}
            ExprKind::Aggregate(items)
            | ExprKind::Array(items)
            | ExprKind::Set(items)
            | ExprKind::ArrayOp(_, items)
            | ExprKind::StrOp(_, items)
            | ExprKind::MapOp(_, items) => self.exprs(items, map),
            ExprKind::Record(fields) | ExprKind::Variant(_, fields) => {
                for (_, value) in fields {
                    self.expr(value, map);
                }
            }
            ExprKind::With(base, updates) => {
                self.expr(base, map);
                for (_, value) in updates {
                    self.expr(value, map);
                }
            }
            ExprKind::Field(inner, _)
            | ExprKind::Retype(inner)
            | ExprKind::Length(inner)
            | ExprKind::Try(inner)
            | ExprKind::Convert(inner)
            | ExprKind::Unary(_, inner)
            | ExprKind::Panic(inner) => self.expr(inner, map),
            ExprKind::Match(scrutinee, arms) => {
                self.expr(scrutinee, map);
                for arm in arms {
                    self.pattern(&mut arm.pattern, map);
                    if let Some(guard) = &mut arm.guard {
                        self.expr(guard, map);
                    }
                    self.expr(&mut arm.body, map);
                }
            }
            ExprKind::Index(base, index) | ExprKind::Binary(_, base, index) => {
                self.expr(base, map);
                self.expr(index, map);
            }
            ExprKind::Slice(base, parts) => {
                self.expr(base, map);
                for part in parts.iter_mut().flatten() {
                    self.expr(part, map);
                }
            }
            ExprKind::Loan(place) => self.place(place, map),
            ExprKind::Mutate(_, place, values) => {
                self.place(place, map);
                self.exprs(values, map);
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.expr(key, map);
                    self.expr(value, map);
                }
            }
            ExprKind::Call(callee, type_args, args) => {
                self.exprs(args, map);
                if let Callee::Fn(id) = callee {
                    let args: Vec<Ty> = type_args.iter().map(|&t| self.types.subst(t, map)).collect();
                    *callee = Callee::Fn(self.instance(*id, args));
                    type_args.clear();
                }
            }
            ExprKind::If(condition, then, otherwise) => {
                self.expr(condition, map);
                self.block(then, map);
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise, map);
                }
            }
            ExprKind::Block(block) => self.block(block, map),
            ExprKind::Interpolate(segments) => {
                for segment in segments {
                    if let Segment::Value(value) = segment {
                        self.expr(value, map);
                    }
                }
            }
            ExprKind::Assert { condition, parts, .. } => {
                self.expr(condition, map);
                self.exprs(parts, map);
            }
        }
    }

    fn pattern(&mut self, pattern: &mut Pattern, map: &HashMap<Ty, Ty>) {
        pattern.ty = self.ty(pattern.ty, map);
        match &mut pattern.kind {
            PatKind::Wildcard | PatKind::Binding(_) | PatKind::Literal(_) => {}
            PatKind::Fields(items) | PatKind::Variant(_, items) | PatKind::Array { items, .. } => {
                for item in items {
                    self.pattern(item, map);
                }
            }
            PatKind::Newtype(inner) => self.pattern(inner, map),
        }
    }
}
