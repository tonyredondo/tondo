//! Room for what a loop pushes, before it starts. An array that a counted
//! loop pushes to once in each turn, as `for item in items { out.push(..) }`
//! does and as `map`, `zip` and `enumerate` build theirs, takes room for
//! every turn when the loop starts, rather than growing as it goes: one
//! allocation where growing takes one each time the length doubles. The
//! turns are `last - start`, and one more over `a..=b`, which wrap to a
//! negative count, and so to no room, past an `Int`. A loop
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
    crate::parallel::each_mut(bodies, |body| {
        let noting = crate::mir_text::noting(&body.name);
        let (found, grown) = loops(body, types);
        for (span, why) in grown {
            crate::mir_text::remark(body, span, "grown", || format!("grows as it is pushed to: {why}"));
        }
        for Found { before, array, counter, last, from_zero, inclusive, span } in found {
            crate::mir_text::remark(body, span, "reserved", || "takes room for every turn before the loop".into());
            if noting {
                body.notes.push(format!("reserve: l{} takes room for l{} pushes after b{before}", array.0, last.0));
            }
            let mut new_local = |ty: Ty| {
                body.locals.push(ty);
                body.names.push(None);
                Local(body.locals.len() as u32 - 1)
            };
            // The turns, `last - counter`, and one more for `a..=b`, as the
            // counter starts: a count past an `Int`, or none, wraps to a
            // negative one, which takes no room.
            let mut stmts = Vec::new();
            let mut turns = last;
            if !from_zero {
                let difference = new_local(Ty::INT);
                let less = Rvalue::Exact(BinOp::Sub, Operand::local(last), Operand::local(counter));
                stmts.push(Statement::Assign(Place::local(difference), Box::new(less), span));
                turns = difference;
            }
            if inclusive {
                let more = new_local(Ty::INT);
                let one = Rvalue::Exact(BinOp::Add, Operand::local(turns), Operand::Const(Const::Int(1, Ty::INT)));
                stmts.push(Statement::Assign(Place::local(more), Box::new(one), span));
                turns = more;
            }
            let done = new_local(Ty::UNIT);
            let room =
                Rvalue::Mutate(Mutation::Reserve, Place::local(array), vec![Operand::local(turns)], vec![false], span);
            stmts.push(Statement::Assign(Place::local(done), Box::new(room), span));
            body.blocks[before].stmts.extend(stmts);
        }
    });
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

/// A push to an array once in each turn of a counted loop that leaves only
/// by its test or by unwinding: the block before the loop, the array, the
/// counter and its bound, whether the counter starts at 0, whether the
/// loop runs the turn of the bound too, and where the push is.
struct Found {
    before: usize,
    array: Local,
    counter: Local,
    last: Local,
    from_zero: bool,
    inclusive: bool,
    span: Span,
}

/// Why the pushes it leaves are, with where they are, for `tondo
/// optimizations`.
type Grown = Vec<(Span, &'static str)>;

fn loops(body: &Body, types: &Types) -> (Vec<Found>, Grown) {
    let mut grown = Vec::new();
    let blocks = &body.blocks;
    let pushes = |statement: &Statement| matches!(statement, Statement::Assign(_, rvalue, _) if matches!(**rvalue, Rvalue::Mutate(Mutation::Push, ..)));
    if !blocks.iter().flat_map(|block| &block.stmts).any(pushes) {
        return (Vec::new(), grown);
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
    for Counted { head, step, entry, before, counter, last, inside, latch } in counted_loops(blocks) {
        // A way in from a block that goes to the loop alone, with a bound
        // that is an `Int`, as the runtime takes it.
        if !matches!(blocks[before].terminator, Terminator::Goto(_))
            || types.repr(body.locals[last.0 as usize]) != Ty::INT
        {
            continue;
        }
        let members: Vec<usize> = (0..blocks.len()).filter(|&b| inside[b]).collect();
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
        // A push inside a loop within this one is that loop's to tell.
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
        let repeats = |pushing: usize| {
            let again = successors(&blocks[pushing].terminator);
            again.contains(&pushing) || reached(again, usize::MAX)[pushing]
        };
        pushes.retain(|&(pushing, ..)| !repeats(pushing));
        let arrays = |pushes: &[(usize, Local, Span)]| {
            pushes
                .iter()
                .filter(|(_, array, _)| {
                    matches!(types.kind(types.repr(body.locals[array.0 as usize])), TyKind::Array(_))
                })
                .map(|&(_, _, span)| span)
                .collect::<Vec<_>>()
        };
        // Out only by the test, its latch's for `a..=b`, or by unwinding:
        // not to a block that cannot be reached, as a `match` that covers
        // every case leaves.
        let leaves = members.iter().any(|&block| {
            Some(block) != latch
                && successors(&blocks[block].terminator).into_iter().any(|to| {
                    to != head
                        && !inside[to]
                        && abnormal(&blocks[block]) != Some(to)
                        && !matches!(blocks[to].terminator, Terminator::Unreachable)
                })
        });
        if leaves {
            grown.extend(arrays(&pushes).into_iter().map(|span| (span, "the loop may leave before its last turn")));
            continue;
        }
        let counted_once = changed.iter().filter(|&&local| local == counter).count() == 1;
        // From 0: its one assignment but the step's is `counter = 0`.
        let from_zero = assigned[counter.0 as usize].iter().any(|statement| {
            matches!(statement, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Use(Operand::Const(Const::Int(0, _)))))
        });
        if changed.contains(&last) || !counted_once || assigned[counter.0 as usize].len() != 2 {
            let why = "the loop does not count its turns to an end it keeps";
            grown.extend(arrays(&pushes).into_iter().map(|span| (span, why)));
            continue;
        }
        for &(pushing, array, span) in &pushes {
            let is_array = matches!(types.kind(types.repr(body.locals[array.0 as usize])), TyKind::Array(_));
            let once = pushes.iter().filter(|push| push.1 == array).count() == 1;
            // Once in every turn: no way from the entry to the step goes
            // around it.
            let around = reached(vec![entry], pushing)[step];
            let why = match () {
                _ if !is_array => continue,
                _ if !once => "the loop pushes to it more than once a turn",
                _ if changed.contains(&array) => "the loop changes it in other ways too",
                _ if around => "the loop pushes to it in some turns only",
                _ => {
                    let inclusive = latch.is_some();
                    found.push(Found { before, array, counter, last, from_zero, inclusive, span });
                    continue;
                }
            };
            grown.push((span, why));
        }
    }
    (found, grown)
}
