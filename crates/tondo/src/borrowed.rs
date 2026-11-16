//! Elements of an array the function holds unchanged, read and handed
//! from local to local, but kept nowhere: they need no reference of their
//! own. A lazy chain over records with a text read each record from its
//! array, retained the text, moved it through the adapters' locals, which
//! `split` keeps in registers, and released it at the end of the step: two
//! atomic operations an element, for a value its array held all along.
//!
//! The array is a local nothing changes, lends or lets go but where the
//! function returns; the element is copied out of it and retained at once.
//! Every local the copy reaches through copies whole is given values only
//! by those copies, or zero, and is read only where nothing keeps it: an
//! operation, a comparison, a call, which borrows its arguments, or a copy
//! to another such local. Its retain goes, and so does every release of
//! those locals.

use crate::mir::*;
use crate::types::Types;

pub fn borrowed(bodies: &mut [Body], types: &Types) {
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    crate::parallel::each_mut(bodies, |body| borrowed_in(body, types, &loans));
}

fn borrowed_in(body: &mut Body, types: &Types, loans: &[Vec<bool>]) {
    // How many blocks go to each.
    let mut entries = vec![0usize; body.blocks.len()];
    for block in &body.blocks {
        match &block.terminator {
            Terminator::Goto(to) => entries[to.0 as usize] += 1,
            Terminator::Branch(_, then, otherwise) => {
                entries[then.0 as usize] += 1;
                entries[otherwise.0 as usize] += 1;
            }
            _ => {}
        }
    }
    // A copy of an element followed by its retain, in the same block or in
    // those it goes on to alone, as a check a pass dropped from between
    // them leaves the two.
    let mut sources: Vec<(usize, usize, Local, Local)> = Vec::new();
    for (index, block) in body.blocks.iter().enumerate() {
        for (at, statement) in block.stmts.iter().enumerate() {
            let Statement::Assign(place, rvalue, _) = statement else { continue };
            let Rvalue::Use(Operand::Copy(from)) = &**rvalue else { continue };
            let element = from.path.iter().any(|proj| matches!(proj, Proj::Index(..) | Proj::InBounds(_)));
            if !place.path.is_empty() || !element || !types.needs_drop(body.locals[place.local.0 as usize]) {
                continue;
            }
            let (mut here, mut start) = (index, at + 1);
            for _ in 0..body.blocks.len() {
                let rest = &body.blocks[here].stmts[start..];
                let retained = rest
                    .iter()
                    .position(|s| matches!(s, Statement::Retain(p) if p.path.is_empty() && p.local == place.local));
                if let Some(after) = retained {
                    sources.push((here, start + after, place.local, from.local));
                    break;
                }
                let mut named = Vec::new();
                for statement in rest {
                    let mut defined = Vec::new();
                    statement_locals(statement, &mut named, &mut defined);
                    named.extend(defined);
                }
                match body.blocks[here].terminator {
                    Terminator::Goto(to) if !named.contains(&place.local) && entries[to.0 as usize] == 1 => {
                        (here, start) = (to.0 as usize, 0);
                    }
                    _ => break,
                }
            }
        }
    }
    for (block, retain, local, array) in sources {
        if let Some(reached) = only_read(body, types, loans, local, array, (block, retain)) {
            drop_counts(body, &reached, (block, retain));
        }
    }
}

