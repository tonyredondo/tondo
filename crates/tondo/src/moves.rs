//! References an owner gives up, taken instead of counted again: after
//! inlining, a value read out of a local that owns it, as a match binds a
//! payload or a call's result is unwrapped, is retained for where it goes,
//! and the local is released later with nothing having read that part of
//! it since. The retain becomes a move: the part is zeroed in the local,
//! which then releases the rest, and the copy keeps the local's reference.
//! A string decoded from an event went through three atomic operations;
//! now it is moved.
//!
//! The local must own what it holds: neither a parameter, which is the
//! caller's, nor the result, nor what a binding borrows or lends; and on
//! every way on, before the function returns or the part is written, the
//! local is released whole or by a part that holds this one. Nothing on
//! the way may read the part, which is zero then.

use crate::mir::*;

/// How far a way is followed, in statements, before giving up.
const REACH: usize = 4000;

pub fn moves(bodies: &mut [Body]) {
    crate::parallel::each_mut(bodies, moves_in);
}

fn moves_in(body: &mut Body) {
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(|s| matches!(s, Statement::Retain(_))) {
        return;
    }
    // What is not the function's own to give up.
    let mut foreign = vec![false; body.locals.len()];
    for local in body.params.iter().chain(&body.ret) {
        foreign[local.0 as usize] = true;
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Lend(alias, place) => {
                foreign[alias.0 as usize] = true;
                foreign[place.local.0 as usize] = true;
            }
            Statement::LendAt(alias, _) => foreign[alias.0 as usize] = true,
            _ => {}
        }
    }
    for block in 0..body.blocks.len() {
        for at in 0..body.blocks[block].stmts.len() {
            let Statement::Retain(place) = &body.blocks[block].stmts[at] else { continue };
            let Some(source) = copied_from(&body.blocks[block].stmts[..at], place) else { continue };
            if foreign[source.local.0 as usize] || !exact(&source) || !given_up(body, block, at, &source) {
                continue;
            }
            body.blocks[block].stmts[at] = Statement::Moved(source);
        }
    }
    drop_released_moves(body);
}

/// A place reached only through fields and variants, which a zero written
/// there leaves the rest of the local's value as it was.
fn exact(place: &Place) -> bool {
    place.path.iter().all(|projection| matches!(projection, Proj::Field(_) | Proj::Variant(..)))
}

/// Where the value retained at `place` was read from, by the statement
/// before that made it, with nothing in between naming either local.
fn copied_from(before: &[Statement], place: &Place) -> Option<Place> {
    let names = |statement: &Statement, local: Local| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        used.contains(&local) || defined.contains(&local)
    };
    for (at, statement) in before.iter().enumerate().rev() {
        if let Statement::Assign(target, value, _) = statement
            && target.path.is_empty()
            && target.local == place.local
        {
            let operand = match (&**value, &place.path[..]) {
                (Rvalue::Use(operand), []) => operand,
                (Rvalue::Variant(variant, operands), [Proj::Variant(of, index)]) if variant == of => {
                    operands.get(*index as usize)?
                }
                (Rvalue::Aggregate(operands), [Proj::Field(index)]) => operands.get(*index as usize)?,
                _ => return None,
            };
            let Operand::Copy(source) = operand else { return None };
            let untouched = !before[at + 1..].iter().any(|statement| names(statement, source.local));
            return (source.local != place.local && untouched).then(|| source.clone());
        }
        if names(statement, place.local) {
            return None;
        }
    }
    None
}

/// Whether `part` covers `place`: the same place, or one inside it.
fn covers(part: &Place, place: &Place) -> bool {
    part.local == place.local && place.path.starts_with(&part.path)
}

/// Whether two places may share memory: one inside the other, or in two
/// variants of one enum, whose payloads overlap.
fn overlap(a: &Place, b: &Place) -> bool {
    if a.local != b.local {
        return false;
    }
    for (x, y) in a.path.iter().zip(&b.path) {
        match (x, y) {
            (Proj::Field(i), Proj::Field(j)) if i != j => return false,
            (Proj::Variant(v, i), Proj::Variant(w, j)) if v == w && i != j => return false,
            _ => {}
        }
    }
    true
}

