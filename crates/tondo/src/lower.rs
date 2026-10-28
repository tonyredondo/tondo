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
use std::collections::HashMap;
use std::rc::Rc;

pub fn lower(program: &hir::Program, live: bool) -> Vec<Body> {
    // Each function on its own, on every core.
    let lower_all = |waits: Option<&[bool]>, recovering: bool| -> Vec<Body> {
        let restart = |index: usize, f: &hir::Function| (live && !f.test).then_some(hir::FnId(index as u32));
        let indices: Vec<usize> = (0..program.functions.len()).collect();
        crate::parallel::map(&indices, 1, |&i| {
            let f = &program.functions[i];
            lower_function(&program.types, f, waits, restart(i, f), recovering)
        })
    };
    let plain = lower_all(None, false);
    let waits = waiting(&plain);
    // A program with a `recover` unwinds to it from any panic (15.9).
    let recovering = plain.iter().flat_map(|b| &b.blocks).flat_map(|b| &b.stmts).any(|s| {
        matches!(s, Statement::Assign(_, r, _)
            if matches!(**r, Rvalue::Call(hir::Callee::Std(hir::StdFn::RecoverEnter | hir::StdFn::WithinEnter), _)))
    });
    if !waits.contains(&true) && !recovering {
        return plain;
    }
    let waits = waits.contains(&true).then_some(&waits[..]);
    lower_all(waits, recovering)
}

/// Drops the checks for a panic after statements the optimizations made
/// unable to panic, as a subtraction proved exact (15.9).
pub fn drop_dead_checks(bodies: &mut [Body]) {
    for body in bodies {
        for block in &mut body.blocks {
            let [.., statement, Statement::Assign(asked, rvalue, _)] = &block.stmts[..] else { continue };
            let Terminator::Branch(Operand::Copy(condition), _, next) = &block.terminator else { continue };
            if !matches!(**rvalue, Rvalue::Unwinding) || condition != asked || may_panic(statement) {
                continue;
            }
            block.terminator = Terminator::Goto(*next);
            block.stmts.pop();
        }
    }
}

/// Whether a statement may panic and return, when a `recover` stops it:
/// a call, an operation of the runtime, checked arithmetic or a read or
/// write by an index (15.9).
fn may_panic(statement: &Statement) -> bool {
    let checked = |place: &Place| place.path.iter().any(|p| matches!(p, Proj::Index(..)));
    let read = |operand: &Operand| matches!(operand, Operand::Copy(place) if checked(place));
    match statement {
        Statement::Assign(place, rvalue, _) => {
            checked(place)
                || match &**rvalue {
                    Rvalue::Use(o) | Rvalue::Length(o) | Rvalue::Convert(o) => read(o),
                    Rvalue::Unary(op, o) => *op == crate::ast::UnaryOp::Neg || read(o),
                    Rvalue::Binary(op, a, b) => {
                        use crate::ast::BinOp::*;
                        !matches!(op, Eq | Ne | Lt | Le | Gt | Ge | BitAnd | BitOr | BitXor) || read(a) || read(b)
                    }
                    Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _) => {
                        operands.iter().any(read)
                    }
                    Rvalue::Discriminant(place) => checked(place),
                    // What only adds or takes an element: an index or a key
                    // of the program's own type may panic.
                    Rvalue::Mutate(mutation, place, operands, ..) => {
                        use hir::Mutation::*;
                        checked(place)
                            || operands.iter().any(read)
                            || !matches!(mutation, Push | PushFront | Pop | PopFront | RemoveAt | Clear)
                    }
                    Rvalue::Exact(..)
                    | Rvalue::Increment(_)
                    | Rvalue::FnValue(_)
                    | Rvalue::ExternFn(_)
                    | Rvalue::Callback(..)
                    | Rvalue::Layout(..)
                    | Rvalue::Erase(..)
                    | Rvalue::Spawn(..)
                    | Rvalue::Cancelled
                    | Rvalue::Unwinding
                    | Rvalue::Restart(_)
                    | Rvalue::Transfer(_)
                    | Rvalue::OsrPoint => false,
                    Rvalue::Call(hir::Callee::Std(hir::StdFn::Number(op)), _) => op.may_panic(),
                    Rvalue::Call(hir::Callee::Std(function), _) => !matches!(
                        function,
                        hir::StdFn::RecoverEnter
                            | hir::StdFn::SpawnDetached
                            | hir::StdFn::WithinEnter
                            | hir::StdFn::WithinLeave
                            | hir::StdFn::WithinCatch
                            | hir::StdFn::ContextGet(_)
                            | hir::StdFn::ContextSet(..)
                            | hir::StdFn::ContextRestore
                            | hir::StdFn::RecoverEnd
                            | hir::StdFn::RecoverLeave
                            | hir::StdFn::RecoverCatch
                            | hir::StdFn::UnwindHold
                            | hir::StdFn::UnwindResume
                    ),
                    _ => true,
                }
        }
        Statement::Lend(_, place) | Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) => {
            checked(place)
        }
        _ => false,
    }
}

/// What `x = "{x}..."` appends to the string at `x`, a local or its
/// fields: text, and values whose `Display` runs no code of the program and
/// that do not read `x` again, which the string's own block may hold.
fn appended<'e>(
    value: &'e hir::Expr,
    local: LocalId,
    path: &[hir::PlaceStep],
    types: &Types,
) -> Option<&'e [hir::Segment]> {
    let hir::ExprKind::Interpolate(segments) = &value.kind else { return None };
    let (hir::Segment::Value(first), rest) = segments.split_first()? else { return None };
    let reads = |mut expr: &hir::Expr, path: &[hir::PlaceStep]| {
        for step in path.iter().rev() {
            match (&expr.kind, step) {
                (hir::ExprKind::Field(base, index), hir::PlaceStep::Field(field)) if index == field => expr = base,
                _ => return false,
            }
        }
        matches!(expr.kind, hir::ExprKind::Local(id) if id == local)
    };
    let rooted = |mut expr: &hir::Expr| loop {
        match &expr.kind {
            hir::ExprKind::Field(base, _) => expr = base,
            hir::ExprKind::Local(id) => return *id == local,
            _ => return false,
        }
    };
    let plain = |ty: Ty| {
        matches!(
            types.kind(types.repr(ty)),
            TyKind::String | TyKind::Int(_) | TyKind::Byte | TyKind::Float(_) | TyKind::Bool | TyKind::Char
        )
    };
    fn fits(segments: &[hir::Segment], ok: &impl Fn(&hir::Expr) -> bool) -> bool {
        segments.iter().all(|segment| match segment {
            hir::Segment::Text(_) => true,
            hir::Segment::Value(hir::Expr { kind: hir::ExprKind::Interpolate(inner), .. }) => fits(inner, ok),
            hir::Segment::Value(value) => ok(value),
        })
    }
    let fits = fits(rest, &|value: &hir::Expr| plain(value.ty) && !rooted(value));
    (first.ty == Ty::STRING && reads(first, path) && fits).then_some(rest)
}

/// Which functions may wait and end early when their task is cancelled
/// (11.15): those that use such an operation, call a function value, or
/// call a function that may.
fn waiting(bodies: &[Body]) -> Vec<bool> {
    let mut waits = vec![false; bodies.len()];
    let mut calls = vec![Vec::new(); bodies.len()];
    for (index, body) in bodies.iter().enumerate() {
        let rvalues = body.blocks.iter().flat_map(|b| &b.stmts).filter_map(|s| match s {
            Statement::Assign(_, rvalue, _) => Some(&**rvalue),
            _ => None,
        });
        for rvalue in rvalues {
            match rvalue {
                Rvalue::Call(hir::Callee::Fn(id), _) => calls[index].push(id.0 as usize),
                Rvalue::Call(hir::Callee::Std(f), _) if f.waits() => waits[index] = true,
                Rvalue::CallIndirect(..) | Rvalue::Select(..) => waits[index] = true,
                Rvalue::TaskOp(op, ..) if op.waits() => waits[index] = true,
                _ => {}
            }
        }
    }
    loop {
        let mut changed = false;
        for index in 0..bodies.len() {
            if !waits[index] && calls[index].iter().any(|&c| waits[c]) {
                waits[index] = true;
                changed = true;
            }
        }
        if !changed {
            return waits;
        }
    }
}

