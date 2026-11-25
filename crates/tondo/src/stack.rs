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
//!
//! A local made from a temporary, as `let label = "{n}"` lowers to, is made
//! by the interpolation itself, when the temporary is only moved there:
//! the local is then made so, under the same rules.

use crate::ast::BinOp;
use crate::hir::StrOp;
use crate::mir::*;
use crate::types::{Ty, Types};

/// The most texts one frame makes so.
const FRAMED: usize = 16;
/// The most zero-copy string views one frame keeps in stack slots (32 B each).
const FRAMED_VIEWS: usize = 16;

pub fn stack(bodies: &mut [Body], types: &Types) {
    bodies.iter_mut().for_each(made_in_place);
    let borrows = borrowing(bodies, types);
    let indices: Vec<usize> = (0..bodies.len()).collect();
    let framed = crate::parallel::map(&indices, 16, |&index| framed(&bodies[index], index, types, &borrows));
    for (body, (framed, mut notes, remarks)) in bodies.iter_mut().zip(framed) {
        body.framed = framed;
        body.notes.append(&mut notes);
        for (span, kind, text, by) in remarks {
            crate::mir_text::remark_by(body, span, kind, by, true, None, || text);
        }
        if crate::mir_text::remarking() {
            for (span, text) in allocations(body, types) {
                crate::mir_text::remark(body, span, "heap", || text);
            }
        }
    }
    between_views(bodies, types);
}

/// Whether a statement makes a text by interpolation, appending to no
/// other text, or a function's value from a closure, into a whole local.
fn making(statement: &Statement) -> Option<(Local, &Rvalue)> {
    let Statement::Assign(place, rvalue, _) = statement else { return None };
    let interpolated = matches!(&**rvalue,
        Rvalue::Concat(pieces) if !pieces.iter().any(|piece| matches!(piece, Piece::Taken(_))))
        || matches!(**rvalue, Rvalue::Erase(..));
    (place.path.is_empty() && interpolated).then_some((place.local, &**rvalue))
}

/// A making into a temporary that the next statement moves to a local of
/// its type, which the making does not read: the temporary and the local.
fn moved_next(body: &Body, first: &Statement, second: &Statement) -> Option<(Local, Local)> {
    let (made, rvalue) = making(first)?;
    let Statement::Assign(to, moved, _) = second else { return None };
    let Rvalue::Use(Operand::Copy(from)) = &**moved else { return None };
    let mut read = Vec::new();
    rvalue_uses(rvalue, &mut read);
    let fits = from.local == made && from.path.is_empty() && to.path.is_empty() && to.local != made;
    (fits
        && body.locals[to.local.0 as usize] == body.locals[made.0 as usize]
        && !read.contains(&to.local)
        && !read.contains(&made))
    .then_some((made, to.local))
}

/// Makes in the local what a temporary only moved there was made as: the
/// temporary is mentioned nowhere but in its making and its move, each
/// time.
fn made_in_place(body: &mut Body) {
    let count = body.locals.len();
    let mut moves = vec![0usize; count];
    for block in &body.blocks {
        for two in block.stmts.windows(2) {
            if let Some((made, _)) = moved_next(body, &two[0], &two[1]) {
                moves[made.0 as usize] += 1;
            }
        }
    }
    if moves.iter().all(|&moved| moved == 0) {
        return;
    }
    // How many statements and terminators mention each local.
    let mut mentions = vec![0usize; count];
    let (mut used, mut defined) = (Vec::new(), Vec::new());
    for block in &body.blocks {
        for statement in &block.stmts {
            statement_locals(statement, &mut used, &mut defined);
            used.append(&mut defined);
            used.sort_unstable_by_key(|local| local.0);
            used.dedup();
            used.drain(..).for_each(|local| mentions[local.0 as usize] += 1);
        }
        terminator_uses(&block.terminator, &mut used);
        used.drain(..).for_each(|local| mentions[local.0 as usize] += 1);
    }
    body.params.iter().chain(&body.ret).for_each(|local| mentions[local.0 as usize] = usize::MAX);
    for at in 0..body.blocks.len() {
        let stmts = std::mem::take(&mut body.blocks[at].stmts);
        let mut kept = Vec::with_capacity(stmts.len());
        let mut statements = stmts.into_iter().peekable();
        while let Some(mut statement) = statements.next() {
            let only = statements.peek().and_then(|next| moved_next(body, &statement, next)).filter(|(made, _)| {
                let made = made.0 as usize;
                mentions[made] == 2 * moves[made]
            });
            if let (Some((_, to)), Statement::Assign(place, ..)) = (only, &mut statement) {
                *place = Place::local(to);
                statements.next();
            }
            kept.push(statement);
        }
        body.blocks[at].stmts = kept;
    }
}

/// What else a body makes on the heap, at the code that makes it: arrays
/// whose values are not all constants, maps and sets, slices, and the
/// boxes of a value whose type holds itself (`tondo optimizations`).
fn allocations(body: &Body, types: &Types) -> Vec<(crate::source::Span, String)> {
    let mut found = Vec::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let Statement::Assign(place, rvalue, span) = statement else { continue };
        let ty = body.place_ty(types, place);
        let text = match &**rvalue {
            Rvalue::Array(values, _) => {
                let crate::types::TyKind::Array(element) = *types.kind(types.repr(ty)) else { continue };
                let constant = |value: &Operand| matches!(value, Operand::Const(constant) if crate::codegen::in_data(types, element, constant));
                if values.iter().all(constant) {
                    continue;
                }
                "is made on the heap: an array whose values are not all constants".to_string()
            }
            Rvalue::MapOp(crate::hir::MapOp::New, ..) => match types.kind(types.repr(ty)) {
                crate::types::TyKind::Set(_) => "is made on the heap with its first element: a set".to_string(),
                _ => "is made on the heap with its first entry: a map".to_string(),
            },
            Rvalue::Slice(..) => "is made on the heap: a slice copies its part".to_string(),
            Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) => {
                let boxed = operands.iter().find_map(|operand| {
                    let component = match operand {
                        Operand::Copy(place) => body.place_ty(types, place),
                        Operand::Const(_) => return None,
                    };
                    types.is_boxed(ty, component).then_some(component)
                });
                let Some(component) = boxed else { continue };
                let name = types.display(component);
                format!("is made with a box on the heap: the `{name}` in it holds itself")
            }
            _ => continue,
        };
        found.push((*span, text));
    }
    found
}

