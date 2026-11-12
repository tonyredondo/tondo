//! Copies of values the function never reads again, as inlining leaves a
//! call's result: `a = value` then `b = a`, with `a` read nowhere after,
//! becomes `b = value`. A value that lives in memory, as an enum's, was
//! copied whole each time, two or three times for `math.checkedSqrt(x)`'s result.
//! A counted value goes only when its reference passes along the copies:
//! when nothing ever retains, releases or clears the local it goes through,
//! and nothing between where it takes the value and the copy may panic,
//! whose unwinding would release what holds it then.

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
        forward_captures(body, types);
        signs(body, types);
    });
}

/// A comparison with zero of a `Float` converted from an `Int` compares the
/// `Int`: the conversion keeps the sign, and gives no NaN, so `prove` tells
/// it from what it knows of the `Int`, as `math.checkedSqrt(Float(i))`'s test of a
/// negative value is `i < 0`, never true where `i` counts from zero. The
/// `Float` is written once, right after the `Int`, in the same block, and
/// neither is lent: wherever the `Float` is read, the `Int` is the one it
/// was made of.
fn signs(body: &mut Body, types: &Types) {
    use crate::ast::BinOp;
    let converts = |statement: &Statement| matches!(statement, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Convert(_)));
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(converts) {
        return;
    }
    let count = body.locals.len();
    let (mut writes, mut lent) = (vec![0u32; count], vec![false; count]);
    for local in &body.params {
        writes[local.0 as usize] += 1;
    }
    // Where each local is written, when it is assigned whole.
    let mut written: Vec<Option<(usize, usize)>> = vec![None; count];
    for (index, block) in body.blocks.iter().enumerate() {
        for (at, statement) in block.stmts.iter().enumerate() {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            crate::mir::statement_locals(statement, &mut used, &mut defined);
            for local in defined {
                writes[local.0 as usize] += 1;
            }
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    if place.path.is_empty() {
                        written[place.local.0 as usize] = Some((index, at));
                    } else {
                        writes[place.local.0 as usize] += 2;
                    }
                    if let Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) = &**rvalue {
                        writes[target.local.0 as usize] += 2;
                    }
                }
                Statement::Lend(alias, place) => {
                    lent[alias.0 as usize] = true;
                    lent[place.local.0 as usize] = true;
                }
                Statement::LendAt(alias, operand) => {
                    lent[alias.0 as usize] = true;
                    if let Operand::Copy(place) = operand {
                        lent[place.local.0 as usize] = true;
                    }
                }
                Statement::OsrLoad(_) => return,
                _ => {}
            }
        }
    }
    let once = |local: Local| writes[local.0 as usize] == 1 && !lent[local.0 as usize];
    let float = |local: Local| matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::Float(_));
    // Each `Float` made of an `Int`, or a copy of one, by the `Int`.
    let mut of: Vec<Option<Local>> = vec![None; count];
    for (index, block) in body.blocks.iter().enumerate() {
        for (at, statement) in block.stmts.iter().enumerate() {
            let Statement::Assign(place, rvalue, _) = statement else { continue };
            if !place.path.is_empty() || !float(place.local) || !once(place.local) {
                continue;
            }
            let before = |local: Local| written[local.0 as usize].is_some_and(|(b, a)| b == index && a < at);
            let param = |local: Local| body.params.contains(&local);
            of[place.local.0 as usize] = match &**rvalue {
                Rvalue::Convert(Operand::Copy(source))
                    if source.path.is_empty()
                        && types.repr(body.locals[source.local.0 as usize]) == crate::types::Ty::INT
                        && once(source.local)
                        && (param(source.local) || before(source.local)) =>
                {
                    Some(source.local)
                }
                Rvalue::Use(Operand::Copy(source)) if source.path.is_empty() && once(source.local) => {
                    of[source.local.0 as usize].filter(|&int| param(int) || (before(source.local) && before(int)))
                }
                _ => None,
            };
        }
    }
    if of.iter().all(Option::is_none) {
        return;
    }
    let zero =
        |operand: &Operand| matches!(operand, Operand::Const(Const::Float(bits, _)) if f64::from_bits(*bits) == 0.0);
    let int = |operand: &Operand| match operand {
        Operand::Copy(place) if place.path.is_empty() => of[place.local.0 as usize],
        _ => None,
    };
    for statement in body.blocks.iter_mut().flat_map(|block| &mut block.stmts) {
        let Statement::Assign(_, rvalue, _) = statement else { continue };
        let Rvalue::Binary(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne), a, b) =
            &mut **rvalue
        else {
            continue;
        };
        let compared = match (int(a), int(b)) {
            (Some(local), None) if zero(b) => Some((local, true)),
            (None, Some(local)) if zero(a) => Some((local, false)),
            _ => None,
        };
        let Some((local, first)) = compared else { continue };
        let (int_operand, zero_operand) = (Operand::local(local), Operand::Const(Const::Int(0, crate::types::Ty::INT)));
        **rvalue = if first {
            Rvalue::Binary(*op, int_operand, zero_operand)
        } else {
            Rvalue::Binary(*op, zero_operand, int_operand)
        };
    }
}

