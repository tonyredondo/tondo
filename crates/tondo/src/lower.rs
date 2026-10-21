//! HIR to MIR lowering.
//!
//! Ownership of reference-counted values follows three rules:
//! - parameters are borrowed: the caller keeps them alive during the call;
//! - results of calls, string construction and aggregates are owned
//!   temporaries, released when the enclosing statement ends unless moved
//!   into a binding or another aggregate;
//! - bindings own their value and release it when their scope ends, and on
//!   every early exit (`return`, `break`, `continue`) that leaves the scope.
//!
//! Each conditionally evaluated region (a branch, the right side of
//! `and`/`or`) gets its own temporary frame, so cleanup never touches a
//! temporary that was not initialized on the current path.

use crate::ast::BinOp;
use crate::hir::{self, LocalId};
use crate::mir::*;

/// A place a call lends, with the region of it when it lends one.
type Lent = (Place, Option<[Option<Operand>; 3]>);

/// A region a call borrows: the argument it is, the array place and bounds
/// it selects, and the copy the callee changes.
struct RegionCopy {
    index: usize,
    place: Place,
    bounds: [Option<Operand>; 3],
    ty: Ty,
    span: Span,
    copy: Option<Operand>,
}
use crate::source::Span;
use crate::types::{CursorKind, ERR, NONE, OK, SOME, Ty, TyKind, Types};
use std::rc::Rc;

pub fn lower(program: &hir::Program) -> Vec<Body> {
    program.functions.iter().map(|f| lower_function(&program.types, f)).collect()
}

struct LoopCx {
    break_to: BlockId,
    continue_to: BlockId,
    scope_depth: usize,
    frame_depth: usize,
}

/// One action of leaving a scope.
#[derive(Clone)]
enum Exit {
    Release(Local),
    /// The call of a registered `defer` (13.7).
    Defer(Rc<hir::Expr>),
}

struct Builder<'a> {
    types: &'a Types,
    function: &'a hir::Function,
    locals: Vec<Ty>,
    blocks: Vec<(Vec<Statement>, Option<Terminator>)>,
    current: BlockId,
    hir_locals: Vec<Option<Local>>,
    /// What leaving each lexical scope does, in reverse order: release
    /// owned bindings and run `defer` cleanups.
    scopes: Vec<Vec<Exit>>,
    /// Owned temporaries per statement or conditional region.
    frames: Vec<Vec<Local>>,
    loops: Vec<LoopCx>,
    ret: Option<Local>,
}

fn lower_function(types: &Types, function: &hir::Function) -> Body {
    let mut b = Builder {
        types,
        function,
        locals: Vec::new(),
        blocks: Vec::new(),
        current: BlockId(0),
        hir_locals: vec![None; function.locals.len()],
        scopes: Vec::new(),
        frames: vec![Vec::new()],
        loops: Vec::new(),
        ret: None,
    };
    b.current = b.new_block();
    let params = function.params.iter().map(|&p| b.bind(p)).collect();
    if function.result != Ty::UNIT && function.result != Ty::NEVER {
        b.ret = Some(b.new_local(function.result));
    }
    let value = b.block(&function.body);
    if function.body.ty != Ty::NEVER {
        b.emit_return(value);
    }
    let blocks = b
        .blocks
        .into_iter()
        .map(|(stmts, terminator)| BlockData { stmts, terminator: terminator.unwrap_or(Terminator::Unreachable) })
        .collect();
    Body {
        name: function.name.clone(),
        locals: b.locals,
        params,
        loans: function.loans.clone(),
        ret: b.ret,
        result: function.result,
        blocks,
    }
}