/// Whether, on every way on from the retain at `at` in `block`, `source`
/// is released, by itself or a place that holds it, before anything reads
/// it, writes it or the function returns.
fn given_up(body: &Body, block: usize, at: usize, source: &Place) -> bool {
    let mut seen = vec![false; body.blocks.len()];
    let mut ways = vec![(block, at + 1)];
    let mut steps = 0;
    while let Some((block, start)) = ways.pop() {
        let data = &body.blocks[block];
        let mut released = false;
        for statement in &data.stmts[start..] {
            steps += 1;
            if steps > REACH {
                return false;
            }
            match step(statement, source) {
                Step::Released => {
                    released = true;
                    break;
                }
                Step::Conflict => return false,
                Step::Unrelated => {}
            }
        }
        if released {
            continue;
        }
        let mut read = false;
        crate::inline::visit_terminator(&mut data.terminator.clone(), &mut |place: &mut Place| {
            read |= overlap(place, source);
        });
        if read {
            return false;
        }
        match &data.terminator {
            Terminator::Goto(to) => ways.extend(next(&mut seen, to.0 as usize)),
            Terminator::Branch(_, then, otherwise) => {
                ways.extend(next(&mut seen, then.0 as usize));
                ways.extend(next(&mut seen, otherwise.0 as usize));
            }
            // Its reference would leave with the function, or the local
            // never owned one.
            Terminator::Return(_) => return false,
            Terminator::Panic { .. } | Terminator::Unreachable => {}
        }
    }
    true
}

/// A block to follow from its start, once.
fn next(seen: &mut [bool], block: usize) -> Option<(usize, usize)> {
    (!std::mem::replace(&mut seen[block], true)).then_some((block, 0))
}

enum Step {
    Released,
    Conflict,
    Unrelated,
}

/// What a statement does with `source`, zeroed after the move.
fn step(statement: &Statement, source: &Place) -> Step {
    match statement {
        Statement::Release(place) if covers(place, source) => return Step::Released,
        // Zeroing it again changes nothing.
        Statement::Moved(place) if overlap(place, source) => return Step::Unrelated,
        // A tag is not the payload moved out of it.
        Statement::Assign(target, value, _) if matches!(&**value, Rvalue::Discriminant(of) if !covers(source, of)) => {
            return if overlap(target, source) { Step::Conflict } else { Step::Unrelated };
        }
        _ => {}
    }
    let mut conflict = false;
    let mut statement = statement.clone();
    crate::inline::visit_statement(&mut statement, &mut |place: &mut Place| {
        if overlap(place, source) {
            conflict = true;
        }
    });
    let cleared = matches!(statement, Statement::Clear(local) if local == source.local);
    if conflict || cleared { Step::Conflict } else { Step::Unrelated }
}

/// A whole local moved and then released, with nothing naming it in
/// between or anywhere after: the release has nothing left to do, and the
/// zero nothing to keep.
fn drop_released_moves(body: &mut Body) {
    let names = |statement: &Statement, local: Local| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        used.contains(&local) || defined.contains(&local)
    };
    for block in 0..body.blocks.len() {
        let mut at = 0;
        while at < body.blocks[block].stmts.len() {
            let statements = &body.blocks[block].stmts;
            let Statement::Moved(place) = &statements[at] else {
                at += 1;
                continue;
            };
            let local = place.local;
            let release = statements[at + 1..]
                .iter()
                .position(|statement| names(statement, local))
                .map(|index| at + 1 + index)
                .filter(|&index| matches!(&statements[index], Statement::Release(r) if *r == Place::local(local)));
            // Named in no other block, nor after the release in this one.
            let elsewhere = |body: &Body, release: usize| {
                body.blocks.iter().enumerate().any(|(index, data)| {
                    let statements = if index == block { &data.stmts[release + 1..] } else { &data.stmts[..] };
                    let mut used = Vec::new();
                    terminator_uses(&data.terminator, &mut used);
                    used.contains(&local) || statements.iter().any(|statement| names(statement, local))
                })
            };
            match release {
                Some(release) if place.path.is_empty() && !elsewhere(body, release) => {
                    body.blocks[block].stmts.remove(release);
                    body.blocks[block].stmts.remove(at);
                }
                _ => at += 1,
            }
        }
    }
}
