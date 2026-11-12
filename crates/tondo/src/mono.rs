//! Monomorphization: one copy of each function per combination of type
//! arguments reachable from `main`, with every type parameter substituted.
//! Generic code is checked once (12.1); lowering and code generation only
//! see concrete types.

use crate::ast::BinOp;
use crate::diag::Diagnostics;
use crate::hir::{
    Arm, Block, Callee, Expr, ExprKind, FnId, Function, ImplInfo, Local, LocalId, PatKind, Pattern, Place, PlaceStep,
    Program, Segment, Stmt, TraitId, TraitInfo,
};
use crate::source::Span;
use crate::types::{AdtKind, CursorKind, Payload, SOME, Ty, TyKind, Types};
use std::collections::{HashMap, HashSet, VecDeque};

/// Instantiation depth beyond which a generic function is considered to
/// call itself with ever larger type arguments.
const MAX_DEPTH: usize = 32;

pub fn monomorphize(program: Program, diags: &mut Diagnostics) -> Option<Program> {
    monomorphize_with(program, &[], diags)
}

/// The same, with more functions that are not generic built as if called:
/// a test build's own, which coverage counts even when nothing calls them.
pub fn monomorphize_with(program: Program, roots: &[FnId], diags: &mut Diagnostics) -> Option<Program> {
    let Program {
        mut types,
        functions,
        main,
        traits,
        impls,
        externs,
        mut exports,
        c_api,
        tests,
        contexts,
        field_defaults,
        unchecked,
        ..
    } = program;
    let index = ImplIndex::new(&types, &impls);
    let mut mono = Mono {
        types: &mut types,
        functions: &functions,
        traits: &traits,
        impls: &impls,
        index: &index,
        unchecked: &unchecked,
        instances: HashMap::new(),
        queue: VecDeque::new(),
        out: Vec::new(),
        display: HashMap::new(),
        shown_parts: HashSet::new(),
        runaway: None,
        conversions: Vec::new(),
        unions: HashMap::new(),
        made: Vec::new(),
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
    // The runtime makes a context's initial value when a task first reads it.
    let contexts: Vec<crate::hir::ContextFns> = contexts
        .into_iter()
        .map(|fns| crate::hir::ContextFns {
            maker: mono.instance(fns.maker, Vec::new()),
            log: fns.log.map(|f| mono.instance(f, Vec::new())),
            label: fns.label.map(|f| mono.instance(f, Vec::new())),
            computed: fns
                .computed
                .into_iter()
                .map(|(field, f, named)| (field, mono.instance(f, Vec::new()), named))
                .collect(),
        })
        .collect();
    mono.drain();
    // The views the debugger shows the values of a type as, for every type
    // a function holds, when the program implements `debug.View`.
    let view = traits.iter().position(|t| t.name == "debug.View").map(|i| TraitId(i as u32));
    let mut views = HashMap::new();
    if let Some(view) = view.filter(|&view| impls.iter().any(|i| i.trait_id == view)) {
        let held: Vec<Ty> = mono.out.iter().flatten().flat_map(|f| f.locals.iter().map(|l| l.ty)).collect();
        let mut seen = HashSet::new();
        for ty in held {
            mono.collect_view(view, ty, &mut seen, &mut views);
        }
        mono.drain();
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
    // Each view with the type it gives.
    let views: HashMap<Ty, (FnId, Ty)> = views
        .into_iter()
        .map(|(ty, view)| (ty, (view, mono.out[view.0 as usize].as_ref().expect("a view's instance").result)))
        .collect();
    let functions = mono.out.into_iter().map(|f| f.expect("every instance is generated")).collect();
    Some(Program {
        types,
        functions,
        main,
        traits,
        impls,
        display,
        views,
        externs,
        exports,
        c_api,
        tests,
        contexts,
        field_defaults,
        unchecked: HashSet::new(),
    })
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
    /// A method of the arithmetic of a number or a duration, which the
    /// compiler computes (18.10).
    Arithmetic,
    /// A function the compiler wrote for this type, already concrete.
    Made(FnId),
}

/// The implementations of each trait that may apply to a type, in their
/// order: those whose target is a record, an enum or an instance of a
/// generic one, which match only that type or instances of the same one,
/// and the others, which any type may match. Looking at all of a trait's
/// for each type made a program's every derived `Encode` cost as much as
/// the program had types.
struct ImplIndex {
    named: HashMap<(TraitId, Head), Vec<usize>>,
    others: HashMap<TraitId, Vec<usize>>,
}

/// What an implementation's target names that a type must name too.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Head {
    Template(crate::types::TemplateId),
    Type(Ty),
}

fn head(types: &Types, ty: Ty) -> Option<Head> {
    match types.kind(ty) {
        TyKind::Adt(_) => Some(types.instance_of(ty).map_or(Head::Type(ty), |(template, _)| Head::Template(template))),
        _ => None,
    }
}

impl ImplIndex {
    fn new(types: &Types, impls: &[ImplInfo]) -> ImplIndex {
        let mut index = ImplIndex { named: HashMap::new(), others: HashMap::new() };
        for (at, implementation) in impls.iter().enumerate() {
            match head(types, implementation.target) {
                Some(head) => index.named.entry((implementation.trait_id, head)).or_default().push(at),
                None => index.others.entry(implementation.trait_id).or_default().push(at),
            }
        }
        index
    }

    /// The implementations of `trait_id` that may apply to `ty`, in order.
    fn candidates(&self, types: &Types, trait_id: TraitId, ty: Ty) -> Vec<usize> {
        let none: &[usize] = &[];
        let named = head(types, ty).and_then(|head| self.named.get(&(trait_id, head))).map_or(none, Vec::as_slice);
        let others = self.others.get(&trait_id).map_or(none, Vec::as_slice);
        let mut merged = Vec::with_capacity(named.len() + others.len());
        let (mut i, mut j) = (0, 0);
        while i < named.len() || j < others.len() {
            if j == others.len() || (i < named.len() && named[i] < others[j]) {
                merged.push(named[i]);
                i += 1;
            } else {
                merged.push(others[j]);
                j += 1;
            }
        }
        merged
    }
}

struct Mono<'a> {
    types: &'a mut Types,
    functions: &'a [Function],
    traits: &'a [TraitInfo],
    impls: &'a [ImplInfo],
    index: &'a ImplIndex,
    /// The functions whose bodies were not checked, which none reaches.
    unchecked: &'a HashSet<FnId>,
    instances: HashMap<(FnId, Vec<Ty>), FnId>,
    /// The user `Display.display` of types displayed inside collections.
    display: HashMap<Ty, FnId>,
    /// The records and enums that show their parts, whose parts' `Display`
    /// is collected.
    shown_parts: HashSet<Ty>,
    queue: VecDeque<(FnId, Vec<Ty>, FnId)>,
    out: Vec<Option<Function>>,
    runaway: Option<String>,
    /// The calls of `reflect.convert`, from and to which type, checked once
    /// every instance is known.
    conversions: Vec<(crate::source::Span, Ty, Ty)>,
    /// The `Encode` or `Decode` written for a union, by trait, union and
    /// the format's type.
    unions: HashMap<(TraitId, Ty, Vec<Ty>), FnId>,
    /// The functions written here whose calls are still to resolve.
    made: Vec<(Function, FnId)>,
}

