//! Monomorphization: one copy of each function per combination of type
//! arguments reachable from `main`, with every type parameter substituted.
//! Generic code is checked once (12.1); lowering and code generation only
//! see concrete types.

use crate::diag::Diagnostics;
use crate::hir::{
    Block, Callee, Expr, ExprKind, FnId, Function, ImplInfo, PatKind, Pattern, Place, PlaceStep, Program, Segment,
    Stmt, TraitId, TraitInfo,
};
use crate::types::{CursorKind, Ty, TyKind, Types};
use std::collections::{HashMap, VecDeque};

/// Instantiation depth beyond which a generic function is considered to
/// call itself with ever larger type arguments.
const MAX_DEPTH: usize = 32;

pub fn monomorphize(program: Program, diags: &mut Diagnostics) -> Option<Program> {
    monomorphize_with(program, &[], diags)
}

/// The same, with more functions that are not generic built as if called:
/// a test build's own, which coverage counts even when nothing calls them.
pub fn monomorphize_with(program: Program, roots: &[FnId], diags: &mut Diagnostics) -> Option<Program> {
    let Program { mut types, functions, main, traits, impls, externs, mut exports, c_api, tests, .. } = program;
    let mut mono = Mono {
        types: &mut types,
        functions: &functions,
        traits: &traits,
        impls: &impls,
        instances: HashMap::new(),
        queue: VecDeque::new(),
        out: Vec::new(),
        display: HashMap::new(),
        runaway: None,
        conversions: Vec::new(),
    };
    // A failing `main` shows its error with `Display` (20.10).
    if let Some(main) = main
        && let TyKind::Result(_, error) = *mono.types.kind(functions[main.0 as usize].result)
    {
        mono.collect_display(error);
    }
    let main = main.map(|main| mono.instance(main, Vec::new()));
    // What C calls is reached from C.
    for export in &mut exports {
        export.function = mono.instance(export.function, Vec::new());
    }
    for &root in roots {
        mono.instance(root, Vec::new());
    }
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
    // `reflect.convert` between the types it is instantiated with (27.10).
    for (span, from, to) in std::mem::take(&mut mono.conversions) {
        let mut missing = Vec::new();
        match mono.types.converts(from, to, &mut missing) {
            Ok(()) => {
                for field in missing {
                    let message =
                        format!("`{field}` is filled with `none`: `{}` has no such field", mono.types.display(from));
                    diags.warning("W1012", span, message);
                }
            }
            Err(why) => diags.error("E2110", span, format!("`reflect.convert` cannot convert here: {why}")),
        }
    }
    if diags.has_errors() {
        return None;
    }
    let display = std::mem::take(&mut mono.display);
    let functions = mono.out.into_iter().map(|f| f.expect("every instance is generated")).collect();
    Some(Program { types, functions, main, traits, impls, display, externs, exports, c_api, tests })
}

/// The trait whose method interpolation uses (see the checker).
const DISPLAY: TraitId = TraitId(0);

/// What a trait method of a concrete `Self` is.
enum Resolved {
    Fn(FnId, Vec<Ty>),
    /// The intrinsic `Display` of a built-in value.
    Display,
    /// `next` of an intrinsic cursor.
    Cursor(CursorKind),
}

