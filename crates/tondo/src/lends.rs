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
//!
//! Such a copy then lives unchanged for the whole call too, and so does an
//! element of an array that does, which nothing releases while the array
//! stays as it is: `for name in names`, as inlining leaves a loop over a
//! parameter, held the array and each name with a reference of its own, a
//! retain and a release for each name read. Reading an element, a field
//! or a length keeps nothing, nor does interpolating a value.
//!
//! An array the function made itself is held for a loop over it by a copy,
//! retained before the loop and released after: an element read in the
//! loop, and used there alone, lives while that copy holds it, so it needs
//! no reference of its own either.

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
                        // A copy of a field: one with a reference of its own
                        // needs nothing of what it copies, and one without
                        // lives only while that does.
                        Rvalue::Use(Operand::Copy(source))
                            if target.path.is_empty()
                                && !source.path.is_empty()
                                && source.path.iter().all(|projection| matches!(projection, Proj::Field(_))) =>
                        {
                            uses[target.local.0 as usize].copy_of = Some(source.clone());
                        }
                        Rvalue::Use(Operand::Copy(source)) if target.path.is_empty() && source.path.is_empty() => {
                            uses[target.local.0 as usize].copy_of = Some(source.clone());
                            uses[source.local.0 as usize].kept = true;
                        }
                        // An element, which the array keeps.
                        Rvalue::Use(Operand::Copy(source))
                            if target.path.is_empty()
                                && matches!(source.path[..], [Proj::InBounds(_) | Proj::Index(..)]) =>
                        {
                            uses[target.local.0 as usize].copy_of = Some(source.clone());
                        }
                        Rvalue::Length(_) => {}
                        // An interpolation reads its values and keeps none,
                        // but the text it takes to append to.
                        Rvalue::Concat(pieces) => {
                            for piece in pieces {
                                let (operand, taken) = match piece {
                                    Piece::Value(operand, _) => (operand.clone(), false),
                                    Piece::Taken(place) => (Operand::Copy(place.clone()), true),
                                    Piece::Text(_) => continue,
                                };
                                let mut read = Vec::new();
                                operand_uses(&operand, &mut read);
                                for local in read {
                                    uses[local.0 as usize].kept |= taken;
                                    uses[local.0 as usize].passed = true;
                                }
                            }
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
    // A copy of what lives that needs no reference of its own lives too,
    // and an element of it: as far as copies of copies go.
    let mut gone = vec![false; body.locals.len()];
    for _ in 0..3 {
        for local in 0..body.locals.len() {
            let u = &uses[local];
            if alive[local] || u.written != 1 || u.changed {
                continue;
            }
            if !u.copy_of.as_ref().is_some_and(|source| alive[source.local.0 as usize]) {
                continue;
            }
            if unchanged(u) {
                alive[local] = true;
            } else if u.retained == 1 && u.released > 0 && !u.kept {
                (alive[local], gone[local]) = (true, true);
            }
        }
    }
    held_elements(body, &uses, &mut gone);
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

/// Marks gone the elements a loop reads from the copy that holds its array,
/// when they are used in the loop alone: the copy is a whole local's,
/// written and retained in the one block that goes to the loop, changed and
/// kept nowhere, and released only out of the loop.
fn held_elements(body: &Body, uses: &[Uses], gone: &mut [bool]) {
    let count = body.locals.len();
    // Where each local is mentioned but by its releases, where it is
    // released, and where it is written whole and retained.
    let (mut mentioned, mut released) = (vec![Vec::new(); count], vec![Vec::new(); count]);
    let mut written: Vec<Option<(usize, usize)>> = vec![None; count];
    let mut retained: Vec<Option<(usize, usize)>> = vec![None; count];
    let (mut read, mut defined) = (Vec::new(), Vec::new());
    for (block, data) in body.blocks.iter().enumerate() {
        for (at, statement) in data.stmts.iter().enumerate() {
            match statement {
                Statement::Release(place) if place.path.is_empty() => released[place.local.0 as usize].push(block),
                Statement::Retain(place) if place.path.is_empty() => {
                    retained[place.local.0 as usize] = Some((block, at));
                }
                _ => {
                    statement_locals(statement, &mut read, &mut defined);
                    if let Statement::Assign(target, ..) = statement
                        && target.path.is_empty()
                    {
                        written[target.local.0 as usize] = Some((block, at));
                    }
                    for local in read.drain(..).chain(defined.drain(..)) {
                        mentioned[local.0 as usize].push(block);
                    }
                }
            }
        }
        terminator_uses(&data.terminator, &mut read);
        for local in read.drain(..) {
            mentioned[local.0 as usize].push(block);
        }
    }
    let loops = counted_loops(&body.blocks);
    for element in 0..count {
        let u = &uses[element];
        let Some(source) = &u.copy_of else { continue };
        if gone[element]
            || !matches!(source.path[..], [Proj::InBounds(_) | Proj::Index(..)])
            || u.written != 1
            || u.retained != 1
            || u.released == 0
            || u.changed
            || u.kept
        {
            continue;
        }
        let array = source.local.0 as usize;
        let a = &uses[array];
        let copy = a.written == 1
            && a.retained == 1
            && !a.changed
            && !a.kept
            && a.copy_of.as_ref().is_some_and(|whole| whole.path.is_empty());
        let held = |counted: &Counted| {
            let Some((block, at)) = written[array] else { return false };
            block == counted.before
                && retained[array] == Some((block, at + 1))
                && released[array].iter().all(|&b| !counted.inside[b] && b != counted.head && b != counted.before)
                && mentioned[element].iter().all(|&b| counted.inside[b])
        };
        if copy && loops.iter().any(held) {
            gone[element] = true;
        }
    }
}
