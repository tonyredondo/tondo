//! The length of a text or an array that a loop reads each turn, read once
//! before the loop when nothing in the loop can change it: a loop over a
//! text's characters or an array's elements tests its position against the
//! length every turn, and Cranelift cannot keep the length in a register
//! across a call of the runtime in the loop, as memory may change there.
//! Both lengths are a load from the value, which is whole when the loop
//! starts, as the loop reads it and does not write it: reading it before
//! the loop costs a load even when the loop ends before it reads it.
//!
//! Nothing in the loop changes the value when its local is not an address,
//! as a loan or a lock's state is, which other code may change while the
//! loop waits; when no statement of the loop writes, releases, lends or
//! clears the local, of which the value may be a field; and no call takes
//! it to
//! write: as a `var` parameter, or as an array the runtime or C is given,
//! which may change it in place. A text the runtime is given it only reads.

use crate::hir::{Callee, StrOp};
use crate::mir::*;
use crate::types::{TyKind, Types};

pub fn hoist(bodies: &mut [Body], types: &Types) {
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    for body in bodies.iter_mut() {
        if body.osr.is_some() {
            continue;
        }
        hoist_body(body, types, &loans);
    }
}

fn hoist_body(body: &mut Body, types: &Types, loans: &[Vec<bool>]) {
    let reads = |body: &Body| {
        body.blocks.iter().flat_map(|block| &block.stmts).any(
            |statement| matches!(statement, Statement::Assign(_, rvalue, _) if length(body, types, rvalue).is_some()),
        )
    };
    if !reads(body) {
        return;
    }
    let count = body.blocks.len();
    let successors: Vec<Vec<usize>> = body.blocks.iter().map(|block| successors(&block.terminator)).collect();
    let mut predecessors = vec![Vec::new(); count];
    for (block, to) in successors.iter().enumerate() {
        for &to in to {
            predecessors[to].push(block);
        }
    }
    // Each loop: its head, its blocks, those the head reaches that reach it
    // again, and the one block before it that goes only to it, if there is
    // one; the innermost first, so that a length leaves an inner loop and
    // then, from the block before it, the loops around it.
    let heads = heads(&successors);
    let mut loops: Vec<(usize, Vec<bool>, Option<usize>)> = (0..count)
        .filter(|&block| heads[block])
        .map(|head| {
            let (forward, backward) = (reach(head, &successors), reach(head, &predecessors));
            let inside: Vec<bool> = (0..count).map(|block| forward[block] && backward[block]).collect();
            let outside: Vec<usize> = predecessors[head].iter().copied().filter(|&block| !inside[block]).collect();
            let before = match outside[..] {
                [before] if matches!(body.blocks[before].terminator, Terminator::Goto(_)) => Some(before),
                _ => None,
            };
            (head, inside, before)
        })
        .collect();
    loops.sort_by_key(|(_, inside, _)| inside.iter().filter(|&&inside| inside).count());
    // The locals that are an address: a loan, and what a statement lends.
    let mut addressed = vec![false; body.locals.len()];
    for (&param, &loan) in body.params.iter().zip(&body.loans) {
        addressed[param.0 as usize] |= loan;
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        if let Statement::Lend(alias, _) | Statement::LendAt(alias, _) = statement {
            addressed[alias.0 as usize] = true;
        }
    }
    // Each read of a length in a loop, by its block and statement: why it
    // stays in its innermost loop, or that it left it, at its place.
    type At = (usize, usize, crate::source::Span);
    let (mut stays, mut left): (Vec<(At, String)>, Vec<At>) = (Vec::new(), Vec::new());
    for (head, inside, before) in &loops {
        let written = written(body, types, loans, inside);
        // Each length read early, by what it is of: in bytes, and the place.
        let mut hoisted: Vec<((bool, Place), Local)> = Vec::new();
        for block in (0..count).filter(|&block| inside[block]) {
            for index in 0..body.blocks[block].stmts.len() {
                let Statement::Assign(place, rvalue, span) = &body.blocks[block].stmts[index] else { continue };
                let Some((bytes, read)) = length(body, types, rvalue) else { continue };
                let (key, rvalue, span, into) = ((bytes, read.clone()), (**rvalue).clone(), *span, place.local);
                let fields = read.path.iter().all(|projection| matches!(projection, Proj::Field(_)));
                let why = match before {
                    _ if !place.path.is_empty() || !fields => Some("it is read out of another value".to_string()),
                    _ if addressed[read.local.0 as usize] => {
                        Some("it is read through a loan or a lock, where other code may change it".to_string())
                    }
                    _ if written[read.local.0 as usize] => Some(match &body.names[read.local.0 as usize] {
                        Some(name) => format!("the loop may change `{name}`"),
                        None => "the loop may change what it is the length of".to_string(),
                    }),
                    None => Some("the loop is entered from more than one place".to_string()),
                    Some(_) => None,
                };
                let at = (block, index, span);
                if let Some(why) = why {
                    if !stays.iter().any(|(known, _)| *known == at) {
                        stays.push((at, why));
                    }
                    continue;
                }
                let before = before.unwrap();
                let early = match hoisted.iter().find(|(known, _)| *known == key) {
                    Some((_, early)) => *early,
                    None => {
                        let early = Local(body.locals.len() as u32);
                        body.locals.push(body.locals[into.0 as usize]);
                        body.names.push(None);
                        body.blocks[before].stmts.push(Statement::Assign(
                            Place::local(early),
                            Box::new(rvalue.clone()),
                            span,
                        ));
                        hoisted.push((key, early));
                        early
                    }
                };
                body.blocks[block].stmts[index] =
                    Statement::Assign(Place::local(into), Box::new(Rvalue::Use(Operand::local(early))), span);
                stays.retain(|(known, _)| *known != at);
                left.push(at);
            }
        }
        if !hoisted.is_empty() {
            let (count, before) = (hoisted.len(), before.unwrap());
            crate::mir_text::note(body, "hoist", || {
                format!("loop at b{head}: {count} length(s) read once before it, in b{before}")
            });
        }
    }
    let mut said = Vec::new();
    for (.., span) in left {
        if !said.contains(&span) {
            said.push(span);
            crate::mir_text::remark(body, span, "hoisted", || "reads the length once, before the loop".to_string());
        }
    }
    for ((.., span), why) in stays {
        crate::mir_text::remark(body, span, "reloaded", || format!("reads the length each turn: {why}"));
    }
}