/// The code that keeps a value, when a statement does.
type By = Option<crate::mir::By>;

/// What `framed` finds: the locals made in the frame; while the MIR is to
/// be written, why the others that could be are not; and, while remarks
/// are asked for, what is made where, at the source that makes it.
type Found = (Vec<Local>, Vec<String>, Vec<(crate::source::Span, &'static str, String, By)>);

const COPIED: &str = "it is copied";
const READ: &str = "it is read in a way that may keep it";

/// Keeps the most telling of what may keep a local: the first but for a
/// read that says less.
fn told(kept: &mut Option<&'static str>, why: &'static str) -> bool {
    let telling = kept.is_none() || *kept == Some(READ) || (*kept == Some(COPIED) && why != READ);
    if telling {
        *kept = Some(why);
    }
    telling
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

/// For each function, which of its parameters that are `fn` values it only
/// borrows: calls, reads for their captures, or passes on to a parameter
/// that borrows its value too, as the copy of a function the inliner made
/// for a closure does. A closure made in the frame of a caller lives for
/// the whole call, so it may be passed to such a parameter.
fn borrowing(bodies: &[Body], types: &Types) -> Vec<Vec<bool>> {
    let function = |body: &Body, local: Local| {
        matches!(types.kind(types.repr(body.locals[local.0 as usize])), crate::types::TyKind::Fn(..))
    };
    let mut borrows: Vec<Vec<bool>> = bodies
        .iter()
        .map(|body| {
            body.params
                .iter()
                .enumerate()
                .map(|(at, &param)| function(body, param) && !body.loans.get(at).copied().unwrap_or(false))
                .collect()
        })
        .collect();
    // From all of them to those whose every use borrows, as a parameter
    // passed on is known only once its callee's is.
    loop {
        let mut changed = false;
        for (index, body) in bodies.iter().enumerate() {
            for (at, &param) in body.params.iter().enumerate() {
                if borrows[index][at] && !only_borrowed(body, param, &borrows) {
                    borrows[index][at] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            return borrows;
        }
    }
}

/// Whether a function only calls the `fn` value a local holds, reads its
/// captures, or passes it whole to a parameter that borrows its value.
fn only_borrowed(body: &Body, local: Local, borrows: &[Vec<bool>]) -> bool {
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let mut statement = statement.clone();
        if let Statement::Assign(target, rvalue, _) = &mut statement {
            if target.local == local {
                return false;
            }
            if let Rvalue::Call(crate::hir::Callee::Fn(callee), operands) = &mut **rvalue {
                for (at, operand) in operands.iter_mut().enumerate() {
                    let whole =
                        matches!(operand, Operand::Copy(place) if place.local == local && place.path.is_empty());
                    if whole {
                        if !borrows.get(callee.0 as usize).and_then(|params| params.get(at)).copied().unwrap_or(false) {
                            return false;
                        }
                        *operand = Operand::Const(Const::Unit);
                    }
                }
            }
        } else {
            let (mut read, mut defined) = (Vec::new(), Vec::new());
            statement_locals(&statement, &mut read, &mut defined);
            if read.contains(&local) || defined.contains(&local) {
                return false;
            }
            continue;
        }
        if !borrowed_function(&statement, local) {
            return false;
        }
    }
    body.blocks.iter().all(|block| {
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        !read.contains(&local)
    })
}

fn framed(body: &Body, index: usize, types: &Types, borrows: &[Vec<bool>]) -> Found {
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
    // And the statement that keeps it, when it is one: the code a remark
    // names.
    let mut kept_by: Vec<By> = vec![None; count];
    let keep =
        |locals: Vec<Local>, kept: &mut Vec<Option<&'static str>>, kept_by: &mut Vec<By>, why: &'static str, by: By| {
            for local in locals {
                if told(&mut kept[local.0 as usize], why) {
                    kept_by[local.0 as usize] = by;
                }
            }
        };
    // Where each local is copied to, first, whose keeping says why.
    let mut copied: Vec<Option<Local>> = vec![None; count];
    let statements = body.blocks.iter().enumerate().flat_map(|(block, data)| {
        let scope = body.scopes.get(block).copied().flatten();
        data.stmts.iter().map(move |statement| (statement, scope))
    });
    for (statement, scope) in statements {
        let why = keeping(statement);
        let by = match statement {
            Statement::Assign(_, _, span) => Some((*span, scope)),
            _ => None,
        };
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
                let interpolated = making(statement).is_some();
                if place.path.is_empty() {
                    let at = place.local.0 as usize;
                    made[at] |= interpolated;
                    otherwise[at] |= !interpolated;
                    if interpolated {
                        made_at[at].get_or_insert(*span);
                    }
                }
                match &**rvalue {
                    // A text among others is copied into the one made, or
                    // appended in place; a text alone is that text.
                    Rvalue::Concat(pieces) if pieces.len() > 1 => {
                        statement_locals(statement, &mut read, &mut defined);
                        let taken = |local: Local| {
                            local == place.local
                                || pieces.iter().any(|piece| matches!(piece, Piece::Taken(at) if at.local == local))
                        };
                        let lent: Vec<Local> = pieces
                            .iter()
                            .filter_map(|piece| match piece {
                                Piece::Value(Operand::Copy(at), ty) if *ty == Ty::STRING && at.path.is_empty() => {
                                    Some(at.local)
                                }
                                _ => None,
                            })
                            .filter(|&local| !taken(local))
                            .collect();
                        read.retain(|local| !lent.contains(local));
                    }
                    Rvalue::Concat(_) | Rvalue::Erase(..) => statement_locals(statement, &mut read, &mut defined),
                    // Borrows that keep nothing.
                    Rvalue::Length(operand) => borrowed(&[operand], &mut read),
                    Rvalue::Call(crate::hir::Callee::Std(callee), operands) if reads_only(*callee) => {
                        borrowed(&operands.iter().collect::<Vec<_>>(), &mut read)
                    }
                    // What a text holds, read and answered with a scalar.
                    Rvalue::StrOp(
                        StrOp::At
                        | StrOp::Get
                        | StrOp::ByteLength
                        | StrOp::ByteAt
                        | StrOp::ByteWithin
                        | StrOp::Contains
                        | StrOp::StartsWith
                        | StrOp::EndsWith
                        | StrOp::Find,
                        operands,
                        _,
                    ) => borrowed(&operands.iter().collect::<Vec<_>>(), &mut read),
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
                keep(indexes, &mut kept, &mut kept_by, why, by);
            }
            // What any other statement reads or defines, as a loan or a
            // move, may keep it.
            _ => {
                statement_locals(statement, &mut read, &mut defined);
                read.append(&mut defined);
            }
        }
        // A function's value read only for its captures, or called.
        read.retain(|&local| {
            !erased[local.0 as usize] || !borrowed_function(&lent_on(statement, local, borrows), local)
        });
        keep(read, &mut kept, &mut kept_by, why, by);
    }
    for block in &body.blocks {
        let mut read = Vec::new();
        terminator_uses(&block.terminator, &mut read);
        let why = match block.terminator {
            Terminator::Return(_) => "it is returned",
            _ => "a branch reads it",
        };
        keep(read, &mut kept, &mut kept_by, why, None);
    }
    body.params.iter().for_each(|&local| _ = told(&mut kept[local.0 as usize], "it is a parameter"));
    body.ret.iter().for_each(|&local| _ = told(&mut kept[local.0 as usize], "it is returned"));
    // A copy keeps what its copy is kept by; one that nothing keeps names
    // where it was copied to, the first local with a name.
    let mut copied_to: Vec<Option<usize>> = vec![None; count];
    for at in 0..count {
        let (mut local, mut steps, mut named) = (at, 0, None);
        while kept[local] == Some(COPIED)
            && let Some(to) = copied[local]
            && steps < 16
        {
            (local, steps) = (to.0 as usize, steps + 1);
            named = named.or(body.names.get(local).and_then(Option::as_ref).map(|_| local));
        }
        match kept[local] {
            Some(_) => (kept[at], kept_by[at]) = (kept[local], kept_by[local]),
            None => copied_to[at] = Some(named.unwrap_or(local)),
        }
    }
    let (mut notes, mut remarks) = (Vec::new(), Vec::new());
    let mut framed = Vec::new();
    // Those made in a loop first, which would take the heap in every turn.
    let looping = inner_loops(&body.blocks, true);
    let mut looped = vec![false; count];
    for (_, data) in body.blocks.iter().enumerate().filter(|(block, _)| looping[*block]) {
        for (local, _) in data.stmts.iter().filter_map(making) {
            looped[local.0 as usize] = true;
        }
    }
    let order = (0..count).filter(|&at| looped[at]).chain((0..count).filter(|&at| !looped[at]));
    for at in order {
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
                    remarks.push((span, "frame", "is made in the frame, without the heap".to_string(), None));
                }
            }
            Some(why) => {
                if noting {
                    notes.push(format!("stack: l{at} stays on the heap: {why}"));
                }
                if let (true, Some(span)) = (remarking, made_at[at]) {
                    remarks.push((span, "heap", format!("is made on the heap: {why}"), kept_by[at]));
                }
            }
        }
    }
    (framed, notes, remarks)
}

