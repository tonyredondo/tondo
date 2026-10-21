//! Ownership of affine values (8.10): moves and obligations.
//!
//! A value whose type is not `Copy` moves when it is used by value, and a
//! value whose type is not `Discard` must be passed on or returned before
//! its scope ends. In 0.1 such types are type parameters declared `~Copy`
//! and the composites holding them, so the analysis runs on each function
//! before monomorphization. It follows the structured HIR:
//! branches merge what may have moved, and a loop body is checked again
//! with what its previous iteration moved.

use crate::ast::Mode;
use crate::diag::Diagnostics;
use crate::hir::{
    ArrayOp, Block, Callee, Expr, ExprKind, Function, LocalId, MapOp, Mutation, PatKind, Pattern, PlaceStep, Segment,
    Stmt,
};
use crate::source::Span;
use crate::types::{Ty, TyKind, Types};
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, PartialEq)]
enum Moved {
    /// On every path that reaches this point.
    Surely,
    /// On some paths only.
    Maybe,
}

/// The owned bindings that do not hold their value; absent ones do.
#[derive(Clone, Default, PartialEq)]
struct Flow {
    moved: HashMap<LocalId, Moved>,
}

/// The state after two paths join; `None` is a path that does not continue.
fn merge(a: Option<Flow>, b: Option<Flow>) -> Option<Flow> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            for state in a.moved.iter_mut().filter(|(l, _)| !b.moved.contains_key(*l)).map(|(_, s)| s) {
                *state = Moved::Maybe;
            }
            for (local, state) in b.moved {
                let joined = match a.moved.get(&local) {
                    Some(Moved::Surely) if state == Moved::Surely => Moved::Surely,
                    _ => Moved::Maybe,
                };
                a.moved.insert(local, joined);
            }
            Some(a)
        }
        (a, None) => a,
        (None, b) => b,
    }
}

struct Loop {
    /// The number of scopes outside the loop.
    depth: usize,
    breaks: Option<Flow>,
    continues: Option<Flow>,
}

/// Checks the moves and obligations of `function`. `params` tells, per
/// local, how a parameter is passed, and `modes` the parameter modes of a
/// callee.
pub fn check(
    types: &Types,
    function: &Function,
    params: &[Option<Mode>],
    modes: &dyn Fn(Callee) -> Vec<Mode>,
    diags: &mut Diagnostics,
) {
    let mut checker = Checker {
        types,
        function,
        params,
        modes,
        diags,
        scopes: Vec::new(),
        loops: Vec::new(),
        reported: HashSet::new(),
    };
    // Parameters passed by value belong to the body; loans do not.
    let owned = function.params.iter().copied().filter(|p| params[p.0 as usize] == Some(Mode::Value)).collect();
    checker.scopes.push(owned);
    if let Some(flow) = checker.block(&function.body, Flow::default()) {
        checker.settle(0, &flow, "the function ends");
    }
}

struct Checker<'a> {
    types: &'a Types,
    function: &'a Function,
    params: &'a [Option<Mode>],
    modes: &'a dyn Fn(Callee) -> Vec<Mode>,
    diags: &'a mut Diagnostics,
    /// The owned bindings of each scope.
    scopes: Vec<Vec<LocalId>>,
    loops: Vec<Loop>,
    /// What was reported, as `(code, position)`: loops are checked twice.
    reported: HashSet<(&'static str, u32)>,
}

