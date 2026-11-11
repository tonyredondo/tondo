//! Room for what a loop pushes, before it starts. An array that a counted
//! loop pushes to once in each turn, as `for item in items { out.push(..) }`
//! does and as `map`, `zip` and `enumerate` build theirs, takes room for
//! every turn when the loop starts, rather than growing as it goes: one
//! allocation where growing takes one each time the length doubles. A loop
//! that may leave before its last turn, by `break` or `return`, or that
//! pushes in some turns only, as `filter` does, is left to grow, so that no
//! array takes much more room than its elements. The room is a hint, which
//! the runtime takes when the system has it (`tondo_rt_array_reserve`).

use crate::ast::BinOp;
use crate::hir::Mutation;
use crate::mir::*;
use crate::source::Span;
use crate::types::{Ty, TyKind, Types};

pub fn reserve(bodies: &mut [Body], types: &Types) {
    for body in bodies {
        let noting = crate::mir_text::noting(&body.name);
        for (before, array, last, span) in loops(body, types) {
            if noting {
                body.notes.push(format!("reserve: l{} takes room for l{} pushes after b{before}", array.0, last.0));
            }
            body.locals.push(Ty::UNIT);
            body.names.push(None);
            let done = Local(body.locals.len() as u32 - 1);
            let room =
                Rvalue::Mutate(Mutation::Reserve, Place::local(array), vec![Operand::local(last)], vec![false], span);
            body.blocks[before].stmts.push(Statement::Assign(Place::local(done), Box::new(room), span));
        }
    }
}

/// Where a block goes when its task unwinds a panic or is cancelled, as the
/// checks that end it say, rather than on.
fn abnormal(block: &BlockData) -> Option<usize> {
    let Terminator::Branch(Operand::Copy(condition), to, _) = &block.terminator else { return None };
    let defined = |local: Local| {
        block.stmts.iter().rev().find_map(|statement| match statement {
            Statement::Assign(place, rvalue, _) if place.path.is_empty() && place.local == local => Some(&**rvalue),
            _ => None,
        })
    };
    let asked = match defined(condition.local)? {
        Rvalue::Unwinding => true,
        Rvalue::Binary(BinOp::Eq, Operand::Copy(flag), Operand::Const(_)) => {
            matches!(defined(flag.local), Some(Rvalue::Cancelled))
        }
        _ => false,
    };
    (asked && condition.path.is_empty()).then_some(to.0 as usize)
}

/// The counted loops from 0 that push to an array once in each turn and
/// leave only by their test or by unwinding: the block before each, the
/// array, the bound of the counter, and where the push is.
fn loops(body: &Body, types: &Types) -> Vec<(usize, Local, Local, Span)> {
    let blocks = &body.blocks;
    let pushes = |statement: &Statement| matches!(statement, Statement::Assign(_, rvalue, _) if matches!(**rvalue, Rvalue::Mutate(Mutation::Push, ..)));
    if !blocks.iter().flat_map(|block| &block.stmts).any(pushes) {
        return Vec::new();
    }
    // The statements that assign each local.
    let mut assigned: Vec<Vec<&Statement>> = vec![Vec::new(); body.locals.len()];
    let (mut used, mut defined) = (Vec::new(), Vec::new());
    for statement in blocks.iter().flat_map(|block| &block.stmts) {
        statement_locals(statement, &mut used, &mut defined);
        used.clear();
        defined.drain(..).for_each(|local| assigned[local.0 as usize].push(statement));
    }
    let mut found = Vec::new();
    for Counted { head, step, entry, before, counter, last, inside } in counted_loops(blocks) {
        // A way in from a block that goes to the loop alone, with a bound
        // that is an `Int`, as the runtime takes it.
        if !matches!(blocks[before].terminator, Terminator::Goto(_))
            || types.repr(body.locals[last.0 as usize]) != Ty::INT
        {
            continue;
        }
        let members: Vec<usize> = (0..blocks.len()).filter(|&b| inside[b]).collect();
        // Out only by the test, or by unwinding.
        let leaves = members.iter().any(|&block| {
            successors(&blocks[block].terminator)
                .into_iter()
                .any(|to| to != head && !inside[to] && abnormal(&blocks[block]) != Some(to))
        });
        if leaves {
            continue;
        }
        // Neither the counter nor its bound changed but by the step, and
        // for each array, its one push, where the loop changes it in no
        // other way.
        let mut pushes: Vec<(usize, Local, Span)> = Vec::new();
        let mut changed = Vec::new();
        for &block in &members {
            for statement in &blocks[block].stmts {
                if let Statement::Assign(_, rvalue, span) = statement
                    && let Rvalue::Mutate(mutation, target, ..) = &**rvalue
                {
                    match (mutation, target.path.is_empty()) {
                        (Mutation::Push, true) => pushes.push((block, target.local, *span)),
                        _ => changed.push(target.local),
                    }
                }
                let (mut used, mut defined) = (Vec::new(), Vec::new());
                statement_locals(statement, &mut used, &mut defined);
                changed.append(&mut defined);
            }
        }
        let counted_once = changed.iter().filter(|&&local| local == counter).count() == 1;
        // From 0: its one assignment but the step's is `counter = 0`.
        let from_zero = assigned[counter.0 as usize].iter().any(|statement| {
            matches!(statement, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Use(Operand::Const(Const::Int(0, _)))))
        });
        if changed.contains(&last) || !counted_once || assigned[counter.0 as usize].len() != 2 || !from_zero {
            continue;
        }
        for &(pushing, array, span) in &pushes {
            let is_array = matches!(types.kind(types.repr(body.locals[array.0 as usize])), TyKind::Array(_));
            let once = pushes.iter().filter(|push| push.1 == array).count() == 1;
            // Once in every turn: no way from the entry to the step goes
            // around it, nor from it back to it but through the head.
            let reached = |from: Vec<usize>, avoiding: usize| {
                let mut seen = vec![false; blocks.len()];
                let mut pending = from;
                while let Some(block) = pending.pop() {
                    if block == avoiding || !inside[block] || std::mem::replace(&mut seen[block], true) {
                        continue;
                    }
                    pending.extend(successors(&blocks[block].terminator));
                }
                seen
            };
            let around = reached(vec![entry], pushing)[step];
            let again = successors(&blocks[pushing].terminator);
            let repeats = again.contains(&pushing) || reached(again, usize::MAX)[pushing];
            if is_array && once && !changed.contains(&array) && !around && !repeats {
                found.push((before, array, last, span));
            }
        }
    }
    found
}