/// Whether the statement reads the function's value at `local` only for
/// its captures, or calls it.
/// The statement with a local passed whole to a parameter that only borrows
/// it, as `borrowing` finds, taken out of the call.
fn lent_on(statement: &Statement, local: Local, borrows: &[Vec<bool>]) -> Statement {
    let mut statement = statement.clone();
    if let Statement::Assign(_, rvalue, _) = &mut statement
        && let Rvalue::Call(crate::hir::Callee::Fn(callee), operands) = &mut **rvalue
    {
        for (at, operand) in operands.iter_mut().enumerate() {
            let whole = matches!(operand, Operand::Copy(place) if place.local == local && place.path.is_empty());
            if whole && borrows.get(callee.0 as usize).and_then(|params| params.get(at)).copied().unwrap_or(false) {
                *operand = Operand::Const(Const::Unit);
            }
        }
    }
    statement
}

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

/// Whether a call of the runtime only reads the texts it is given, while it
/// runs: console output copies them, `testing.keep` only observes the value,
/// and a socket's write sends them before it returns.
fn reads_only(callee: crate::hir::StdFn) -> bool {
    use crate::hir::StdFn;
    match callee {
        StdFn::ConsolePrint | StdFn::ConsolePrintln | StdFn::ConsoleEprint | StdFn::ConsoleEprintln | StdFn::Keep => {
            true
        }
        // A native whose code was read to keep none of the texts it is
        // given, that answers with a scalar, which holds none of them.
        StdFn::Native(index) => crate::hir::NATIVES.get(index as usize).is_some_and(|native| {
            use crate::hir::NativeTy::{Bool, Char, Float, Int, Unit};
            native.symbol == "tondo_rt_net_write"
                || crate::regions::reads_texts(native) && matches!(native.result, Int | Bool | Float | Char | Unit)
        }),
        _ => false,
    }
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

// ---------------------------------------------------------------------------
// Frame-lent zero-copy string views (`StrOp::BetweenView`)
// ---------------------------------------------------------------------------

/// A compact set of locals (one bit per local) used for liveness and view flow.
#[derive(Clone, PartialEq, Eq)]
struct BitSet(Vec<u64>);

impl BitSet {
    fn empty(count: usize) -> BitSet {
        BitSet(vec![0; count.div_ceil(64)])
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    fn has(&self, local: Local) -> bool {
        let i = local.0 as usize;
        (self.0[i / 64] >> (i % 64)) & 1 != 0
    }

    fn add(&mut self, local: Local) {
        let i = local.0 as usize;
        self.0[i / 64] |= 1 << (i % 64);
    }

    fn remove(&mut self, local: Local) {
        let i = local.0 as usize;
        self.0[i / 64] &= !(1 << (i % 64));
    }

    fn join(&mut self, other: &BitSet) -> bool {
        let mut changed = false;
        for (a, &b) in self.0.iter_mut().zip(&other.0) {
            let next = *a | b;
            if next != *a {
                *a = next;
                changed = true;
            }
        }
        changed
    }

    fn meet(&mut self, other: &BitSet) {
        for (a, &b) in self.0.iter_mut().zip(&other.0) {
            *a &= b;
        }
    }

    fn intersects(&self, other: &BitSet) -> bool {
        self.0.iter().zip(&other.0).any(|(&a, &b)| (a & b) != 0)
    }

    fn iter(&self) -> impl Iterator<Item = Local> + '_ {
        self.0.iter().enumerate().flat_map(|(wi, &word)| {
            let mut bits = word;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let tz = bits.trailing_zeros();
                bits &= bits - 1;
                Some(Local((wi * 64) as u32 + tz))
            })
        })
    }
}