struct Mono<'a> {
    types: &'a mut Types,
    functions: &'a [Function],
    traits: &'a [TraitInfo],
    impls: &'a [ImplInfo],
    instances: HashMap<(FnId, Vec<Ty>), FnId>,
    /// The user `Display.display` of types displayed inside collections.
    display: HashMap<Ty, FnId>,
    queue: VecDeque<(FnId, Vec<Ty>, FnId)>,
    out: Vec<Option<Function>>,
    runaway: Option<String>,
    /// The calls of `reflect.convert`, from and to which type, checked once
    /// every instance is known.
    conversions: Vec<(crate::source::Span, Ty, Ty)>,
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
            fallible: false,
            locals: Vec::new(),
            result: Ty::UNIT,
            body: Block { stmts: Vec::new(), tail: None, ty: Ty::UNIT },
            moves: Default::default(),
            test: false,
            span: self.functions[id.0 as usize].span,
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
        let ty = self.types.subst(ty, map);
        self.types.reveal(ty)
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
            Stmt::Defer { closure, call, .. } => {
                self.expr(closure, map);
                self.expr(call, map);
            }
            Stmt::Lend { place, .. } => self.place(place, map),
            Stmt::LockSync { handle, .. } => self.expr(handle, map),
            Stmt::AwaitChange(..) => {}
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

    fn exprs(&mut self, exprs: &mut [Expr], map: &HashMap<Ty, Ty>) {
        for e in exprs {
            self.expr(e, map);
        }
    }

    fn expr(&mut self, e: &mut Expr, map: &HashMap<Ty, Ty>) {
        e.ty = self.ty(e.ty, map);
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::ExternFn(_)
            | ExprKind::Error => {}
            ExprKind::Layout(ty, _) => *ty = self.ty(*ty, map),
            ExprKind::Reflect(op, ty, values) => {
                *ty = self.ty(*ty, map);
                self.exprs(values, map);
                if *op == crate::hir::ReflectOp::Describe {
                    self.collect_display(*ty);
                }
            }
            ExprKind::Aggregate(items)
            | ExprKind::Array(items)
            | ExprKind::Set(items)
            | ExprKind::ArrayOp(_, items)
            | ExprKind::StrOp(_, items)
            | ExprKind::Pointer(_, items)
            | ExprKind::TaskOp(_, items)
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
            | ExprKind::RefValue(inner)
            | ExprKind::Fixed(inner)
            | ExprKind::Length(inner)
            | ExprKind::Try(inner)
            | ExprKind::Convert(inner)
            | ExprKind::Unary(_, inner)
            | ExprKind::Spawn(inner)
            | ExprKind::Panic(inner, _) => self.expr(inner, map),
            ExprKind::TestMembers(members) => members.iter_mut().for_each(|(_, member)| self.expr(member, map)),
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
            ExprKind::Select(arms, otherwise) => {
                for arm in arms {
                    self.expr(&mut arm.operation, map);
                    if let Some(pattern) = &mut arm.pattern {
                        self.pattern(pattern, map);
                    }
                    self.expr(&mut arm.body, map);
                }
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise, map);
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
            ExprKind::Loan(place) | ExprKind::CursorNext(place, _) => self.place(place, map),
            ExprKind::Region(place, parts) => {
                self.place(place, map);
                for part in parts.iter_mut().flatten() {
                    self.expr(part, map);
                }
            }
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
                let types: Vec<Ty> = type_args.iter().map(|&t| self.ty(t, map)).collect();
                type_args.clear();
                match *callee {
                    Callee::Fn(id) => {
                        if self.functions[id.0 as usize].name == "reflect.convert" {
                            self.conversions.push((e.span, types[1], types[0]));
                        }
                        *callee = Callee::Fn(self.instance(id, types))
                    }
                    Callee::Trait(trait_id, method) => match self.resolve(trait_id, method, &types) {
                        Resolved::Fn(id, types) => *callee = Callee::Fn(self.instance(id, types)),
                        // A built-in value displays itself.
                        Resolved::Display => {
                            let value = args.pop().expect("`display` has a receiver");
                            self.collect_display(value.ty);
                            e.kind = ExprKind::Interpolate(vec![Segment::Value(value)]);
                        }
                        Resolved::Cursor(kind) => {
                            let Some(Expr { kind: ExprKind::Loan(place), .. }) = args.pop() else {
                                unreachable!("`next` borrows its cursor")
                            };
                            e.kind = ExprKind::CursorNext(place, kind);
                        }
                    },
                    Callee::Std(_) | Callee::Extern(_) => {}
                }
            }
            ExprKind::FnValue(id, type_args) => {
                let types: Vec<Ty> = type_args.iter().map(|&t| self.ty(t, map)).collect();
                type_args.clear();
                *id = self.instance(*id, types);
            }
            ExprKind::CallValue(callee, args) => {
                self.expr(callee, map);
                self.exprs(args, map);
                // A closure is called directly; its body borrows the closure.
                if let Some((body, types, _)) = self.types.closure(callee.ty) {
                    let body = self.instance(FnId(body), types);
                    let callee =
                        std::mem::replace(&mut **callee, Expr { kind: ExprKind::Unit, ty: Ty::UNIT, span: e.span });
                    let (ty, span) = (callee.ty, callee.span);
                    let place = Place::of(callee).expect("the checker calls closures through places");
                    let mut values = vec![Expr { kind: ExprKind::Loan(place), ty, span }];
                    values.append(args);
                    e.kind = ExprKind::Call(Callee::Fn(body), Vec::new(), values);
                }
            }
            ExprKind::Callback(inner, target) => {
                self.expr(inner, map);
                *target = match &inner.kind {
                    ExprKind::FnValue(id, _) => *id,
                    _ => {
                        let (id, types, _) = self.types.closure(inner.ty).expect("a function or a closure");
                        self.instance(FnId(id), types)
                    }
                };
            }
            ExprKind::Erase(closure, body) => {
                self.expr(closure, map);
                let (id, types, _) = self.types.closure(closure.ty).expect("only a closure is erased");
                *body = self.instance(FnId(id), types);
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
                        self.collect_display(value.ty);
                    }
                }
            }
            ExprKind::Assert { condition, parts, detail, .. } => {
                self.expr(condition, map);
                self.exprs(parts, map);
                if let Some(detail) = detail {
                    self.expr(detail, map);
                }
            }
        }
    }

    /// The function and type arguments of a trait method for a concrete
    /// `Self` (the first of `types`); `Display` for the intrinsic `Display` of
    /// built-in values and standard errors. The checker guarantees an implementation exists.
    fn resolve(&mut self, trait_id: TraitId, method: u32, types: &[Ty]) -> Resolved {
        let self_ty = types[0];
        let count = self.traits[trait_id.0 as usize].param_count;
        let (trait_args, own) = types[1..].split_at(count);
        if trait_id == DISPLAY && !matches!(self.types.kind(self_ty), TyKind::Adt(_)) {
            return Resolved::Display;
        }
        for implementation in self.impls.iter().filter(|i| i.trait_id == trait_id) {
            let mut map = HashMap::new();
            let matched = self.types.matches(implementation.target, self_ty, &implementation.generics, &mut map)
                && implementation
                    .trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(&p, &a)| self.types.matches(p, a, &implementation.generics, &mut map));
            if !matched {
                continue;
            }
            if implementation.intrinsic && method == 0 {
                return Resolved::Cursor(self.types.cursor(self_ty).expect("an intrinsic cursor"));
            }
            return match implementation.methods.get(method as usize).copied().flatten() {
                Some(id) => {
                    let binders = implementation.generics.iter().map(|g| map[g]);
                    Resolved::Fn(id, binders.chain(own.iter().copied()).collect())
                }
                None => {
                    let default = self.traits[trait_id.0 as usize].defaults[method as usize];
                    Resolved::Fn(default.expect("a method without implementation has a default"), types.to_vec())
                }
            };
        }
        // The standard library's error enums display intrinsically too.
        if trait_id == DISPLAY {
            return Resolved::Display;
        }
        panic!("no implementation of trait {} for {}", trait_id.0, self.types.display(self_ty));
    }

    /// Records the user `Display` of the types a displayed collection holds,
    /// which code generation calls element by element.
    fn collect_display(&mut self, ty: Ty) {
        let parts = match self.types.kind(ty).clone() {
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::Option(element) => {
                vec![element]
            }
            TyKind::Map(key, value) | TyKind::Result(key, value) => vec![key, value],
            TyKind::Union(members) => members,
            TyKind::Adt(_) => {
                if !self.display.contains_key(&ty)
                    && let Resolved::Fn(id, types) = self.resolve(DISPLAY, 0, &[ty])
                {
                    let instance = self.instance(id, types);
                    self.display.insert(ty, instance);
                }
                return;
            }
            _ => return,
        };
        for part in parts {
            self.collect_display(part);
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
