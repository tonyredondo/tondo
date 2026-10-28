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
    ArrayOp, Block, Callee, Expr, ExprKind, Function, LocalId, MapOp, PatKind, Pattern, PlaceStep, Segment, Stmt,
};
use crate::source::Span;
use crate::types::{Handle, Ty, TyKind, Types};
use std::collections::HashMap as Map;
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, PartialEq)]
enum Moved {
    /// On every path that reaches this point.
    Surely,
    /// On some paths only.
    Maybe,
}

/// The owned bindings that do not hold their value; absent ones do. In a
/// closure body, also the captures it gave away, by their index.
#[derive(Clone, Default, PartialEq)]
struct Flow {
    moved: HashMap<LocalId, Moved>,
    captures: HashMap<u32, Moved>,
}

/// The state after two paths join; `None` is a path that does not continue.
fn merge(a: Option<Flow>, b: Option<Flow>) -> Option<Flow> {
    fn join<K: Copy + Eq + std::hash::Hash>(a: &mut HashMap<K, Moved>, b: HashMap<K, Moved>) {
        for state in a.iter_mut().filter(|(k, _)| !b.contains_key(*k)).map(|(_, s)| s) {
            *state = Moved::Maybe;
        }
        for (key, state) in b {
            let joined = match a.get(&key) {
                Some(Moved::Surely) if state == Moved::Surely => Moved::Surely,
                _ => Moved::Maybe,
            };
            a.insert(key, joined);
        }
    }
    match (a, b) {
        (Some(mut a), Some(b)) => {
            join(&mut a.moved, b.moved);
            join(&mut a.captures, b.captures);
            Some(a)
        }
        (a, None) => a,
        (None, b) => b,
    }
}

struct Loop {
    breaks: Option<Flow>,
    continues: Option<Flow>,
}

/// What the analysis needs to know besides the function.
pub struct Context<'a> {
    /// Per local: how a parameter is passed, or how a binding borrows.
    pub params: &'a [Option<Mode>],
    /// The parameter modes of a callee.
    pub modes: &'a dyn Fn(Callee) -> Vec<Mode>,
    /// The parameter modes of the type parameters with a call bound.
    pub bound_modes: &'a Map<Ty, Vec<Mode>>,
    /// The type parameters bound by `CallOnce`, which a call consumes.
    pub once: &'a HashSet<Ty>,
}

/// Checks the moves and obligations of `function`, and returns where a
/// binding that is not `Copy` moves, its reads that give its value away,
/// and whether a closure body gives a capture away, which makes calling it
/// consume it (`CallOnce`, 11.8).
pub fn check(
    types: &mut Types,
    function: &Function,
    context: Context,
    diags: &mut Diagnostics,
) -> (HashSet<(LocalId, Span)>, bool) {
    let Context { params, modes, bound_modes, once } = context;
    let mut checker = Checker {
        types,
        function,
        params,
        modes,
        bound_modes,
        diags,
        loops: Vec::new(),
        reported: HashSet::new(),
        moves: HashSet::new(),
        quiet: false,
        gives: false,
        once,
    };
    checker.block(&function.body, Flow::default());
    (checker.moves, checker.gives)
}

