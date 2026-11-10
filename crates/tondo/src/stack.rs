//! Texts and functions' values made where the function's frame keeps
//! them, rather than on the heap, when nothing but this function's own
//! borrows reads them: a text an interpolation makes to be measured or
//! compared, as `"{i}".length()`, was allocated and freed each time, and so
//! was the value of a closure passed to a function inlined here, which
//! then only reads its captures. Its room in the frame gives it the count
//! of a value nothing counts, as a literal's, so that retaining and
//! releasing it do nothing: a text that does not fit there goes on the heap
//! as before, and its release frees it; a function's value releases its
//! captures where it is released.
//!
//! A text made so is assigned only by interpolations that append to no
//! other text, as many times as copies of a loop do, and is otherwise only
//! released, measured or compared; a function's value only by closures,
//! and is otherwise only released, called, or read for its captures: any
//! other read could keep it past the frame, or past the next value made in
//! its room. A function that calls itself makes none, as each of its frames
//! would keep room for them.

use crate::ast::BinOp;
use crate::mir::*;
use crate::types::{Ty, Types};

/// The most texts one frame makes so.
const FRAMED: usize = 8;

pub fn stack(bodies: &mut [Body], types: &Types) {
    let indices: Vec<usize> = (0..bodies.len()).collect();
    let framed = crate::parallel::map(&indices, 16, |&index| framed(&bodies[index], index, types));
    for (body, (framed, mut notes)) in bodies.iter_mut().zip(framed) {
        body.framed = framed;
        body.notes.append(&mut notes);
    }
}

/// The locals made in the frame, and, while its MIR is to be written, why
/// the others that could be are not.
fn framed(body: &Body, index: usize, types: &Types) -> (Vec<Local>, Vec<String>) {
    let noting = crate::mir_text::noting(&body.name);
    let count = body.locals.len();
    let mut erased = vec![false; count];
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && matches!(**rvalue, Rvalue::Erase(..))
        {
            erased[place.local.0 as usize] = true;
        }
    }
    let (mut made, mut kept, mut otherwise) = (vec![false; count], vec![false; count], vec![false; count]);
    let keep = |locals: Vec<Local>, kept: &mut Vec<bool>| locals.into_iter().for_each(|l| kept[l.0 as usize] = true);
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let (mut read, mut defined) = (Vec::new(), Vec::new());
        match statement {
            Statement::Release(place) if place.path.is_empty() => continue,
            Statement::Assign(place, rvalue, _) => {
                let interpolated = matches!(&**rvalue,
                    Rvalue::Concat(pieces) if !pieces.iter().any(|piece| matches!(piece, Piece::Taken(_))))
                    || matches!(**rvalue, Rvalue::Erase(..));
                if place.path.is_empty() {
                    let at = place.local.0 as usize;
                    made[at] |= interpolated;
                    otherwise[at] |= !interpolated;
                }
                match &**rvalue {
                    Rvalue::Concat(_) | Rvalue::Erase(..) => statement_locals(statement, &mut read, &mut defined),
                    // Borrows that keep nothing.
                    Rvalue::Length(operand) => borrowed(&[operand], &mut read),
                    Rvalue::Binary(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b) => {
                        borrowed(&[a, b], &mut read)
                    }
                    Rvalue::Call(crate::hir::Callee::Fn(callee), _) if callee.0 as usize == index => {
                        let notes = match noting {
                            true => vec!["stack: nothing in the frame: the function calls itself".to_string()],
                            false => Vec::new(),
                        };
                        return (Vec::new(), notes);
                    }
                    _ => statement_locals(statement, &mut read, &mut defined),
                }
                // What the place's indexes read.
                let mut indexes = Vec::new();
                for projection in &place.path {
                    if let Proj::Index(local, _) | Proj::InBounds(local) = projection {
                        indexes.push(*local);
                    }
                }
                keep(indexes, &mut kept);
            }
            // What any other statement reads or defines, as a loan or a
            // move, may keep it.
            _ => {
                statement_locals(statement, &mut read, &mut defined);
                read.append(&mut defined);
            }
        }
        // A function's value read only for its captures, or called.
        read.retain(|&local| !erased[local.0 as usize] || !borrowed_function(statement, local));
        keep(read, &mut kept);
    }
    for block in &body.blocks {
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        keep(read, &mut kept);
    }
    body.params.iter().chain(&body.ret).for_each(|local| kept[local.0 as usize] = true);
    let mut notes = Vec::new();
    let mut framed = Vec::new();
    for at in 0..count {
        if !made[at] || !(erased[at] || types.repr(body.locals[at]) == Ty::STRING) {
            continue;
        }
        let why = if otherwise[at] {
            Some("something else is assigned to it")
        } else if kept[at] {
            Some("it is read in a way that may keep it: copied, passed, returned or lent")
        } else if framed.len() == FRAMED {
            Some("the frame holds as many as it keeps")
        } else {
            None
        };
        match why {
            None => framed.push(Local(at as u32)),
            Some(why) if noting => notes.push(format!("stack: l{at} stays on the heap: {why}")),
            Some(_) => {}
        }
    }
    (framed, notes)
}

/// Whether the statement reads the function's value at `local` only for
/// its captures, or calls it.
fn borrowed_function(statement: &Statement, local: Local) -> bool {
    let mut statement = statement.clone();
    if let Statement::Assign(_, rvalue, _) = &mut statement
        && let Rvalue::CallIndirect(callee, _) = &mut **rvalue
        && matches!(callee, Operand::Copy(place) if place.local == local && place.path.is_empty())
    {
        *callee = Operand::Const(Const::Unit);
    }
    let mut only = true;
    crate::inline::visit_statement(&mut statement, &mut |place: &mut Place| {
        if place.local == local && !matches!(place.path.first(), Some(Proj::Captures(_))) {
            only = false;
        }
    });
    only
}

/// The locals the operands read, but a whole local, which they only borrow.
fn borrowed(operands: &[&Operand], read: &mut Vec<Local>) {
    for operand in operands {
        match operand {
            Operand::Copy(place) if place.path.is_empty() => {}
            operand => operand_uses(operand, read),
        }
    }
}