/// A closure's capture read where the closure is made, as inlining leaves
/// it, reads what was captured instead: `f.captures.0`, with `f = closure
/// of env` and `env = (step)`, is `step`, whose value `prove` then knows, as
/// `x + step` with `step = 3` cannot overflow for `x` below a thousand. The
/// closure, its captures and what they hold are each written once and lent
/// to nothing, so that what was captured is what is read; and only a value
/// that needs no release, which the closure's could outlive.
fn forward_captures(body: &mut Body, types: &Types) {
    let count = body.locals.len();
    let erases = |statement: &Statement| matches!(statement, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Erase(..)));
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(erases) {
        return;
    }
    let (mut writes, mut lent) = (vec![0u32; count], vec![false; count]);
    let mut made: Vec<Option<Rvalue>> = vec![None; count];
    for local in &body.params {
        writes[local.0 as usize] += 1;
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Assign(place, rvalue, _) => {
                writes[place.local.0 as usize] += 1;
                if place.path.is_empty() {
                    made[place.local.0 as usize] = Some((**rvalue).clone());
                }
                match &**rvalue {
                    Rvalue::Select(cases, ..) => cases.iter().for_each(|case| writes[case.out.0 as usize] += 1),
                    Rvalue::Mutate(_, place, ..) | Rvalue::AssignSlice(place, ..) => {
                        writes[place.local.0 as usize] += 1
                    }
                    _ => {}
                }
            }
            Statement::Clear(local) => writes[local.0 as usize] += 1,
            Statement::Moved(place) => writes[place.local.0 as usize] += 1,
            Statement::Lend(alias, place) => {
                writes[alias.0 as usize] += 1;
                lent[alias.0 as usize] = true;
                lent[place.local.0 as usize] = true;
            }
            Statement::LendAt(alias, operand) => {
                writes[alias.0 as usize] += 1;
                lent[alias.0 as usize] = true;
                if let Operand::Copy(place) = operand {
                    lent[place.local.0 as usize] = true;
                }
            }
            // A live build's frame comes back from its old code whole.
            Statement::OsrLoad(_) => return,
            _ => {}
        }
    }
    let fixed = |local: Local| {
        let index = local.0 as usize;
        writes[index] == 1 && !lent[index] && made[index].is_some() && !body.params.contains(&local)
    };
    // The captures of each closure made once of captures made once.
    let mut captures: Vec<Option<Vec<Operand>>> = vec![None; count];
    for local in 0..count {
        let Some(Rvalue::Erase(Operand::Copy(env), ..)) = &made[local] else { continue };
        if !fixed(Local(local as u32)) || !env.path.is_empty() || !fixed(env.local) {
            continue;
        }
        if let Some(Rvalue::Aggregate(operands)) = &made[env.local.0 as usize] {
            captures[local] = Some(operands.clone());
        }
    }
    if captures.iter().all(Option::is_none) {
        return;
    }
    let locals = body.locals.clone();
    let mut forward = |place: &mut Place| {
        let Some(operands) = &captures[place.local.0 as usize] else { return };
        let [Proj::Captures(_), Proj::Field(index), ..] = place.path[..] else { return };
        let Some(Operand::Copy(captured)) = operands.get(index as usize) else { return };
        if captured.path.is_empty() && fixed(captured.local) && !types.needs_drop(locals[captured.local.0 as usize]) {
            *place = Place { local: captured.local, path: place.path[2..].to_vec() };
        }
    };
    for block in &mut body.blocks {
        for statement in &mut block.stmts {
            crate::inline::visit_statement(statement, &mut forward);
        }
        crate::inline::visit_terminator(&mut block.terminator, &mut forward);
    }
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
    // The locals a counted value only passes through.
    let mut owns = vec![false; body.locals.len()];
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) => {
                owns[place.local.0 as usize] = true;
            }
            Statement::Clear(local) => owns[local.0 as usize] = true,
            _ => {}
        }
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
                || (types.needs_drop(ty) && owns[from.0 as usize])
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
            if types.needs_drop(ty) && statements[made + 1..copy].iter().any(crate::lower::may_panic) {
                copy += 1;
                continue;
            }
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