/// A length the rvalue loads, of a text in bytes or of an array, and of
/// what. A text's length in characters, or a map's, is a call.
fn length<'r>(body: &Body, types: &Types, rvalue: &'r Rvalue) -> Option<(bool, &'r Place)> {
    match rvalue {
        Rvalue::Length(Operand::Copy(of))
            if matches!(
                types.kind(types.repr(body.locals[of.local.0 as usize])),
                TyKind::Array(_) | TyKind::Deque(_)
            ) =>
        {
            Some((false, of))
        }
        Rvalue::StrOp(StrOp::ByteLength, operands, _) => match &operands[..] {
            [Operand::Copy(of)] => Some((true, of)),
            _ => None,
        },
        _ => None,
    }
}

/// The locals the loop may change: those its statements write, release,
/// lend or clear, those a call takes as a `var` parameter, and the arrays
/// and other collections a call of the runtime, of C or through a value is
/// given, which it may change in place.
fn written(body: &Body, types: &Types, loans: &[Vec<bool>], inside: &[bool]) -> Vec<bool> {
    let mut written = vec![false; body.locals.len()];
    let text = |local: Local| matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::String);
    for (_, block) in body.blocks.iter().enumerate().filter(|(index, _)| inside[*index]) {
        for statement in &block.stmts {
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    written[place.local.0 as usize] = true;
                    match &**rvalue {
                        Rvalue::Mutate(_, target, ..) => written[target.local.0 as usize] = true,
                        Rvalue::Call(Callee::Fn(id), args) => {
                            for (at, arg) in args.iter().enumerate() {
                                if let Operand::Copy(arg) = arg
                                    && loans[id.0 as usize].get(at).copied().unwrap_or(true)
                                {
                                    written[arg.local.0 as usize] = true;
                                }
                            }
                        }
                        Rvalue::Call(callee, args) => {
                            for arg in args {
                                if let Operand::Copy(arg) = arg
                                    && !(matches!(callee, Callee::Std(_)) && text(arg.local))
                                {
                                    written[arg.local.0 as usize] = true;
                                }
                            }
                        }
                        Rvalue::CallIndirect(..) => {
                            let mut used = Vec::new();
                            rvalue_uses(rvalue, &mut used);
                            for local in used {
                                written[local.0 as usize] = true;
                            }
                        }
                        _ => {}
                    }
                }
                Statement::Release(place) | Statement::Moved(place) | Statement::DeferPush(_, place) => {
                    written[place.local.0 as usize] = true;
                }
                Statement::Lend(alias, place) => {
                    written[alias.0 as usize] = true;
                    written[place.local.0 as usize] = true;
                }
                Statement::LendAt(alias, _) => written[alias.0 as usize] = true,
                Statement::Clear(local) => written[local.0 as usize] = true,
                Statement::OsrLoad(_) => written.iter_mut().for_each(|written| *written = true),
                Statement::Retain(_) | Statement::DeferPop | Statement::Failing(..) => {}
            }
        }
    }
    written
}

