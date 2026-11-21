//! Elements of an array the function holds unchanged, read and handed
//! from local to local, but kept nowhere: they need no reference of their
//! own. A lazy chain over records with a text read each record from its
//! array, retained the text, moved it through the adapters' locals, which
//! `split` keeps in registers, and released it at the end of the step: two
//! atomic operations an element, for a value its array held all along.
//!
//! The array is a local nothing changes, lends or lets go but where the
//! function returns; the element is copied out of it and retained at once.
//! A part of a parameter is the same: the caller keeps the parameter alive
//! through the call, and a plain one, not `var` or `mut`, nothing in the
//! function writes, as a specialized adapter's copy of its iterator's
//! record takes the record's array and closures, which it then reads or
//! calls no more, and kept them, and their registers, to the end.
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
    let mut sources: Vec<(usize, usize, Local, Local, crate::source::Span)> = Vec::new();
    for (index, block) in body.blocks.iter().enumerate() {
        for (at, statement) in block.stmts.iter().enumerate() {
            let Statement::Assign(place, rvalue, span) = statement else { continue };
            let Rvalue::Use(Operand::Copy(from)) = &**rvalue else { continue };
            let element = from.path.iter().any(|proj| matches!(proj, Proj::Index(..) | Proj::InBounds(_)));
            if !place.path.is_empty() || !(element || part_of_parameter(body, from)) {
                continue;
            }
            if !types.needs_drop(body.locals[place.local.0 as usize]) {
                continue;
            }
            let (mut here, mut start) = (index, at + 1);
            for _ in 0..body.blocks.len() {
                let rest = &body.blocks[here].stmts[start..];
                let retained = rest
                    .iter()
                    .position(|s| matches!(s, Statement::Retain(p) if p.path.is_empty() && p.local == place.local));
                if let Some(after) = retained {
                    sources.push((here, start + after, place.local, from.local, *span));
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
    // The last first: dropping a retain moves the statements after it, and
    // so the places of the retains still to look at, but none before it.
    sources.sort_by_key(|&(block, retain, ..)| std::cmp::Reverse((block, retain)));
    for (block, retain, local, holder, span) in sources {
        let parameter = body.params.contains(&holder);
        match only_read(body, types, loans, local, holder, (block, retain)) {
            Ok(reached) => {
                drop_counts(body, &reached, (block, retain));
                if parameter {
                    crate::mir_text::note(body, "borrowed", || {
                        format!(
                            "l{}, a part of parameter l{}, needs no count of its own: the caller keeps the parameter, and nothing here writes, keeps or lets it go",
                            local.0, holder.0
                        )
                    });
                    crate::mir_text::remark(body, span, "lent", || {
                        "is read with no count of its own: the caller keeps the parameter".to_string()
                    });
                }
            }
            Err((why, at)) if parameter => {
                crate::mir_text::note(body, "borrowed", || {
                    format!("l{}, a part of parameter l{}, keeps its count: {why}", local.0, holder.0)
                });
                // Where it is kept or given away, when a statement says.
                let by = at.filter(|at| *at != span).map(|at| (at, None));
                crate::mir_text::remark_by(body, span, "counted", by, true, None, || {
                    format!("keeps a count of its own: {why}")
                });
            }
            Err(_) => {}
        }
    }
}

/// Whether a place is a part of a plain parameter, by fields alone: one the
/// caller keeps alive through the call and the function cannot write, as
/// it could a `var` or `mut` one, which is lent.
fn part_of_parameter(body: &Body, place: &Place) -> bool {
    let Some(index) = body.params.iter().position(|&param| param == place.local) else { return false };
    !place.path.is_empty()
        && place.path.iter().all(|proj| matches!(proj, Proj::Field(_)))
        && !body.loans.get(index).copied().unwrap_or(false)
}

/// The locals a borrowed element or part reaches, when it can be borrowed:
/// what holds it is only read, and its array or parameter stays as it is
/// wherever it is read; or why not, which a note says.
fn only_read(
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    source: Local,
    array: Local,
    retain: (usize, usize),
) -> Result<Vec<bool>, (&'static str, Option<crate::source::Span>)> {
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
        return Err(("it or what holds it is given a value more than once", None));
    }
    // The parts copied out of it, and their own whole copies: read through
    // it, as it is read through its array, unless a part takes a count of
    // its own at once. Nothing may let such a part go, keep it or give it
    // away: a count that leaves through its parts, as when a value is moved
    // out by them, which `split` writes as copies of its parts beside a
    // `moved`, and which `moves` may leave with no mark at all, ends in one
    // of those places, and the source keeps its count.
    let counted_at_once = |block: usize, at: usize, local: Local| {
        let names = |statement: &&Statement| {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            used.contains(&local) || defined.contains(&local)
        };
        body.blocks[block].stmts[at + 1..]
            .iter()
            .find(names)
            .is_some_and(|next| matches!(next, Statement::Retain(p) if p.path.is_empty() && p.local == local))
    };
    let mut parts = vec![false; count];
    let mut changed = true;
    while changed {
        changed = false;
        for (index, block) in body.blocks.iter().enumerate() {
            for (at, statement) in block.stmts.iter().enumerate() {
                let Statement::Assign(place, rvalue, _) = statement else { continue };
                let Rvalue::Use(Operand::Copy(from)) = &**rvalue else { continue };
                let to = place.local.0 as usize;
                if !place.path.is_empty() || reached[to] || parts[to] {
                    continue;
                }
                let of = reached[from.local.0 as usize] || parts[from.local.0 as usize];
                let part = !from.path.is_empty()
                    && of
                    && types.needs_drop(body.locals[to])
                    && !counted_at_once(index, at, place.local);
                if part || from.path.is_empty() && parts[from.local.0 as usize] {
                    parts[to] = true;
                    changed = true;
                }
            }
        }
    }
    let holds = |local: Local| reached[local.0 as usize];
    let part = |local: Local| parts[local.0 as usize];
    let reads = |operand: &Operand| matches!(operand, Operand::Copy(place) if holds(place.local) || part(place.local));
    for (index, block) in body.blocks.iter().enumerate() {
        let returns = matches!(block.terminator, Terminator::Return(_));
        let mut array_gone = false;
        for (at, statement) in block.stmts.iter().enumerate() {
            let here = match statement {
                Statement::Assign(_, _, span) => Some(*span),
                _ => None,
            };
            let refuse = |why: &'static str| Err((why, here));
            match statement {
                // The source, its retain, and what only zeroes or lets go.
                Statement::Retain(_) if (index, at) == retain => {}
                Statement::Retain(place) if place.local == array => {}
                Statement::Release(place) | Statement::Moved(place) if place.path.is_empty() && holds(place.local) => {}
                Statement::Clear(_) => {}
                // A part handed whole to a copy, which is a part as well.
                Statement::Moved(place) if place.path.is_empty() && part(place.local) => {}
                Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) if part(place.local) => {
                    return refuse("a part of it is given away, counted or let go as if it were its own");
                }
                // The array, let go only on the way out.
                Statement::Release(place) if place.local == array => {
                    if !returns || !place.path.is_empty() {
                        return refuse("what holds it is let go before the function returns");
                    }
                    array_gone = true;
                }
                Statement::Assign(place, rvalue, _) => {
                    if place.local == array {
                        // Its one value, before any element is read.
                        if !place.path.is_empty() {
                            return refuse("what holds it is written");
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
                            loan && matches!(arg, Operand::Copy(p) if holds(p.local) || part(p.local) || p.local == array)
                        })
                    {
                        return refuse("a call is lent it, or what holds it, and may write it");
                    }
                    let mut used = Vec::new();
                    crate::mir::rvalue_uses(rvalue, &mut used);
                    let element_read = used.iter().any(|&local| holds(local) || part(local));
                    if array_gone && element_read {
                        return refuse("it is read after what holds it is let go");
                    }
                    if holds(place.local) {
                        // Given a value only by a copy of the element, or
                        // the element itself.
                        let copied = place.path.is_empty()
                            && matches!(&**rvalue, Rvalue::Use(Operand::Copy(from))
                                if from.path.is_empty() && holds(from.local)
                                    || place.local == source && from.local == array);
                        if !copied {
                            return refuse("a value not its own is put where it is held");
                        }
                        continue;
                    }
                    if part(place.local) {
                        // Given a value only by a copy of a part, or of a
                        // whole copy of one.
                        let copied = place.path.is_empty()
                            && matches!(&**rvalue, Rvalue::Use(Operand::Copy(from))
                                if !from.path.is_empty() && (holds(from.local) || part(from.local))
                                    || from.path.is_empty() && part(from.local));
                        if !copied {
                            return refuse("a value not its own is put where a part of it is");
                        }
                        continue;
                    }
                    if !element_read {
                        if matches!(&**rvalue, Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..)
                            if target.local == array)
                        {
                            return refuse("what holds it is changed");
                        }
                        continue;
                    }
                    // Read where nothing keeps it.
                    let read_only = match &**rvalue {
                        Rvalue::Unary(..) | Rvalue::Length(_) | Rvalue::Convert(_) => true,
                        Rvalue::Binary(..) | Rvalue::Exact(..) => true,
                        Rvalue::StrOp(..) | Rvalue::MapOp(..) => true,
                        Rvalue::Concat(pieces) => pieces
                            .iter()
                            .all(|piece| !matches!(piece, Piece::Taken(p) if holds(p.local) || part(p.local))),
                        Rvalue::Call(crate::hir::Callee::Fn(_), _) | Rvalue::CallIndirect(..) => true,
                        // A part of it read, as an element of a borrowed
                        // array, into a copy that counts it at once, or
                        // that is a part as well and is followed so.
                        Rvalue::Use(Operand::Copy(from)) => {
                            (holds(from.local) || part(from.local)) && !from.path.is_empty()
                        }
                        _ => false,
                    };
                    if !read_only {
                        return refuse("something keeps it: it is stored, or put in a value");
                    }
                }
                Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) => {
                    if holds(place.local) || place.local == array {
                        return refuse("its count, or what holds it, is taken or let go elsewhere");
                    }
                }
                Statement::Lend(alias, place) => {
                    if holds(place.local) || part(place.local) || place.local == array || holds(*alias) {
                        return refuse("it is lent");
                    }
                }
                Statement::LendAt(alias, operand) => {
                    if reads(operand) || holds(*alias) {
                        return refuse("it is lent");
                    }
                }
                Statement::DeferPush(_, place) => {
                    if holds(place.local) || part(place.local) {
                        return refuse("a `defer` keeps it");
                    }
                }
                Statement::Failing(operand, ..) => {
                    if reads(operand) {
                        return refuse("a failure carries it");
                    }
                }
                Statement::DeferPop | Statement::OsrLoad(_) => {}
            }
        }
        if let Terminator::Return(operand) = &block.terminator
            && reads(operand)
        {
            return Err(("it is returned", None));
        }
    }
    Ok(reached)
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