/// Whether `rvalue` is a `textNative.betweenWithin(source, start, end)` call,
/// returning its `source` operand.
fn between_within_source(rvalue: &Rvalue) -> Option<&Operand> {
    let Rvalue::Call(crate::hir::Callee::Std(crate::hir::StdFn::Native(index)), args) = rvalue else {
        return None;
    };
    if crate::hir::NATIVES.get(*index as usize).is_some_and(|n| n.name == "betweenWithin") && args.len() == 3 {
        Some(&args[0])
    } else {
        None
    }
}

/// Whether a `StdFn` call only borrows any `String` arguments during the call
/// without retaining or returning pointers into them.
fn view_reads_only(callee: crate::hir::StdFn) -> bool {
    if reads_only(callee) {
        return true;
    }
    let crate::hir::StdFn::Native(index) = callee else { return false };
    crate::hir::NATIVES.get(index as usize).is_some_and(|native| {
        matches!(
            native.symbol,
            "tondo_rt_text_between"
                | "tondo_rt_text_trim"
                | "tondo_rt_text_split"
                | "tondo_rt_text_lines"
                | "tondo_rt_text_words"
                | "tondo_rt_text_repeat"
        )
    })
}

/// Reachable blocks from entry `0` in reverse postorder, plus a reachability
/// bitmap.
fn reachable_rpo(body: &Body) -> (Vec<usize>, Vec<bool>) {
    let n = body.blocks.len();
    let mut visited = vec![false; n];
    let mut post = Vec::with_capacity(n);
    if n == 0 {
        return (post, visited);
    }
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    visited[0] = true;
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        let succs = crate::hoist::targets(&body.blocks[block].terminator);
        if let Some(&to) = succs.get(*next) {
            *next += 1;
            if !visited[to] {
                visited[to] = true;
                stack.push((to, 0));
            }
        } else {
            post.push(block);
            stack.pop();
        }
    }
    post.reverse();
    (post, visited)
}

/// Locals whose address may be aliased (`var` loan parameters and `Lend`/
/// `LendAt` targets or aliases).
fn addressed_locals(body: &Body, reachable: &[bool]) -> Vec<bool> {
    let mut addressed = vec![false; body.locals.len()];
    for (&param, &loan) in body.params.iter().zip(&body.loans) {
        addressed[param.0 as usize] |= loan;
    }
    for (b, data) in body.blocks.iter().enumerate() {
        if !reachable[b] {
            continue;
        }
        for stmt in &data.stmts {
            match stmt {
                Statement::Lend(alias, place) => {
                    addressed[alias.0 as usize] = true;
                    addressed[place.local.0 as usize] = true;
                }
                Statement::LendAt(alias, _) => addressed[alias.0 as usize] = true,
                _ => {}
            }
        }
    }
    addressed
}

