//! Copies of what the caller keeps alive that are only lent, as inlining
//! leaves `self` of a method: `counter.update(...)` copied `counter` into
//! the method's parameter, retained it, lent it to the lock and released it
//! after, two atomic operations on the mutex for each update. A parameter
//! lives for the whole call, as does a field of one, which inlining binds
//! to a local of its own when it passes it on, so a copy of either that
//! only lends it, to calls, which borrow their arguments, and to operations
//! that take nothing, needs no reference of its own: its retain and its
//! releases go. Only the program's functions, `fn` values and locks count
//! as lending: an operation of the runtime may take what it is given.
//!
//! What it copies must change nowhere in the function: neither written,
//! released, cleared, moved nor lent, nor a loan of the caller's place. The
//! copy must take its value once, retained at once, and be read nowhere
//! else.

use crate::mir::*;

pub fn lends(bodies: &mut [Body]) {
    crate::parallel::each_mut(bodies, lends_in);
}

/// How the function uses each local.
#[derive(Clone, Default)]
struct Uses {
    /// Written whole, by a copy of this place when written once.
    written: usize,
    copy_of: Option<Place>,
    retained: usize,
    released: usize,
    /// Written in part, cleared, moved, lent, or set by an operation.
    changed: bool,
    /// Read in a way that may keep it: copied, or given to what takes it.
    kept: bool,
    /// Given to a call or an operation, which writes through a loan.
    passed: bool,
}

fn lends_in(body: &mut Body) {
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(|s| matches!(s, Statement::Retain(_))) {
        return;
    }
    let mut uses = vec![Uses::default(); body.locals.len()];
    let lend = |operands: &[Operand], flags: Option<&[bool]>, uses: &mut Vec<Uses>| {
        for (index, operand) in operands.iter().enumerate() {
            let mut read = Vec::new();
            operand_uses(operand, &mut read);
            // A whole local without a flag to retain it is lent; a part of
            // one, or one retained as it moves in, is taken.
            let whole = matches!(operand, Operand::Copy(place) if place.path.is_empty());
            let flagged = flags.is_some_and(|flags| flags.get(index).copied().unwrap_or(true));
            for local in read {
                uses[local.0 as usize].kept |= !whole || flagged;
                uses[local.0 as usize].passed = true;
            }
        }
    };
    for block in &body.blocks {
        for statement in &block.stmts {
            match statement {
                Statement::Retain(place) | Statement::Release(place) if !place.path.is_empty() => {
                    uses[place.local.0 as usize].kept = true;
                }
                Statement::Retain(place) => uses[place.local.0 as usize].retained += 1,
                Statement::Release(place) => uses[place.local.0 as usize].released += 1,
                Statement::Assign(target, value, _) => {
                    let mine = &mut uses[target.local.0 as usize];
                    if target.path.is_empty() {
                        mine.written += 1;
                    } else {
                        mine.changed = true;
                    }
                    match &**value {
                        Rvalue::Use(Operand::Copy(source))
                            if target.path.is_empty()
                                && source.path.iter().all(|projection| matches!(projection, Proj::Field(_))) =>
                        {
                            uses[target.local.0 as usize].copy_of = Some(source.clone());
                            uses[source.local.0 as usize].kept = true;
                        }
                        // The program's functions and `fn` values borrow their
                        // arguments, and a lock borrows what it locks; what the
                        // runtime does with others, as a channel's send, may
                        // take them.
                        Rvalue::Call(crate::hir::Callee::Fn(_), operands) => lend(operands, None, &mut uses),
                        Rvalue::CallIndirect(function, operands) => {
                            lend(std::slice::from_ref(function), Some(&[true]), &mut uses);
                            lend(operands, None, &mut uses);
                        }
                        Rvalue::TaskOp(crate::hir::TaskOp::Lock(_), operands, flags, _) => {
                            lend(operands, Some(flags), &mut uses)
                        }
                        _ => {
                            let (mut read, mut defined) = (Vec::new(), Vec::new());
                            statement_locals(statement, &mut read, &mut defined);
                            for local in read {
                                uses[local.0 as usize].kept = true;
                                uses[local.0 as usize].passed = true;
                            }
                        }
                    }
                }
                _ => {
                    let (mut read, mut defined) = (Vec::new(), Vec::new());
                    statement_locals(statement, &mut read, &mut defined);
                    for local in read {
                        uses[local.0 as usize].kept = true;
                    }
                    for local in defined {
                        uses[local.0 as usize].changed = true;
                    }
                    match statement {
                        Statement::Clear(local) => uses[local.0 as usize].changed = true,
                        Statement::Moved(place) | Statement::Lend(_, place) => {
                            uses[place.local.0 as usize].changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        for local in read {
            uses[local.0 as usize].kept = true;
        }
    }
    // What lives, unchanged, for the whole call: a parameter the caller
    // gave, and a local that holds a field of one, without a reference. A
    // loan of the caller's place, as a closure's captures are, only while
    // the function itself reads it and gives it to nothing that could
    // write through it.
    let unchanged = |u: &Uses| !u.changed && u.released == 0 && u.retained == 0;
    let mut alive = vec![false; body.locals.len()];
    for (index, &param) in body.params.iter().enumerate() {
        let u = &uses[param.0 as usize];
        let loan = body.loans.get(index).copied().unwrap_or(false);
        alive[param.0 as usize] = u.written == 0 && unchanged(u) && !(loan && u.passed);
    }
    for local in 0..body.locals.len() {
        let u = &uses[local];
        if u.written == 1 && unchanged(u) && u.copy_of.as_ref().is_some_and(|source| alive[source.local.0 as usize]) {
            alive[local] = true;
        }
    }
    let mut gone: Vec<bool> = uses
        .iter()
        .map(|u| {
            u.written == 1
                && u.retained == 1
                && u.released > 0
                && !u.changed
                && !u.kept
                && u.copy_of.as_ref().is_some_and(|source| alive[source.local.0 as usize])
        })
        .collect();
    // The retain must follow the copy at once: nothing between them may
    // release what it took.
    for block in &body.blocks {
        for (at, statement) in block.stmts.iter().enumerate() {
            let Statement::Retain(place) = statement else { continue };
            let after_copy = at.checked_sub(1).is_some_and(|before| {
                matches!(&block.stmts[before], Statement::Assign(target, value, _)
                    if target.local == place.local && matches!(&**value, Rvalue::Use(_)))
            });
            if !after_copy {
                gone[place.local.0 as usize] = false;
            }
        }
    }
    if !gone.iter().any(|&gone| gone) {
        return;
    }
    for block in &mut body.blocks {
        block.stmts.retain(|statement| match statement {
            Statement::Retain(place) | Statement::Release(place) => {
                !(place.path.is_empty() && gone[place.local.0 as usize])
            }
            _ => true,
        });
    }
}