/// The loops of the body, the innermost first: each one's head and its
/// blocks, those the head reaches that reach it again.
pub(crate) fn loops(body: &Body) -> Vec<(usize, Vec<bool>)> {
    let count = body.blocks.len();
    let successors: Vec<Vec<usize>> = body.blocks.iter().map(|block| successors(&block.terminator)).collect();
    let mut predecessors = vec![Vec::new(); count];
    for (block, to) in successors.iter().enumerate() {
        for &to in to {
            predecessors[to].push(block);
        }
    }
    let heads = heads(&successors);
    let mut loops: Vec<(usize, Vec<bool>)> = (0..count)
        .filter(|&block| heads[block])
        .map(|head| {
            let (forward, backward) = (reach(head, &successors), reach(head, &predecessors));
            (head, (0..count).map(|block| forward[block] && backward[block]).collect())
        })
        .collect();
    loops.sort_by_key(|(_, inside)| inside.iter().filter(|&&inside| inside).count());
    loops
}

/// The blocks a jump from a block goes to.
pub(crate) fn targets(terminator: &Terminator) -> Vec<usize> {
    successors(terminator)
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Goto(to) => vec![to.0 as usize],
        Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
        _ => Vec::new(),
    }
}

/// The blocks an edge goes back to, from a block being visited from the
/// entry: the heads of loops.
fn heads(successors: &[Vec<usize>]) -> Vec<bool> {
    let count = successors.len();
    let (mut state, mut heads) = (vec![0u8; count], vec![false; count]);
    let mut stack = vec![(0usize, 0usize)];
    state[0] = 1;
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        if let Some(&to) = successors[block].get(*next) {
            *next += 1;
            match state[to] {
                0 => {
                    state[to] = 1;
                    stack.push((to, 0));
                }
                1 => heads[to] = true,
                _ => {}
            }
        } else {
            state[block] = 2;
            stack.pop();
        }
    }
    heads
}

/// The blocks reached from `from` along `edges`, not counting `from` unless
/// a path comes back to it.
fn reach(from: usize, edges: &[Vec<usize>]) -> Vec<bool> {
    let mut seen = vec![false; edges.len()];
    let mut pending = edges[from].clone();
    while let Some(block) = pending.pop() {
        if !std::mem::replace(&mut seen[block], true) {
            pending.extend(&edges[block]);
        }
    }
    seen
}