struct LoopCx {
    break_to: BlockId,
    continue_to: BlockId,
    scope_depth: usize,
    frame_depth: usize,
}

/// One action of leaving a scope.
/// What a `recover` or a `within` stops (15.9, 11.17), and how it reads
/// what stopped it.
#[derive(Clone, Copy)]
enum Catch<'h> {
    Recover(&'h hir::Expr),
    Within(&'h hir::Expr, &'h hir::Expr),
}

#[derive(Clone)]
enum Exit {
    Release(Local),
    /// The call of a registered `defer` (13.7).
    Defer(Rc<hir::Expr>),
    /// The end of a `recover` block (15.9).
    Recover,
    /// The end of a `within` block (11.17).
    Within,
    /// The end of a `with`: the contexts its local saved come back (11.16).
    Restore(Local),
}

/// Where a statement starts again: its first block, how many scopes and
/// frames were open there and how many temporaries the last had, and
/// whether it moved a binding away since, which it could not do again.
#[derive(Clone, Copy)]
struct Again {
    block: BlockId,
    scopes: usize,
    frames: usize,
    temps: usize,
    moved: bool,
}

struct Builder<'a> {
    types: &'a Types,
    function: &'a hir::Function,
    locals: Vec<Ty>,
    names: Vec<Option<String>>,
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
    /// Which functions may wait, once known; `None` lowers without
    /// cancellation points.
    waits: Option<&'a [bool]>,
    /// Inside the lowering of a cleanup, which has no cancellation points.
    cleaning: usize,
    /// In a live build, the function itself, which a restarted frame calls
    /// again; and its parameters.
    restart: Option<hir::FnId>,
    params: Vec<Local>,
    /// In a live build: the points, where each local comes from, the
    /// statement being lowered, and how many blocks each statement has
    /// lowered and locals each has made.
    points: Vec<Point>,
    origins: Vec<Origin>,
    /// In a live build, the statement being lowered: where it starts
    /// again after a wait the runtime ends for a reload (toolchain 7.10).
    again: Option<Again>,
    pauses: Vec<(Span, bool)>,
    /// Where each statement starts, and the ways each branch can go, for
    /// coverage.
    starts: Vec<(BlockId, usize, Span)>,
    ways: Vec<Way>,
    path: Vec<u32>,
    blocks_of: HashMap<Vec<u32>, u32>,
    made_by: HashMap<Vec<u32>, u32>,
    /// The temporary a `match` arm's bindings take their values from,
    /// rather than copy them: the arm has no guard, so the match uses
    /// nothing of it after them.
    moving: Option<Local>,
    /// Whether the program has a `recover`: then a statement that may panic
    /// is followed by a check that unwinds to the nearest one (15.9).
    recovering: bool,
    /// The `recover` blocks being lowered, innermost last: how many scopes
    /// and frames were open outside it, and where it stops a panic.
    recovers: Vec<(usize, usize, BlockId)>,
    /// The cleanups that unwind from each state of the scopes and frames,
    /// made once each.
    unwinds: HashMap<Vec<u64>, BlockId>,
}

fn lower_function(
    types: &Types,
    function: &hir::Function,
    waits: Option<&[bool]>,
    restart: Option<hir::FnId>,
    recovering: bool,
) -> Body {
    let mut b = Builder {
        types,
        function,
        locals: Vec::new(),
        names: Vec::new(),
        blocks: Vec::new(),
        current: BlockId(0),
        hir_locals: vec![None; function.locals.len()],
        scopes: Vec::new(),
        frames: vec![Vec::new()],
        loops: Vec::new(),
        ret: None,
        waits,
        cleaning: 0,
        restart,
        params: Vec::new(),
        points: Vec::new(),
        origins: Vec::new(),
        again: None,
        pauses: Vec::new(),
        starts: Vec::new(),
        ways: Vec::new(),
        path: Vec::new(),
        blocks_of: HashMap::new(),
        made_by: HashMap::new(),
        moving: None,
        recovering,
        recovers: Vec::new(),
        unwinds: HashMap::new(),
    };
    // A live build's first block is where a frame from the old code comes
    // in, which `osr_entry` fills once the points are known.
    b.current = b.new_block();
    let params: Vec<Local> = function.params.iter().map(|&p| b.bind(p)).collect();
    for (index, &param) in params.iter().enumerate() {
        b.origins[param.0 as usize] = Origin::Param(index as u32);
    }
    b.params = params.clone();
    let osr = restart.map(|_| b.new_local(Ty::INT));
    let start = b.new_block();
    b.terminate(Terminator::Goto(start));
    b.switch_to(start);
    if function.result != Ty::UNIT && function.result != Ty::NEVER {
        b.ret = Some(b.new_local(function.result));
    }
    let value = b.block(&function.body);
    if function.body.ty != Ty::NEVER {
        let span = function.body.tail.as_ref().map_or(Span::default(), |tail| tail.span);
        b.emit_return(value, span);
    }
    if let Some(osr) = osr {
        b.osr_entry(osr, start);
    }
    let blocks: Vec<BlockData> = b
        .blocks
        .into_iter()
        .map(|(stmts, terminator)| BlockData { stmts, terminator: terminator.unwrap_or(Terminator::Unreachable) })
        .collect();
    let mut points = b.points;
    if osr.is_some() {
        let live = crate::mir::live_in(&blocks, b.locals.len());
        for point in &mut points {
            point.state = live[point.continues.0 as usize]
                .iter()
                .copied()
                .filter(|local| !params.contains(local) && Some(*local) != osr)
                .collect();
        }
    }
    Body {
        name: function.name.clone(),
        locals: b.locals,
        names: b.names,
        params,
        loans: function.loans.clone(),
        ret: b.ret,
        result: function.result,
        blocks,
        osr,
        points,
        origins: b.origins,
        pauses: b.pauses,
        span: function.span,
        starts: b.starts,
        ways: b.ways,
        inlined: Vec::new(),
        scopes: Vec::new(),
    }
}

