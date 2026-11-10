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
    for (body, (framed, mut notes, remarks)) in bodies.iter_mut().zip(framed) {
        body.framed = framed;
        body.notes.append(&mut notes);
        for (span, kind, text) in remarks {
            crate::mir_text::remark(body, span, kind, || text);
        }
    }
}

/// What `framed` finds: the locals made in the frame; while the MIR is to
/// be written, why the others that could be are not; and, while remarks
/// are asked for, what is made where, at the source that makes it.
type Found = (Vec<Local>, Vec<String>, Vec<(crate::source::Span, &'static str, String)>);

const COPIED: &str = "it is copied";
const READ: &str = "it is read in a way that may keep it";

/// Keeps the most telling of what may keep a local: the first but for a
/// read that says less.
fn told(kept: &mut Option<&'static str>, why: &'static str) {
    if kept.is_none() || *kept == Some(READ) || (*kept == Some(COPIED) && why != READ) {
        *kept = Some(why);
    }
}

/// What in a statement may keep what it reads, in words.
fn keeping(statement: &Statement) -> &'static str {
    match statement {
        Statement::Assign(place, _, _) if !place.path.is_empty() => "it is stored in a field or an element",
        Statement::Assign(_, rvalue, _) => match &**rvalue {
            Rvalue::Call(..) | Rvalue::CallIndirect(..) => "it is passed to a call that may keep it",
            Rvalue::Erase(..) => "a closure captures it",
            Rvalue::Spawn(..) => "a task takes it",
            Rvalue::Aggregate(_) | Rvalue::Variant(..) | Rvalue::Array(..) => "a value holds it",
            Rvalue::Mutate(..) => "a collection takes it",
            Rvalue::Use(_) => COPIED,
            _ => READ,
        },
        Statement::Lend(..) | Statement::LendAt(..) => "it is lent",
        _ => READ,
    }
}

fn framed(body: &Body, index: usize, types: &Types) -> Found {
    let noting = crate::mir_text::noting(&body.name);
    let remarking = crate::mir_text::remarking();
    let mut made_at: Vec<Option<crate::source::Span>> = vec![None; body.locals.len()];
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
    let (mut made, mut otherwise) = (vec![false; count], vec![false; count]);
    // What may keep each local, first.
    let mut kept: Vec<Option<&'static str>> = vec![None; count];
    let keep = |locals: Vec<Local>, kept: &mut Vec<Option<&'static str>>, why: &'static str| {
        locals.into_iter().for_each(|l| told(&mut kept[l.0 as usize], why))
    };
    // Where each local is copied to, first, whose keeping says why.
    let mut copied: Vec<Option<Local>> = vec![None; count];
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let why = keeping(statement);
        if let Statement::Assign(to, rvalue, _) = statement
            && let Rvalue::Use(Operand::Copy(from)) = &**rvalue
            && to.path.is_empty()
            && from.path.is_empty()
        {
            copied[from.local.0 as usize].get_or_insert(to.local);
        }
        let (mut read, mut defined) = (Vec::new(), Vec::new());
        match statement {
            Statement::Release(place) if place.path.is_empty() => continue,
            Statement::Assign(place, rvalue, span) => {
                let interpolated = matches!(&**rvalue,
                    Rvalue::Concat(pieces) if !pieces.iter().any(|piece| matches!(piece, Piece::Taken(_))))
                    || matches!(**rvalue, Rvalue::Erase(..));
                if place.path.is_empty() {
                    let at = place.local.0 as usize;
                    made[at] |= interpolated;
                    otherwise[at] |= !interpolated;
                    if interpolated {
                        made_at[at].get_or_insert(*span);
                    }
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
                        return (Vec::new(), notes, Vec::new());
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
                keep(indexes, &mut kept, why);
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
        keep(read, &mut kept, why);
    }
    for block in &body.blocks {
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        let why = match block.terminator {
            Terminator::Return(_) => "it is returned",
            _ => "a branch reads it",
        };
        keep(read, &mut kept, why);
    }
    body.params.iter().for_each(|&local| told(&mut kept[local.0 as usize], "it is a parameter"));
    body.ret.iter().for_each(|&local| told(&mut kept[local.0 as usize], "it is returned"));
    // A copy keeps what its copy is kept by; one that nothing keeps names
    // where it was copied to.
    let mut copied_to: Vec<Option<usize>> = vec![None; count];
    for at in 0..count {
        let (mut local, mut steps) = (at, 0);
        while kept[local] == Some(COPIED)
            && let Some(to) = copied[local]
            && steps < 16
        {
            (local, steps) = (to.0 as usize, steps + 1);
        }
        match kept[local] {
            Some(_) => kept[at] = kept[local],
            None => copied_to[at] = Some(local),
        }
    }
    let (mut notes, mut remarks) = (Vec::new(), Vec::new());
    let mut framed = Vec::new();
    for at in 0..count {
        if !made[at] || !(erased[at] || types.repr(body.locals[at]) == Ty::STRING) {
            continue;
        }
        let why = if otherwise[at] {
            Some("something else is assigned to it".to_string())
        } else if let Some(why) = kept[at] {
            let name = copied_to[at].and_then(|to| body.names.get(to).cloned().flatten());
            Some(match (why == COPIED, name) {
                (true, Some(name)) => format!("it is copied to `{name}`"),
                _ => why.to_string(),
            })
        } else if framed.len() == FRAMED {
            Some("the frame holds as many as it keeps".to_string())
        } else {
            None
        };
        match why {
            None => {
                framed.push(Local(at as u32));
                if let (true, Some(span)) = (remarking, made_at[at]) {
                    remarks.push((span, "frame", "is made in the frame, without the heap".to_string()));
                }
            }
            Some(why) => {
                if noting {
                    notes.push(format!("stack: l{at} stays on the heap: {why}"));
                }
                if let (true, Some(span)) = (remarking, made_at[at]) {
                    remarks.push((span, "heap", format!("is made on the heap: {why}")));
                }
            }
        }
    }
    (framed, notes, remarks)
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