/// Checks whether `stmt` is safe for any locals currently in `held` (holding a
/// borrowed `StrFrameView` or borrowed `String` parameter), returning the
/// destination local if `stmt` propagates a view via a whole-local copy, or
/// `Err(())` if `stmt` could retain, escape, or unsafely use a local in `held`.
fn check_view_statement(
    body: &Body,
    types: &Types,
    stmt: &Statement,
    held: &BitSet,
    addressed: &[bool],
    str_borrows: &[Vec<bool>],
) -> Result<Option<Local>, ()> {
    match stmt {
        Statement::Release(place) | Statement::Moved(place) if place.path.is_empty() => Ok(None),
        Statement::Clear(_) | Statement::DeferPop | Statement::OsrLoad(_) => Ok(None),
        Statement::Assign(place, rvalue, _) => {
            if !place.path.is_empty() {
                if held.has(place.local) {
                    return Err(());
                }
                for proj in &place.path {
                    if let Proj::Index(i, _) | Proj::InBounds(i) = proj
                        && held.has(*i)
                    {
                        return Err(());
                    }
                }
            }
            let mut rused = Vec::new();
            rvalue_uses(rvalue, &mut rused);
            if !rused.iter().any(|&l| held.has(l)) {
                return Ok(None);
            }
            // Reject any projection read of a local in `held`.
            let whole_only = |ops: &[&Operand]| -> bool {
                ops.iter().all(|op| match op {
                    Operand::Copy(p) if held.has(p.local) => p.path.is_empty(),
                    Operand::Copy(p) => {
                        !p.path.iter().any(|proj| matches!(proj, Proj::Index(i, _) | Proj::InBounds(i) if held.has(*i)))
                    }
                    Operand::Const(_) => true,
                })
            };
            match &**rvalue {
                Rvalue::Use(Operand::Copy(from))
                    if place.path.is_empty() && from.path.is_empty() && held.has(from.local) =>
                {
                    if addressed[place.local.0 as usize] || body.ret == Some(place.local) {
                        return Err(());
                    }
                    Ok(Some(place.local))
                }
                Rvalue::Length(op) if whole_only(&[op]) => Ok(None),
                Rvalue::StrOp(
                    StrOp::At
                    | StrOp::Get
                    | StrOp::ByteLength
                    | StrOp::ByteAt
                    | StrOp::ByteWithin
                    | StrOp::Contains
                    | StrOp::StartsWith
                    | StrOp::EndsWith
                    | StrOp::Find
                    | StrOp::Decode
                    | StrOp::Next
                    | StrOp::Replace
                    | StrOp::LowerAscii
                    | StrOp::UpperAscii
                    | StrOp::SliceChecked
                    | StrOp::Bytes
                    | StrOp::CopyOut
                    | StrOp::BetweenView,
                    operands,
                    _,
                ) if whole_only(&operands.iter().collect::<Vec<_>>()) => Ok(None),
                Rvalue::Slice(op, ..) if types.repr(body.place_ty(types, place)) == Ty::STRING && whole_only(&[op]) => {
                    Ok(None)
                }
                Rvalue::Binary(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b)
                    if whole_only(&[a, b]) =>
                {
                    Ok(None)
                }
                Rvalue::Concat(pieces) if pieces.len() > 1 => {
                    for piece in pieces {
                        match piece {
                            Piece::Taken(p) if held.has(p.local) => return Err(()),
                            Piece::Value(Operand::Copy(p), ty) if held.has(p.local) => {
                                if !p.path.is_empty() || types.repr(*ty) != Ty::STRING {
                                    return Err(());
                                }
                            }
                            Piece::Value(Operand::Copy(p), _)
                                if p.path.iter().any(
                                    |proj| matches!(proj, Proj::Index(i, _) | Proj::InBounds(i) if held.has(*i)),
                                ) =>
                            {
                                return Err(());
                            }
                            _ => {}
                        }
                    }
                    Ok(None)
                }
                Rvalue::MapOp(
                    crate::hir::MapOp::Contains | crate::hir::MapOp::Get | crate::hir::MapOp::Position,
                    operands,
                    _,
                ) if operands.len() == 2 => {
                    let mut map_used = Vec::new();
                    operand_uses(&operands[0], &mut map_used);
                    if map_used.iter().any(|&l| held.has(l)) || !whole_only(&[&operands[1]]) {
                        return Err(());
                    }
                    Ok(None)
                }
                Rvalue::Call(crate::hir::Callee::Std(callee), operands)
                    if view_reads_only(*callee) && whole_only(&operands.iter().collect::<Vec<_>>()) =>
                {
                    Ok(None)
                }
                Rvalue::Call(crate::hir::Callee::Fn(callee), operands) => {
                    let params = str_borrows.get(callee.0 as usize);
                    for (at, op) in operands.iter().enumerate() {
                        let mut op_used = Vec::new();
                        operand_uses(op, &mut op_used);
                        if !op_used.iter().any(|&l| held.has(l)) {
                            continue;
                        }
                        let whole = matches!(op, Operand::Copy(p) if p.path.is_empty() && held.has(p.local));
                        if !whole || !params.and_then(|p| p.get(at)).copied().unwrap_or(false) {
                            return Err(());
                        }
                    }
                    Ok(None)
                }
                _ => Err(()),
            }
        }
        _ => {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(stmt, &mut used, &mut defined);
            if used.iter().chain(&defined).any(|&l| held.has(l)) { Err(()) } else { Ok(None) }
        }
    }
}