impl Builder<'_> {
    fn needs_drop(&self, ty: Ty) -> bool {
        self.types.needs_drop(ty)
    }

    fn place_ty(&self, place: &Place) -> Ty {
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &proj| project(self.types, ty, proj))
    }

    fn new_local(&mut self, ty: Ty) -> Local {
        self.locals.push(ty);
        Local(self.locals.len() as u32 - 1)
    }

    fn bind(&mut self, id: LocalId) -> Local {
        if let Some(local) = self.hir_locals[id.0 as usize] {
            return local;
        }
        let local = self.new_local(self.function.locals[id.0 as usize].ty);
        self.hir_locals[id.0 as usize] = Some(local);
        local
    }

    fn new_block(&mut self) -> BlockId {
        self.blocks.push((Vec::new(), None));
        BlockId(self.blocks.len() as u32 - 1)
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current = block;
    }

    fn push(&mut self, stmt: Statement) {
        self.blocks[self.current.0 as usize].0.push(stmt);
    }

    /// Ends the current block. Code lowered afterwards lands in a fresh,
    /// unreachable block until the caller switches elsewhere.
    fn terminate(&mut self, terminator: Terminator) {
        self.blocks[self.current.0 as usize].1 = Some(terminator);
        self.current = self.new_block();
    }

    fn goto(&mut self, target: BlockId) {
        self.terminate(Terminator::Goto(target));
    }

    fn assign(&mut self, place: Place, rvalue: Rvalue, span: Span) {
        self.push(Statement::Assign(place, Box::new(rvalue), span));
    }

    fn own(&mut self, local: Local) {
        if self.needs_drop(self.locals[local.0 as usize]) {
            self.frames.last_mut().unwrap().push(local);
        }
    }

    /// Evaluates `rvalue` into a new temporary; owned results join the frame.
    fn temp(&mut self, ty: Ty, rvalue: Rvalue, span: Span) -> Operand {
        // Everything but a copy of a place produces a new value.
        let owned = !matches!(rvalue, Rvalue::Use(_));
        let local = self.new_local(ty);
        self.assign(Place::local(local), rvalue, span);
        if owned {
            self.own(local);
        }
        Operand::local(local)
    }

    /// Whether `value` is a whole temporary owned by the current frame, which
    /// can then be moved instead of retained. Removes it from the frame.
    fn take_temporary(&mut self, value: &Operand) -> bool {
        let Operand::Copy(Place { local, path }) = value else { return false };
        if !path.is_empty() {
            return false;
        }
        let frame = self.frames.last_mut().unwrap();
        let index = frame.iter().position(|l| l == local);
        index.map(|i| frame.remove(i)).is_some()
    }

    /// Stores `value` into `dst`, taking ownership: a temporary of the
    /// current frame is moved, anything else is retained.
    fn store(&mut self, dst: Place, value: Operand, span: Span) {
        let counted = self.needs_drop(self.place_ty(&dst));
        let moved = counted && self.take_temporary(&value);
        let copied = matches!(value, Operand::Copy(_));
        self.assign(dst.clone(), Rvalue::Use(value), span);
        if counted && copied && !moved {
            self.push(Statement::Retain(dst));
        }
    }

    /// Builds an aggregate into a new owned temporary. Each component is
    /// moved if it is a temporary of the frame, and retained otherwise.
    fn aggregate(&mut self, ty: Ty, components: Vec<Operand>, span: Span) -> Operand {
        let types = self.types.components(ty).expect("aggregate type");
        self.construct(ty, None, types, components, span)
    }

    fn variant(&mut self, ty: Ty, variant: u32, components: Vec<Operand>, span: Span) -> Operand {
        let types = self.types.variants(ty).expect("enum type")[variant as usize].clone();
        self.construct(ty, Some(variant), types, components, span)
    }

    fn construct(
        &mut self,
        ty: Ty,
        variant: Option<u32>,
        types: Vec<Ty>,
        components: Vec<Operand>,
        span: Span,
    ) -> Operand {
        let mut retains = Vec::new();
        for ((index, component), &component_ty) in components.iter().enumerate().zip(&types) {
            let copied = matches!(component, Operand::Copy(_));
            if self.needs_drop(component_ty) && copied && !self.take_temporary(component) {
                retains.push(index as u32);
            }
        }
        let local = self.new_local(ty);
        let place = Place::local(local);
        let rvalue = match variant {
            Some(variant) => Rvalue::Variant(variant, components),
            None => Rvalue::Aggregate(components),
        };
        self.assign(place.clone(), rvalue, span);
        for index in retains {
            let component = match variant {
                Some(variant) => place.variant_field(variant, index),
                None => place.field(index),
            };
            self.push(Statement::Retain(component));
        }
        self.own(local);
        Operand::local(local)
    }

    /// A place holding `value`; constants are materialized in a temporary.
    fn place_of(&mut self, value: Operand, ty: Ty) -> Place {
        match value {
            Operand::Copy(place) => place,
            constant => {
                let local = self.new_local(ty);
                self.assign(Place::local(local), Rvalue::Use(constant), Span::default());
                Place::local(local)
            }
        }
    }

    /// Branches to `fail` unless the value at `place` matches `pattern`.
    fn test_pattern(&mut self, pattern: &hir::Pattern, place: &Place, fail: BlockId) {
        match &pattern.kind {
            hir::PatKind::Wildcard | hir::PatKind::Binding(_) | hir::PatKind::Literal(hir::Literal::Unit) => {}
            hir::PatKind::Literal(literal) => {
                let constant = match literal {
                    hir::Literal::Int(value) => Const::Int(*value, pattern.ty),
                    hir::Literal::Float(bits) => Const::Float(*bits, pattern.ty),
                    hir::Literal::Char(c) => Const::Char(*c),
                    hir::Literal::Bool(value) => Const::Bool(*value),
                    hir::Literal::Str(text) => Const::Str(text.clone()),
                    hir::Literal::Unit => unreachable!(),
                };
                let rvalue = Rvalue::Binary(BinOp::Eq, Operand::Copy(place.clone()), Operand::Const(constant));
                self.branch_on(rvalue, pattern.span, fail);
            }
            hir::PatKind::Fields(fields) => {
                for (index, field) in fields.iter().enumerate() {
                    self.test_pattern(field, &place.field(index as u32), fail);
                }
            }
            hir::PatKind::Variant(variant, fields) => {
                if self.types.variants(pattern.ty).is_some_and(|v| v.len() > 1) {
                    let tag_ty = self.types.tag_ty(pattern.ty);
                    let tag = self.temp(tag_ty, Rvalue::Discriminant(place.clone()), pattern.span);
                    let expected = Operand::Const(Const::Int(*variant as i128, tag_ty));
                    self.branch_on(Rvalue::Binary(BinOp::Eq, tag, expected), pattern.span, fail);
                }
                for (index, field) in fields.iter().enumerate() {
                    self.test_pattern(field, &place.variant_field(*variant, index as u32), fail);
                }
            }
            hir::PatKind::Newtype(inner) => self.test_pattern(inner, place, fail),
            hir::PatKind::Array { items, rest } => {
                let length = self.temp(Ty::INT, Rvalue::Length(Operand::Copy(place.clone())), pattern.span);
                let compare = if rest.is_some() { BinOp::Ge } else { BinOp::Eq };
                let count = Operand::Const(Const::Int(items.len() as i128, Ty::INT));
                self.branch_on(Rvalue::Binary(compare, length, count), pattern.span, fail);
                for (index, item) in items.iter().enumerate() {
                    let element = self.element(place, index, pattern.span);
                    self.test_pattern(item, &element, fail);
                }
            }
        }
    }

    /// The element at a constant position of the array at `place`.
    fn element(&mut self, place: &Place, index: usize, span: Span) -> Place {
        let local = self.new_local(Ty::INT);
        self.assign(Place::local(local), Rvalue::Use(Operand::Const(Const::Int(index as i128, Ty::INT))), span);
        place.index(local, span)
    }

    fn branch_on(&mut self, condition: Rvalue, span: Span, fail: BlockId) {
        let matched = self.temp(Ty::BOOL, condition, span);
        let next = self.new_block();
        self.terminate(Terminator::Branch(matched, next, fail));
        self.switch_to(next);
    }

    /// Copies the matched components into the pattern's bindings, which the
    /// current scope then owns.
    fn bind_pattern(&mut self, pattern: &hir::Pattern, place: &Place) {
        match &pattern.kind {
            hir::PatKind::Wildcard | hir::PatKind::Literal(_) => {}
            hir::PatKind::Binding(id) => {
                let local = self.bind(*id);
                self.store(Place::local(local), Operand::Copy(place.clone()), pattern.span);
                if self.needs_drop(self.locals[local.0 as usize]) {
                    self.scopes.last_mut().unwrap().push(Exit::Release(local));
                }
            }
            hir::PatKind::Fields(fields) => {
                for (index, field) in fields.iter().enumerate() {
                    self.bind_pattern(field, &place.field(index as u32));
                }
            }
            hir::PatKind::Variant(variant, fields) => {
                for (index, field) in fields.iter().enumerate() {
                    self.bind_pattern(field, &place.variant_field(*variant, index as u32));
                }
            }
            hir::PatKind::Newtype(inner) => self.bind_pattern(inner, place),
            hir::PatKind::Array { items, rest } => {
                for (index, item) in items.iter().enumerate() {
                    let element = self.element(place, index, pattern.span);
                    self.bind_pattern(item, &element);
                }
                if let Some(Some(id)) = rest {
                    // The remainder is a new array (14.3).
                    let local = self.bind(*id);
                    let start = Some(Operand::Const(Const::Int(items.len() as i128, Ty::INT)));
                    let remainder = Rvalue::Slice(Operand::Copy(place.clone()), [start, None, None], pattern.span);
                    self.assign(Place::local(local), remainder, pattern.span);
                    self.scopes.last_mut().unwrap().push(Exit::Release(local));
                }
            }
        }
    }

    fn match_expr(&mut self, e: &hir::Expr, scrutinee: &hir::Expr, arms: &[hir::Arm]) -> Operand {
        let value = self.expr(scrutinee);
        let place = self.place_of(value, scrutinee.ty);
        let has_value = e.ty != Ty::UNIT && e.ty != Ty::NEVER;
        let result = has_value.then(|| self.new_local(e.ty));
        let join = self.new_block();
        for arm in arms {
            let next = self.new_block();
            let scope_depth = self.scopes.len();
            self.scopes.push(Vec::new());
            self.test_pattern(&arm.pattern, &place, next);
            self.bind_pattern(&arm.pattern, &place);
            if let Some(guard) = &arm.guard {
                let passed = self.in_frame(|b| b.expr(guard));
                let (body, failed) = (self.new_block(), self.new_block());
                self.terminate(Terminator::Branch(passed, body, failed));
                // A false guard releases the arm's bindings before trying the next arm.
                self.switch_to(failed);
                let frames = self.frames.len();
                self.cleanup_to(scope_depth, frames);
                self.goto(next);
                self.switch_to(body);
            }
            self.in_frame(|b| {
                let value = b.expr(&arm.body);
                if let Some(result) = result.filter(|_| arm.body.ty != Ty::NEVER) {
                    b.store(Place::local(result), value, e.span);
                }
            });
            let scope = self.scopes.pop().unwrap();
            self.leave(scope);
            self.goto(join);
            self.switch_to(next);
        }
        // Exhaustiveness was checked; no value reaches past the last arm.
        self.terminate(Terminator::Unreachable);
        self.switch_to(join);
        match result {
            Some(result) => {
                self.own(result);
                Operand::local(result)
            }
            None => Operand::Const(Const::Unit),
        }
    }

    /// Leaves a scope: its exits run in reverse order of registration.
    fn leave(&mut self, scope: Vec<Exit>) {
        for exit in scope.into_iter().rev() {
            self.exit_action(exit);
        }
    }

    fn exit_action(&mut self, exit: Exit) {
        match exit {
            Exit::Release(local) => self.push(Statement::Release(Place::local(local))),
            Exit::Defer(call) => {
                // Unregister it first, so a panic in the cleanup cannot run it again.
                self.push(Statement::DeferPop);
                self.in_frame(|b| {
                    b.expr(&call);
                });
            }
        }
    }

    fn release_all(&mut self, locals: Vec<Local>) {
        for local in locals.into_iter().rev() {
            self.push(Statement::Release(Place::local(local)));
        }
    }

    /// Releases everything owned by scopes and frames deeper than the given
    /// depths, without popping them: lowering continues structurally.
    fn cleanup_to(&mut self, scope_depth: usize, frame_depth: usize) {
        let frames: Vec<Local> =
            self.frames[frame_depth..].iter().rev().flat_map(|f| f.iter().rev().copied()).collect();
        let scopes: Vec<Exit> = self.scopes[scope_depth..].iter().rev().flat_map(|s| s.iter().rev().cloned()).collect();
        for local in frames {
            self.push(Statement::Release(Place::local(local)));
        }
        for exit in scopes {
            self.exit_action(exit);
        }
    }

    /// Whether the function is fallible (`T ! E`, 9.8).
    fn fallible(&self) -> bool {
        self.function.fallible
    }

    /// The type that `return` takes: the success type of a fallible function.
    fn success_ty(&self) -> Ty {
        match self.types.kind(self.function.result) {
            TyKind::Result(ok, _) if self.fallible() => *ok,
            _ => self.function.result,
        }
    }

    /// An error that `?` propagates, as a value of the error union the
    /// function fails with when it is a member or a smaller union (15.3).
    fn widen(&mut self, error: Operand, from: Ty, span: Span) -> Operand {
        let TyKind::Result(_, to) = *self.types.kind(self.function.result) else { unreachable!() };
        if from == to {
            return error;
        }
        let members = self.types.members(to).expect("the checker allows only widening").to_vec();
        let index = |member: Ty| members.iter().position(|&m| m == member).expect("a member of the union") as u32;
        if let Some(from_members) = self.types.members(from).map(<[Ty]>::to_vec) {
            // Re-tag each member of the smaller union.
            let source = self.place_of(error, from);
            let result = self.new_local(to);
            let tag_ty = self.types.tag_ty(from);
            let tag = self.temp(tag_ty, Rvalue::Discriminant(source.clone()), span);
            let done = self.new_block();
            for (variant, &member) in from_members.iter().enumerate() {
                let (this, next) = (self.new_block(), self.new_block());
                let test = Rvalue::Binary(BinOp::Eq, tag.clone(), Operand::Const(Const::Int(variant as i128, tag_ty)));
                let test = self.temp(Ty::BOOL, test, span);
                self.terminate(Terminator::Branch(test, this, next));
                self.switch_to(this);
                let payload = Operand::Copy(source.variant_field(variant as u32, 0));
                let value = self.variant(to, index(member), vec![payload], span);
                self.store(Place::local(result), value, span);
                self.goto(done);
                self.switch_to(next);
            }
            self.terminate(Terminator::Unreachable);
            self.switch_to(done);
            self.own(result);
            return Operand::local(result);
        }
        self.variant(to, index(from), vec![error], span)
    }

    /// `next` of an intrinsic cursor (10.17): `some` of the entry, key or
    /// value at its position, which then advances, or `none` at the end.
    fn cursor_next(&mut self, cursor: &hir::Place, kind: CursorKind, e: &hir::Expr) -> Operand {
        let cursor = self.place(cursor.local, &cursor.path);
        let (collection, position) = (cursor.field(0), cursor.field(1));
        let length = self.temp(Ty::INT, Rvalue::Length(Operand::Copy(collection.clone())), e.span);
        let more = Rvalue::Binary(BinOp::Lt, Operand::Copy(position.clone()), length);
        let more = self.temp(Ty::BOOL, more, e.span);
        let result = self.new_local(e.ty);
        let (found, done, join) = (self.new_block(), self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(more, found, done));
        self.switch_to(found);
        let index = self.new_local(Ty::INT);
        self.assign(Place::local(index), Rvalue::Use(Operand::Copy(position.clone())), e.span);
        let element = collection.index(index, e.span);
        let is_map = matches!(self.types.kind(self.place_ty(&collection)), TyKind::Map(..));
        let value = match (kind, is_map) {
            (CursorKind::Keys, true) => element.field(0),
            (CursorKind::Values, true) => element.field(1),
            _ => element,
        };
        let some = self.variant(e.ty, SOME, vec![Operand::Copy(value)], e.span);
        self.store(Place::local(result), some, e.span);
        self.assign(position.clone(), Rvalue::Increment(Operand::Copy(position)), e.span);
        self.goto(join);
        self.switch_to(done);
        let none = self.variant(e.ty, NONE, Vec::new(), e.span);
        self.store(Place::local(result), none, e.span);
        self.goto(join);
        self.switch_to(join);
        self.own(result);
        Operand::local(result)
    }

    /// A copy of `value` in a new temporary of the frame.
    fn copy_operand(&mut self, value: Operand, ty: Ty, span: Span) -> Operand {
        let local = self.new_local(ty);
        self.assign(Place::local(local), Rvalue::Use(value), span);
        if self.needs_drop(ty) {
            self.push(Statement::Retain(Place::local(local)));
            self.own(local);
        }
        Operand::local(local)
    }

    /// The place with its indices copied, so that later changes to the
    /// bindings that held them do not move it.
    fn frozen(&mut self, place: Place, span: Span) -> Place {
        let mut path = Vec::new();
        for step in place.path {
            path.push(match step {
                Proj::Index(index, at) => {
                    let copy = self.new_local(Ty::INT);
                    self.assign(Place::local(copy), Rvalue::Use(Operand::local(index)), span);
                    Proj::Index(copy, at)
                }
                step => step,
            });
        }
        Place { local: place.local, path }
    }

    /// Exclusive loans of one call that select elements of the same array
    /// by run-time indices or regions must reach different elements
    /// (`P0004`). The checker already rejected loans that overlap for sure.
    fn check_disjoint(&mut self, loans: &[Lent], span: Span) {
        for (index, (first, first_region)) in loans.iter().enumerate() {
            for (second, second_region) in &loans[index + 1..] {
                if first.local != second.local {
                    continue;
                }
                let shared = first.path.iter().zip(&second.path).take_while(|(a, b)| a == b).count();
                let (region, other, array) = match (first.path.get(shared), second.path.get(shared)) {
                    (Some(Proj::Index(..)), Some(Proj::Index(..))) => (None, None, None),
                    (None, None) => match (first_region, second_region) {
                        (Some(a), Some(b)) => (Some(a), Some(Overlap::Region(b.clone())), Some(first)),
                        _ => continue,
                    },
                    (None, Some(Proj::Index(b, _))) if first_region.is_some() => {
                        (first_region.as_ref(), Some(Overlap::Index(Operand::local(*b))), Some(first))
                    }
                    (Some(Proj::Index(a, _)), None) if second_region.is_some() => {
                        (second_region.as_ref(), Some(Overlap::Index(Operand::local(*a))), Some(second))
                    }
                    _ => continue,
                };
                if let (Some(region), Some(other), Some(array)) = (region, other, array) {
                    let check = Rvalue::CheckRegion(Operand::Copy(array.clone()), region.clone(), other, span);
                    self.temp(Ty::UNIT, check, span);
                    continue;
                }
                let (Some(Proj::Index(a, _)), Some(Proj::Index(b, _))) =
                    (first.path.get(shared), second.path.get(shared))
                else {
                    continue;
                };
                let array = Place { local: first.local, path: first.path[..shared].to_vec() };
                let length = self.temp(Ty::INT, Rvalue::Length(Operand::Copy(array)), span);
                let a = self.position(Operand::local(*a), length.clone(), span);
                let b = self.position(Operand::local(*b), length, span);
                let same = Rvalue::Binary(BinOp::Eq, Operand::local(a), Operand::local(b));
                let same = self.temp(Ty::BOOL, same, span);
                let (overlap, next) = (self.new_block(), self.new_block());
                self.terminate(Terminator::Branch(same, overlap, next));
                self.switch_to(overlap);
                self.terminate(Terminator::Panic { code: 4, message: None, span });
                self.switch_to(next);
            }
        }
    }

    /// An index as a position, counting from the end when negative (10.3).
    fn position(&mut self, index: Operand, length: Operand, span: Span) -> Local {
        let result = self.new_local(Ty::INT);
        self.assign(Place::local(result), Rvalue::Use(index.clone()), span);
        let zero = Operand::Const(Const::Int(0, Ty::INT));
        let negative = self.temp(Ty::BOOL, Rvalue::Binary(BinOp::Lt, index.clone(), zero), span);
        let (from_end, done) = (self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(negative, from_end, done));
        self.switch_to(from_end);
        self.assign(Place::local(result), Rvalue::Binary(BinOp::Add, index, length), span);
        self.goto(done);
        self.switch_to(done);
        result
    }

    /// Returns `value`, as `ok(value)` from a fallible function.
    fn emit_return(&mut self, value: Operand) {
        let value =
            if self.fallible() { self.variant(self.function.result, OK, vec![value], Span::default()) } else { value };
        self.exit(value);
    }

    /// Returns `err(error)` from a fallible function.
    fn emit_fail(&mut self, error: Operand) {
        let value = self.variant(self.function.result, ERR, vec![error], Span::default());
        self.exit(value);
    }

    fn exit(&mut self, value: Operand) {
        let operand = match self.ret {
            Some(ret) => {
                self.store(Place::local(ret), value, Span::default());
                Operand::local(ret)
            }
            None => Operand::Const(Const::Unit),
        };
        self.cleanup_to(0, 0);
        self.terminate(Terminator::Return(operand));
    }

    /// Runs `f` in a fresh temporary frame that is released before returning.
    fn in_frame<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.frames.push(Vec::new());
        let result = f(self);
        let frame = self.frames.pop().unwrap();
        self.release_all(frame);
        result
    }

    // ---- blocks and statements --------------------------------------------

    fn block(&mut self, block: &hir::Block) -> Operand {
        self.scopes.push(Vec::new());
        for stmt in &block.stmts {
            self.in_frame(|b| b.stmt(stmt));
        }
        let mut value = match &block.tail {
            Some(tail) => self.expr(tail),
            None => Operand::Const(Const::Unit),
        };
        // Leaving the scope releases its bindings and runs its cleanups, so
        // a value read from a place is copied first into a temporary of the
        // enclosing frame.
        if let Operand::Copy(_) = &value
            && !self.scopes.last().unwrap().is_empty()
        {
            let copy = self.new_local(block.ty);
            self.assign(Place::local(copy), Rvalue::Use(value), Span::default());
            if self.needs_drop(block.ty) {
                self.push(Statement::Retain(Place::local(copy)));
                self.frames.last_mut().unwrap().push(copy);
            }
            value = Operand::local(copy);
        }
        let scope = self.scopes.pop().unwrap();
        self.leave(scope);
        value
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::Let(id, value) => {
                let value = self.expr(value);
                let local = self.bind(*id);
                self.store(Place::local(local), value, Span::default());
                if self.needs_drop(self.locals[local.0 as usize]) {
                    self.scopes.last_mut().unwrap().push(Exit::Release(local));
                }
            }
            hir::Stmt::Lend { local, place } => {
                let place = self.place(place.local, &place.path);
                let local = self.bind(*local);
                self.push(Statement::Lend(local, place));
            }
            hir::Stmt::Destructure(pattern, value) => {
                let value = self.expr(value);
                let place = self.place_of(value, pattern.ty);
                self.bind_pattern(pattern, &place);
            }
            hir::Stmt::Assign { local, path, op, value, span } => {
                // The location (and its indices) is resolved before the value (13.8).
                let place = self.place(*local, path);
                let value = self.expr(value);
                let ty = self.place_ty(&place);
                let rvalue = match op {
                    Some(op) => Rvalue::Binary(*op, Operand::Copy(place.clone()), value),
                    None => Rvalue::Use(value),
                };
                if self.needs_drop(ty) {
                    // Keep the old value until the new one is stored, so
                    // `x = x` and `x = f(x.field)` stay valid. A compound
                    // assignment (lifted array arithmetic) builds a new value.
                    let value = match rvalue {
                        Rvalue::Use(value) => value,
                        rvalue => self.temp(ty, rvalue, *span),
                    };
                    let old = self.new_local(ty);
                    self.assign(Place::local(old), Rvalue::Use(Operand::Copy(place.clone())), *span);
                    self.store(place, value, *span);
                    self.push(Statement::Release(Place::local(old)));
                } else {
                    self.assign(place, rvalue, *span);
                }
            }
            hir::Stmt::AssignSlice { local, path, bounds, op, value, span } => {
                let place = self.place(*local, path);
                let bounds = bounds.each_ref().map(|bound| bound.as_ref().map(|b| self.expr(b)));
                let value = self.expr(value);
                let ty = self.place_ty(&place);
                let value = match op {
                    Some(op) => {
                        let current = Rvalue::Slice(Operand::Copy(place.clone()), bounds.clone(), *span);
                        let current = self.temp(ty, current, *span);
                        self.temp(ty, Rvalue::Binary(*op, current, value), *span)
                    }
                    None => value,
                };
                self.temp(Ty::UNIT, Rvalue::AssignSlice(place, bounds, value, *span), *span);
            }
            hir::Stmt::Expr(value) => {
                self.expr(value);
            }
            hir::Stmt::Defer { local, closure, call } => {
                // The closure is stored now; the runtime keeps its body and
                // address to run it if a panic unwinds this scope (15.7).
                let value = self.expr(closure);
                let local = self.bind(*local);
                self.store(Place::local(local), value, closure.span);
                if self.needs_drop(closure.ty) {
                    self.scopes.last_mut().unwrap().push(Exit::Release(local));
                }
                let hir::ExprKind::Call(hir::Callee::Fn(body), _, _) = &call.kind else {
                    unreachable!("monomorphization calls a deferred closure directly")
                };
                self.push(Statement::DeferPush(*body, Place::local(local)));
                self.scopes.last_mut().unwrap().push(Exit::Defer(Rc::new(call.clone())));
            }
            hir::Stmt::Return(value) => {
                let value = value.as_ref().map_or(Operand::Const(Const::Unit), |v| self.expr(v));
                self.emit_return(value);
            }
            hir::Stmt::Fail(error) => {
                let error = self.expr(error);
                self.emit_fail(error);
            }
            hir::Stmt::Break | hir::Stmt::Continue => {
                let lp = self.loops.last().unwrap();
                let target = if matches!(stmt, hir::Stmt::Break) { lp.break_to } else { lp.continue_to };
                let (scope_depth, frame_depth) = (lp.scope_depth, lp.frame_depth);
                self.cleanup_to(scope_depth, frame_depth);
                self.goto(target);
            }
            hir::Stmt::Loop { condition, body } => {
                let head = self.new_block();
                let body_block = self.new_block();
                let exit = self.new_block();
                self.goto(head);
                self.switch_to(head);
                match condition {
                    Some(condition) => {
                        let value = self.in_frame(|b| b.expr(condition));
                        self.terminate(Terminator::Branch(value, body_block, exit));
                    }
                    None => self.goto(body_block),
                }
                self.switch_to(body_block);
                self.loop_body(body, exit, head);
                self.switch_to(exit);
            }
            hir::Stmt::ForEach { pattern, source: array, body } => {
                // Iterate a snapshot: the loop sees the collection as it was
                // when the loop started, even if the body changes the binding.
                let value = self.expr(array);
                let snapshot = self.new_local(array.ty);
                self.store(Place::local(snapshot), value, array.span);
                self.frames.last_mut().unwrap().push(snapshot);
                // A string is walked by byte offsets, one scalar at a time.
                let text = array.ty == Ty::STRING;
                let length = if text {
                    Rvalue::StrOp(hir::StrOp::ByteLength, vec![Operand::local(snapshot)], array.span)
                } else {
                    Rvalue::Length(Operand::local(snapshot))
                };
                let length = self.temp(Ty::INT, length, array.span);
                let cursor = self.new_local(Ty::INT);
                let zero = Rvalue::Use(Operand::Const(Const::Int(0, Ty::INT)));
                self.assign(Place::local(cursor), zero, array.span);
                let (head, body_block, step, exit) =
                    (self.new_block(), self.new_block(), self.new_block(), self.new_block());
                self.goto(head);
                self.switch_to(head);
                let more = Rvalue::Binary(BinOp::Lt, Operand::local(cursor), length);
                let more = self.temp(Ty::BOOL, more, array.span);
                self.terminate(Terminator::Branch(more, body_block, exit));
                self.switch_to(body_block);
                self.loops.push(LoopCx {
                    break_to: exit,
                    continue_to: step,
                    scope_depth: self.scopes.len(),
                    frame_depth: self.frames.len(),
                });
                // The element bindings belong to one iteration.
                self.scopes.push(Vec::new());
                let element = if text {
                    let operands = vec![Operand::local(snapshot), Operand::local(cursor)];
                    let scalar =
                        self.temp(Ty::CHAR, Rvalue::StrOp(hir::StrOp::Decode, operands, array.span), array.span);
                    let Operand::Copy(place) = scalar else { unreachable!() };
                    place
                } else {
                    Place::local(snapshot).index(cursor, array.span)
                };
                self.bind_pattern(pattern, &element);
                self.in_frame(|b| b.block(body));
                let scope = self.scopes.pop().unwrap();
                self.leave(scope);
                self.loops.pop();
                self.goto(step);
                self.switch_to(step);
                let next = if text {
                    let operands = vec![Operand::local(snapshot), Operand::local(cursor)];
                    Rvalue::StrOp(hir::StrOp::Next, operands, array.span)
                } else {
                    Rvalue::Increment(Operand::local(cursor))
                };
                self.assign(Place::local(cursor), next, array.span);
                self.goto(head);
                self.switch_to(exit);
            }
            hir::Stmt::ForRange { local, start, end, inclusive, body } => {
                let ty = start.ty;
                let start = self.expr(start);
                let end = self.expr(end);
                let cursor = self.new_local(ty);
                self.assign(Place::local(cursor), Rvalue::Use(start), Span::default());
                let last = self.new_local(ty);
                self.assign(Place::local(last), Rvalue::Use(end), Span::default());
                let (head, body_block, step, exit) =
                    (self.new_block(), self.new_block(), self.new_block(), self.new_block());
                self.goto(head);
                self.switch_to(head);
                let compare = if *inclusive { BinOp::Le } else { BinOp::Lt };
                let more = self.temp(
                    Ty::BOOL,
                    Rvalue::Binary(compare, Operand::local(cursor), Operand::local(last)),
                    Span::default(),
                );
                self.terminate(Terminator::Branch(more, body_block, exit));
                self.switch_to(body_block);
                let binding = self.bind(*local);
                self.assign(Place::local(binding), Rvalue::Use(Operand::local(cursor)), Span::default());
                self.loop_body(body, exit, step);
                // The successor is only computed while `cursor < last`, so the
                // increment cannot overflow even at the type's maximum.
                self.switch_to(step);
                if *inclusive {
                    let done = self.temp(
                        Ty::BOOL,
                        Rvalue::Binary(BinOp::Eq, Operand::local(cursor), Operand::local(last)),
                        Span::default(),
                    );
                    let next = self.new_block();
                    self.terminate(Terminator::Branch(done, exit, next));
                    self.switch_to(next);
                }
                self.assign(Place::local(cursor), Rvalue::Increment(Operand::local(cursor)), Span::default());
                self.goto(head);
                self.switch_to(exit);
            }
        }
    }

    /// For operands that move into a collection: which must be retained
    /// because they were copied rather than taken from a temporary.
    fn move_flags(&mut self, operands: &[Operand], moves: &[bool], types: &[Ty]) -> Vec<bool> {
        operands
            .iter()
            .zip(moves)
            .zip(types)
            .map(|((operand, &moves), &ty)| {
                moves && self.needs_drop(ty) && matches!(operand, Operand::Copy(_)) && !self.take_temporary(operand)
            })
            .collect()
    }

    /// A local holding the value of an index expression.
    fn index_local(&mut self, index: &hir::Expr) -> Local {
        match self.expr(index) {
            Operand::Copy(Place { local, path }) if path.is_empty() => local,
            value => {
                let local = self.new_local(Ty::INT);
                self.assign(Place::local(local), Rvalue::Use(value), index.span);
                local
            }
        }
    }

    fn place(&mut self, local: LocalId, path: &[hir::PlaceStep]) -> Place {
        let mut place = Place::local(self.bind(local));
        for step in path {
            place = match step {
                hir::PlaceStep::Field(index) => place.field(*index),
                hir::PlaceStep::Index(index) => {
                    let local = self.index_local(index);
                    place.index(local, index.span)
                }
            };
        }
        place
    }

    fn loop_body(&mut self, body: &hir::Block, break_to: BlockId, continue_to: BlockId) {
        self.loops.push(LoopCx {
            break_to,
            continue_to,
            scope_depth: self.scopes.len(),
            frame_depth: self.frames.len(),
        });
        // Each iteration owns its temporaries, including those of the
        // body's final expression.
        self.in_frame(|b| b.block(body));
        self.loops.pop();
        self.goto(continue_to);
    }

    // ---- expressions -------------------------------------------------------

    /// The operands of a call's arguments, in order. Values read from a
    /// place the call lends are copied first, so the callee's changes
    /// through the loan cannot reach them; a region is lent as a copy of its
    /// elements, which the callee changes but cannot resize and which go
    /// back in place after the call (16.4).
    fn call_operands(&mut self, args: &[hir::Expr], span: Span) -> (Vec<Operand>, Vec<RegionCopy>) {
        let lent: Vec<hir::LocalId> = args
            .iter()
            .filter_map(|a| match &a.kind {
                hir::ExprKind::Loan(place) | hir::ExprKind::Region(place, _) => Some(place.local),
                _ => None,
            })
            .collect();
        let mut operands = Vec::new();
        let mut loans: Vec<Lent> = Vec::new();
        let mut regions = Vec::new();
        for arg in args {
            if let hir::ExprKind::Region(place, bounds) = &arg.kind {
                // The place and bounds are fixed now, for the copy back.
                let place = self.place(place.local, &place.path);
                let place = self.frozen(place, arg.span);
                let bounds = bounds.each_ref().map(|b| {
                    b.as_ref().map(|b| match self.expr(b) {
                        read @ Operand::Copy(_) => self.copy_operand(read, Ty::INT, b.span),
                        constant => constant,
                    })
                });
                loans.push((place.clone(), Some(bounds.clone())));
                regions.push(RegionCopy {
                    index: operands.len(),
                    place,
                    bounds,
                    ty: arg.ty,
                    span: arg.span,
                    copy: None,
                });
                operands.push(Operand::Const(Const::Unit));
                continue;
            }
            let operand = self.expr(arg);
            let loan = matches!(arg.kind, hir::ExprKind::Loan(_));
            let reads_lent = !loan
                && matches!(&operand, Operand::Copy(place)
                    if lent.iter().any(|&l| self.hir_locals[l.0 as usize] == Some(place.local)));
            if let (true, Operand::Copy(place)) = (loan, &operand) {
                loans.push((place.clone(), None));
            }
            operands.push(if reads_lent { self.copy_operand(operand, arg.ty, arg.span) } else { operand });
        }
        self.check_disjoint(&loans, span);
        for region in &mut regions {
            let copy = Rvalue::Slice(Operand::Copy(region.place.clone()), region.bounds.clone(), region.span);
            operands[region.index] = self.temp(region.ty, copy, region.span);
            region.copy = Some(operands[region.index].clone());
        }
        (operands, regions)
    }

    /// After a call: nothing follows a call that never returns; otherwise
    /// the regions it borrowed go back in place.
    fn finish_call(&mut self, ty: Ty, regions: Vec<RegionCopy>) {
        if ty == Ty::NEVER {
            self.terminate(Terminator::Unreachable);
            return;
        }
        for region in regions {
            let copy = region.copy.expect("made before the call");
            self.temp(Ty::UNIT, Rvalue::AssignSlice(region.place, region.bounds, copy, region.span), region.span);
        }
    }

    fn expr(&mut self, e: &hir::Expr) -> Operand {
        match &e.kind {
            hir::ExprKind::Int(value) => Operand::Const(Const::Int(*value, e.ty)),
            hir::ExprKind::Float(value) => Operand::Const(Const::Float(value.to_bits(), e.ty)),
            hir::ExprKind::Char(c) => Operand::Const(Const::Char(*c)),
            hir::ExprKind::Convert(value) => {
                let value = self.expr(value);
                self.temp(e.ty, Rvalue::Convert(value), e.span)
            }
            hir::ExprKind::Bool(value) => Operand::Const(Const::Bool(*value)),
            hir::ExprKind::Str(text) => Operand::Const(Const::Str(text.clone())),
            hir::ExprKind::Unit => Operand::Const(Const::Unit),
            hir::ExprKind::Local(id) => {
                let local = self.hir_locals[id.0 as usize].expect("local lowered before use");
                // A binding whose type is not `Copy` gives its references
                // away where it moves, so a `Sender` closes its channel and a
                // `Join` waits when its new owner is done with it. Parameters
                // are the caller's.
                if self.function.moves.contains(&(*id, e.span))
                    && !self.types.is_copy(e.ty)
                    && !self.function.params.contains(id)
                {
                    let moved = self.temp(e.ty, Rvalue::Use(Operand::local(local)), e.span);
                    let Operand::Copy(Place { local: temp, .. }) = moved else { unreachable!() };
                    self.own(temp);
                    self.push(Statement::Clear(local));
                    return moved;
                }
                Operand::local(local)
            }
            hir::ExprKind::Aggregate(items) => {
                let components = items.iter().map(|item| self.expr(item)).collect();
                self.aggregate(e.ty, components, e.span)
            }
            hir::ExprKind::Record(values) => {
                // Evaluate in source order, then place in declaration order.
                let mut components = vec![Operand::Const(Const::Unit); values.len()];
                for (index, value) in values {
                    components[*index as usize] = self.expr(value);
                }
                self.aggregate(e.ty, components, e.span)
            }
            hir::ExprKind::With(base, updates) => {
                let Operand::Copy(base) = self.expr(base) else { unreachable!("records are places") };
                let count = self.types.components(e.ty).expect("record").len() as u32;
                let mut components: Vec<Operand> = (0..count).map(|i| Operand::Copy(base.field(i))).collect();
                for (index, value) in updates {
                    components[*index as usize] = self.expr(value);
                }
                self.aggregate(e.ty, components, e.span)
            }
            hir::ExprKind::Field(base, index) => match self.expr(base) {
                Operand::Copy(place) => Operand::Copy(place.field(*index)),
                Operand::Const(_) => unreachable!("aggregates are places"),
            },
            // Newtypes share their value's representation.
            hir::ExprKind::Retype(inner) => self.expr(inner),
            hir::ExprKind::Variant(variant, values) => {
                let count = self.types.variants(e.ty).expect("enum")[*variant as usize].len();
                let mut components = vec![Operand::Const(Const::Unit); count];
                for (index, value) in values {
                    components[*index as usize] = self.expr(value);
                }
                self.variant(e.ty, *variant, components, e.span)
            }
            hir::ExprKind::Match(scrutinee, arms) => self.match_expr(e, scrutinee, arms),
            hir::ExprKind::Array(items) => {
                let TyKind::Array(element) = *self.types.kind(e.ty) else { unreachable!() };
                let values: Vec<Operand> = items.iter().map(|item| self.expr(item)).collect();
                let counted = self.needs_drop(element);
                let retain = values
                    .iter()
                    .map(|v| counted && matches!(v, Operand::Copy(_)) && !self.take_temporary(v))
                    .collect();
                self.temp(e.ty, Rvalue::Array(values, retain), e.span)
            }
            hir::ExprKind::Index(base, index) => {
                let array_ty = base.ty;
                let base = self.expr(base);
                let base = self.place_of(base, array_ty);
                let index = self.index_local(index);
                Operand::Copy(base.index(index, e.span))
            }
            hir::ExprKind::Slice(base, parts) => {
                let base = self.expr(base);
                let parts = parts.each_ref().map(|part| part.as_ref().map(|p| self.expr(p)));
                self.temp(e.ty, Rvalue::Slice(base, parts, e.span), e.span)
            }
            hir::ExprKind::ArrayOp(op, operands) => {
                let operands = operands.iter().map(|o| self.expr(o)).collect();
                self.temp(e.ty, Rvalue::ArrayOp(*op, operands, e.span), e.span)
            }
            // The callee receives the place's address (see `Body::loans`).
            hir::ExprKind::Loan(place) => Operand::Copy(self.place(place.local, &place.path)),
            hir::ExprKind::Region(..) => unreachable!("a region is only lent to a call"),
            hir::ExprKind::Length(collection) => {
                let collection = self.expr(collection);
                self.temp(e.ty, Rvalue::Length(collection), e.span)
            }
            hir::ExprKind::StrOp(op, operands) => {
                let operands = operands.iter().map(|o| self.expr(o)).collect();
                self.temp(e.ty, Rvalue::StrOp(*op, operands, e.span), e.span)
            }
            hir::ExprKind::Map(entries) => {
                let map = self.temp(e.ty, Rvalue::MapOp(hir::MapOp::New, Vec::new(), e.span), e.span);
                let Operand::Copy(place) = map.clone() else { unreachable!() };
                let TyKind::Map(_, value) = *self.types.kind(e.ty) else { unreachable!() };
                let result = self.types.lookup(&TyKind::Option(value));
                for (key, value) in entries {
                    let operands = vec![self.expr(key), self.expr(value)];
                    let flags = self.move_flags(&operands, &[true, true], &[key.ty, value.ty]);
                    let rvalue = Rvalue::Mutate(hir::Mutation::Insert, place.clone(), operands, flags, e.span);
                    // A repeated dynamic key replaces the value (10.12); the
                    // replaced one is released with this temporary.
                    self.temp(result.expect("the checker interns `V?` with every map"), rvalue, e.span);
                }
                map
            }
            hir::ExprKind::Set(items) => {
                let set = self.temp(e.ty, Rvalue::MapOp(hir::MapOp::New, Vec::new(), e.span), e.span);
                let Operand::Copy(place) = set.clone() else { unreachable!() };
                for item in items {
                    let operands = vec![self.expr(item)];
                    let flags = self.move_flags(&operands, &[true], &[item.ty]);
                    let rvalue = Rvalue::Mutate(hir::Mutation::Insert, place.clone(), operands, flags, e.span);
                    self.temp(Ty::BOOL, rvalue, e.span);
                }
                set
            }
            hir::ExprKind::MapOp(op, operands) => {
                let operands = operands.iter().map(|o| self.expr(o)).collect();
                self.temp(e.ty, Rvalue::MapOp(*op, operands, e.span), e.span)
            }
            hir::ExprKind::Mutate(mutation, place, values) => {
                let place = self.place(place.local, &place.path);
                let types: Vec<Ty> = values.iter().map(|v| v.ty).collect();
                let operands: Vec<Operand> = values.iter().map(|v| self.expr(v)).collect();
                // Inserted values move into the collection; positions and
                // looked-up keys do not.
                let flags = self.move_flags(&operands, &mutation.moves(operands.len()), &types);
                self.temp(e.ty, Rvalue::Mutate(*mutation, place, operands, flags, e.span), e.span)
            }
            hir::ExprKind::Try(inner) => {
                let value = self.expr(inner);
                let place = self.place_of(value, inner.ty);
                let is_option = matches!(self.types.kind(inner.ty), TyKind::Option(_));
                let (failure, success) = if is_option { (NONE, SOME) } else { (ERR, OK) };
                let tag_ty = self.types.tag_ty(inner.ty);
                let tag = self.temp(tag_ty, Rvalue::Discriminant(place.clone()), e.span);
                let failed = Rvalue::Binary(BinOp::Eq, tag, Operand::Const(Const::Int(failure as i128, tag_ty)));
                let failed = self.temp(Ty::BOOL, failed, e.span);
                let (exit, next) = (self.new_block(), self.new_block());
                self.terminate(Terminator::Branch(failed, exit, next));
                self.switch_to(exit);
                if is_option {
                    let none = self.variant(self.success_ty(), NONE, Vec::new(), e.span);
                    self.emit_return(none);
                } else {
                    let TyKind::Result(_, error_ty) = *self.types.kind(inner.ty) else { unreachable!() };
                    let error = self.widen(Operand::Copy(place.variant_field(ERR, 0)), error_ty, e.span);
                    self.emit_fail(error);
                }
                self.switch_to(next);
                Operand::Copy(place.variant_field(success, 0))
            }
            hir::ExprKind::FnValue(id, _) => self.temp(e.ty, Rvalue::FnValue(*id), e.span),
            hir::ExprKind::CursorNext(cursor, kind) => self.cursor_next(cursor, *kind, e),
            hir::ExprKind::CallValue(callee, args) => {
                let callee = self.expr(callee);
                let (operands, regions) = self.call_operands(args, e.span);
                let result = self.temp(e.ty, Rvalue::CallIndirect(callee, operands), e.span);
                self.finish_call(e.ty, regions);
                result
            }
            hir::ExprKind::Erase(closure, body) => {
                let ty = closure.ty;
                let value = self.expr(closure);
                let retain = self.move_flags(std::slice::from_ref(&value), &[true], &[ty])[0];
                self.temp(e.ty, Rvalue::Erase(value, *body, retain), e.span)
            }
            hir::ExprKind::Spawn(body) => {
                let ty = body.ty;
                let value = self.expr(body);
                let retain = self.move_flags(std::slice::from_ref(&value), &[true], &[ty])[0];
                self.temp(e.ty, Rvalue::Spawn(value, retain), e.span)
            }
            hir::ExprKind::TaskOp(hir::TaskOp::GroupWait, operands) => self.group_wait(e, &operands[0]),
            hir::ExprKind::Select(arms, otherwise) => self.select(e, arms, otherwise.as_deref()),
            hir::ExprKind::TaskOp(op, operands) => {
                let types: Vec<Ty> = operands.iter().map(|o| o.ty).collect();
                let values: Vec<Operand> = operands.iter().map(|o| self.expr(o)).collect();
                // A join moves into a group, and a value into a channel.
                let moves: Vec<bool> = (0..values.len())
                    .map(|i| matches!(op, hir::TaskOp::GroupAdd | hir::TaskOp::Send) && i == 1)
                    .collect();
                let flags = self.move_flags(&values, &moves, &types);
                self.temp(e.ty, Rvalue::TaskOp(*op, values, flags, e.span), e.span)
            }
            hir::ExprKind::Call(callee, _, args) => {
                let (operands, regions) = self.call_operands(args, e.span);
                let result = self.temp(e.ty, Rvalue::Call(*callee, operands), e.span);
                self.finish_call(e.ty, regions);
                result
            }
            hir::ExprKind::Unary(op, operand) => {
                let operand = self.expr(operand);
                self.temp(e.ty, Rvalue::Unary(*op, operand), e.span)
            }
            hir::ExprKind::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                let result = self.new_local(Ty::BOOL);
                let left = self.expr(left);
                let (rhs, short, join) = (self.new_block(), self.new_block(), self.new_block());
                let branch = if *op == BinOp::And {
                    Terminator::Branch(left, rhs, short)
                } else {
                    Terminator::Branch(left, short, rhs)
                };
                self.terminate(branch);
                self.switch_to(short);
                let short_value = Rvalue::Use(Operand::Const(Const::Bool(*op == BinOp::Or)));
                self.assign(Place::local(result), short_value, e.span);
                self.goto(join);
                self.switch_to(rhs);
                self.in_frame(|b| {
                    let right = b.expr(right);
                    b.assign(Place::local(result), Rvalue::Use(right), e.span);
                });
                self.goto(join);
                self.switch_to(join);
                Operand::local(result)
            }
            hir::ExprKind::Binary(op, left, right) => {
                let left = self.expr(left);
                let right = self.expr(right);
                self.temp(e.ty, Rvalue::Binary(*op, left, right), e.span)
            }
            hir::ExprKind::If(condition, then, otherwise) => self.if_expr(e, condition, then, otherwise.as_deref()),
            hir::ExprKind::Block(block) => self.block(block),
            hir::ExprKind::Interpolate(segments) => {
                let pieces = segments
                    .iter()
                    .map(|s| match s {
                        hir::Segment::Text(text) => Piece::Text(text.clone()),
                        hir::Segment::Value(value) => Piece::Value(self.expr(value), value.ty),
                    })
                    .collect();
                self.temp(Ty::STRING, Rvalue::Concat(pieces), e.span)
            }
            hir::ExprKind::Panic(message) => {
                let message = self.expr(message);
                self.terminate(Terminator::Panic { code: 8, message: Some(message), span: e.span });
                Operand::Const(Const::Unit)
            }
            hir::ExprKind::Assert { condition, parts, source } => {
                let condition = self.expr(condition);
                let parts: Vec<Operand> = parts.iter().map(|p| self.expr(p)).collect();
                let (ok, failed) = (self.new_block(), self.new_block());
                self.terminate(Terminator::Branch(condition, ok, failed));
                self.switch_to(failed);
                let message = if parts.is_empty() {
                    Operand::Const(Const::Str(source.clone()))
                } else {
                    let pieces = parts.into_iter().map(|p| Piece::Value(p, Ty::STRING)).collect();
                    self.temp(Ty::STRING, Rvalue::Concat(pieces), e.span)
                };
                self.terminate(Terminator::Panic { code: 7, message: Some(message), span: e.span });
                self.switch_to(ok);
                Operand::Const(Const::Unit)
            }
            hir::ExprKind::Error => unreachable!("programs with errors are not lowered"),
        }
    }

    /// `group.wait()` (11.12): the results in the order the tasks were
    /// added or, when they are `R ! E`, the first error. For results the
    /// runtime first waits for every task or for the first that fails,
    /// which is then the only one taken. Each result taken is moved into
    /// the array or the error.
    fn group_wait(&mut self, e: &hir::Expr, group: &hir::Expr) -> Operand {
        let span = e.span;
        let item = self.types.handle_result(group.ty);
        let option = self.types.lookup(&TyKind::Option(item)).expect("interned with the group's type");
        let (array, fallible) = match *self.types.kind(e.ty) {
            TyKind::Result(array, _) => (array, true),
            _ => (e.ty, false),
        };
        let group = self.expr(group);
        let index = if fallible {
            self.temp(Ty::INT, Rvalue::TaskOp(hir::TaskOp::GroupSettle, vec![group.clone()], vec![false], span), span)
        } else {
            Operand::Const(Const::Int(-1, Ty::INT))
        };
        let values = self.new_local(array);
        self.assign(Place::local(values), Rvalue::ArrayOp(hir::ArrayOp::New, Vec::new(), span), span);
        let result = self.new_local(e.ty);
        let (head, body, done, exit) = (self.new_block(), self.new_block(), self.new_block(), self.new_block());
        self.goto(head);
        self.switch_to(head);
        let taken = self.new_local(option);
        let take = Rvalue::TaskOp(hir::TaskOp::GroupTake, vec![group, index], vec![false, false], span);
        self.assign(Place::local(taken), take, span);
        let taken = Place::local(taken);
        let some = self.tag_is(&taken, option, SOME, span);
        self.terminate(Terminator::Branch(some, body, done));
        self.switch_to(body);
        let mut element = taken.variant_field(SOME, 0);
        if fallible {
            let (failed, succeeded) = (self.new_block(), self.new_block());
            let error = self.tag_is(&element, item, ERR, span);
            self.terminate(Terminator::Branch(error, failed, succeeded));
            self.switch_to(failed);
            let error = Rvalue::Variant(ERR, vec![Operand::Copy(element.variant_field(ERR, 0))]);
            self.assign(Place::local(result), error, span);
            self.push(Statement::Release(Place::local(values)));
            self.goto(exit);
            self.switch_to(succeeded);
            element = element.variant_field(OK, 0);
        }
        let push =
            Rvalue::Mutate(hir::Mutation::Push, Place::local(values), vec![Operand::Copy(element)], vec![false], span);
        self.temp(Ty::UNIT, push, span);
        self.goto(head);
        self.switch_to(done);
        let all = if fallible {
            Rvalue::Variant(OK, vec![Operand::local(values)])
        } else {
            Rvalue::Use(Operand::local(values))
        };
        self.assign(Place::local(result), all, span);
        self.goto(exit);
        self.switch_to(exit);
        self.own(result);
        Operand::local(result)
    }

    /// `select` (11.14): evaluates the operations' operands in order, lets
    /// the runtime complete one, and runs its arm with the result in the
    /// case's `out`, which the arm owns; or runs the `else` arm. A value to
    /// send stays the sender's unless its send completes.
    fn select(&mut self, e: &hir::Expr, arms: &[hir::SelectArm], otherwise: Option<&hir::Expr>) -> Operand {
        let span = e.span;
        let mut cases = Vec::new();
        for arm in arms {
            let (op, operands) = match &arm.operation.kind {
                hir::ExprKind::TaskOp(op, operands) => (Some(*op), operands),
                hir::ExprKind::Call(_, _, operands) => (None, operands),
                _ => unreachable!("the checker allows only these operations"),
            };
            let values: Vec<Operand> = operands.iter().map(|o| self.expr(o)).collect();
            let retain = op == Some(hir::TaskOp::Send) && {
                let ty = operands[1].ty;
                self.needs_drop(ty) && matches!(values[1], Operand::Copy(_)) && !self.take_temporary(&values[1])
            };
            let out = self.new_local(arm.operation.ty);
            cases.push(SelectCase { op, operands: values, out, retain });
        }
        let outs: Vec<Local> = cases.iter().map(|c| c.out).collect();
        let won = self.temp(Ty::INT, Rvalue::Select(cases, otherwise.is_none(), span), span);
        let has_value = e.ty != Ty::UNIT && e.ty != Ty::NEVER;
        let result = has_value.then(|| self.new_local(e.ty));
        let join = self.new_block();
        let bodies = arms.iter().map(|a| (a.pattern.as_ref(), &a.body)).chain(otherwise.map(|o| (None, o)));
        for (index, (pattern, body)) in bodies.enumerate() {
            let next = self.new_block();
            if index < outs.len() {
                let expected = Operand::Const(Const::Int(index as i128, Ty::INT));
                let this = self.temp(Ty::BOOL, Rvalue::Binary(BinOp::Eq, won.clone(), expected), span);
                let block = self.new_block();
                self.terminate(Terminator::Branch(this, block, next));
                self.switch_to(block);
            }
            self.scopes.push(Vec::new());
            if let Some(&out) = outs.get(index) {
                if self.needs_drop(self.locals[out.0 as usize]) {
                    self.scopes.last_mut().unwrap().push(Exit::Release(out));
                }
                if let Some(pattern) = pattern {
                    self.bind_pattern(pattern, &Place::local(out));
                }
            }
            self.in_frame(|b| {
                let value = b.expr(body);
                if let Some(result) = result.filter(|_| body.ty != Ty::NEVER) {
                    b.store(Place::local(result), value, span);
                }
            });
            let scope = self.scopes.pop().unwrap();
            self.leave(scope);
            self.goto(join);
            self.switch_to(next);
        }
        self.terminate(Terminator::Unreachable);
        self.switch_to(join);
        match result {
            Some(result) => {
                self.own(result);
                Operand::local(result)
            }
            None => Operand::Const(Const::Unit),
        }
    }

    /// Whether the enum value at `place` holds `variant`.
    fn tag_is(&mut self, place: &Place, ty: Ty, variant: u32, span: Span) -> Operand {
        let tag_ty = self.types.tag_ty(ty);
        let tag = self.temp(tag_ty, Rvalue::Discriminant(place.clone()), span);
        let expected = Operand::Const(Const::Int(variant as i128, tag_ty));
        self.temp(Ty::BOOL, Rvalue::Binary(BinOp::Eq, tag, expected), span)
    }

    fn if_expr(
        &mut self,
        e: &hir::Expr,
        condition: &hir::Expr,
        then: &hir::Block,
        otherwise: Option<&hir::Expr>,
    ) -> Operand {
        let condition = self.expr(condition);
        let has_value = e.ty != Ty::UNIT && e.ty != Ty::NEVER;
        let result = has_value.then(|| self.new_local(e.ty));
        let (then_block, else_block, join) = (self.new_block(), self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(condition, then_block, else_block));

        self.switch_to(then_block);
        self.in_frame(|b| {
            let value = b.block(then);
            if let Some(result) = result.filter(|_| then.ty != Ty::NEVER) {
                b.store(Place::local(result), value, e.span);
            }
        });
        self.goto(join);

        self.switch_to(else_block);
        if let Some(otherwise) = otherwise {
            self.in_frame(|b| {
                let value = b.expr(otherwise);
                if let Some(result) = result.filter(|_| otherwise.ty != Ty::NEVER) {
                    b.store(Place::local(result), value, e.span);
                }
            });
        }
        self.goto(join);

        self.switch_to(join);
        match result {
            Some(result) => {
                self.own(result);
                Operand::local(result)
            }
            None => Operand::Const(Const::Unit),
        }
    }
}