/// The locals a borrowed element reaches, when it can be borrowed: what
/// holds it is only read, and its array stays as it is wherever it is read.
fn only_read(
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    source: Local,
    array: Local,
    retain: (usize, usize),
) -> Option<Vec<bool>> {
    let count = body.locals.len();
    let mut reached = vec![false; count];
    reached[source.0 as usize] = true;
    // The locals the element is copied to, whole.
    let mut changed = true;
    while changed {
        changed = false;
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            if let Statement::Assign(place, rvalue, _) = statement
                && place.path.is_empty()
                && let Rvalue::Use(Operand::Copy(from)) = &**rvalue
                && from.path.is_empty()
                && reached[from.local.0 as usize]
                && !reached[place.local.0 as usize]
            {
                reached[place.local.0 as usize] = true;
                changed = true;
            }
        }
    }
    // The array and the element are each given a value once.
    let assigned = |local: Local| {
        body.blocks
            .iter()
            .flat_map(|block| &block.stmts)
            .filter(|s| matches!(s, Statement::Assign(place, ..) if place.local == local))
            .count()
    };
    if reached[array.0 as usize] || assigned(array) > 1 || assigned(source) != 1 {
        return None;
    }
    let holds = |local: Local| reached[local.0 as usize];
    let reads = |operand: &Operand| matches!(operand, Operand::Copy(place) if holds(place.local));
    for (index, block) in body.blocks.iter().enumerate() {
        let returns = matches!(block.terminator, Terminator::Return(_));
        let mut array_gone = false;
        for (at, statement) in block.stmts.iter().enumerate() {
            match statement {
                // The source, its retain, and what only zeroes or lets go.
                Statement::Retain(_) if (index, at) == retain => {}
                Statement::Retain(place) if place.local == array => {}
                Statement::Release(place) | Statement::Moved(place) if place.path.is_empty() && holds(place.local) => {}
                Statement::Clear(_) => {}
                // The array, let go only on the way out.
                Statement::Release(place) if place.local == array => {
                    if !returns || !place.path.is_empty() {
                        return None;
                    }
                    array_gone = true;
                }
                Statement::Assign(place, rvalue, _) => {
                    if place.local == array {
                        // Its one value, before any element is read.
                        if !place.path.is_empty() {
                            return None;
                        }
                        continue;
                    }
                    // A call that takes a loan may write what it is lent.
                    let lent: Vec<bool> = match &**rvalue {
                        Rvalue::Call(crate::hir::Callee::Fn(callee), _) => {
                            loans.get(callee.0 as usize).cloned().unwrap_or_default()
                        }
                        Rvalue::CallIndirect(Operand::Copy(callee), _) => {
                            match types.kind(types.repr(body.place_ty(types, callee))) {
                                crate::types::TyKind::Fn(_, _, _, modes, _) => modes
                                    .iter()
                                    .map(|mode| matches!(mode, crate::ast::Mode::Mut | crate::ast::Mode::Var))
                                    .collect(),
                                _ => Vec::new(),
                            }
                        }
                        _ => Vec::new(),
                    };
                    if let Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) = &**rvalue
                        && args.iter().zip(&lent).any(|(arg, &loan)| {
                            loan && matches!(arg, Operand::Copy(p) if holds(p.local) || p.local == array)
                        })
                    {
                        return None;
                    }
                    let mut used = Vec::new();
                    crate::mir::rvalue_uses(rvalue, &mut used);
                    let element_read = used.iter().any(|&local| holds(local));
                    if array_gone && element_read {
                        return None;
                    }
                    if holds(place.local) {
                        // Given a value only by a copy of the element, or
                        // the element itself.
                        let copied = place.path.is_empty()
                            && matches!(&**rvalue, Rvalue::Use(Operand::Copy(from))
                                if from.path.is_empty() && holds(from.local)
                                    || place.local == source && from.local == array);
                        if !copied {
                            return None;
                        }
                        continue;
                    }
                    if !element_read {
                        if matches!(&**rvalue, Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..)
                            if target.local == array)
                        {
                            return None;
                        }
                        continue;
                    }
                    // Read where nothing keeps it.
                    let read_only = match &**rvalue {
                        Rvalue::Unary(..) | Rvalue::Length(_) | Rvalue::Convert(_) => true,
                        Rvalue::Binary(..) | Rvalue::Exact(..) => true,
                        Rvalue::StrOp(..) | Rvalue::MapOp(..) => true,
                        Rvalue::Concat(pieces) => {
                            pieces.iter().all(|piece| !matches!(piece, Piece::Taken(p) if holds(p.local)))
                        }
                        Rvalue::Call(crate::hir::Callee::Fn(_), _) | Rvalue::CallIndirect(..) => true,
                        _ => false,
                    };
                    if !read_only {
                        return None;
                    }
                }
                Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) => {
                    if holds(place.local) || place.local == array {
                        return None;
                    }
                }
                Statement::Lend(alias, place) => {
                    if holds(place.local) || place.local == array || holds(*alias) {
                        return None;
                    }
                }
                Statement::LendAt(alias, operand) => {
                    if reads(operand) || holds(*alias) {
                        return None;
                    }
                }
                Statement::DeferPush(_, place) => {
                    if holds(place.local) {
                        return None;
                    }
                }
                Statement::Failing(operand, ..) => {
                    if reads(operand) {
                        return None;
                    }
                }
                Statement::DeferPop | Statement::OsrLoad(_) => {}
            }
        }
        if let Terminator::Return(operand) = &block.terminator
            && reads(operand)
        {
            return None;
        }
    }
    Some(reached)
}

/// Drops the element's retain and every release of what it reached.
fn drop_counts(body: &mut Body, reached: &[bool], retain: (usize, usize)) {
    for (index, block) in body.blocks.iter_mut().enumerate() {
        let mut at = 0;
        block.stmts.retain(|statement| {
            let here = (index, at);
            at += 1;
            match statement {
                Statement::Retain(_) if here == retain => false,
                Statement::Release(place) => !(place.path.is_empty() && reached[place.local.0 as usize]),
                _ => true,
            }
        });
    }
}