/// For each function, which of its non-loan, non-`takes` `String` parameters
/// it only borrows (never retains, stores, or returns), so callers may pass a
/// `StrFrameView` to them safely.
fn borrowing_strings(bodies: &[Body], types: &Types) -> Vec<Vec<bool>> {
    let mut borrows: Vec<Vec<bool>> = bodies
        .iter()
        .map(|body| {
            body.params
                .iter()
                .enumerate()
                .map(|(at, &param)| {
                    body.osr.is_none()
                        && types.repr(body.locals[param.0 as usize]) == Ty::STRING
                        && !body.loans.get(at).copied().unwrap_or(false)
                        && !body.takes.get(at).copied().unwrap_or(false)
                })
                .collect()
        })
        .collect();
    let reachable: Vec<Vec<bool>> = bodies.iter().map(|body| reachable_rpo(body).1).collect();
    let addressed: Vec<Vec<bool>> =
        bodies.iter().zip(&reachable).map(|(body, reach)| addressed_locals(body, reach)).collect();
    loop {
        let mut changed = false;
        for (index, body) in bodies.iter().enumerate() {
            for (at, &param) in body.params.iter().enumerate() {
                if borrows[index][at]
                    && !only_borrowed_string(body, param, types, &reachable[index], &addressed[index], &borrows)
                {
                    borrows[index][at] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            return borrows;
        }
    }
}

/// Whether `body` only borrows `param` (a `String` parameter), tracking whole-
/// local copies through reachable blocks.
fn only_borrowed_string(
    body: &Body,
    param: Local,
    types: &Types,
    reachable: &[bool],
    addressed: &[bool],
    str_borrows: &[Vec<bool>],
) -> bool {
    if addressed[param.0 as usize] || body.ret == Some(param) {
        return false;
    }
    let count = body.locals.len();
    let mut held = BitSet::empty(count);
    held.add(param);
    // Monotone flow-insensitive closure of locals that may receive `param` via
    // whole-local copies.
    loop {
        let mut grew = false;
        for (b, data) in body.blocks.iter().enumerate() {
            if !reachable[b] {
                continue;
            }
            for stmt in &data.stmts {
                match check_view_statement(body, types, stmt, &held, addressed, str_borrows) {
                    Ok(Some(to)) if !held.has(to) => {
                        held.add(to);
                        grew = true;
                    }
                    Ok(_) => {}
                    Err(()) => return false,
                }
            }
            let mut term_used = Vec::new();
            terminator_uses(&data.terminator, &mut term_used);
            if term_used.iter().any(|&l| held.has(l)) {
                return false;
            }
        }
        if !grew {
            return true;
        }
    }
}

/// Backward liveness step for one statement (updates `live` from after `stmt`
/// to before `stmt`). When `release_reads` is `None`, every `Release` counts
/// as a use (slot liveness: the stack header must remain intact until
/// released). When `release_reads` is `Some(named)`, only releases of named
/// locals count as uses (data liveness: unnamed temporaries' backing bytes are
/// never dereferenced by `tondo_rt_str_release`, and dumps never inspect them).
fn liveness_step(stmt: &Statement, live: &mut BitSet, release_reads: Option<&[bool]>) {
    match stmt {
        Statement::Assign(place, rvalue, _) => {
            if place.path.is_empty() {
                live.remove(place.local);
            }
            if let Rvalue::Select(cases, ..) = &**rvalue {
                for case in cases {
                    live.remove(case.out);
                }
            }
            if !place.path.is_empty() {
                live.add(place.local);
                for proj in &place.path {
                    if let Proj::Index(i, _) | Proj::InBounds(i) = proj {
                        live.add(*i);
                    }
                }
            }
            let mut used = Vec::new();
            rvalue_uses(rvalue, &mut used);
            if let Rvalue::Select(cases, ..) = &**rvalue {
                used.retain(|l| !cases.iter().any(|c| c.out == *l));
            }
            for local in used {
                live.add(local);
            }
        }
        Statement::Clear(local) => live.remove(*local),
        Statement::Moved(place) => {
            if place.path.is_empty() {
                live.remove(place.local);
            } else {
                live.add(place.local);
                for proj in &place.path {
                    if let Proj::Index(i, _) | Proj::InBounds(i) = proj {
                        live.add(*i);
                    }
                }
            }
        }
        Statement::Release(place) => {
            if place.path.is_empty() {
                live.remove(place.local);
                if release_reads.is_none_or(|named| named[place.local.0 as usize]) {
                    live.add(place.local);
                }
            } else {
                live.add(place.local);
                for proj in &place.path {
                    if let Proj::Index(i, _) | Proj::InBounds(i) = proj {
                        live.add(*i);
                    }
                }
            }
        }
        Statement::Retain(place) | Statement::DeferPush(_, place) => {
            live.add(place.local);
            for proj in &place.path {
                if let Proj::Index(i, _) | Proj::InBounds(i) = proj {
                    live.add(*i);
                }
            }
        }
        Statement::Lend(alias, place) => {
            live.remove(*alias);
            live.add(place.local);
            for proj in &place.path {
                if let Proj::Index(i, _) | Proj::InBounds(i) = proj {
                    live.add(*i);
                }
            }
        }
        Statement::LendAt(alias, operand) => {
            live.remove(*alias);
            let mut used = Vec::new();
            operand_uses(operand, &mut used);
            for local in used {
                live.add(local);
            }
        }
        Statement::Failing(operand, ..) => {
            let mut used = Vec::new();
            operand_uses(operand, &mut used);
            for local in used {
                live.add(local);
            }
        }
        Statement::DeferPop | Statement::OsrLoad(_) => {}
    }
}

/// Computes backward liveness (`live_out` per block) over reachable blocks.
fn compute_liveness(body: &Body, reachable: &[bool], release_reads: Option<&[bool]>) -> (Vec<BitSet>, Vec<BitSet>) {
    let n = body.blocks.len();
    let count = body.locals.len();
    let mut live_in = vec![BitSet::empty(count); n];
    let mut live_out = vec![BitSet::empty(count); n];
    loop {
        let mut changed = false;
        for b in (0..n).rev() {
            if !reachable[b] {
                continue;
            }
            let mut out = BitSet::empty(count);
            for to in crate::hoist::targets(&body.blocks[b].terminator) {
                out.join(&live_in[to]);
            }
            let mut term_used = Vec::new();
            terminator_uses(&body.blocks[b].terminator, &mut term_used);
            for local in term_used {
                out.add(local);
            }
            live_out[b] = out.clone();
            let mut cur = out;
            for stmt in body.blocks[b].stmts.iter().rev() {
                liveness_step(stmt, &mut cur, release_reads);
            }
            if cur != live_in[b] {
                live_in[b] = cur;
                changed = true;
            }
        }
        if !changed {
            return (live_in, live_out);
        }
    }
}

/// Whether `stmt` may free, overwrite, move, or mutate any local in `owners`.
fn invalidates_owner(
    body: &Body,
    types: &Types,
    stmt: &Statement,
    owners: &BitSet,
    fn_loans: &[Vec<bool>],
    fn_takes: &[Vec<bool>],
) -> bool {
    if owners.is_empty() {
        return false;
    }
    let is_string = |l: Local| types.repr(body.locals[l.0 as usize]) == Ty::STRING;
    match stmt {
        Statement::Assign(place, rvalue, _) => {
            if owners.has(place.local) {
                return true;
            }
            match &**rvalue {
                Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => owners.has(target.local),
                Rvalue::Concat(pieces) => pieces.iter().any(|p| matches!(p, Piece::Taken(at) if owners.has(at.local))),
                Rvalue::StrOp(StrOp::CopyOut, args, _) => {
                    let mut used = Vec::new();
                    args.iter().for_each(|a| operand_uses(a, &mut used));
                    used.iter().any(|&l| owners.has(l))
                }
                Rvalue::Call(crate::hir::Callee::Fn(callee), args) => args.iter().enumerate().any(|(at, arg)| {
                    let takes = fn_takes.get(callee.0 as usize).and_then(|t| t.get(at)).copied().unwrap_or(false);
                    let loans = fn_loans.get(callee.0 as usize).and_then(|l| l.get(at)).copied().unwrap_or(false);
                    if !(takes || loans) {
                        return false;
                    }
                    let mut used = Vec::new();
                    operand_uses(arg, &mut used);
                    used.iter().any(|&l| owners.has(l))
                }),
                Rvalue::Call(crate::hir::Callee::Std(callee), args) => {
                    if reads_only(*callee) {
                        return false;
                    }
                    let mut used = Vec::new();
                    args.iter().for_each(|a| operand_uses(a, &mut used));
                    used.iter().any(|&l| owners.has(l) && !is_string(l))
                }
                Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) => {
                    let mut used = Vec::new();
                    args.iter().for_each(|a| operand_uses(a, &mut used));
                    used.iter().any(|&l| owners.has(l) && !is_string(l))
                }
                Rvalue::Spawn(..) | Rvalue::TaskOp(..) => {
                    let mut used = Vec::new();
                    rvalue_uses(rvalue, &mut used);
                    used.iter().any(|&l| owners.has(l))
                }
                _ => false,
            }
        }
        Statement::Release(place) | Statement::Moved(place) | Statement::Lend(_, place) => owners.has(place.local),
        Statement::Clear(local) => owners.has(*local),
        Statement::OsrLoad(_) => true,
        Statement::Retain(_)
        | Statement::LendAt(..)
        | Statement::DeferPush(..)
        | Statement::DeferPop
        | Statement::Failing(..) => false,
    }
}

/// Converts non-escaping `textNative.betweenWithin(source, start, end)` calls
/// into zero-copy frame-lent `StrOp::BetweenView` when static analysis proves
/// the view never escapes, `source` (and its backing owners) stays alive and
/// unmutated for the view's entire live range, and the site's 32-byte slot is
/// never overwritten while a previous view from it is still live.
fn between_views(bodies: &mut [Body], types: &Types) {
    let fn_loans: Vec<Vec<bool>> = bodies.iter().map(|b| b.loans.clone()).collect();
    let fn_takes: Vec<Vec<bool>> = bodies.iter().map(|b| b.takes.clone()).collect();
    let str_borrows = borrowing_strings(bodies, types);
    for (index, body) in bodies.iter_mut().enumerate() {
        between_views_in(body, index, types, &fn_loans, &fn_takes, &str_borrows);
    }
}

fn between_views_in(
    body: &mut Body,
    index: usize,
    types: &Types,
    fn_loans: &[Vec<bool>],
    fn_takes: &[Vec<bool>],
    str_borrows: &[Vec<bool>],
) {
    if body.osr.is_some() || body.blocks.is_empty() {
        return;
    }
    let (rpo, reachable) = reachable_rpo(body);
    let mut candidates = Vec::new();
    for &b in &rpo {
        for (at, stmt) in body.blocks[b].stmts.iter().enumerate() {
            if let Statement::Assign(place, rvalue, _) = stmt {
                if let Rvalue::Call(crate::hir::Callee::Fn(callee), _) = &**rvalue
                    && callee.0 as usize == index
                {
                    return;
                }
                if place.path.is_empty() && between_within_source(rvalue).is_some() {
                    candidates.push((b, at));
                }
            }
        }
    }
    if candidates.is_empty() {
        return;
    }

    let count = body.locals.len();
    let addressed = addressed_locals(body, &reachable);
    // Backing-owner edges: `dep[dst]` lists locals whose buffer `dst` borrows
    // without owning its own reference count (or from a framed `Concat`).
    let mut dep: Vec<Vec<Local>> = vec![Vec::new(); count];
    for &b in &rpo {
        let stmts = &body.blocks[b].stmts;
        for (at, stmt) in stmts.iter().enumerate() {
            let Statement::Assign(place, rvalue, _) = stmt else { continue };
            if !place.path.is_empty() {
                continue;
            }
            if let Rvalue::Use(Operand::Copy(src)) = &**rvalue {
                let retained = matches!(
                    stmts.get(at + 1),
                    Some(Statement::Retain(p)) if p.path.is_empty() && p.local == place.local
                ) && !body.framed.contains(&src.local);
                if !retained {
                    dep[place.local.0 as usize].push(src.local);
                }
            }
        }
    }

    let mut named = vec![false; count];
    for (i, name) in body.names.iter().enumerate() {
        named[i] |= name.is_some();
    }
    for &(local, ..) in &body.scoped {
        named[local.0 as usize] = true;
    }

    let (_, slot_live_out) = compute_liveness(body, &reachable, None);
    let (_, data_live_out) = compute_liveness(body, &reachable, Some(&named));
    // Precompute `live_after[b][at]` (slot liveness) and `data_live_before[b][at]`
    // (content liveness) once for the function.
    let mut live_after: Vec<Vec<BitSet>> = vec![Vec::new(); body.blocks.len()];
    let mut data_live_before: Vec<Vec<BitSet>> = vec![Vec::new(); body.blocks.len()];
    for &b in &rpo {
        let len = body.blocks[b].stmts.len();
        let mut after = vec![BitSet::empty(count); len];
        let mut cur = slot_live_out[b].clone();
        for at in (0..len).rev() {
            after[at] = cur.clone();
            liveness_step(&body.blocks[b].stmts[at], &mut cur, None);
        }
        live_after[b] = after;

        let mut before = vec![BitSet::empty(count); len];
        let mut dcur = data_live_out[b].clone();
        for at in (0..len).rev() {
            liveness_step(&body.blocks[b].stmts[at], &mut dcur, Some(&named));
            before[at] = dcur.clone();
        }
        data_live_before[b] = before;
    }

    let mut converted = 0usize;
    for (def_block, def_stmt) in candidates {
        if converted == FRAMED_VIEWS {
            break;
        }
        let Statement::Assign(place, rvalue, span) = &body.blocks[def_block].stmts[def_stmt] else {
            continue;
        };
        let (dst0, span) = (place.local, *span);
        if addressed[dst0.0 as usize] || body.ret == Some(dst0) {
            continue;
        }
        let Some(src_op) = between_within_source(rvalue).cloned() else { continue };

        // Compute transitive backing owners of `src_op`.
        let mut owners = BitSet::empty(count);
        let src_local = match &src_op {
            Operand::Const(Const::Str(_)) => None,
            Operand::Copy(p) if p.path.is_empty() => {
                let mut stack = vec![p.local];
                owners.add(p.local);
                while let Some(cur) = stack.pop() {
                    for &parent in &dep[cur.0 as usize] {
                        if !owners.has(parent) {
                            owners.add(parent);
                            stack.push(parent);
                        }
                    }
                }
                Some(p.local)
            }
            _ => continue,
        };
        if owners.iter().any(|o| addressed[o.0 as usize]) {
            continue;
        }

        if !can_be_between_view(
            body,
            types,
            def_block,
            def_stmt,
            dst0,
            &owners,
            &addressed,
            &live_after,
            &data_live_before,
            fn_loans,
            fn_takes,
            str_borrows,
        ) {
            continue;
        }

        let Statement::Assign(_, rvalue, _) = &mut body.blocks[def_block].stmts[def_stmt] else { unreachable!() };
        let Rvalue::Call(_, args) = &mut **rvalue else { unreachable!() };
        let args = std::mem::take(args);
        **rvalue = Rvalue::StrOp(StrOp::BetweenView, args, span);
        if let Some(src) = src_local {
            dep[dst0.0 as usize].push(src);
        }
        converted += 1;

        let scope = body.scopes.get(def_block).copied().flatten();
        crate::mir_text::note(body, "stack", || {
            format!("b{def_block}[{def_stmt}]: l{} is a view in the frame, without copying", dst0.0)
        });
        crate::mir_text::remark_by(body, span, "frame", None, true, scope, || {
            "is a view in the frame, without copying".to_string()
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn can_be_between_view(
    body: &Body,
    types: &Types,
    def_block: usize,
    def_stmt: usize,
    dst0: Local,
    owners: &BitSet,
    addressed: &[bool],
    live_after: &[Vec<BitSet>],
    data_live_before: &[Vec<BitSet>],
    fn_loans: &[Vec<bool>],
    fn_takes: &[Vec<bool>],
    str_borrows: &[Vec<bool>],
) -> bool {
    let count = body.locals.len();
    let mut view_in = vec![BitSet::empty(count); body.blocks.len()];
    let mut worklist: Vec<usize> = Vec::new();

    let step = |b: usize, at: usize, held: &mut BitSet| -> bool {
        let stmt = &body.blocks[b].stmts[at];
        if held.intersects(&data_live_before[b][at]) && invalidates_owner(body, types, stmt, owners, fn_loans, fn_takes)
        {
            return false;
        }
        let Ok(copied) = check_view_statement(body, types, stmt, held, addressed, str_borrows) else {
            return false;
        };
        if let Some(to) = copied {
            held.add(to);
        } else if let Statement::Assign(place, ..) = stmt
            && place.path.is_empty()
        {
            held.remove(place.local);
        }
        held.meet(&live_after[b][at]);
        true
    };

    let finish_block = |b: usize, held: &BitSet, view_in: &mut [BitSet], worklist: &mut Vec<usize>| -> bool {
        if held.is_empty() {
            return true;
        }
        let mut term_used = Vec::new();
        terminator_uses(&body.blocks[b].terminator, &mut term_used);
        if term_used.iter().any(|&l| held.has(l)) {
            return false;
        }
        for to in crate::hoist::targets(&body.blocks[b].terminator) {
            if view_in[to].join(held) {
                worklist.push(to);
            }
        }
        true
    };

    // Seed right after `(def_block, def_stmt)`.
    let mut held = BitSet::empty(count);
    held.add(dst0);
    held.meet(&live_after[def_block][def_stmt]);
    for at in (def_stmt + 1)..body.blocks[def_block].stmts.len() {
        if held.is_empty() {
            break;
        }
        if !step(def_block, at, &mut held) {
            return false;
        }
    }
    if !finish_block(def_block, &held, &mut view_in, &mut worklist) {
        return false;
    }

    while let Some(b) = worklist.pop() {
        let mut held = view_in[b].clone();
        let mut stopped_at_def = false;
        for at in 0..body.blocks[b].stmts.len() {
            if b == def_block && at == def_stmt {
                if !held.is_empty() {
                    return false;
                }
                stopped_at_def = true;
                break;
            }
            if held.is_empty() {
                break;
            }
            if !step(b, at, &mut held) {
                return false;
            }
        }
        if !stopped_at_def && !finish_block(b, &held, &mut view_in, &mut worklist) {
            return false;
        }
    }
    true
}