impl Mono<'_> {
    /// The concrete function for `id` applied to `args`, queued on first use.
    fn instance(&mut self, id: FnId, args: Vec<Ty>) -> FnId {
        if let Some(&instance) = self.instances.get(&(id, args.clone())) {
            return instance;
        }
        let name = &self.functions[id.0 as usize].name;
        assert!(
            !self.unchecked.contains(&id),
            "`{name}` is compiled but its body was not checked: check/reach.rs misses what reaches it"
        );
        let instance = FnId(self.out.len() as u32);
        if args.iter().any(|&a| self.types.depth(a) > MAX_DEPTH) {
            self.runaway.get_or_insert_with(|| self.functions[id.0 as usize].name.clone());
            return instance;
        }
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
            Stmt::ContextSet { value, .. } => self.expr(value, map),
            Stmt::ContextsActivate { activate, .. } => self.expr(activate, map),
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
            match step {
                PlaceStep::Index(index) => self.expr(index, map),
                PlaceStep::Entry { key, default } => {
                    self.expr(key, map);
                    self.expr(default, map);
                }
                PlaceStep::Field(_) => {}
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
            | ExprKind::Bytes(_)
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::ExternFn(_)
            | ExprKind::ContextRead(..)
            | ExprKind::Error => {}
            ExprKind::Layout(ty, _) => *ty = self.ty(*ty, map),
            ExprKind::Reflect(op, ty, values) => {
                *ty = self.ty(*ty, map);
                self.exprs(values, map);
                if *op == crate::hir::ReflectOp::Describe {
                    self.collect_display(*ty);
                }
            }
            // `Array.join` displays each element, as an interpolation of
            // the array would.
            ExprKind::ArrayOp(crate::hir::ArrayOp::Join, operands) => {
                self.exprs(operands, map);
                self.collect_display(operands[0].ty);
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
            ExprKind::Loan(place) | ExprKind::CursorNext(place, _) | ExprKind::PlaceValue(place) => {
                self.place(place, map)
            }
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
                        Resolved::Arithmetic => {
                            let name = self.traits[trait_id.0 as usize].methods[method as usize].clone();
                            e.kind = self.arithmetic(&name, std::mem::take(args), e.ty);
                        }
                        Resolved::Made(instance) => *callee = Callee::Fn(instance),
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
            ExprKind::Recover(block, caught) => {
                self.block(block, map);
                self.expr(caught, map);
            }
            ExprKind::Within(limit, block, failure) => {
                self.expr(limit, map);
                self.block(block, map);
                self.expr(failure, map);
            }
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
        let intrinsic = matches!(self.types.kind(self_ty), TyKind::Int(_) | TyKind::Float(_))
            || self.types.shown.get(&self_ty) == Some(&crate::types::Shown::Duration);
        if self.traits[trait_id.0 as usize].arithmetic && intrinsic {
            return Resolved::Arithmetic;
        }
        // A union is no `Array[T]` nor any other target an implementation
        // could match: the compiler writes its own.
        if let TyKind::Union(members) = self.types.kind(self_ty).clone() {
            let name = self.traits[trait_id.0 as usize].name.clone();
            if name == "serialization.Encode" || name == "serialization.Decode" {
                return Resolved::Made(self.union_codec(trait_id, self_ty, &members, own));
            }
        }
        for at in self.index.candidates(self.types, trait_id, self_ty) {
            let implementation = &self.impls[at];
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

    /// A union's `Encode`, which writes the member it holds, or its
    /// `Decode`, which reads the first member that reads the value, in the
    /// order the union writes them (standard library 14.6), for the format
    /// of type `own[0]`.
    fn union_codec(&mut self, trait_id: TraitId, union: Ty, members: &[Ty], own: &[Ty]) -> FnId {
        let key = (trait_id, union, own.to_vec());
        if let Some(&made) = self.unions.get(&key) {
            return made;
        }
        let instance = FnId(self.out.len() as u32);
        self.out.push(None);
        self.unions.insert(key, instance);
        let format = own[0];
        let span = Span::default();
        let mk = |kind: ExprKind, ty: Ty| Expr { kind, ty, span };
        let mut locals: Vec<Local> = Vec::new();
        let mut local = |name: &str, ty: Ty| {
            locals.push(Local { name: name.into(), ty, mutable: false, span });
            LocalId(locals.len() as u32 - 1)
        };
        let shown = self.types.display(union);
        let function = if self.traits[trait_id.0 as usize].name == "serialization.Encode" {
            // `match self { member => member.encode(out) }`
            let (value, out) = (local("self", union), local("out", format));
            let arms = members
                .iter()
                .enumerate()
                .map(|(index, &member)| {
                    let held = local("member", member);
                    let args = vec![mk(ExprKind::Local(held), member), mk(ExprKind::Local(out), format)];
                    Arm {
                        pattern: Pattern {
                            kind: PatKind::Variant(
                                index as u32,
                                vec![Pattern { kind: PatKind::Binding(held), ty: member, span }],
                            ),
                            ty: union,
                            span,
                        },
                        guard: None,
                        body: mk(ExprKind::Call(Callee::Trait(trait_id, 0), vec![member, format], args), Ty::UNIT),
                    }
                })
                .collect();
            let tail = mk(ExprKind::Match(Box::new(mk(ExprKind::Local(value), union)), arms), Ty::UNIT);
            Function {
                name: format!("Encode for {shown}"),
                generics: Vec::new(),
                params: vec![value, out],
                loans: vec![false, false],
                locals,
                result: Ty::UNIT,
                fallible: false,
                body: Block { stmts: Vec::new(), tail: Some(Box::new(tail)), ty: Ty::UNIT },
                moves: Default::default(),
                test: false,
                span,
            }
        } else {
            // `let attempt = unionAttempt(input, first)?`, then for each member
            // `let (read, attempt) = unionMember[T](input, attempt)` and
            // `match read { some(value) => return value }`, and at the end
            // `fail unionFailure(input, attempt, "...")`.
            let helper = |name: &str| {
                let full = format!("serialization.{name}");
                FnId(self.functions.iter().position(|f| f.name == full).expect("a helper of std.serialization") as u32)
            };
            let (attempt_fn, member_fn, failure_fn) =
                (helper("unionAttempt"), helper("unionMember"), helper("unionFailure"));
            let result_of = |this: &mut Self, id: FnId, types: Vec<Ty>| {
                let f = &this.functions[id.0 as usize];
                let map: HashMap<Ty, Ty> = f.generics.iter().copied().zip(types).collect();
                this.ty(f.result, &map)
            };
            let attempt_result = result_of(self, attempt_fn, vec![format]);
            let TyKind::Result(attempt_ty, error) = *self.types.kind(attempt_result) else {
                unreachable!("`unionAttempt` can fail")
            };
            let event = self.functions[attempt_fn.0 as usize].locals[1].ty;
            let (input, first) = (local("input", format), local("first", event));
            let mut attempt = local("attempt", attempt_ty);
            // The members that read the fewest values first: floats, which
            // read integers too, `String`, and a format's `Value`, which
            // reads anything, last; the others by their names, as the union
            // orders them.
            let mut tried: Vec<(usize, Ty)> = members.iter().copied().enumerate().collect();
            tried.sort_by_key(|&(index, member)| {
                let rank = match self.types.kind(member) {
                    TyKind::Float(_) => 1,
                    TyKind::String => 2,
                    _ if matches!(
                        self.types.display(member).as_str(),
                        "json.Value"
                            | "yaml.Value"
                            | "toml.Value"
                            | "messagepack.Value"
                            | "cbor.Value"
                            | "reflect.Value"
                    ) =>
                    {
                        3
                    }
                    _ => 0,
                };
                (rank, index)
            });
            let call = mk(
                ExprKind::Call(
                    Callee::Fn(attempt_fn),
                    vec![format],
                    vec![mk(ExprKind::Local(input), format), mk(ExprKind::Local(first), event)],
                ),
                attempt_result,
            );
            let mut stmts = vec![Stmt::Let(attempt, mk(ExprKind::Try(Box::new(call)), attempt_ty))];
            for (index, member) in tried {
                let pair = result_of(self, member_fn, vec![member, format]);
                let TyKind::Tuple(items) = self.types.kind(pair).clone() else { unreachable!("a pair") };
                let (read, next) = (local("read", items[0]), local("attempt", attempt_ty));
                let call = mk(
                    ExprKind::Call(
                        Callee::Fn(member_fn),
                        vec![member, format],
                        vec![mk(ExprKind::Local(input), format), mk(ExprKind::Local(attempt), attempt_ty)],
                    ),
                    pair,
                );
                let binding = |local: LocalId, ty: Ty| Pattern { kind: PatKind::Binding(local), ty, span };
                let pattern = Pattern {
                    kind: PatKind::Fields(vec![binding(read, items[0]), binding(next, attempt_ty)]),
                    ty: pair,
                    span,
                };
                stmts.push(Stmt::Destructure(pattern, call));
                let value = local("value", member);
                let found = mk(ExprKind::Variant(index as u32, vec![(0, mk(ExprKind::Local(value), member))]), union);
                let returned = Block { stmts: vec![Stmt::Return(Some(found))], tail: None, ty: Ty::NEVER };
                let arms = vec![
                    Arm {
                        pattern: Pattern {
                            kind: PatKind::Variant(SOME, vec![binding(value, member)]),
                            ty: items[0],
                            span,
                        },
                        guard: None,
                        body: mk(ExprKind::Block(returned), Ty::NEVER),
                    },
                    Arm {
                        pattern: Pattern { kind: PatKind::Wildcard, ty: items[0], span },
                        guard: None,
                        body: mk(ExprKind::Unit, Ty::UNIT),
                    },
                ];
                stmts.push(Stmt::Expr(mk(
                    ExprKind::Match(Box::new(mk(ExprKind::Local(read), items[0])), arms),
                    Ty::UNIT,
                )));
                attempt = next;
            }
            let names: Vec<String> = members.iter().map(|&m| format!("`{}`", self.types.display(m))).collect();
            let expected = match names.split_last() {
                Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
                _ => names.join(""),
            };
            let failure = mk(
                ExprKind::Call(
                    Callee::Fn(failure_fn),
                    vec![format],
                    vec![
                        mk(ExprKind::Local(input), format),
                        mk(ExprKind::Local(attempt), attempt_ty),
                        mk(ExprKind::Str(expected), Ty::STRING),
                    ],
                ),
                error,
            );
            stmts.push(Stmt::Fail(failure));
            Function {
                name: format!("Decode for {shown}"),
                generics: Vec::new(),
                params: vec![input, first],
                loans: vec![false, false],
                locals,
                result: self.types.intern(TyKind::Result(union, error)),
                fallible: true,
                body: Block { stmts, tail: None, ty: Ty::NEVER },
                moves: Default::default(),
                test: false,
                span,
            }
        };
        self.made.push((function, instance));
        instance
    }

    /// A method of `Additive` or `Number` of a number or a duration, as the
    /// expression the compiler computes it with (18.10).
    fn arithmetic(&mut self, name: &str, mut args: Vec<Expr>, ty: Ty) -> ExprKind {
        let float = matches!(self.types.kind(ty), TyKind::Float(_));
        let mut operator = |op: BinOp| {
            let right = args.pop().expect("two operands");
            let left = args.pop().expect("two operands");
            ExprKind::Binary(op, Box::new(left), Box::new(right))
        };
        match name {
            "zero" | "one" if float => ExprKind::Float(if name == "zero" { 0.0 } else { 1.0 }),
            "zero" => ExprKind::Int(0),
            "one" => ExprKind::Int(1),
            "plus" => operator(BinOp::Add),
            "minus" => operator(BinOp::Sub),
            "times" => operator(BinOp::Mul),
            "dividedBy" => operator(BinOp::Div),
            "fromInt" => {
                let value = args.pop().expect("a value");
                match (float, &value.kind) {
                    (true, _) => ExprKind::Convert(Box::new(value)),
                    // A literal that fits is the number itself.
                    (false, ExprKind::Int(n)) if self.types.as_int(ty).is_some_and(|int| int.holds(*n)) => {
                        ExprKind::Int(*n)
                    }
                    (false, _) => ExprKind::Call(
                        Callee::Std(crate::hir::StdFn::Number(crate::numbers::NumOp::FromInt)),
                        Vec::new(),
                        vec![value],
                    ),
                }
            }
            _ => unreachable!("`{name}` is not of the arithmetic"),
        }
    }

    /// Makes the instances the queue asks for, until none is left.
    fn drain(&mut self) {
        loop {
            if let Some((id, args, instance)) = self.queue.pop_front() {
                let generic = &self.functions[id.0 as usize];
                let map: HashMap<Ty, Ty> = generic.generics.iter().copied().zip(args).collect();
                let mut function = generic.clone();
                self.function(&mut function, &map);
                self.out[instance.0 as usize] = Some(function);
            } else if let Some((mut function, instance)) = self.made.pop() {
                self.function(&mut function, &HashMap::new());
                self.out[instance.0 as usize] = Some(function);
            } else {
                break;
            }
        }
    }

    /// Records the `debug.View.view` of `ty` and of the types inside it, for
    /// those that implement it.
    fn collect_view(&mut self, view: TraitId, ty: Ty, seen: &mut HashSet<Ty>, views: &mut HashMap<Ty, FnId>) {
        if !seen.insert(ty) {
            return;
        }
        if let TyKind::Adt(_) = self.types.kind(ty) {
            for at in self.index.candidates(self.types, view, ty) {
                let implementation = &self.impls[at];
                let mut map = HashMap::new();
                if !self.types.matches(implementation.target, ty, &implementation.generics, &mut map) {
                    continue;
                }
                if let Some(id) = implementation.methods.first().copied().flatten() {
                    let binders: Vec<Ty> = implementation.generics.iter().map(|g| map[g]).collect();
                    let instance = self.instance(id, binders);
                    views.insert(ty, instance);
                }
                break;
            }
        }
        let parts = match self.types.kind(ty).clone() {
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) => vec![t],
            TyKind::Map(k, v) | TyKind::Result(k, v) => vec![k, v],
            TyKind::Union(members) => members,
            _ => self.types.components(ty).unwrap_or_default(),
        };
        for part in parts {
            self.collect_view(view, part, seen, views);
        }
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
            TyKind::Adt(id) => {
                if self.display.contains_key(&ty) || self.shown_parts.contains(&ty) {
                    return;
                }
                if let Resolved::Fn(function, types) = self.resolve(DISPLAY, 0, &[ty]) {
                    let instance = self.instance(function, types);
                    self.display.insert(ty, instance);
                    return;
                }
                // A record or an enum without an `impl Display`, which
                // shows its parts with theirs (language 9.4).
                self.shown_parts.insert(ty);
                match &self.types.adt(id).kind {
                    AdtKind::Record(fields) => fields.iter().filter(|f| !f.secret).map(|f| f.ty).collect(),
                    AdtKind::Enum(variants) => variants
                        .iter()
                        .flat_map(|v| match &v.payload {
                            Payload::Record(fields) => fields.iter().filter(|f| !f.secret).map(|f| f.ty).collect(),
                            payload => payload.types(),
                        })
                        .collect(),
                    AdtKind::Newtype(_) => return,
                }
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
            PatKind::Fields(items) | PatKind::Variant(_, items) | PatKind::Array { items, .. } | PatKind::Or(items) => {
                for item in items {
                    self.pattern(item, map);
                }
            }
            PatKind::Newtype(inner) => self.pattern(inner, map),
        }
    }
}