impl Builder<'_> {
    fn needs_drop(&self, ty: Ty) -> bool {
        self.types.needs_drop(ty)
    }

    /// Whether a statement only computes with floats, which never panics:
    /// their arithmetic, and the functions of `std.math` the processor
    /// computes.
    fn computes_floats(&self, statement: &Statement) -> bool {
        let Statement::Assign(place, rvalue, _) = statement else { return false };
        let plain = |operand: &Operand| match operand {
            Operand::Copy(place) => !place.path.iter().any(|p| matches!(p, Proj::Index(..))),
            Operand::Const(_) => true,
        };
        let float = matches!(self.types.kind(self.place_ty(place)), TyKind::Float(_));
        plain(&Operand::Copy(place.clone()))
            && match &**rvalue {
                Rvalue::Binary(_, a, b) => float && plain(a) && plain(b),
                Rvalue::Unary(_, a) => float && plain(a),
                Rvalue::Call(hir::Callee::Std(hir::StdFn::Native(index)), arguments) => {
                    arguments.iter().all(plain)
                        && matches!(
                            hir::NATIVES[*index as usize].symbol,
                            "tondo_rt_math_sqrt"
                                | "tondo_rt_math_floor"
                                | "tondo_rt_math_ceil"
                                | "tondo_rt_math_trunc"
                                | "tondo_rt_math_round"
                        )
                }
                _ => false,
            }
    }

    fn place_ty(&self, place: &Place) -> Ty {
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &proj| project(self.types, ty, proj))
    }

    fn new_local(&mut self, ty: Ty) -> Local {
        self.locals.push(ty);
        self.names.push(None);
        let made = self.made_by.entry(self.path.clone()).or_insert(0);
        self.origins.push(Origin::Made(self.path.clone(), *made));
        *made += 1;
        Local(self.locals.len() as u32 - 1)
    }

    fn bind(&mut self, id: LocalId) -> Local {
        if let Some(local) = self.hir_locals[id.0 as usize] {
            return local;
        }
        let declared = &self.function.locals[id.0 as usize];
        let local = self.new_local(declared.ty);
        // The compiler's own are `_` and `<...>`.
        if declared.name != "_" && !declared.name.starts_with('<') {
            self.names[local.0 as usize] = Some(declared.name.clone());
            self.origins[local.0 as usize] = Origin::Named(declared.name.clone());
        }
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

    /// A statement the program wrote starts to run here.
    fn start(&mut self, span: Span) {
        if span != Span::default() {
            let at = self.blocks[self.current.0 as usize].0.len();
            self.starts.push((self.current, at, span));
        }
    }

    /// A way of a branch, which starts here, or at the start of `block`.
    fn way(&mut self, block: Option<BlockId>, branch: Span, span: Span, label: &'static str) {
        let (block, at) = match block {
            Some(block) => (block, 0),
            None => (self.current, self.blocks[self.current.0 as usize].0.len()),
        };
        self.ways.push(Way { block, at, branch, span, label });
    }

    fn push(&mut self, stmt: Statement) {
        let check = self.recovering && self.cleaning == 0 && may_panic(&stmt) && !self.computes_floats(&stmt);
        self.blocks[self.current.0 as usize].0.push(stmt);
        if check {
            self.unwind_check();
        }
    }

    /// After a statement that may panic: whether it did, and if so, the
    /// way to the nearest `recover` (15.9).
    fn unwind_check(&mut self) {
        let unwind = self.unwind_target();
        let asked = self.new_local(Ty::BOOL);
        let check = Statement::Assign(Place::local(asked), Box::new(Rvalue::Unwinding), Span::default());
        self.blocks[self.current.0 as usize].0.push(check);
        let next = self.new_block();
        self.terminate(Terminator::Branch(Operand::local(asked), unwind, next));
        self.switch_to(next);
    }

    /// The way a panic unwinds from here: the panic waits while what the
    /// scopes and frames hold is released and their cleanups run, then it
    /// goes on to the caller, or to the `recover` that stops it. Made once
    /// for each state of the scopes and frames.
    fn unwind_target(&mut self) -> BlockId {
        let (scope_depth, frame_depth, catch) = match self.recovers.last() {
            Some(&(scopes, frames, catch)) => (scopes, frames, Some(catch)),
            None => (0, 0, None),
        };
        let mut key: Vec<u64> = Vec::new();
        for scope in &self.scopes[scope_depth..] {
            for exit in scope {
                key.push(match exit {
                    Exit::Release(local) => u64::from(local.0),
                    Exit::Defer(call) => Rc::as_ptr(call) as u64 | 1 << 63,
                    Exit::Recover => u64::MAX - 1,
                    Exit::Within => u64::MAX - 2,
                    Exit::Restore(local) => u64::from(local.0) | 1 << 61,
                });
            }
            key.push(u64::MAX);
        }
        for frame in &self.frames[frame_depth..] {
            key.extend(frame.iter().map(|local| u64::from(local.0) | 1 << 62));
            key.push(u64::MAX);
        }
        key.push(catch.map_or(u64::MAX, |c| u64::from(c.0)));
        if let Some(&block) = self.unwinds.get(&key) {
            return block;
        }
        let current = self.current;
        let block = self.new_block();
        self.switch_to(block);
        self.cleaning += 1;
        self.std_call(hir::StdFn::UnwindHold, Span::default());
        self.cleanup_to(scope_depth, frame_depth);
        match catch {
            Some(catch) => self.goto(catch),
            None => {
                self.std_call(hir::StdFn::UnwindResume, Span::default());
                let result = match self.ret {
                    Some(ret) => {
                        self.push(Statement::Clear(ret));
                        Operand::local(ret)
                    }
                    None => Operand::Const(Const::Unit),
                };
                self.terminate(Terminator::Return(result));
            }
        }
        self.cleaning -= 1;
        self.unwinds.insert(key, block);
        self.switch_to(current);
        block
    }

    /// A call of the runtime for its effect.
    fn std_call(&mut self, function: hir::StdFn, span: Span) {
        let unit = self.new_local(Ty::UNIT);
        let call = Rvalue::Call(hir::Callee::Std(function), Vec::new());
        self.push(Statement::Assign(Place::local(unit), Box::new(call), span));
    }

    fn spawn(&mut self, e: &hir::Expr, body: &hir::Expr, detached: bool) -> Operand {
        let ty = body.ty;
        let value = self.expr(body);
        let retain = self.move_flags(std::slice::from_ref(&value), &[true], &[ty])[0];
        if detached {
            self.std_call(hir::StdFn::SpawnDetached, e.span);
        }
        self.temp(e.ty, Rvalue::Spawn(value, retain), e.span)
    }

    /// `recover { ... }` (15.9): `ok` with the block's value, or `err` with
    /// the panic that stopped it, which `caught` reads; and `within limit {
    /// ... }` (11.17): the block's value, already the result's, or `err` with
    /// `failure` when its deadline passed first. What unwinds to one of them
    /// and is not its own, a deadline to a `recover` or a panic to a
    /// `within`, goes on unwinding, after the cleanups of what it abandoned.
    fn catcher(&mut self, e: &hir::Expr, block: &hir::Block, catch_kind: Catch) -> Operand {
        let result = self.new_local(e.ty);
        let exit = match catch_kind {
            Catch::Recover(_) => {
                self.std_call(hir::StdFn::RecoverEnter, e.span);
                Exit::Recover
            }
            Catch::Within(limit, _) => {
                let limit = self.expr(limit);
                let unit = self.new_local(Ty::UNIT);
                let enter = Rvalue::Call(hir::Callee::Std(hir::StdFn::WithinEnter), vec![limit]);
                self.push(Statement::Assign(Place::local(unit), Box::new(enter), e.span));
                Exit::Within
            }
        };
        let (catch, join) = (self.new_block(), self.new_block());
        // Leaving the block another way, by `return`, `break`, `?`, `fail`
        // or a cancellation, ends it too.
        self.scopes.push(vec![exit]);
        self.recovers.push((self.scopes.len(), self.frames.len(), catch));
        self.in_frame(|b| {
            let value = b.block(block);
            if block.ty != Ty::NEVER {
                // A `within`'s block that fails as `R ! E` gives `R ! (E |
                // time.TimedOut)`.
                let value = match *b.types.kind(block.ty) {
                    TyKind::Result(..) if matches!(catch_kind, Catch::Within(..)) && block.ty != e.ty => {
                        b.widen_result(value, block.ty, e.ty, e.span)
                    }
                    TyKind::Result(..) if matches!(catch_kind, Catch::Within(..)) => value,
                    _ => b.variant(e.ty, OK, vec![value], e.span),
                };
                b.store(Place::local(result), value, e.span);
            }
        });
        self.recovers.pop();
        self.scopes.pop();
        // The way out for what is not its own.
        let outer = self.unwind_target();
        match catch_kind {
            // Its tasks are done: if one ended with a panic, the block's
            // value goes and the `recover` stops the panic.
            Catch::Recover(_) => {
                let call = Rvalue::Call(hir::Callee::Std(hir::StdFn::RecoverEnd), Vec::new());
                let failed = self.temp(Ty::BOOL, call, e.span);
                let given = self.new_block();
                self.terminate(Terminator::Branch(failed, given, join));
                self.switch_to(given);
                if block.ty != Ty::NEVER {
                    self.push(Statement::Release(Place::local(result)));
                }
                self.goto(catch);
            }
            Catch::Within(..) => {
                self.std_call(hir::StdFn::WithinLeave, e.span);
                self.goto(join);
            }
        }
        self.switch_to(catch);
        let asked = match catch_kind {
            Catch::Recover(_) => hir::StdFn::RecoverCatch,
            Catch::Within(..) => hir::StdFn::WithinCatch,
        };
        let mine = self.temp(Ty::BOOL, Rvalue::Call(hir::Callee::Std(asked), Vec::new()), e.span);
        let (stop, pass) = (self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(mine, stop, pass));
        self.switch_to(pass);
        self.std_call(hir::StdFn::UnwindResume, Span::default());
        self.goto(outer);
        self.switch_to(stop);
        self.in_frame(|b| {
            let error = match catch_kind {
                Catch::Recover(caught) => b.expr(caught),
                Catch::Within(_, failure) => b.expr(failure),
            };
            let err = b.variant(e.ty, ERR, vec![error], e.span);
            b.store(Place::local(result), err, e.span);
        });
        self.goto(join);
        self.switch_to(join);
        self.own(result);
        Operand::local(result)
    }

    /// A panic with `code` and `message`: where a `recover` may stop it,
    /// a call that returns and unwinds (15.9); otherwise the end.
    fn panic_with(&mut self, code: u32, message: Operand, span: Span) {
        if self.recovering && self.cleaning == 0 && code < 2000 {
            let unit = self.new_local(Ty::UNIT);
            let call = Rvalue::Call(hir::Callee::Std(hir::StdFn::PanicUnwind(code)), vec![message]);
            self.push(Statement::Assign(Place::local(unit), Box::new(call), span));
            self.terminate(Terminator::Unreachable);
            return;
        }
        self.terminate(Terminator::Panic { code, message: Some(message), span });
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
    /// In a test or a setup, tells the runner where an error leaves it.
    fn note_failure(&mut self, span: Span) {
        if self.function.test {
            let call = Rvalue::Call(hir::Callee::Std(hir::StdFn::TestWhere), Vec::new());
            self.temp(Ty::UNIT, call, span);
        }
    }

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

    /// Whether a local is a temporary the current frame owns.
    fn owned(&self, local: Local) -> bool {
        self.frames.last().unwrap().contains(&local)
    }

    /// Moves the value at `place`, inside a temporary of the frame, into a
    /// new temporary of its own, and leaves zero where it was: what a
    /// binding then takes without a retain, and the temporary's release no
    /// longer releases.
    fn move_out(&mut self, place: Place, span: Span) -> Operand {
        let local = self.new_local(self.place_ty(&place));
        self.assign(Place::local(local), Rvalue::Use(Operand::Copy(place.clone())), span);
        self.own(local);
        self.push(Statement::Moved(place));
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
                let inside = |p: &Proj| matches!(p, Proj::Field(_) | Proj::Variant(..));
                match self.moving == Some(place.local) && place.path.iter().all(inside) {
                    true => {
                        self.assign(Place::local(local), Rvalue::Use(Operand::Copy(place.clone())), pattern.span);
                        if self.needs_drop(self.locals[local.0 as usize]) {
                            self.push(Statement::Moved(place.clone()));
                        }
                    }
                    false => self.store(Place::local(local), Operand::Copy(place.clone()), pattern.span),
                }
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
        // The arms are lowered one after another, so a binding of the whole
        // value must not move it: the match holds it and releases it after
        // every arm.
        let held = self.take_temporary(&value);
        let place = self.place_of(value, scrutinee.ty);
        self.scopes.push(Vec::new());
        if held {
            self.scopes.last_mut().unwrap().push(Exit::Release(place.local));
        }
        let has_value = e.ty != Ty::UNIT && e.ty != Ty::NEVER;
        let result = has_value.then(|| self.new_local(e.ty));
        let join = self.new_block();
        for arm in arms {
            let next = self.new_block();
            let scope_depth = self.scopes.len();
            self.scopes.push(Vec::new());
            self.test_pattern(&arm.pattern, &place, next);
            self.moving = (held && arm.guard.is_none()).then_some(place.local);
            self.bind_pattern(&arm.pattern, &place);
            self.moving = None;
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
            self.way(None, e.span, arm.pattern.span, "arm");
            self.start(arm.body.span);
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
        let held = self.scopes.pop().unwrap();
        self.leave(held);
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
            Exit::Recover => self.std_call(hir::StdFn::RecoverLeave, Span::default()),
            Exit::Within => self.std_call(hir::StdFn::WithinLeave, Span::default()),
            Exit::Restore(saved) => {
                let unit = self.new_local(Ty::UNIT);
                let call = Rvalue::Call(hir::Callee::Std(hir::StdFn::ContextRestore), vec![Operand::local(saved)]);
                self.push(Statement::Assign(Place::local(unit), Box::new(call), Span::default()));
            }
            Exit::Defer(call) => {
                // Unregister it first, so a panic in the cleanup cannot run it again.
                self.push(Statement::DeferPop);
                self.cleaning += 1;
                self.in_frame(|b| {
                    b.expr(&call);
                });
                self.cleaning -= 1;
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

    /// After an operation that may wait: a cancelled task unwinds from here
    /// as a `return` does, releasing what the function owns and running its
    /// cleanups, and returns zeros its caller never uses (11.15).
    /// A wait that may start its statement again changes nothing when it
    /// ends early: it had not taken, released or changed anything yet.
    fn cancellation_point(&mut self, span: Span, restartable: bool) {
        if self.waits.is_none() || self.cleaning > 0 {
            return;
        }
        // 1: cancelled; 2, in a live build: the statement starts again.
        let byte = Ty::int(crate::types::IntTy::U8);
        let asked = self.temp(byte, Rvalue::Cancelled, span);
        let is = |b: &mut Self, code: i128| {
            let compare = Rvalue::Binary(BinOp::Eq, asked.clone(), Operand::Const(Const::Int(code, byte)));
            b.temp(Ty::BOOL, compare, span)
        };
        let cancelled = is(self, 1);
        let (unwind, next) = (self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(cancelled, unwind, next));
        self.switch_to(unwind);
        let result = match self.ret {
            Some(ret) => {
                self.push(Statement::Clear(ret));
                Operand::local(ret)
            }
            None => Operand::Const(Const::Unit),
        };
        self.cleaning += 1;
        self.cleanup_to(0, 0);
        self.cleaning -= 1;
        self.terminate(Terminator::Return(result));
        self.switch_to(next);
        // What the statement made since it started goes, and it starts
        // again, unless it gave a binding away.
        let again = self.again.filter(|again| restartable && !again.moved);
        if self.restart.is_some() {
            self.pauses.push((span, again.is_some()));
        }
        let Some(again) = again else { return };
        let restart = is(self, 2);
        let (back, next) = (self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(restart, back, next));
        self.switch_to(back);
        self.cleaning += 1;
        let mut temps: Vec<Local> = self.frames[again.frames][again.temps..].to_vec();
        temps.extend(self.frames[again.frames + 1..].iter().flatten());
        for local in temps.into_iter().rev() {
            self.push(Statement::Release(Place::local(local)));
        }
        let exits: Vec<Exit> = self.scopes[again.scopes..].iter().rev().flat_map(|s| s.iter().rev().cloned()).collect();
        for exit in exits {
            self.exit_action(exit);
        }
        self.cleaning -= 1;
        self.terminate(Terminator::Goto(again.block));
        self.switch_to(next);
    }

    /// In a live build, where a statement starts (toolchain 7.10). The
    /// probe of its line says what to do: go on; restart the frame, which
    /// returns as `return` would, running its cleanups, and calls its
    /// function again with the same arguments; or move the frame to the
    /// new code, which takes its state as it is here and goes on from the
    /// same point, while this frame returns what the new one does.
    fn restart_point(&mut self, span: Span, kind: &'static str) {
        let Some(function) = self.restart else { return };
        if self.cleaning > 0 {
            return;
        }
        let point = self.points.len() as u32;
        let start = self.new_block();
        self.terminate(Terminator::Goto(start));
        self.switch_to(start);
        let frames = self.frames.len() - 1;
        let temps = self.frames[frames].len();
        self.again = Some(Again { block: start, scopes: self.scopes.len(), frames, temps, moved: false });
        let asked = self.new_local(Ty::INT);
        self.assign(Place::local(asked), Rvalue::Restart(point), span);
        let (again, check, transfer, moving, next) =
            (self.new_block(), self.new_block(), self.new_block(), self.new_block(), self.new_block());
        let is = |b: &mut Self, code: i128| {
            let compare = Rvalue::Binary(BinOp::Eq, Operand::local(asked), Operand::Const(Const::Int(code, Ty::INT)));
            let yes = b.new_local(Ty::BOOL);
            b.assign(Place::local(yes), compare, span);
            Operand::local(yes)
        };
        let restart = is(self, 1);
        self.terminate(Terminator::Branch(restart, again, check));
        self.switch_to(check);
        let moves = is(self, 2);
        self.terminate(Terminator::Branch(moves, transfer, next));
        let arguments: Vec<Operand> = self.params.iter().map(|&p| Operand::local(p)).collect();
        // The new code returns for this frame, which returns its result.
        let finish = |b: &mut Self, call: Rvalue| {
            let result = match b.ret {
                Some(ret) => {
                    b.assign(Place::local(ret), call, span);
                    Operand::local(ret)
                }
                None => {
                    let unit = b.new_local(Ty::UNIT);
                    b.assign(Place::local(unit), call, span);
                    Operand::Const(Const::Unit)
                }
            };
            b.terminate(Terminator::Return(result));
        };
        self.switch_to(again);
        self.cleaning += 1;
        self.cleanup_to(0, 0);
        self.cleaning -= 1;
        finish(self, Rvalue::Call(hir::Callee::Fn(function), arguments.clone()));
        // What the frame owns goes to the new one, which releases it.
        self.switch_to(transfer);
        let state = self.new_local(Ty::INT);
        self.assign(Place::local(state), Rvalue::Transfer(point), span);
        let given = self.new_local(Ty::BOOL);
        let compare = Rvalue::Binary(BinOp::Ne, Operand::local(state), Operand::Const(Const::Int(0, Ty::INT)));
        self.assign(Place::local(given), compare, span);
        self.terminate(Terminator::Branch(Operand::local(given), moving, next));
        self.switch_to(moving);
        finish(self, Rvalue::CallOsr(function, arguments, Operand::local(state)));
        let defers = self.scopes.iter().flatten().filter(|exit| matches!(exit, Exit::Defer(_))).count() as u32;
        self.points.push(Point { path: self.path.clone(), span, continues: next, state: Vec::new(), kind, defers });
        self.switch_to(next);
    }

    /// The first block of a live build's function: a frame that comes from
    /// the old code loads the state of its point and goes on from there.
    fn osr_entry(&mut self, osr: Local, start: BlockId) {
        let first = std::mem::take(&mut self.blocks[0]);
        debug_assert!(first.0.is_empty());
        self.switch_to(BlockId(0));
        let given = self.new_local(Ty::BOOL);
        let compare = Rvalue::Binary(BinOp::Ne, Operand::local(osr), Operand::Const(Const::Int(0, Ty::INT)));
        self.assign(Place::local(given), compare, Span::default());
        let dispatch = self.new_block();
        self.terminate(Terminator::Branch(Operand::local(given), dispatch, start));
        self.switch_to(dispatch);
        let point = self.new_local(Ty::INT);
        self.assign(Place::local(point), Rvalue::OsrPoint, Span::default());
        for index in 0..self.points.len() {
            let (load, other) = (self.new_block(), self.new_block());
            let compare =
                Rvalue::Binary(BinOp::Eq, Operand::local(point), Operand::Const(Const::Int(index as i128, Ty::INT)));
            let this = self.new_local(Ty::BOOL);
            self.assign(Place::local(this), compare, Span::default());
            self.terminate(Terminator::Branch(Operand::local(this), load, other));
            self.switch_to(load);
            self.push(Statement::OsrLoad(index as u32));
            let continues = self.points[index].continues;
            self.terminate(Terminator::Goto(continues));
            self.switch_to(other);
        }
        self.terminate(Terminator::Goto(start));
    }

    /// Whether a call may wait (see `waiting`).
    fn call_waits(&self, callee: hir::Callee) -> bool {
        match callee {
            hir::Callee::Fn(id) => self.waits.is_some_and(|w| w[id.0 as usize]),
            hir::Callee::Std(f) => f.waits(),
            _ => false,
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
        self.widen_into(error, from, to, span)
    }

    /// A result `R ! E` as one of type `to`, `R ! U`, whose error union
    /// holds `E`: what a `within` gives when its block fails (11.17).
    fn widen_result(&mut self, value: Operand, from: Ty, to: Ty, span: Span) -> Operand {
        let (&TyKind::Result(_, error), &TyKind::Result(_, union)) = (self.types.kind(from), self.types.kind(to))
        else {
            unreachable!("results")
        };
        let place = self.place_of(value, from);
        let tag_ty = self.types.tag_ty(from);
        let tag = self.temp(tag_ty, Rvalue::Discriminant(place.clone()), span);
        let failed = Rvalue::Binary(BinOp::Eq, tag, Operand::Const(Const::Int(ERR as i128, tag_ty)));
        let failed = self.temp(Ty::BOOL, failed, span);
        let result = self.new_local(to);
        let (bad, good, done) = (self.new_block(), self.new_block(), self.new_block());
        self.terminate(Terminator::Branch(failed, bad, good));
        self.switch_to(bad);
        let widened = self.widen_into(Operand::Copy(place.variant_field(ERR, 0)), error, union, span);
        let value = self.variant(to, ERR, vec![widened], span);
        self.store(Place::local(result), value, span);
        self.goto(done);
        self.switch_to(good);
        let value = self.variant(to, OK, vec![Operand::Copy(place.variant_field(OK, 0))], span);
        self.store(Place::local(result), value, span);
        self.goto(done);
        self.switch_to(done);
        self.own(result);
        Operand::local(result)
    }

    /// An error of type `from` as one of the error union `to`, which holds
    /// it as a member or as a smaller union (15.3).
    fn widen_into(&mut self, error: Operand, from: Ty, to: Ty, span: Span) -> Operand {
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
    /// The operands of expressions evaluated in order (5.12): a binding read
    /// is copied when an expression after it may change it, as in
    /// `f(xs, xs.removeAt(0))`, so that it gives the value it had then.
    fn in_order<'h>(&mut self, exprs: impl IntoIterator<Item = &'h hir::Expr>) -> Vec<Operand> {
        let exprs: Vec<&hir::Expr> = exprs.into_iter().collect();
        let later = later_written(&exprs);
        exprs
            .iter()
            .zip(later)
            .map(|(e, later)| {
                let operand = self.expr(e);
                self.kept(operand, &later, e)
            })
            .collect()
    }

    /// An operand, copied if it reads a binding that `later` may change.
    fn kept(&mut self, operand: Operand, later: &[hir::LocalId], e: &hir::Expr) -> Operand {
        let changed =
            |this: &Self, place: &Place| later.iter().any(|&l| this.hir_locals[l.0 as usize] == Some(place.local));
        match &operand {
            Operand::Copy(place) if changed(self, place) => self.copy_operand(operand, e.ty, e.span),
            _ => operand,
        }
    }

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
                Proj::InBounds(index) => {
                    let copy = self.new_local(Ty::INT);
                    self.assign(Place::local(copy), Rvalue::Use(Operand::local(index)), span);
                    Proj::InBounds(copy)
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
                let pieces = vec![
                    Piece::Text("one call cannot lend an element twice: two of its arguments reach index ".into()),
                    Piece::Value(Operand::local(a), Ty::INT),
                    Piece::Text(" of the same array".into()),
                ];
                let message = self.temp(Ty::STRING, Rvalue::Concat(pieces), span);
                self.panic_with(4, message, span);
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
    /// Returns the value of the expression at `span`, which a debugger
    /// stops at.
    fn emit_return(&mut self, value: Operand, span: Span) {
        let value =
            if self.fallible() { self.variant(self.function.result, OK, vec![value], Span::default()) } else { value };
        self.exit(value, span);
    }

    /// Returns `err(error)` from a fallible function.
    fn emit_fail(&mut self, error: Operand, span: Span) {
        let value = self.variant(self.function.result, ERR, vec![error], Span::default());
        self.exit(value, span);
    }

    fn exit(&mut self, value: Operand, span: Span) {
        let operand = match self.ret {
            Some(ret) => {
                self.store(Place::local(ret), value, span);
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
        // Each statement's place: its block's among those of the statement
        // around it, then its own.
        let around = self.path.clone();
        let count = self.blocks_of.entry(around.clone()).or_insert(0);
        let ordinal = *count;
        *count += 1;
        let at = |index: usize| -> Vec<u32> { around.iter().copied().chain([ordinal, index as u32]).collect() };
        for (index, stmt) in block.stmts.iter().enumerate() {
            self.path = at(index);
            let outer = self.again.take();
            self.in_frame(|b| b.stmt(stmt));
            self.leave_statement(outer);
        }
        self.path = at(block.stmts.len());
        let mut value = match &block.tail {
            Some(tail) => {
                let outer = self.again.take();
                self.restart_point(tail.span, "tail");
                self.start(tail.span);
                let value = self.expr(tail);
                self.leave_statement(outer);
                value
            }
            None => Operand::Const(Const::Unit),
        };
        self.path = around;
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

    /// Back in the statement around one that ends: a binding the inner one
    /// gave away, the outer gave away too.
    fn leave_statement(&mut self, outer: Option<Again>) {
        let inner = std::mem::replace(&mut self.again, outer);
        if let (Some(outer), Some(inner)) = (&mut self.again, inner) {
            outer.moved |= inner.moved;
        }
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        self.restart_point(stmt_span(stmt), stmt_kind(stmt));
        // A loop starts again at its head, each time it checks.
        if !matches!(stmt, hir::Stmt::Loop { .. } | hir::Stmt::ForEach { .. } | hir::Stmt::ForRange { .. }) {
            self.start(stmt_span(stmt));
        }
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
            // The context has a new value until the block ends, which
            // restores what the call saved (11.16).
            hir::Stmt::ContextSet { context, value, fields, span } => {
                let made = self.expr(value);
                let saved = self.new_local(Ty::int(crate::types::IntTy::U64));
                let set = Rvalue::Call(hir::Callee::Std(hir::StdFn::ContextSet(*context, *fields)), vec![made]);
                self.assign(Place::local(saved), set, *span);
                self.scopes.last_mut().unwrap().push(Exit::Restore(saved));
            }
            hir::Stmt::LockSync { guard, inner, handle, mode, span } => {
                let handle = self.expr(handle);
                let guard = self.bind(*guard);
                let lock = Rvalue::TaskOp(hir::TaskOp::Lock(*mode), vec![handle], vec![false], *span);
                self.assign(Place::local(guard), lock, *span);
                // A cancelled task gets an empty guard, which releases nothing.
                self.scopes.last_mut().unwrap().push(Exit::Release(guard));
                if *mode != hir::LockMode::Try {
                    self.cancellation_point(*span, true);
                }
                let inner = self.bind(*inner);
                self.push(Statement::LendAt(inner, Operand::local(guard)));
            }
            hir::Stmt::AwaitChange(guard, span) => {
                let guard = self.bind(*guard);
                let wait = Rvalue::TaskOp(hir::TaskOp::SyncAwait, vec![Operand::local(guard)], vec![false], *span);
                self.assign(Place::local(guard), wait, *span);
                // It let go of the lock.
                self.cancellation_point(*span, false);
            }
            hir::Stmt::Destructure(pattern, value) => {
                let value = self.expr(value);
                let place = self.place_of(value, pattern.ty);
                self.bind_pattern(pattern, &place);
            }
            hir::Stmt::Assign { local, path, op, value, span } => {
                // The location (and its indices) is resolved before the value (13.8).
                let place = self.place(*local, path);
                // The string at the place grows in place, and releases
                // itself when it cannot.
                if let Some(rest) = appended(value, *local, path, self.types).filter(|_| op.is_none()) {
                    let mut pieces = vec![Piece::Taken(place.clone())];
                    pieces.extend(self.pieces(rest));
                    self.assign(place, Rvalue::Concat(pieces), *span);
                    return;
                }
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
                // A discarded place is still read, for its checks.
                if let Operand::Copy(place) = self.expr(value)
                    && !place.path.is_empty()
                {
                    self.temp(value.ty, Rvalue::Use(Operand::Copy(place)), value.span);
                }
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
                let span = value.as_ref().map_or(Span::default(), |v| v.span);
                let value = value.as_ref().map_or(Operand::Const(Const::Unit), |v| self.expr(v));
                self.emit_return(value, span);
            }
            hir::Stmt::Fail(error) => {
                let span = error.span;
                let error = self.expr(error);
                self.note_failure(span);
                self.emit_fail(error, span);
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
                self.start(stmt_span(stmt));
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
                self.start(stmt_span(stmt));
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
                    // An array's snapshot keeps its length, which the cursor
                    // stays under: no check (but in a live build, whose
                    // frames may move to code that reads them differently).
                    let sequence =
                        matches!(self.types.kind(self.types.repr(array.ty)), TyKind::Array(_) | TyKind::Deque(_));
                    match sequence && self.restart.is_none() {
                        true => Place::local(snapshot).in_bounds(cursor),
                        false => Place::local(snapshot).index(cursor, array.span),
                    }
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
                self.start(stmt_span(stmt));
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

    /// The pieces of an interpolation, those of one inside it among them, as
    /// a generic value's `Display` is.
    fn pieces(&mut self, segments: &[hir::Segment]) -> Vec<Piece> {
        fn flat<'h>(segments: &'h [hir::Segment], out: &mut Vec<&'h hir::Segment>) {
            for segment in segments {
                match segment {
                    hir::Segment::Value(hir::Expr { kind: hir::ExprKind::Interpolate(inner), .. }) => flat(inner, out),
                    segment => out.push(segment),
                }
            }
        }
        let mut segments_flat = Vec::new();
        flat(segments, &mut segments_flat);
        let values: Vec<&hir::Expr> = segments_flat
            .iter()
            .filter_map(|segment| match segment {
                hir::Segment::Value(value) => Some(value),
                hir::Segment::Text(_) => None,
            })
            .collect();
        let mut operands = self.in_order(values.iter().copied()).into_iter();
        segments_flat
            .into_iter()
            .map(|segment| match segment {
                hir::Segment::Text(text) => Piece::Text(text.clone()),
                hir::Segment::Value(value) => Piece::Value(operands.next().expect("one per value"), value.ty),
            })
            .collect()
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
        let later = later_written(&args.iter().collect::<Vec<_>>());
        for (index, arg) in args.iter().enumerate() {
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
            let operand = match (reads_lent, loan) {
                (true, _) => self.copy_operand(operand, arg.ty, arg.span),
                (false, false) => self.kept(operand, &later[index], arg),
                (false, true) => operand,
            };
            operands.push(operand);
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
                    if let Some(again) = &mut self.again {
                        again.moved = true;
                    }
                    self.push(Statement::Clear(local));
                    return moved;
                }
                Operand::local(local)
            }
            hir::ExprKind::Aggregate(items) => {
                let components = self.in_order(items);
                self.aggregate(e.ty, components, e.span)
            }
            hir::ExprKind::Record(values) => {
                // Evaluate in source order, then place in declaration order.
                let mut components = vec![Operand::Const(Const::Unit); values.len()];
                let evaluated = self.in_order(values.iter().map(|(_, value)| value));
                for ((index, _), value) in values.iter().zip(evaluated) {
                    components[*index as usize] = value;
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
            // The task's value of the context, read in place: the task's
            // contexts hold its `Ref` while the code runs (11.16).
            hir::ExprKind::ContextRead(context, reference) => {
                let held = self.new_local(*reference);
                let get = Rvalue::Call(hir::Callee::Std(hir::StdFn::ContextGet(*context)), Vec::new());
                self.push(Statement::Assign(Place::local(held), Box::new(get), e.span));
                Operand::Copy(Place::local(held).deref())
            }
            hir::ExprKind::RefValue(reference) => {
                let value = self.expr(reference);
                Operand::Copy(self.place_of(value, reference.ty).deref())
            }
            hir::ExprKind::Variant(variant, values) => {
                let count = self.types.variants(e.ty).expect("enum")[*variant as usize].len();
                let mut components = vec![Operand::Const(Const::Unit); count];
                let evaluated = self.in_order(values.iter().map(|(_, value)| value));
                for ((index, _), value) in values.iter().zip(evaluated) {
                    components[*index as usize] = value;
                }
                self.variant(e.ty, *variant, components, e.span)
            }
            hir::ExprKind::Match(scrutinee, arms) => self.match_expr(e, scrutinee, arms),
            hir::ExprKind::Array(items) => {
                let TyKind::Array(element) = *self.types.kind(e.ty) else { unreachable!() };
                let values: Vec<Operand> = self.in_order(items);
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
                let operands = self.in_order(operands);
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
                let operands = self.in_order(operands);
                self.temp(e.ty, Rvalue::StrOp(*op, operands, e.span), e.span)
            }
            hir::ExprKind::Pointer(op, operands) => {
                let types: Vec<Ty> = operands.iter().map(|o| o.ty).collect();
                let operands: Vec<Operand> = self.in_order(operands);
                let retain = *op == hir::PointerOp::Write && self.move_flags(&operands[1..], &[true], &types[1..])[0];
                self.temp(e.ty, Rvalue::Pointer(*op, operands, retain), e.span)
            }
            hir::ExprKind::Map(entries) => {
                let map = self.temp(e.ty, Rvalue::MapOp(hir::MapOp::New, Vec::new(), e.span), e.span);
                let Operand::Copy(place) = map.clone() else { unreachable!() };
                let TyKind::Map(_, value) = *self.types.kind(e.ty) else { unreachable!() };
                let result = self.types.lookup(&TyKind::Option(value));
                for (key, value) in entries {
                    let operands = self.in_order([key, value]);
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
                let operands = self.in_order(operands);
                self.temp(e.ty, Rvalue::MapOp(*op, operands, e.span), e.span)
            }
            hir::ExprKind::Mutate(mutation, place, values) => {
                let place = self.place(place.local, &place.path);
                let types: Vec<Ty> = values.iter().map(|v| v.ty).collect();
                let operands: Vec<Operand> = self.in_order(values);
                // Inserted values move into the collection; positions and
                // looked-up keys do not.
                let flags = self.move_flags(&operands, &mutation.moves(operands.len()), &types);
                self.temp(e.ty, Rvalue::Mutate(*mutation, place, operands, flags, e.span), e.span)
            }
            hir::ExprKind::Try(inner) => {
                let value = self.expr(inner);
                let place = self.place_of(value, inner.ty);
                if let TyKind::Result(_, Ty::NEVER) = *self.types.kind(inner.ty) {
                    return Operand::Copy(place.variant_field(OK, 0));
                }
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
                    self.emit_return(none, e.span);
                } else {
                    let TyKind::Result(_, error_ty) = *self.types.kind(inner.ty) else { unreachable!() };
                    let error = self.widen(Operand::Copy(place.variant_field(ERR, 0)), error_ty, e.span);
                    self.note_failure(e.span);
                    self.emit_fail(error, e.span);
                }
                self.switch_to(next);
                let payload = place.variant_field(success, 0);
                // A temporary gives its payload up rather than lend it.
                match place.path.is_empty() && self.owned(place.local) && self.needs_drop(self.place_ty(&payload)) {
                    true => self.move_out(payload, e.span),
                    false => Operand::Copy(payload),
                }
            }
            hir::ExprKind::FnValue(id, _) => self.temp(e.ty, Rvalue::FnValue(*id), e.span),
            hir::ExprKind::ExternFn(id) => self.temp(e.ty, Rvalue::ExternFn(*id), e.span),
            // A function or a closure without captures: nothing to evaluate.
            hir::ExprKind::Callback(inner, id) => {
                let closure = !matches!(inner.kind, hir::ExprKind::FnValue(..));
                self.temp(e.ty, Rvalue::Callback(*id, closure), e.span)
            }
            hir::ExprKind::Layout(ty, align) => self.temp(Ty::INT, Rvalue::Layout(*ty, *align), e.span),
            hir::ExprKind::Reflect(op, ty, values) => {
                let operands: Vec<Operand> = self.in_order(values);
                // A boxed value moves into the box, or is retained there.
                let retain = *op == hir::ReflectOp::Box && self.move_flags(&operands, &[true], &[values[0].ty])[0];
                self.temp(e.ty, Rvalue::Reflect(*op, *ty, operands, retain), e.span)
            }
            hir::ExprKind::Fixed(value) => {
                let value = self.expr(value);
                self.temp(e.ty, Rvalue::Fixed(value, e.span), e.span)
            }
            hir::ExprKind::CursorNext(cursor, kind) => self.cursor_next(cursor, *kind, e),
            hir::ExprKind::CallValue(callee, args) => {
                let callee = self.expr(callee);
                let (operands, regions) = self.call_operands(args, e.span);
                let result = self.temp(e.ty, Rvalue::CallIndirect(callee, operands), e.span);
                self.finish_call(e.ty, regions);
                if e.ty != Ty::NEVER {
                    self.cancellation_point(e.span, true);
                }
                result
            }
            hir::ExprKind::Erase(closure, body) => {
                let ty = closure.ty;
                let value = self.expr(closure);
                let retain = self.move_flags(std::slice::from_ref(&value), &[true], &[ty])[0];
                self.temp(e.ty, Rvalue::Erase(value, *body, retain), e.span)
            }
            hir::ExprKind::TestMembers(members) => {
                let mut values = Vec::new();
                for (node, member) in members {
                    let value = self.expr(member);
                    let retain = self.move_flags(std::slice::from_ref(&value), &[true], &[member.ty])[0];
                    values.push((*node, value, retain));
                }
                self.temp(Ty::UNIT, Rvalue::TestMembers(values), e.span)
            }
            hir::ExprKind::Spawn(body) => self.spawn(e, body, false),
            // `spawn detached`: the task starts detached, so that it never
            // belongs to a `recover` nor holds a deadline (11.11, 11.17).
            hir::ExprKind::TaskOp(hir::TaskOp::Detach, operands)
                if let hir::ExprKind::Spawn(body) = &operands[0].kind =>
            {
                let join = self.spawn(&operands[0], body, true);
                self.temp(e.ty, Rvalue::TaskOp(hir::TaskOp::Detach, vec![join], vec![false], e.span), e.span)
            }
            hir::ExprKind::TaskOp(hir::TaskOp::GroupWait, operands) => self.group_wait(e, &operands[0]),
            hir::ExprKind::Select(arms, otherwise) => self.select(e, arms, otherwise.as_deref()),
            hir::ExprKind::TaskOp(op, operands) => {
                let types: Vec<Ty> = operands.iter().map(|o| o.ty).collect();
                let values: Vec<Operand> = self.in_order(operands);
                // A join moves into a group, a value into a channel, and a
                // state into its cell.
                let moves: Vec<bool> = (0..values.len()).map(|i| op.moves_in(i)).collect();
                let flags = self.move_flags(&values, &moves, &types);
                let result = self.temp(e.ty, Rvalue::TaskOp(*op, values, flags, e.span), e.span);
                if op.waits() {
                    // A barrier counted the task.
                    self.cancellation_point(e.span, *op != hir::TaskOp::BarrierWait);
                }
                result
            }
            hir::ExprKind::Call(callee, _, args) => {
                let (operands, regions) = self.call_operands(args, e.span);
                let result = self.temp(e.ty, Rvalue::Call(*callee, operands), e.span);
                self.finish_call(e.ty, regions);
                if e.ty != Ty::NEVER && self.call_waits(*callee) {
                    // A virtual clock moved.
                    let moved = matches!(callee, hir::Callee::Std(hir::StdFn::ClockSettle | hir::StdFn::ClockAdvance));
                    self.cancellation_point(e.span, !moved);
                }
                result
            }
            hir::ExprKind::Unary(op, operand) => {
                let operand = self.expr(operand);
                self.temp(e.ty, Rvalue::Unary(*op, operand), e.span)
            }
            hir::ExprKind::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                let sides = (left.span, right.span);
                let result = self.new_local(Ty::BOOL);
                let left = self.expr(left);
                let (rhs, short, join) = (self.new_block(), self.new_block(), self.new_block());
                self.way(Some(rhs), e.span, sides.1, "evaluated");
                self.way(Some(short), e.span, sides.0, "short-circuited");
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
                let [left, right]: [Operand; 2] = self.in_order([&**left, &**right]).try_into().expect("two operands");
                self.temp(e.ty, Rvalue::Binary(*op, left, right), e.span)
            }
            hir::ExprKind::If(condition, then, otherwise) => self.if_expr(e, condition, then, otherwise.as_deref()),
            hir::ExprKind::Block(block) => self.block(block),
            hir::ExprKind::Interpolate(segments) => {
                let pieces = self.pieces(segments);
                self.temp(Ty::STRING, Rvalue::Concat(pieces), e.span)
            }
            hir::ExprKind::Recover(block, caught) => self.catcher(e, block, Catch::Recover(caught)),
            hir::ExprKind::Within(limit, block, failure) => self.catcher(e, block, Catch::Within(limit, failure)),
            hir::ExprKind::Panic(message, code) => {
                let message = self.expr(message);
                self.panic_with(*code, message, e.span);
                Operand::Const(Const::Unit)
            }
            hir::ExprKind::Assert { condition, parts, source, detail } => {
                let condition = self.expr(condition);
                let parts: Vec<Operand> = parts.iter().map(|p| self.expr(p)).collect();
                let (ok, failed) = (self.new_block(), self.new_block());
                self.terminate(Terminator::Branch(condition, ok, failed));
                self.switch_to(failed);
                // The message, or the condition, then the condition's sides.
                let mut pieces: Vec<Piece> = if parts.is_empty() {
                    vec![Piece::Text(source.clone())]
                } else {
                    let mut pieces: Vec<Piece> = parts.into_iter().map(|p| Piece::Value(p, Ty::STRING)).collect();
                    pieces.push(Piece::Text(format!("\n  condition: {source}")));
                    pieces
                };
                if let Some(detail) = detail {
                    pieces.push(Piece::Value(self.expr(detail), Ty::STRING));
                }
                let message = self.temp(Ty::STRING, Rvalue::Concat(pieces), e.span);
                self.panic_with(7, message, e.span);
                self.switch_to(ok);
                Operand::Const(Const::Unit)
            }
            hir::ExprKind::Error => unreachable!("programs with errors are not lowered"),
        }
    }

    /// `group.wait()` (11.12): the results in the order the tasks were
    /// added or, when they are `R ! E`, the first error. The runtime first
    /// waits for every task or, for results, for the first that fails,
    /// which cancels the others and is then the only one taken. Each result
    /// taken is moved into the array or the error.
    fn group_wait(&mut self, e: &hir::Expr, group: &hir::Expr) -> Operand {
        let span = e.span;
        let item = self.types.handle_result(group.ty);
        let option = self.types.lookup(&TyKind::Option(item)).expect("interned with the group's type");
        let (array, fallible) = match *self.types.kind(e.ty) {
            TyKind::Result(array, _) => (array, true),
            _ => (e.ty, false),
        };
        let group = self.expr(group);
        let settle = Rvalue::TaskOp(hir::TaskOp::GroupSettle, vec![group.clone()], vec![false], span);
        let index = self.temp(Ty::INT, settle, span);
        self.cancellation_point(span, true);
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
            let values: Vec<Operand> = self.in_order(operands);
            let retain = op == Some(hir::TaskOp::Send) && {
                let ty = operands[1].ty;
                self.needs_drop(ty) && matches!(values[1], Operand::Copy(_)) && !self.take_temporary(&values[1])
            };
            let out = self.new_local(arm.operation.ty);
            cases.push(SelectCase { op, operands: values, out, retain });
        }
        let outs: Vec<Local> = cases.iter().map(|c| c.out).collect();
        let won = self.temp(Ty::INT, Rvalue::Select(cases, otherwise.is_none(), span), span);
        self.cancellation_point(span, true);
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
        let first = then.tail.as_ref().map(|tail| tail.span).filter(|_| then.stmts.is_empty());
        self.way(Some(then_block), e.span, first.unwrap_or(e.span), "then");
        self.way(Some(else_block), e.span, otherwise.map_or(e.span, |o| o.span), "else");
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

/// Where a statement is, for its line's probe: a restart point comes first
/// on the line, after the probe.
fn stmt_span(stmt: &hir::Stmt) -> Span {
    match stmt {
        hir::Stmt::Let(_, value)
        | hir::Stmt::Destructure(_, value)
        | hir::Stmt::Expr(value)
        | hir::Stmt::Fail(value)
        | hir::Stmt::Return(Some(value)) => value.span,
        hir::Stmt::Assign { span, .. }
        | hir::Stmt::AssignSlice { span, .. }
        | hir::Stmt::LockSync { span, .. }
        | hir::Stmt::ContextSet { span, .. }
        | hir::Stmt::AwaitChange(_, span) => *span,
        hir::Stmt::Defer { closure, .. } => closure.span,
        hir::Stmt::Loop { condition: Some(condition), .. } => condition.span,
        hir::Stmt::ForEach { source, .. } => source.span,
        hir::Stmt::ForRange { start, .. } => start.span,
        _ => Span::default(),
    }
}

/// What a statement is, for matching it with one of the new code.
fn stmt_kind(stmt: &hir::Stmt) -> &'static str {
    match stmt {
        hir::Stmt::Let(..) => "let",
        hir::Stmt::Destructure(..) => "let",
        hir::Stmt::Assign { .. } | hir::Stmt::AssignSlice { .. } => "assign",
        hir::Stmt::Defer { .. } => "defer",
        hir::Stmt::Lend { .. } => "lend",
        hir::Stmt::LockSync { .. } | hir::Stmt::AwaitChange(..) => "lock",
        hir::Stmt::ContextSet { .. } => "with",
        hir::Stmt::Expr(_) => "expr",
        hir::Stmt::Return(_) => "return",
        hir::Stmt::Fail(_) => "fail",
        hir::Stmt::Break => "break",
        hir::Stmt::Continue => "continue",
        hir::Stmt::Loop { .. } | hir::Stmt::ForEach { .. } | hir::Stmt::ForRange { .. } => "for",
    }
}

/// For each expression, the bindings the ones after it may change.
fn later_written(exprs: &[&hir::Expr]) -> Vec<Vec<hir::LocalId>> {
    let mut later = vec![Vec::new(); exprs.len()];
    for at in (0..exprs.len().saturating_sub(1)).rev() {
        let mut changed = later[at + 1].clone();
        hir::written(exprs[at + 1], &mut changed);
        later[at] = changed;
    }
    later
}
