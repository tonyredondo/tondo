//! Copies of values the function never reads again, as inlining leaves a
//! call's result: `a = value` then `b = a`, with `a` read nowhere after,
//! becomes `b = value`. A value that lives in memory, as an enum's, was
//! copied whole each time, two or three times for `math.sqrt(x)`'s result.
//! Only values without references to count go: a counted one would also
//! have to move its releases.

use crate::mir::*;
use crate::types::{TyKind, Types};

pub fn elide(bodies: &mut [Body], types: &Types) {
    crate::parallel::each_mut(bodies, |body| {
        // A chain of copies goes in one round; a second finds what the
        // first made possible.
        for _ in 0..2 {
            if !elide_once(body, types) {
                break;
            }
        }
    });
}

/// A value kept in memory, whose copies cost: what Cranelift holds in
/// registers it copies for nothing already.
fn in_memory(types: &Types, ty: crate::types::Ty) -> bool {
    matches!(types.kind(types.repr(ty)), TyKind::Tuple(_) | TyKind::Adt(_) | TyKind::Option(_) | TyKind::Result(..))
}

fn whole(place: &Place) -> Option<Local> {
    place.path.is_empty().then_some(place.local)
}

fn elide_once(body: &mut Body, types: &Types) -> bool {
    let candidate = |statement: &Statement| match statement {
        Statement::Assign(target, value, _) => {
            target.path.is_empty()
                && matches!(&**value, Rvalue::Use(Operand::Copy(source)) if source.path.is_empty())
                && in_memory(types, body.locals[target.local.0 as usize])
        }
        _ => false,
    };
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(candidate) {
        return false;
    }
    let live = live_in(&body.blocks, body.locals.len());
    // What is lent keeps its storage, as a loan holds its address; and a
    // binding that borrows writes through it to what it borrows.
    let mut lent = vec![false; body.locals.len()];
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Lend(alias, place) => {
                lent[place.local.0 as usize] = true;
                lent[alias.0 as usize] = true;
            }
            Statement::LendAt(alias, _) => lent[alias.0 as usize] = true,
            _ => {}
        }
    }
    // And what the parameters, which may be the caller's own through a
    // loan, and the result hold is seen after the function returns: they
    // may take a value directly, but never give it up.
    let mut seen = lent.clone();
    for local in body.params.iter().chain(&body.ret) {
        seen[local.0 as usize] = true;
    }
    let locals = |statement: &Statement| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        (used, defined)
    };
    let mut changed = false;
    for block in 0..body.blocks.len() {
        if !body.blocks[block].stmts.iter().any(candidate) {
            continue;
        }
        let after: std::collections::HashSet<Local> = match &body.blocks[block].terminator {
            Terminator::Goto(to) => live[to.0 as usize].iter().copied().collect(),
            Terminator::Branch(_, then, otherwise) => {
                live[then.0 as usize].iter().chain(&live[otherwise.0 as usize]).copied().collect()
            }
            _ => Default::default(),
        };
        let mut terminator_used = Vec::new();
        terminator_uses(&body.blocks[block].terminator, &mut terminator_used);
        // What each statement reads and writes, kept as the block changes.
        let mut effects: Vec<(Vec<Local>, Vec<Local>)> = body.blocks[block].stmts.iter().map(locals).collect();
        let mut copy = 0;
        while copy < body.blocks[block].stmts.len() {
            let statements = &body.blocks[block].stmts;
            let Statement::Assign(target, value, _) = &statements[copy] else {
                copy += 1;
                continue;
            };
            let (Some(to), Rvalue::Use(Operand::Copy(source))) = (whole(target), &**value) else {
                copy += 1;
                continue;
            };
            let Some(from) = whole(source) else {
                copy += 1;
                continue;
            };
            let ty = body.locals[from.0 as usize];
            if from == to
                || ty != body.locals[to.0 as usize]
                || !in_memory(types, ty)
                || types.needs_drop(ty)
                || seen[from.0 as usize]
                || lent[to.0 as usize]
            {
                copy += 1;
                continue;
            }
            // Where `from` took the value, with neither read nor written
            // since, and the value made without reading `to`.
            let mut made = None;
            for index in (0..copy).rev() {
                let (used, defined) = &effects[index];
                if defined.contains(&from) {
                    let whole_value =
                        matches!(&statements[index], Statement::Assign(place, ..) if place.path.is_empty());
                    made = (whole_value && !used.contains(&to)).then_some(index);
                    break;
                }
                if used.contains(&from) || used.contains(&to) || defined.contains(&to) {
                    break;
                }
            }
            let Some(made) = made else {
                copy += 1;
                continue;
            };
            // `from` is not read again before it takes another value.
            let read_later = effects[copy + 1..]
                .iter()
                .find_map(|(used, defined)| {
                    if used.contains(&from) {
                        Some(true)
                    } else if defined.contains(&from) {
                        Some(false)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| terminator_used.contains(&from) || after.contains(&from));
            if read_later {
                copy += 1;
                continue;
            }
            let statements = &mut body.blocks[block].stmts;
            if let Statement::Assign(place, ..) = &mut statements[made] {
                place.local = to;
            }
            for local in effects[made].1.iter_mut().filter(|local| **local == from) {
                *local = to;
            }
            statements.remove(copy);
            effects.remove(copy);
            changed = true;
        }
    }
    changed
}
