//! Texts made where the function's frame keeps them, rather than on the
//! heap, when nothing but this function's own borrows reads them: a text
//! an interpolation makes to be measured or compared, as `"{i}".length()`,
//! was allocated and freed each time. Its room in the frame gives it the
//! count of a text nothing counts, as a literal's, so that retaining and
//! releasing it do nothing; a text that does not fit there goes on the
//! heap as before, and its release frees it.
//!
//! A text made so is assigned only by interpolations that append to no
//! other text, as many times as copies of a loop do, and is otherwise only
//! released, measured or compared: any other read could keep it past the
//! frame, or past the next text made in its room. A function that calls
//! itself makes none, as each of its frames would keep room for them.

use crate::ast::BinOp;
use crate::mir::*;
use crate::types::{Ty, Types};

/// The most texts one frame makes so.
const FRAMED: usize = 8;

pub fn stack(bodies: &mut [Body], types: &Types) {
    let indices: Vec<usize> = (0..bodies.len()).collect();
    let framed = crate::parallel::map(&indices, 16, |&index| framed(&bodies[index], index, types));
    for (body, framed) in bodies.iter_mut().zip(framed) {
        body.framed = framed;
    }
}

fn framed(body: &Body, index: usize, types: &Types) -> Vec<Local> {
    let count = body.locals.len();
    let (mut made, mut kept, mut otherwise) = (vec![false; count], vec![false; count], vec![false; count]);
    let keep = |locals: Vec<Local>, kept: &mut Vec<bool>| locals.into_iter().for_each(|l| kept[l.0 as usize] = true);
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let (mut read, mut defined) = (Vec::new(), Vec::new());
        match statement {
            Statement::Release(place) if place.path.is_empty() => continue,
            Statement::Assign(place, rvalue, _) => {
                let interpolated = matches!(&**rvalue,
                    Rvalue::Concat(pieces) if !pieces.iter().any(|piece| matches!(piece, Piece::Taken(_))));
                if place.path.is_empty() {
                    let at = place.local.0 as usize;
                    made[at] |= interpolated;
                    otherwise[at] |= !interpolated;
                }
                match &**rvalue {
                    Rvalue::Concat(_) => statement_locals(statement, &mut read, &mut defined),
                    // Borrows that keep nothing.
                    Rvalue::Length(operand) => borrowed(&[operand], &mut read),
                    Rvalue::Binary(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b) => {
                        borrowed(&[a, b], &mut read)
                    }
                    Rvalue::Call(crate::hir::Callee::Fn(callee), _) if callee.0 as usize == index => return Vec::new(),
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
        keep(read, &mut kept);
    }
    for block in &body.blocks {
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        keep(read, &mut kept);
    }
    body.params.iter().chain(&body.ret).for_each(|local| kept[local.0 as usize] = true);
    (0..count)
        .map(|local| Local(local as u32))
        .filter(|&local| {
            let at = local.0 as usize;
            made[at] && !kept[at] && !otherwise[at] && types.repr(body.locals[at]) == Ty::STRING
        })
        .take(FRAMED)
        .collect()
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