struct Checker<'a> {
    types: &'a mut Types,
    function: &'a Function,
    params: &'a [Option<Mode>],
    modes: &'a dyn Fn(Callee) -> Vec<Mode>,
    bound_modes: &'a Map<Ty, Vec<Mode>>,
    diags: &'a mut Diagnostics,
    loops: Vec<Loop>,
    /// What was reported, as `(code, position)`: loops are checked twice.
    reported: HashSet<(&'static str, u32)>,
    moves: HashSet<(LocalId, Span)>,
    /// Inside the operations of a `select`, which take their values only in
    /// the arm that wins: such moves are not recorded for lowering.
    quiet: bool,
    /// Whether the closure body gave a capture away.
    gives: bool,
    once: &'a HashSet<Ty>,
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

    fn block(&mut self, block: &Block, flow: Flow) -> Option<Flow> {
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
                // A complete assignment gives a moved binding its value back.
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
            Stmt::Lend { place, .. } => {
                let flow = self.indices(&place.path, flow)?;
                self.use_local(place.local, Span::default(), &flow);
                Some(flow)
            }
            // The state stays in the shared cell; `inner` is a loan of it.
            Stmt::LockSync { handle, .. } => self.consume(handle, flow),
            Stmt::ContextSet { value, .. } => self.consume(value, flow),
            Stmt::AwaitChange(..) => Some(flow),
            Stmt::Expr(value) => self.consume(value, flow),
            Stmt::Return(value) => {
                if let Some(value) = value {
                    self.consume(value, flow)?;
                }
                None
            }
            Stmt::Fail(value) => {
                self.consume(value, flow)?;
                None
            }
            Stmt::Break | Stmt::Continue => {
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
                // `ref` bindings observe the collection; others take it.
                let flow =
                    if self.borrows(pattern) { self.observe(source, flow)? } else { self.consume(source, flow)? };
                self.repeat(flow, true, |this, mut flow| {
                    this.bind(pattern, &mut flow);
                    this.block(body, flow)
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
            self.loops.push(Loop { breaks: None, continues: None });
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
            PatKind::Binding(local) => self.owns(*local),
            PatKind::Fields(items) | PatKind::Variant(_, items) => items.iter().any(|p| self.takes(p)),
            PatKind::Array { items, rest } => {
                items.iter().any(|p| self.takes(p)) || matches!(rest, Some(Some(local)) if self.owns(*local))
            }
            PatKind::Newtype(inner) => self.takes(inner),
            PatKind::Wildcard | PatKind::Literal(_) => false,
        }
    }

    /// Whether a pattern has `ref` bindings.
    fn borrows(&self, pattern: &Pattern) -> bool {
        match &pattern.kind {
            PatKind::Binding(local) => self.params[local.0 as usize].is_some(),
            PatKind::Fields(items) | PatKind::Variant(_, items) => items.iter().any(|p| self.borrows(p)),
            PatKind::Array { items, rest } => {
                items.iter().any(|p| self.borrows(p))
                    || matches!(rest, Some(Some(local)) if self.params[local.0 as usize].is_some())
            }
            PatKind::Newtype(inner) => self.borrows(inner),
            PatKind::Wildcard | PatKind::Literal(_) => false,
        }
    }

    /// Whether a pattern binding takes its part: it is not a loan and its
    /// type is not `Copy`.
    fn owns(&self, local: LocalId) -> bool {
        self.params[local.0 as usize].is_none() && !self.types.is_copy(self.local(local).ty)
    }

    /// How to make a value `Copy`, when a type parameter declared `~Copy`
    /// is what makes it affine rather than a task or a sender.
    fn copy_hint(&self, ty: Ty) -> &'static str {
        let mut params = Vec::new();
        self.types.params_in(ty, &mut params);
        if params.iter().any(|&p| !self.types.is_copy(p)) { ", or remove `~Copy` from its type parameter" } else { "" }
    }

    fn use_local(&mut self, local: LocalId, span: Span, flow: &Flow) {
        if let Some(state) = flow.moved.get(&local) {
            let local = self.local(local);
            let when = if *state == Moved::Maybe { " on some paths" } else { "" };
            let hint = match self.types.kind(local.ty) {
                TyKind::Handle(Handle::Sender, _) => format!(", or pass `{}.copy()` to keep sending here", local.name),
                _ => self.copy_hint(local.ty).into(),
            };
            let message = format!(
                "`{}` was moved{when} before this use, since its type {} is not `Copy`; use it before passing it on{hint}",
                local.name,
                self.display(local.ty),
            );
            self.error("E1401", span, message);
        }
    }

    /// A read of a capture the closure gave away is a use after a move.
    fn use_capture(&mut self, base: &Expr, index: u32, span: Span, flow: &Flow) {
        if let Some(state) = flow.captures.get(&index)
            && let Some(capture) = self.capture_name(base, index)
        {
            let when = if *state == Moved::Maybe { " on some paths" } else { "" };
            let message = format!(
                "the closure gave its capture `{capture}` away{when} before this use; use it before passing it on"
            );
            self.error("E1401", span, message);
        }
    }

    /// The name of the capture that `base.field` reads in a closure body,
    /// whose first parameter is the closure itself.
    fn capture_name(&self, base: &Expr, index: u32) -> Option<String> {
        let ExprKind::Local(local) = base.kind else { return None };
        if self.function.params.first() != Some(&local) || !self.types.is_closure(base.ty) {
            return None;
        }
        let TyKind::Adt(id) = self.types.kind(base.ty) else { return None };
        match &self.types.adt(*id).kind {
            crate::types::AdtKind::Record(fields) => Some(fields[index as usize].name.clone()),
            _ => None,
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
            ExprKind::Field(base, index) => {
                self.use_capture(base, *index, expr.span, &flow);
                self.observe(base, flow)
            }
            ExprKind::Retype(base) | ExprKind::RefValue(base) => self.observe(base, flow),
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
                "this copies a value of type {} out of `{}`, but it is not `Copy`; take values out with `pop()`, `removeAt()` or a `for` loop{}",
                self.display(expr.ty),
                self.local(root).name,
                self.copy_hint(expr.ty)
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
                        if !self.quiet {
                            self.moves.insert((*local, expr.span));
                        }
                    }
                }
                Some(flow)
            }
            // The closure gives a capture away, so a call consumes it; the
            // capture still holds its reference until the closure goes.
            ExprKind::Field(base, index) if !copy && self.capture_name(base, *index).is_some() => {
                self.use_capture(base, *index, expr.span, &flow);
                let mut flow = flow;
                flow.captures.insert(*index, Moved::Surely);
                self.gives = true;
                Some(flow)
            }
            ExprKind::Field(base, index) if !copy && Self::root(base).is_some() => {
                let root = Self::root(base).expect("checked above");
                let message = if self.params[root.0 as usize].is_some_and(|m| m != Mode::Value) {
                    format!(
                        "`{}` is a loan, so its part of type {} cannot be moved out of it",
                        self.local(root).name,
                        self.display(expr.ty)
                    )
                } else {
                    format!(
                        "moving a part of type {} out of `{}` would leave it incomplete; destructure the whole value with `let`{}",
                        self.display(expr.ty),
                        self.local(root).name,
                        self.copy_hint(expr.ty)
                    )
                };
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
            ExprKind::RefValue(base) if !copy => {
                let message = format!(
                    "a `Ref` keeps its value, so this value of type {} cannot be moved out of it; use it where it is, as in `reference.value.method()` or `inspect(ref reference.value)`{}",
                    self.display(expr.ty),
                    self.copy_hint(expr.ty)
                );
                self.error("E1406", expr.span, message);
                self.observe(base, flow)
            }
            ExprKind::RefValue(base) => self.observe(base, flow),
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
            ExprKind::Pointer(op, operands) => {
                // A written value moves into the memory.
                let mut flow = self.observe(&operands[0], flow)?;
                if let Some(value) = operands.get(1) {
                    flow = if *op == crate::hir::PointerOp::Write {
                        self.consume(value, flow)?
                    } else {
                        self.observe(value, flow)?
                    };
                }
                Some(flow)
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
            | ExprKind::ExternFn(_)
            | ExprKind::ContextRead(..)
            | ExprKind::Layout(..)
            | ExprKind::Callback(..)
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
                    let mut arm_flow = flow.clone();
                    self.bind(&arm.pattern, &mut arm_flow);
                    let mut end = match &arm.guard {
                        Some(guard) => self.consume(guard, arm_flow),
                        None => Some(arm_flow),
                    };
                    end = end.and_then(|f| self.consume(&arm.body, f));
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
            ExprKind::Block(block) | ExprKind::Recover(block, _) => self.block(block, flow),
            ExprKind::Within(limit, block, _) => {
                let flow = self.consume(limit, flow)?;
                self.block(block, flow)
            }
            ExprKind::Call(callee, _, args) => {
                let modes = (self.modes)(*callee);
                let taken =
                    args.iter().enumerate().map(|(i, arg)| (arg, matches!(modes.get(i), Some(Mode::Value) | None)));
                self.call_operands(taken, flow)
            }
            ExprKind::CallValue(callee, args) => {
                let modes = self.value_modes(callee.ty);
                let taken =
                    args.iter().enumerate().map(|(i, arg)| (arg, matches!(modes.get(i), Some(Mode::Value) | None)));
                let mut flow = self.call_operands(std::iter::once((&**callee, false)).chain(taken), flow)?;
                if self.once.contains(&callee.ty) || self.types.closure(callee.ty).is_some_and(|(_, _, info)| info.once)
                {
                    flow = self.call_once(callee, flow);
                }
                Some(flow)
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
            ExprKind::Try(inner) => self.consume(inner, flow),
            ExprKind::Convert(inner)
            | ExprKind::Panic(inner, _)
            | ExprKind::Erase(inner, _)
            | ExprKind::Spawn(inner) => self.consume(inner, flow),
            ExprKind::TestMembers(members) => {
                let mut flow = flow;
                for (_, member) in members {
                    flow = self.consume(member, flow)?;
                }
                Some(flow)
            }
            ExprKind::Fixed(inner) => self.observe(inner, flow),
            // A boxed value moves into the box; a box is only read.
            ExprKind::Reflect(op, _, values) => self.all(values, flow, *op == crate::hir::ReflectOp::Box),
            ExprKind::Select(arms, otherwise) => {
                // Every operation is evaluated first; each arm then takes
                // what its own operation takes.
                let mut flow = flow;
                for arm in arms {
                    for operand in Self::operands(&arm.operation) {
                        flow = self.observe(operand, flow)?;
                    }
                }
                let mut out = None;
                for arm in arms {
                    let mut arm_flow = flow.clone();
                    if let ExprKind::TaskOp(op, operands) = &arm.operation.kind {
                        self.quiet = true;
                        let taken = operands
                            .iter()
                            .enumerate()
                            .filter(|&(i, _)| op.takes(i))
                            .try_fold(arm_flow, |flow, (_, operand)| self.consume(operand, flow));
                        self.quiet = false;
                        arm_flow = taken?;
                    }
                    if let Some(pattern) = &arm.pattern {
                        self.bind(pattern, &mut arm_flow);
                    }
                    let end = self.consume(&arm.body, arm_flow);
                    out = merge(out, end);
                }
                if let Some(otherwise) = otherwise {
                    out = merge(out, self.consume(otherwise, flow));
                }
                out
            }
            ExprKind::TaskOp(op, operands) => {
                self.call_operands(operands.iter().enumerate().map(|(i, operand)| (operand, op.takes(i))), flow)
            }
            ExprKind::Assert { condition, parts, detail, .. } => {
                let flow = self.consume(condition, flow)?;
                let flow = self.all(parts, flow, false)?;
                match detail {
                    Some(detail) => self.observe(detail, flow),
                    None => Some(flow),
                }
            }
        };
        flow.filter(|_| expr.ty != Ty::NEVER)
    }

    /// The operands of a `select` operation.
    fn operands(operation: &Expr) -> &[Expr] {
        match &operation.kind {
            ExprKind::TaskOp(_, operands) | ExprKind::Call(_, _, operands) => operands,
            _ => &[],
        }
    }

    /// The parameter modes of a callable value of type `ty`.
    fn value_modes(&mut self, ty: Ty) -> Vec<Mode> {
        if let TyKind::Fn(_, _, _, modes, _) = self.types.kind(ty) {
            return modes.clone();
        }
        match self.types.closure(ty) {
            Some((_, _, info)) => info.modes,
            None => self.bound_modes.get(&ty).cloned().unwrap_or_default(),
        }
    }

    /// A call of a closure that gives a capture away consumes the closure
    /// (`CallOnce`); the binding keeps its reference until its scope ends.
    fn call_once(&mut self, callee: &Expr, mut flow: Flow) -> Flow {
        let Some(root) = Self::root(callee) else { return flow };
        if self.params[root.0 as usize].is_some_and(|m| m != Mode::Value) {
            let message = format!(
                "`{}` is a loan, and this closure gives a capture away, so calling it would consume it; take it by value",
                self.local(root).name
            );
            self.error("E1407", callee.span, message);
        } else {
            flow.moved.insert(root, Moved::Surely);
        }
        flow
    }

    /// Evaluates the operands of a call in order, taking those `take` says.
    /// One that moves a binding an earlier operand lends is an error: the
    /// call reads the loan when it runs, after the move.
    fn call_operands<'e>(&mut self, operands: impl IntoIterator<Item = (&'e Expr, bool)>, flow: Flow) -> Option<Flow> {
        let mut flow = flow;
        let mut lent: Vec<LocalId> = Vec::new();
        for (operand, take) in operands {
            if !take {
                lent.extend(Self::root(operand));
                flow = self.observe(operand, flow)?;
                continue;
            }
            let before: Vec<bool> = lent.iter().map(|local| flow.moved.contains_key(local)).collect();
            flow = self.consume(operand, flow)?;
            let moved = lent.iter().zip(before).find(|&(local, was)| !was && flow.moved.contains_key(local));
            if let Some((&local, _)) = moved {
                let local = self.local(local);
                let hint = match self.types.kind(local.ty) {
                    TyKind::Handle(Handle::Sender, _) => format!(": pass `{}.copy()` instead", local.name),
                    _ => self.copy_hint(local.ty).into(),
                };
                let message = format!(
                    "`{}` moves into this call, which also lends it and would use it after the move{hint}",
                    local.name
                );
                self.error("E1406", operand.span, message);
            }
        }
        Some(flow)
    }

    fn all(&mut self, values: &[Expr], flow: Flow, take: bool) -> Option<Flow> {
        let mut flow = flow;
        for value in values {
            flow = if take { self.consume(value, flow)? } else { self.observe(value, flow)? };
        }
        Some(flow)
    }
}
