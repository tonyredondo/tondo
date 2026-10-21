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
use crate::source::Span;
use crate::types::{Ty, Types};

pub fn lower(program: &hir::Program) -> Vec<Body> {
    program.functions.iter().map(|f| lower_function(&program.types, f)).collect()
}

struct LoopCx {
    break_to: BlockId,
    continue_to: BlockId,
    scope_depth: usize,
    frame_depth: usize,
}

struct Builder<'a> {
    types: &'a Types,
    function: &'a hir::Function,
    locals: Vec<Ty>,
    blocks: Vec<(Vec<Statement>, Option<Terminator>)>,
    current: BlockId,
    hir_locals: Vec<Option<Local>>,
    /// Owned bindings per lexical scope.
    scopes: Vec<Vec<Local>>,
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
    Body { name: function.name.clone(), locals: b.locals, params, ret: b.ret, result: function.result, blocks }
}

impl Builder<'_> {
    fn needs_drop(&self, ty: Ty) -> bool {
        self.types.needs_drop(ty)
    }

    fn place_ty(&self, place: &Place) -> Ty {
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &index| {
            self.types.components(ty).expect("field path")[index as usize]
        })
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
        self.push(Statement::Assign(place, rvalue, span));
    }

    fn own(&mut self, local: Local) {
        if self.needs_drop(self.locals[local.0 as usize]) {
            self.frames.last_mut().unwrap().push(local);
        }
    }

    /// Evaluates `rvalue` into a new temporary; owned results join the frame.
    fn temp(&mut self, ty: Ty, rvalue: Rvalue, span: Span) -> Operand {
        let owned = matches!(rvalue, Rvalue::Call(..) | Rvalue::Concat(_));
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
        let mut retains = Vec::new();
        for ((index, component), &component_ty) in components.iter().enumerate().zip(&types) {
            let copied = matches!(component, Operand::Copy(_));
            if self.needs_drop(component_ty) && copied && !self.take_temporary(component) {
                retains.push(index as u32);
            }
        }
        let local = self.new_local(ty);
        self.assign(Place::local(local), Rvalue::Aggregate(components), span);
        for index in retains {
            self.push(Statement::Retain(Place::local(local).field(index)));
        }
        self.own(local);
        Operand::local(local)
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
        let scopes: Vec<Local> =
            self.scopes[scope_depth..].iter().rev().flat_map(|s| s.iter().rev().copied()).collect();
        for local in frames.into_iter().chain(scopes) {
            self.push(Statement::Release(Place::local(local)));
        }
    }

    fn emit_return(&mut self, value: Operand) {
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
        // A binding of this scope cannot outlive it; hand its value (or the
        // part of it the block yields) to a temporary of the enclosing frame.
        if let Operand::Copy(place) = &value
            && self.needs_drop(block.ty)
            && self.scopes.last().unwrap().contains(&place.local)
        {
            let copy = self.new_local(block.ty);
            self.assign(Place::local(copy), Rvalue::Use(value), Span::default());
            self.push(Statement::Retain(Place::local(copy)));
            self.frames.last_mut().unwrap().push(copy);
            value = Operand::local(copy);
        }
        let scope = self.scopes.pop().unwrap();
        self.release_all(scope);
        value
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::Let(id, value) => {
                let value = self.expr(value);
                let local = self.bind(*id);
                self.store(Place::local(local), value, Span::default());
                if self.needs_drop(self.locals[local.0 as usize]) {
                    self.scopes.last_mut().unwrap().push(local);
                }
            }
            hir::Stmt::Assign { local, path, op, value, span } => {
                let place = Place { local: self.bind(*local), path: path.clone() };
                let value = self.expr(value);
                let ty = self.place_ty(&place);
                match op {
                    Some(op) => self.assign(place.clone(), Rvalue::Binary(*op, Operand::Copy(place), value), *span),
                    None if self.needs_drop(ty) => {
                        // Keep the old value until the new one is stored, so
                        // `x = x` and `x = f(x.field)` stay valid.
                        let old = self.new_local(ty);
                        self.assign(Place::local(old), Rvalue::Use(Operand::Copy(place.clone())), *span);
                        self.store(place, value, *span);
                        self.push(Statement::Release(Place::local(old)));
                    }
                    None => self.assign(place, Rvalue::Use(value), *span),
                }
            }
            hir::Stmt::Expr(value) => {
                self.expr(value);
            }
            hir::Stmt::Return(value) => {
                let value = value.as_ref().map_or(Operand::Const(Const::Unit), |v| self.expr(v));
                self.emit_return(value);
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

    fn loop_body(&mut self, body: &hir::Block, break_to: BlockId, continue_to: BlockId) {
        self.loops.push(LoopCx {
            break_to,
            continue_to,
            scope_depth: self.scopes.len(),
            frame_depth: self.frames.len(),
        });
        self.block(body);
        self.loops.pop();
        self.goto(continue_to);
    }

    // ---- expressions -------------------------------------------------------

    fn expr(&mut self, e: &hir::Expr) -> Operand {
        match &e.kind {
            hir::ExprKind::Int(value) => Operand::Const(Const::Int(*value, e.ty)),
            hir::ExprKind::Bool(value) => Operand::Const(Const::Bool(*value)),
            hir::ExprKind::Str(text) => Operand::Const(Const::Str(text.clone())),
            hir::ExprKind::Unit => Operand::Const(Const::Unit),
            hir::ExprKind::Local(id) => {
                Operand::local(self.hir_locals[id.0 as usize].expect("local lowered before use"))
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
            hir::ExprKind::Call(callee, args) => {
                let args = args.iter().map(|a| self.expr(a)).collect();
                let result = self.temp(e.ty, Rvalue::Call(*callee, args), e.span);
                if e.ty == Ty::NEVER {
                    self.terminate(Terminator::Unreachable);
                }
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
