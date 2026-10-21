//! HIR to MIR lowering.
//!
//! Ownership of reference-counted values follows three rules:
//! - parameters are borrowed: the caller keeps them alive during the call;
//! - results of calls and string construction are owned temporaries,
//!   released when the enclosing statement ends unless moved into a binding;
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
use crate::types::Ty;

pub fn lower(program: &hir::Program) -> Vec<Body> {
    program.functions.iter().map(lower_function).collect()
}

fn is_managed(ty: Ty) -> bool {
    ty == Ty::STRING
}

struct LoopCx {
    break_to: BlockId,
    continue_to: BlockId,
    scope_depth: usize,
    frame_depth: usize,
}

struct Builder<'a> {
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

fn lower_function(function: &hir::Function) -> Body {
    let mut b = Builder {
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
    Body { name: function.name.clone(), locals: b.locals, params, result: function.result, blocks }
}

impl Builder<'_> {
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

    fn assign(&mut self, local: Local, rvalue: Rvalue, span: Span) {
        self.push(Statement::Assign(local, rvalue, span));
    }

    /// Evaluates `rvalue` into a new temporary. Owned results join the
    /// current frame.
    fn temp(&mut self, ty: Ty, rvalue: Rvalue, span: Span) -> Operand {
        let owned = matches!(rvalue, Rvalue::Call(..) | Rvalue::Concat(_));
        let local = self.new_local(ty);
        self.assign(local, rvalue, span);
        if owned && is_managed(ty) {
            self.frames.last_mut().unwrap().push(local);
        }
        Operand::Copy(local)
    }

    /// Stores `value` into `dst`, taking ownership: a temporary of the
    /// current frame is moved, anything else is retained.
    fn store(&mut self, dst: Local, value: Operand, span: Span) {
        self.assign(dst, Rvalue::Use(value.clone()), span);
        if !is_managed(self.locals[dst.0 as usize]) {
            return;
        }
        if let Operand::Copy(src) = value {
            let frame = self.frames.last_mut().unwrap();
            match frame.iter().position(|&l| l == src) {
                Some(index) => {
                    frame.remove(index);
                }
                None => self.push(Statement::Retain(dst)),
            }
        }
    }

    fn release_all(&mut self, locals: Vec<Local>) {
        for local in locals.into_iter().rev() {
            self.push(Statement::Release(local));
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
            self.push(Statement::Release(local));
        }
    }

    fn emit_return(&mut self, value: Operand) {
        let operand = match self.ret {
            Some(ret) => {
                self.store(ret, value, Span::default());
                Operand::Copy(ret)
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
        // A binding of this scope cannot outlive it; hand its value over to a
        // temporary of the enclosing frame first.
        if let Operand::Copy(local) = value
            && is_managed(block.ty)
            && self.scopes.last().unwrap().contains(&local)
        {
            let copy = self.new_local(block.ty);
            self.assign(copy, Rvalue::Use(value), Span::default());
            self.push(Statement::Retain(copy));
            self.frames.last_mut().unwrap().push(copy);
            value = Operand::Copy(copy);
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
                self.store(local, value, Span::default());
                if is_managed(self.locals[local.0 as usize]) {
                    self.scopes.last_mut().unwrap().push(local);
                }
            }
            hir::Stmt::Assign { local, op, value, span } => {
                let local = self.bind(*local);
                let value = self.expr(value);
                match op {
                    Some(op) => self.assign(local, Rvalue::Binary(*op, Operand::Copy(local), value), *span),
                    None if is_managed(self.locals[local.0 as usize]) => {
                        let old = self.new_local(self.locals[local.0 as usize]);
                        self.assign(old, Rvalue::Use(Operand::Copy(local)), *span);
                        self.store(local, value, *span);
                        self.push(Statement::Release(old));
                    }
                    None => self.assign(local, Rvalue::Use(value), *span),
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
                self.assign(cursor, Rvalue::Use(start), Span::default());
                let last = self.new_local(ty);
                self.assign(last, Rvalue::Use(end), Span::default());
                let (head, body_block, step, exit) =
                    (self.new_block(), self.new_block(), self.new_block(), self.new_block());
                self.goto(head);
                self.switch_to(head);
                let compare = if *inclusive { BinOp::Le } else { BinOp::Lt };
                let more = self.temp(
                    Ty::BOOL,
                    Rvalue::Binary(compare, Operand::Copy(cursor), Operand::Copy(last)),
                    Span::default(),
                );
                self.terminate(Terminator::Branch(more, body_block, exit));
                self.switch_to(body_block);
                let binding = self.bind(*local);
                self.assign(binding, Rvalue::Use(Operand::Copy(cursor)), Span::default());
                self.loop_body(body, exit, step);
                // The successor is only computed while `cursor < last`, so the
                // increment cannot overflow even at the type's maximum.
                self.switch_to(step);
                if *inclusive {
                    let done = self.temp(
                        Ty::BOOL,
                        Rvalue::Binary(BinOp::Eq, Operand::Copy(cursor), Operand::Copy(last)),
                        Span::default(),
                    );
                    let next = self.new_block();
                    self.terminate(Terminator::Branch(done, exit, next));
                    self.switch_to(next);
                }
                self.assign(cursor, Rvalue::Increment(Operand::Copy(cursor)), Span::default());
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
                Operand::Copy(self.hir_locals[id.0 as usize].expect("local lowered before use"))
            }
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
                self.assign(result, Rvalue::Use(Operand::Const(Const::Bool(*op == BinOp::Or))), e.span);
                self.goto(join);
                self.switch_to(rhs);
                self.in_frame(|b| {
                    let right = b.expr(right);
                    b.assign(result, Rvalue::Use(right), e.span);
                });
                self.goto(join);
                self.switch_to(join);
                Operand::Copy(result)
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
                b.store(result, value, e.span);
            }
        });
        self.goto(join);

        self.switch_to(else_block);
        if let Some(otherwise) = otherwise {
            self.in_frame(|b| {
                let value = b.expr(otherwise);
                if let Some(result) = result.filter(|_| otherwise.ty != Ty::NEVER) {
                    b.store(result, value, e.span);
                }
            });
        }
        self.goto(join);

        self.switch_to(join);
        match result {
            Some(result) => {
                if is_managed(e.ty) {
                    self.frames.last_mut().unwrap().push(result);
                }
                Operand::Copy(result)
            }
            None => Operand::Const(Const::Unit),
        }
    }
}