impl Checker<'_> {
    fn error(&mut self, code: &'static str, span: Span, message: String) {
        if self.reported.insert((code, span.start)) {
            self.diags.error(code, span, message);
        }
    }

    fn local(&self, local: LocalId) -> &crate::hir::Local {
        &self.function.locals[local.0 as usize]
    }

    fn display(&self, ty: Ty) -> String {
        format!("`{}`", self.types.display(ty))
    }

    /// Reports the owned bindings of `scopes[from..]` that may still hold a
    /// value that cannot be dropped when `leaving` happens.
    fn settle(&mut self, from: usize, flow: &Flow, leaving: &str) {
        let pending: Vec<(LocalId, Option<Moved>)> = self.scopes[from..]
            .iter()
            .flatten()
            .map(|&l| (l, flow.moved.get(&l).copied()))
            .filter(|&(l, state)| state != Some(Moved::Surely) && !self.types.is_discard(self.local(l).ty))
            .collect();
        for (local, state) in pending {
            let (name, ty, span) = (self.local(local).name.clone(), self.local(local).ty, self.local(local).span);
            let paths = if state == Some(Moved::Maybe) { "on some paths " } else { "" };
            let message = format!(
                "`{name}` still holds its value {paths}when {leaving}, and its type {} may need to be consumed; pass it on or return it on every path, or add `Discard` to its type parameter",
                self.display(ty)
            );
            self.error("E1409", span, message);
        }
    }

    fn block(&mut self, block: &Block, flow: Flow) -> Option<Flow> {
        self.scopes.push(Vec::new());
        let flow = self.statements(block, flow);
        if let Some(flow) = &flow {
            let depth = self.scopes.len() - 1;
            self.settle(depth, flow, "its block ends");
        }
        self.scopes.pop();
        flow
    }

    fn statements(&mut self, block: &Block, flow: Flow) -> Option<Flow> {
        let mut flow = flow;
        for stmt in &block.stmts {
            flow = self.stmt(stmt, flow)?;
        }
        match &block.tail {
            Some(tail) => self.consume(tail, flow),
            None => Some(flow),
        }
    }

    /// A binding takes a new value. Hidden bindings only pass values along.
    fn declare(&mut self, local: LocalId, flow: &mut Flow) {
        flow.moved.remove(&local);
        if self.local(local).name != "_" {
            self.scopes.last_mut().unwrap().push(local);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, flow: Flow) -> Option<Flow> {
        match stmt {
            Stmt::Let(local, value) => {
                let mut flow = self.consume(value, flow)?;
                self.declare(*local, &mut flow);
                Some(flow)
            }
            Stmt::Destructure(pattern, value) => {
                let mut flow = self.consume(value, flow)?;
                self.dropped_parts(pattern);
                self.bind(pattern, &mut flow);
                Some(flow)
            }
            Stmt::Assign { local, path, op, value, span } => {
                let mut flow = self.consume(value, flow)?;
                flow = self.indices(path, flow)?;
                let whole = path.is_empty() && op.is_none();
                if !whole {
                    self.use_local(*local, *span, &flow);
                }
                // An assignment must not drop a value that may need consuming;
                // a complete one gives a moved binding its value back.
                let ty = self.place_ty(*local, path);
                let holds = !whole || flow.moved.get(local) != Some(&Moved::Surely);
                if op.is_none() && holds && !self.types.is_discard(ty) {
                    let message = format!(
                        "assigning `{}` here would drop the value it holds, and its type {} may need to be consumed; pass that value on first, or add `Discard` to its type parameter",
                        self.local(*local).name,
                        self.display(ty)
                    );
                    self.error("E1408", *span, message);
                }
                if whole {
                    flow.moved.remove(local);
                }
                Some(flow)
            }
            Stmt::AssignSlice { local, path, bounds, value, span, .. } => {
                let mut flow = self.consume(value, flow)?;
                flow = self.indices(path, flow)?;
                for bound in bounds.iter().flatten() {
                    flow = self.consume(bound, flow)?;
                }
                self.use_local(*local, *span, &flow);
                Some(flow)
            }
            // The cleanup takes what it uses now and consumes it at the end
            // of the scope.
            Stmt::Defer { closure, .. } => self.consume(closure, flow),
            Stmt::Expr(value) => self.consume(value, flow),
            Stmt::Return(value) => {
                let flow = match value {
                    Some(value) => self.consume(value, flow)?,
                    None => flow,
                };
                self.settle(0, &flow, "the function returns");
                None
            }
            Stmt::Fail(value) => {
                let flow = self.consume(value, flow)?;
                self.settle(0, &flow, "the function fails");
                None
            }
            Stmt::Break | Stmt::Continue => {
                let depth = self.loops.last()?.depth;
                self.settle(depth, &flow, "the loop is left");
                let current = self.loops.last_mut()?;
                let exits = if matches!(stmt, Stmt::Break) { &mut current.breaks } else { &mut current.continues };
                *exits = merge(exits.take(), Some(flow));
                None
            }
            Stmt::Loop { condition, body } => self.repeat(flow, condition.is_some(), |this, flow| {
                let flow = match condition {
                    Some(condition) => this.consume(condition, flow)?,
                    None => flow,
                };
                this.block(body, flow)
            }),
            Stmt::ForEach { pattern, source, body } => {
                let flow = self.consume(source, flow)?;
                self.dropped_parts(pattern);
                self.repeat(flow, true, |this, mut flow| {
                    this.scopes.push(Vec::new());
                    this.bind(pattern, &mut flow);
                    let end = this.block(body, flow);
                    if let Some(end) = &end {
                        let depth = this.scopes.len() - 1;
                        this.settle(depth, end, "the iteration ends");
                    }
                    this.scopes.pop();
                    end
                })
            }
            Stmt::ForRange { start, end, body, .. } => {
                let flow = self.consume(start, flow)?;
                let flow = self.consume(end, flow)?;
                self.repeat(flow, true, |this, flow| this.block(body, flow))
            }
        }
    }

    /// A loop: its body runs again from the state at its head until that
    /// state is stable, so what one iteration moves the next one sees.
    fn repeat(
        &mut self,
        flow: Flow,
        exhausts: bool,
        mut body: impl FnMut(&mut Self, Flow) -> Option<Flow>,
    ) -> Option<Flow> {
        let mut head = flow;
        let mut breaks = None;
        loop {
            let depth = self.scopes.len();
            self.loops.push(Loop { depth, breaks: None, continues: None });
            let end = body(self, head.clone());
            let current = self.loops.pop().expect("pushed above");
            breaks = merge(breaks, current.breaks);
            let next = merge(merge(Some(head.clone()), end), current.continues).expect("the head continues");
            if next == head {
                break;
            }
            head = next;
        }
        if exhausts { merge(Some(head), breaks) } else { breaks }
    }

    /// The type of the place at `path` inside `local`.
    fn place_ty(&self, local: LocalId, path: &[PlaceStep]) -> Ty {
        path.iter().fold(self.local(local).ty, |ty, step| match step {
            PlaceStep::Field(index) => self.types.components(ty).map_or(Ty::ERROR, |c| c[*index as usize]),
            PlaceStep::Index(_) => match self.types.kind(self.types.repr(ty)) {
                TyKind::Array(element) | TyKind::Deque(element) | TyKind::Map(_, element) => *element,
                _ => Ty::ERROR,
            },
        })
    }

    fn indices(&mut self, path: &[PlaceStep], flow: Flow) -> Option<Flow> {
        let mut flow = flow;
        for step in path {
            if let PlaceStep::Index(index) = step {
                flow = self.consume(index, flow)?;
            }
        }
        Some(flow)
    }

    /// Declares the bindings a pattern introduces.
    fn bind(&mut self, pattern: &Pattern, flow: &mut Flow) {
        match &pattern.kind {
            PatKind::Binding(local) => self.declare(*local, flow),
            PatKind::Fields(items) | PatKind::Variant(_, items) => items.iter().for_each(|p| self.bind(p, flow)),
            PatKind::Array { items, rest } => {
                items.iter().for_each(|p| self.bind(p, flow));
                if let Some(Some(local)) = rest {
                    self.declare(*local, flow);
                }
            }
            PatKind::Newtype(inner) => self.bind(inner, flow),
            PatKind::Wildcard | PatKind::Literal(_) => {}
        }
    }

    /// Whether a pattern binds a part that is not `Copy`, taking it.
    fn takes(&self, pattern: &Pattern) -> bool {
        match &pattern.kind {
            PatKind::Binding(local) => !self.types.is_copy(self.local(*local).ty),
            PatKind::Fields(items) | PatKind::Variant(_, items) => items.iter().any(|p| self.takes(p)),
            PatKind::Array { items, rest } => {
                items.iter().any(|p| self.takes(p))
                    || matches!(rest, Some(Some(local)) if !self.types.is_copy(self.local(*local).ty))
            }
            PatKind::Newtype(inner) => self.takes(inner),
            PatKind::Wildcard | PatKind::Literal(_) => false,
        }
    }

    /// Reports the parts a pattern that consumes its value leaves unbound
    /// although they cannot be dropped.
    fn dropped_parts(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatKind::Wildcard | PatKind::Array { rest: Some(None), .. } if !self.types.is_discard(pattern.ty) => {
                let message = format!(
                    "this pattern drops a part of type {}, which may need to be consumed; bind it to a name and pass it on, or add `Discard` to its type parameter",
                    self.display(pattern.ty)
                );
                self.error("E1409", pattern.span, message);
            }
            PatKind::Fields(items) | PatKind::Variant(_, items) | PatKind::Array { items, .. } => {
                items.iter().for_each(|p| self.dropped_parts(p))
            }
            PatKind::Newtype(inner) => self.dropped_parts(inner),
            _ => {}
        }
    }

    fn use_local(&mut self, local: LocalId, span: Span, flow: &Flow) {
        if let Some(state) = flow.moved.get(&local) {
            let local = self.local(local);
            let when = if *state == Moved::Maybe { " on some paths" } else { "" };
            let message = format!(
                "`{}` was moved{when} before this use, since its type {} is not `Copy`; use it before passing it on, or remove `~Copy` from its type parameter",
                local.name,
                self.display(local.ty)
            );
            self.error("E1401", span, message);
        }
    }

    /// The binding a place expression reads.
    fn root(expr: &Expr) -> Option<LocalId> {
        match &expr.kind {
            ExprKind::Local(local) => Some(*local),
            ExprKind::Field(base, _) | ExprKind::Retype(base) | ExprKind::Index(base, _) => Self::root(base),
            _ => None,
        }
    }

    /// Reads `expr` without taking its value: a place stays available.
    fn observe(&mut self, expr: &Expr, flow: Flow) -> Option<Flow> {
        match &expr.kind {
            ExprKind::Local(local) => {
                self.use_local(*local, expr.span, &flow);
                Some(flow)
            }
            ExprKind::Field(base, _) | ExprKind::Retype(base) => self.observe(base, flow),
            ExprKind::Index(base, index) => {
                let flow = self.observe(base, flow)?;
                self.consume(index, flow)
            }
            // A `ref` region (16.4) is observed where it is.
            ExprKind::Slice(base, bounds) => {
                let mut flow = self.observe(base, flow)?;
                for bound in bounds.iter().flatten() {
                    flow = self.consume(bound, flow)?;
                }
                Some(flow)
            }
            _ => self.consume(expr, flow),
        }
    }

    /// Reads a collection place whose values the expression copies out:
    /// they must be `Copy`, since the place keeps them.
    fn copy_out(&mut self, expr: &Expr, collection: &Expr, flow: Flow) -> Option<Flow> {
        if let Some(root) = Self::root(collection)
            && !self.types.is_copy(expr.ty)
        {
            let message = format!(
                "this copies a value of type {} out of `{}`, but it is not `Copy`; remove `~Copy` from its type parameter, or take values out with `pop()`, `removeAt()` or a `for` loop",
                self.display(expr.ty),
                self.local(root).name
            );
            self.error("E1406", expr.span, message);
            let flow = self.observe(collection, flow)?;
            return Some(self.forget(root, flow));
        }
        self.observe(collection, flow)
    }

    /// After a rejected move of a part, the binding counts as moved so that
    /// it is not reported again.
    fn forget(&self, root: LocalId, mut flow: Flow) -> Flow {
        if self.params[root.0 as usize].is_none_or(|m| m == Mode::Value) {
            flow.moved.insert(root, Moved::Surely);
        }
        flow
    }

    /// Evaluates `expr` and takes its value: a place that is not `Copy`
    /// moves.
    fn consume(&mut self, expr: &Expr, flow: Flow) -> Option<Flow> {
        let copy = self.types.is_copy(expr.ty);
        let flow = match &expr.kind {
            ExprKind::Local(local) => {
                self.use_local(*local, expr.span, &flow);
                let mut flow = flow;
                if !copy {
                    if self.params[local.0 as usize].is_some_and(|m| m != Mode::Value) {
                        let message = format!(
                            "`{}` is a loan, so its value of type {} cannot be moved out of it",
                            self.local(*local).name,
                            self.display(expr.ty)
                        );
                        self.error("E1406", expr.span, message);
                    } else {
                        flow.moved.insert(*local, Moved::Surely);
                    }
                }
                Some(flow)
            }
            ExprKind::Field(base, _) if !copy && Self::root(base).is_some() => {
                let root = Self::root(base).expect("checked above");
                let message = format!(
                    "moving a part of type {} out of `{}` would leave it incomplete; destructure the whole value with `let`, or remove `~Copy` from its type parameter",
                    self.display(expr.ty),
                    self.local(root).name
                );
                self.error("E1406", expr.span, message);
                let flow = self.observe(expr, flow)?;
                Some(self.forget(root, flow))
            }
            ExprKind::Field(base, _) | ExprKind::Retype(base) => {
                if copy {
                    self.observe(base, flow)
                } else {
                    self.consume(base, flow)
                }
            }
            ExprKind::Index(base, index) => {
                let flow = self.copy_out(expr, base, flow)?;
                self.consume(index, flow)
            }
            ExprKind::Slice(base, bounds) => {
                let mut flow = self.copy_out(expr, base, flow)?;
                for bound in bounds.iter().flatten() {
                    flow = self.consume(bound, flow)?;
                }
                Some(flow)
            }
            ExprKind::ArrayOp(ArrayOp::Get | ArrayOp::SliceChecked, operands)
            | ExprKind::MapOp(MapOp::Get, operands) => {
                let flow = self.copy_out(expr, &operands[0], flow)?;
                self.all(&operands[1..], flow, false)
            }
            ExprKind::ArrayOp(ArrayOp::Repeat, operands) => {
                let flow = self.copy_out(expr, &operands[0], flow)?;
                self.all(&operands[1..], flow, true)
            }
            ExprKind::ArrayOp(ArrayOp::Contains, operands) | ExprKind::MapOp(MapOp::Contains, operands) => {
                self.all(operands, flow, false)
            }
            ExprKind::ArrayOp(_, operands) | ExprKind::MapOp(_, operands) | ExprKind::StrOp(_, operands) => {
                self.all(operands, flow, true)
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::FnValue(..)
            | ExprKind::Error => Some(flow),
            ExprKind::Aggregate(items) | ExprKind::Array(items) | ExprKind::Set(items) => self.all(items, flow, true),
            ExprKind::Record(fields) | ExprKind::Variant(_, fields) => {
                let mut flow = flow;
                for (_, value) in fields {
                    flow = self.consume(value, flow)?;
                }
                Some(flow)
            }
            ExprKind::With(base, updates) => {
                let mut flow = self.consume(base, flow)?;
                for (_, value) in updates {
                    flow = self.consume(value, flow)?;
                }
                Some(flow)
            }
            ExprKind::Map(entries) => {
                let mut flow = flow;
                for (key, value) in entries {
                    flow = self.consume(key, flow)?;
                    flow = self.consume(value, flow)?;
                }
                Some(flow)
            }
            ExprKind::Match(scrutinee, arms) => {
                // A place keeps its value unless an arm binds a part that
                // is not `Copy`; then, like a temporary, it is consumed.
                let place = Self::root(scrutinee).is_some();
                let takes = !place || arms.iter().any(|a| self.takes(&a.pattern));
                let flow = if takes { self.consume(scrutinee, flow)? } else { self.observe(scrutinee, flow)? };
                let mut out = None;
                for arm in arms {
                    if takes {
                        self.dropped_parts(&arm.pattern);
                    }
                    self.scopes.push(Vec::new());
                    let mut arm_flow = flow.clone();
                    self.bind(&arm.pattern, &mut arm_flow);
                    let mut end = match &arm.guard {
                        Some(guard) => self.consume(guard, arm_flow),
                        None => Some(arm_flow),
                    };
                    end = end.and_then(|f| self.consume(&arm.body, f));
                    if let Some(end) = &end {
                        let depth = self.scopes.len() - 1;
                        self.settle(depth, end, "its arm ends");
                    }
                    self.scopes.pop();
                    out = merge(out, end);
                }
                out
            }
            ExprKind::If(condition, then, otherwise) => {
                let flow = self.consume(condition, flow)?;
                let then = self.block(then, flow.clone());
                let otherwise = match otherwise {
                    Some(otherwise) => self.consume(otherwise, flow),
                    None => Some(flow),
                };
                merge(then, otherwise)
            }
            ExprKind::Block(block) => self.block(block, flow),
            ExprKind::Call(callee, _, args) => {
                let modes = (self.modes)(*callee);
                let mut flow = flow;
                for (index, arg) in args.iter().enumerate() {
                    flow = match modes.get(index) {
                        Some(Mode::Value) | None => self.consume(arg, flow)?,
                        Some(_) => self.observe(arg, flow)?,
                    };
                }
                Some(flow)
            }
            ExprKind::CallValue(callee, args) => {
                let flow = self.observe(callee, flow)?;
                self.all(args, flow, true)
            }
            ExprKind::Loan(place) | ExprKind::CursorNext(place, _) => {
                let flow = self.indices(&place.path, flow)?;
                self.use_local(place.local, expr.span, &flow);
                Some(flow)
            }
            ExprKind::Region(place, bounds) => {
                let mut flow = self.indices(&place.path, flow)?;
                for bound in bounds.iter().flatten() {
                    flow = self.consume(bound, flow)?;
                }
                self.use_local(place.local, expr.span, &flow);
                Some(flow)
            }
            ExprKind::Mutate(mutation, place, values) => {
                let mut flow = self.indices(&place.path, flow)?;
                self.use_local(place.local, expr.span, &flow);
                let ty = self.place_ty(place.local, &place.path);
                if *mutation == Mutation::Clear && !self.types.is_discard(ty) {
                    let message = format!(
                        "`clear()` would drop the elements of `{}`, and its type {} may need to be consumed; take them out with `pop()` or a `for` loop, or add `Discard` to its type parameter",
                        self.local(place.local).name,
                        self.display(ty)
                    );
                    self.error("E1409", expr.span, message);
                }
                for (value, moves) in values.iter().zip(mutation.moves(values.len())) {
                    flow = if moves { self.consume(value, flow)? } else { self.observe(value, flow)? };
                }
                Some(flow)
            }
            ExprKind::Interpolate(segments) => {
                let mut flow = flow;
                for segment in segments {
                    if let Segment::Value(value) = segment {
                        flow = self.observe(value, flow)?;
                    }
                }
                Some(flow)
            }
            ExprKind::Binary(_, left, right) => {
                let flow = self.observe(left, flow)?;
                self.observe(right, flow)
            }
            ExprKind::Length(value) | ExprKind::Unary(_, value) => self.observe(value, flow),
            ExprKind::Try(inner) => {
                let flow = self.consume(inner, flow)?;
                self.settle(0, &flow, "`?` fails");
                Some(flow)
            }
            ExprKind::Convert(inner) | ExprKind::Panic(inner) | ExprKind::Erase(inner, _) => self.consume(inner, flow),
            ExprKind::Assert { condition, parts, .. } => {
                let flow = self.consume(condition, flow)?;
                self.all(parts, flow, false)
            }
        };
        flow.filter(|_| expr.ty != Ty::NEVER)
    }

    fn all(&mut self, values: &[Expr], flow: Flow, take: bool) -> Option<Flow> {
        let mut flow = flow;
        for value in values {
            flow = if take { self.consume(value, flow)? } else { self.observe(value, flow)? };
        }
        Some(flow)
    }
}
