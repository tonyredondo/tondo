//! The captures a closure gets the same constant for wherever it is made,
//! as one that captures `let step = 3` (`docs/decisions.md`): its body reads
//! them as that constant, as a call of it inlined where it was made would,
//! so that `prove` bounds what the body computes of them. Before the
//! inliner, so that every copy of the body has them.

use crate::hir::{Callee, FnId};
use crate::mir::{Body, Const, Local, Operand, Place, Proj, Rvalue, Statement};
use crate::source::Span;
use crate::types::Types;

/// What the closures' bodies are given: each capture's constant, the
/// same wherever the closure is made, or none.
type Given = Vec<Option<Const>>;

pub fn captured(bodies: &mut [Body], types: &Types) {
    // A closure made inside another reads its captures there: once the
    // outer one's are known, the inner one's may be too.
    for _ in 0..4 {
        if !round(bodies, types) {
            break;
        }
    }
}

/// One round: the closures made anywhere, what they are given, and each
/// body that reads a constant capture given the constant. Whether any did.
fn round(bodies: &mut [Body], types: &Types) -> bool {
    // By closure: what every place that makes or calls it gives it; none
    // when one gives what is not known, or uses its code otherwise.
    let mut given: std::collections::HashMap<FnId, Option<Given>> = Default::default();
    // The functions given a closure first: its body, and those that take
    // one to call, which no code outside the program's calls.
    let closures: std::collections::HashSet<FnId> = (bodies.iter().enumerate())
        .filter(|(_, body)| body.params.first().is_some_and(|env| types.is_closure(body.locals[env.0 as usize])))
        .map(|(index, _)| FnId(index as u32))
        .collect();
    if closures.is_empty() {
        return false;
    }
    let mut give = |closure: FnId, what: Option<Given>| {
        let entry = given.entry(closure).or_insert_with(|| what.clone());
        *entry = match (entry.take(), what) {
            (Some(known), Some(other)) if known.len() == other.len() => {
                Some(known.into_iter().zip(other).map(|(a, b)| if a == b { a } else { None }).collect())
            }
            _ => None,
        };
    };
    for body in bodies.iter() {
        let made = made_once(body);
        let constant = |operand: &Operand| -> Option<Const> {
            match operand {
                Operand::Const(constant) => scalar(constant),
                Operand::Copy(place) if place.path.is_empty() => match &made[place.local.0 as usize] {
                    Some(Rvalue::Use(Operand::Const(constant))) => scalar(constant),
                    _ => None,
                },
                Operand::Copy(_) => None,
            }
        };
        // The captures of the closure a local holds, through its copies.
        let captures = |env: &Operand| -> Option<Given> {
            let Operand::Copy(mut env) = env.clone() else { return None };
            for _ in 0..8 {
                if !env.path.is_empty() {
                    return None;
                }
                match &made[env.local.0 as usize] {
                    Some(Rvalue::Aggregate(operands)) => return Some(operands.iter().map(constant).collect()),
                    Some(Rvalue::Use(Operand::Copy(copied))) => env = copied.clone(),
                    _ => return None,
                }
            }
            None
        };
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            match statement {
                Statement::DeferPush(closure, _) => give(*closure, None),
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    Rvalue::Erase(env, closure, _) => give(*closure, captures(env)),
                    Rvalue::Call(Callee::Fn(closure), operands) if closures.contains(closure) => {
                        give(*closure, operands.first().and_then(captures))
                    }
                    Rvalue::FnValue(closure) | Rvalue::Callback(closure, _) | Rvalue::CallOsr(closure, ..) => {
                        give(*closure, None)
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    let mut changed = false;
    for (closure, given) in given {
        let Some(given) = given.filter(|given| given.iter().any(Option::is_some)) else { continue };
        changed |= read_as_given(&mut bodies[closure.0 as usize], types, &given);
    }
    changed
}

/// What each local is made of, when it is written once, by an assignment
/// of it whole, and nothing lends it.
fn made_once(body: &Body) -> Vec<Option<Rvalue>> {
    let count = body.locals.len();
    let (mut writes, mut lent) = (vec![0u32; count], vec![false; count]);
    let mut made: Vec<Option<Rvalue>> = vec![None; count];
    for local in &body.params {
        writes[local.0 as usize] += 1;
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        crate::mir::statement_locals(statement, &mut used, &mut defined);
        for local in defined {
            writes[local.0 as usize] += 1;
        }
        match statement {
            Statement::Assign(place, rvalue, _) => {
                if place.path.is_empty() {
                    made[place.local.0 as usize] = Some((**rvalue).clone());
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
            Statement::Moved(place) => writes[place.local.0 as usize] += 2,
            Statement::Clear(local) => writes[local.0 as usize] += 2,
            // A live build's frame comes back from its old code whole.
            Statement::OsrLoad(_) => return vec![None; count],
            _ => {}
        }
    }
    (0..count).map(|local| made[local].take().filter(|_| writes[local] == 1 && !lent[local])).collect()
}

/// A constant of a type with nothing to release.
fn scalar(constant: &Const) -> Option<Const> {
    matches!(constant, Const::Int(..) | Const::Float(..) | Const::Char(_) | Const::Bool(_)).then(|| constant.clone())
}

/// In a closure's body, each capture given a constant that the body never
/// changes, nor lends, is read from a local assigned the constant at the
/// start. Whether any was.
fn read_as_given(body: &mut Body, types: &Types, given: &Given) -> bool {
    let Some(&env) = body.params.first() else { return false };
    // A closure that changes its captures is lent to its body.
    // The closure comes lent to its body, which alone may change it: what
    // the body does not change keeps what it was made with.
    let Some(captured) = types.components(body.locals[env.0 as usize]) else { return false };
    let mut known = given.clone();
    let field = |place: &Place| -> Option<usize> {
        match place.path[..] {
            [Proj::Field(index)] if place.local == env => Some(index as usize),
            _ => None,
        }
    };
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        // What it writes, lends or gives away, or passes where it may be lent.
        let mut changed: Vec<&Place> = Vec::new();
        let mut whole = false;
        match statement {
            Statement::Assign(place, rvalue, _) => {
                changed.push(place);
                match &**rvalue {
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => changed.push(target),
                    Rvalue::Call(_, operands) | Rvalue::CallIndirect(_, operands) | Rvalue::CallOsr(_, operands, _) => {
                        changed.extend(operands.iter().filter_map(|operand| match operand {
                            Operand::Copy(place) => Some(place),
                            Operand::Const(_) => None,
                        }))
                    }
                    _ => {}
                }
            }
            Statement::Lend(_, place) | Statement::Moved(place) | Statement::DeferPush(_, place) => changed.push(place),
            Statement::LendAt(_, Operand::Copy(place)) => changed.push(place),
            Statement::Clear(local) => whole |= *local == env,
            Statement::OsrLoad(_) => return false,
            _ => {}
        }
        for place in changed.into_iter().filter(|place| place.local == env) {
            match place.path.first() {
                Some(Proj::Field(index)) => {
                    if let Some(slot) = known.get_mut(*index as usize) {
                        *slot = None;
                    }
                }
                _ => whole = true,
            }
        }
        if whole {
            return false;
        }
    }
    if known.iter().all(Option::is_none) {
        return false;
    }
    // A local for each, assigned at the start.
    let mut locals: Vec<Option<Local>> = vec![None; known.len()];
    let mut start = Vec::new();
    for (index, constant) in known.iter().enumerate() {
        let Some(constant) = constant else { continue };
        let local = Local(body.locals.len() as u32);
        let Some(&ty) = captured.get(index) else { continue };
        body.locals.push(ty);
        body.names.push(None);
        locals[index] = Some(local);
        start.push(Statement::Assign(
            Place::local(local),
            Box::new(Rvalue::Use(Operand::Const(constant.clone()))),
            Span::default(),
        ));
        crate::mir_text::note(body, "captured", || {
            let constant = crate::mir_text::operand_text(&Operand::Const(constant.clone()));
            format!("l{}.{index} is {constant} wherever the closure is made: l{}", env.0, local.0)
        });
    }
    let mut read = 0;
    let mut forward = |place: &mut Place| {
        if let Some(index) = field(place)
            && let Some(Some(local)) = locals.get(index)
        {
            *place = Place::local(*local);
            read += 1;
        }
    };
    for block in &mut body.blocks {
        for statement in &mut block.stmts {
            crate::inline::visit_statement(statement, &mut forward);
        }
        crate::inline::visit_terminator(&mut block.terminator, &mut forward);
    }
    let Some(entry) = body.blocks.first_mut() else { return false };
    entry.stmts.splice(0..0, start);
    read > 0
}
