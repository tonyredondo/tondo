//! Parameters a function takes rather than borrows. A call borrows its
//! arguments, so a function that copies a parameter into a local it then
//! changes, as `var next = store` before `next.values.remove(key)`, gave
//! the copy a count of its own: the value was shared, and the change copied
//! it whole, though neither the parameter nor the caller's value was read
//! again. Such a parameter is taken: every call gives it a reference, which
//! the moves pass makes a move when the caller has no use for its value
//! after the call and which is a retain otherwise, and the function
//! releases it where it returns, so that the copy is a move too and the
//! change is made in place.
//!
//! A parameter is taken when the function copies it whole into a local it
//! then changes, and reads it nowhere after the copy; and only in a
//! function that every caller calls directly, with the argument in a place
//! or a constant: not one called as a `fn` value, exported to C, called
//! back from C, or with versions for wider vectors, whose callers the pass
//! does not see, nor one that returns the parameter itself.

use crate::hir::Callee;
use crate::mir::*;
use crate::types::Types;

pub fn takes(bodies: &mut [Body], types: &Types, program: &crate::hir::Program) {
    let count = bodies.len();
    // The functions some code reaches other than by a direct call of the
    // program's: C, the runtime, the generated code's glue, a `fn` value.
    let mut reached: Vec<Option<Kept>> = vec![None; count];
    let mut reach = |id: crate::hir::FnId, why: Kept| reached[id.0 as usize] = Some(why);
    program.main.into_iter().for_each(|id| reach(id, Kept::Outside));
    program.exports.iter().for_each(|export| reach(export.function, Kept::Outside));
    program.display.values().for_each(|&id| reach(id, Kept::Outside));
    program.views.values().for_each(|&(view, _)| reach(view, Kept::Outside));
    for context in &program.contexts {
        [Some(context.maker), context.log, context.label].into_iter().flatten().for_each(|id| reach(id, Kept::Outside));
        context.computed.iter().for_each(|&(_, id, _)| reach(id, Kept::Outside));
    }
    for (&base, &clone) in &program.clones {
        reach(base, Kept::Versions);
        reach(clone, Kept::Versions);
    }
    for body in bodies.iter() {
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            let Statement::Assign(_, rvalue, _) = statement else { continue };
            match &**rvalue {
                Rvalue::FnValue(id) | Rvalue::Erase(_, id, _) | Rvalue::Callback(id, ..) => {
                    reached[id.0 as usize] = Some(Kept::Value)
                }
                _ => {}
            }
        }
    }
    // Of each parameter a function copies and changes, whether it takes
    // it, and why not.
    let mut taken: Vec<Vec<Option<Result<(), Kept>>>> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            (0..body.params.len())
                .map(|at| {
                    let copies = copies(body, types, at);
                    if copies.is_empty() {
                        return None;
                    }
                    if let Some(why) = reached[index] {
                        return Some(Err(why));
                    }
                    let param = body.params[at];
                    match copies.iter().any(|&(block, index, _)| !read_after(body, block, index + 2, param)) {
                        true => Some(Ok(())),
                        false => Some(Err(Kept::ReadAfter)),
                    }
                })
                .collect()
        })
        .collect();
    for body in bodies.iter() {
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            let Statement::Assign(_, rvalue, _) = statement else { continue };
            let Rvalue::Call(Callee::Fn(id), args) = &**rvalue else { continue };
            for (at, arg) in args.iter().enumerate() {
                if !matches!(arg, Operand::Copy(_) | Operand::Const(_))
                    && let Some(Some(state @ Ok(()))) = taken[id.0 as usize].get_mut(at)
                {
                    *state = Err(Kept::MadeInPlace);
                }
            }
        }
    }
    // Where a parameter is copied and changed but not taken, the change
    // copies what the caller holds: say so, and why.
    for (index, body) in bodies.iter_mut().enumerate() {
        for (at, state) in taken[index].iter().enumerate() {
            let Some(Err(why)) = state else { continue };
            let param = body.params[at];
            let name = body.names[param.0 as usize].clone().unwrap_or_else(|| "it".into());
            let function = body.name.clone();
            for (_, _, copy) in copies(body, types, at) {
                let Some((span, noun)) = first_change(body, types, copy) else { continue };
                let reason = match why {
                    Kept::Value => format!("as `{function}` is called as a value"),
                    Kept::Outside => format!("as `{function}` is called from outside the program's code"),
                    Kept::Versions => format!("as `{function}` has a version for wider vectors"),
                    Kept::ReadAfter => format!("as `{function}` reads `{name}` after copying it"),
                    Kept::MadeInPlace => format!("as a call gives `{function}` a value made in place for `{name}`"),
                };
                crate::mir_text::remark(body, span, "heap", || {
                    format!("copies the {noun}, which `{name}` holds too: a parameter its caller keeps, {reason}")
                });
            }
        }
    }
    let taken: Vec<Vec<bool>> =
        taken.iter().map(|states| states.iter().map(|state| matches!(state, Some(Ok(())))).collect()).collect();
    for (index, body) in bodies.iter_mut().enumerate() {
        let takes = &taken[index];
        if takes.contains(&true) {
            for (at, &param) in body.params.iter().enumerate() {
                if takes[at] {
                    for block in &mut body.blocks {
                        if matches!(block.terminator, Terminator::Return(_)) {
                            block.stmts.push(Statement::Release(Place::local(param)));
                        }
                    }
                }
            }
            body.takes = takes.clone();
        }
        give(body, types, &taken);
    }
}

/// Why a function does not take a parameter it copies and changes.
#[derive(Clone, Copy)]
enum Kept {
    /// It is called as a `fn` value, a closure or a callback of C.
    Value,
    /// Code outside the program's calls it: C, the runtime or the glue.
    Outside,
    /// It has a version for wider vectors.
    Versions,
    /// It reads the parameter after every copy it changes.
    ReadAfter,
    /// A call gives it a value made in place, not in a place.
    MadeInPlace,
}

/// Where the function copies its parameter `at` whole into a local that
/// is retained at once and changed after, by block and position, with the
/// copy: none for a parameter that counts nothing, a loan, or one it
/// returns as it is.
fn copies(body: &Body, types: &Types, at: usize) -> Vec<(usize, usize, Local)> {
    let param = body.params[at];
    if body.loans.get(at).copied().unwrap_or(false) || !types.needs_drop(body.locals[param.0 as usize]) {
        return Vec::new();
    }
    let returned = body
        .blocks
        .iter()
        .any(|block| matches!(&block.terminator, Terminator::Return(Operand::Copy(place)) if place.local == param));
    if returned {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (block, data) in body.blocks.iter().enumerate() {
        for (index, pair) in data.stmts.windows(2).enumerate() {
            let [Statement::Assign(copy, rvalue, _), Statement::Retain(retained)] = pair else { continue };
            let whole = |place: &Place| place.path.is_empty();
            if matches!(&**rvalue, Rvalue::Use(Operand::Copy(source)) if source.local == param && whole(source))
                && whole(copy)
                && whole(retained)
                && retained.local == copy.local
                && changed(body, copy.local)
            {
                found.push((block, index, copy.local));
            }
        }
    }
    found
}

/// The first change of a collection in `local` that may copy it, and what
/// the collection is.
fn first_change(body: &Body, types: &Types, local: Local) -> Option<(crate::source::Span, &'static str)> {
    body.blocks.iter().flat_map(|block| &block.stmts).find_map(|statement| {
        let Statement::Assign(_, rvalue, span) = statement else { return None };
        let (Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..)) = &**rvalue else { return None };
        if target.local != local {
            return None;
        }
        let noun = match types.kind(types.repr(body.place_ty(types, target))) {
            crate::types::TyKind::Array(_) => "array",
            crate::types::TyKind::Deque(_) => "deque",
            crate::types::TyKind::Map(..) => "map",
            crate::types::TyKind::Set(_) => "set",
            _ => return None,
        };
        Some((*span, noun))
    })
}

/// Whether the function changes `local`: a mutation or a slice written into
/// it, or a write into one of its parts.
fn changed(body: &Body, local: Local) -> bool {
    body.blocks.iter().flat_map(|block| &block.stmts).any(|statement| {
        let Statement::Assign(place, rvalue, _) = statement else { return false };
        match &**rvalue {
            Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) if target.local == local => true,
            _ => place.local == local && !place.path.is_empty(),
        }
    })
}

/// Whether any way on from statement `from` of `block` reads `local`.
fn read_after(body: &Body, block: usize, from: usize, local: Local) -> bool {
    let mut seen = vec![false; body.blocks.len()];
    let mut pending = vec![(block, from)];
    while let Some((block, from)) = pending.pop() {
        let data = &body.blocks[block];
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        for statement in &data.stmts[from.min(data.stmts.len())..] {
            statement_locals(statement, &mut used, &mut defined);
        }
        terminator_uses(&data.terminator, &mut used);
        if used.contains(&local) {
            return true;
        }
        for next in successors(&data.terminator) {
            if !std::mem::replace(&mut seen[next], true) {
                pending.push((next, 0));
            }
        }
    }
    false
}

/// Gives each argument a function takes a reference of its own, copied and
/// retained before the call, which the moves pass makes a move where the
/// caller lets its value go after the call.
fn give(body: &mut Body, types: &Types, taken: &[Vec<bool>]) {
    for block in 0..body.blocks.len() {
        let mut at = 0;
        while at < body.blocks[block].stmts.len() {
            let given: Vec<(usize, Place)> = match &body.blocks[block].stmts[at] {
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    Rvalue::Call(Callee::Fn(id), args) => args
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| taken[id.0 as usize].get(*index).copied().unwrap_or(false))
                        .filter_map(|(index, arg)| match arg {
                            Operand::Copy(place) => Some((index, place.clone())),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let mut before = Vec::new();
            for (index, place) in given {
                let ty = body.place_ty(types, &place);
                body.locals.push(ty);
                body.names.push(None);
                let copy = Local(body.locals.len() as u32 - 1);
                let span = match &body.blocks[block].stmts[at] {
                    Statement::Assign(_, _, span) => *span,
                    _ => crate::source::Span::default(),
                };
                before.push(Statement::Assign(Place::local(copy), Box::new(Rvalue::Use(Operand::Copy(place))), span));
                before.push(Statement::Retain(Place::local(copy)));
                if let Statement::Assign(_, rvalue, _) = &mut body.blocks[block].stmts[at]
                    && let Rvalue::Call(_, args) = &mut **rvalue
                {
                    args[index] = Operand::local(copy);
                }
            }
            let added = before.len();
            body.blocks[block].stmts.splice(at..at, before);
            at += added + 1;
        }
    }
}

/// Once the moves pass decided, drops the release of a taken parameter
/// that every way to it moved away, which would release nothing: the
/// local a function changes is most often the parameter moved into it.
pub fn settle(bodies: &mut [Body]) {
    for body in bodies.iter_mut() {
        for (at, &param) in body.params.iter().enumerate() {
            if !body.takes.get(at).copied().unwrap_or(false) {
                continue;
            }
            let after = |state: bool, statement: &Statement| match statement {
                Statement::Moved(place) if place.local == param && place.path.is_empty() => true,
                Statement::Clear(local) if *local == param => true,
                Statement::Assign(place, ..) if place.local == param => false,
                _ => state,
            };
            // Whether the parameter is moved at the start of each block on
            // every way there, from what each way leaves it.
            let mut moved: Vec<Option<bool>> = vec![None; body.blocks.len()];
            moved[0] = Some(false);
            let mut pending = vec![0usize];
            while let Some(block) = pending.pop() {
                let state = body.blocks[block].stmts.iter().fold(moved[block].unwrap_or(false), &after);
                for next in successors(&body.blocks[block].terminator) {
                    let met = moved[next].map_or(state, |before| before && state);
                    if moved[next] != Some(met) {
                        moved[next] = Some(met);
                        pending.push(next);
                    }
                }
            }
            for (block, data) in body.blocks.iter_mut().enumerate() {
                let Some(mut state) = moved[block] else { continue };
                data.stmts.retain(|statement| {
                    let released = matches!(statement, Statement::Release(place)
                        if place.local == param && place.path.is_empty());
                    state = after(state, statement);
                    !(released && state)
                });
            }
        }
    }
}

/// What `tondo optimizations` says at each call that gives a function an
/// argument it takes, once the moves pass decided: given without a count,
/// as nothing reads it after the call, or retained first, and where it is
/// read.
pub fn tell(bodies: &mut [Body], sources: &crate::source::SourceMap) {
    if !crate::mir_text::remarking() {
        return;
    }
    let takers: Vec<(String, Vec<Option<String>>)> = bodies
        .iter()
        .map(|body| {
            let names = body
                .params
                .iter()
                .zip(body.takes.iter().chain(std::iter::repeat(&false)))
                .map(|(param, &taken)| taken.then(|| body.names[param.0 as usize].clone().unwrap_or_default()))
                .collect();
            (body.name.clone(), names)
        })
        .collect();
    for body in bodies.iter_mut() {
        let mut told = Vec::new();
        for (block, data) in body.blocks.iter().enumerate() {
            for (at, statement) in data.stmts.iter().enumerate() {
                let Statement::Assign(_, rvalue, span) = statement else { continue };
                let Rvalue::Call(Callee::Fn(id), args) = &**rvalue else { continue };
                let (callee, names) = &takers[id.0 as usize];
                for (index, arg) in args.iter().enumerate() {
                    let Some(Some(param)) = names.get(index) else { continue };
                    let Operand::Copy(given) = arg else { continue };
                    // The copy made for the call, and what it was made of.
                    let made = data.stmts[..at].iter().rev().find_map(|statement| match statement {
                        Statement::Assign(target, value, _) if *target == *given => match &**value {
                            Rvalue::Use(Operand::Copy(source)) => Some(source.clone()),
                            _ => None,
                        },
                        _ => None,
                    });
                    let Some(source) = made else { continue };
                    let moved = data.stmts[..at]
                        .iter()
                        .any(|statement| matches!(statement, Statement::Moved(place) if *place == source))
                        || !data.stmts[..at]
                            .iter()
                            .any(|statement| matches!(statement, Statement::Retain(place) if place == given));
                    let text = match moved {
                        true => format!(
                            "gives `{param}` without a count: `{callee}` takes it, and nothing reads it after the call"
                        ),
                        false => {
                            let why = match owner(body, source.local) {
                                Some(owner) => owner.to_string(),
                                None => match read_at(body, block, at + 1, source.local) {
                                    Some(span) => {
                                        let (_, line, column) = sources.position(span.start);
                                        format!("the argument is read after the call, at {line}:{column}")
                                    }
                                    None => "the argument is kept after the call".into(),
                                },
                            };
                            format!("retains `{param}` first: `{callee}` takes it, and {why}")
                        }
                    };
                    told.push((*span, if moved { "given" } else { "retained" }, text));
                }
            }
        }
        for (span, kind, text) in told {
            crate::mir_text::remark(body, span, kind, || text);
        }
    }
}

/// Who holds a local this function may not give away, when it is not its
/// own: a parameter it borrows, or what a binding or a lock lends.
fn owner(body: &Body, local: Local) -> Option<&'static str> {
    if let Some(at) = body.params.iter().position(|&param| param == local)
        && !body.takes.get(at).copied().unwrap_or(false)
    {
        return Some("the argument is a parameter its caller keeps");
    }
    body.blocks.iter().flat_map(|block| &block.stmts).find_map(|statement| match statement {
        Statement::LendAt(alias, _) if *alias == local => Some("the argument is the state a lock keeps"),
        Statement::Lend(alias, place) if *alias == local || place.local == local => Some("the argument is lent"),
        _ => None,
    })
}

/// Where the first statement on from statement `from` of `block` that reads
/// `local` is, by its span: not the copy a reassignment of it makes of its
/// old value, `old = x; x = new`, which reads nothing of it.
fn read_at(body: &Body, block: usize, from: usize, local: Local) -> Option<crate::source::Span> {
    let mut seen = vec![false; body.blocks.len()];
    let mut pending = std::collections::VecDeque::from([(block, from)]);
    while let Some((block, from)) = pending.pop_front() {
        let data = &body.blocks[block];
        let statements = &data.stmts[from.min(data.stmts.len())..];
        let names = |statement: &Statement| {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            (used.contains(&local), defined.contains(&local))
        };
        for (index, statement) in statements.iter().enumerate() {
            let reassigned = statements[index + 1..]
                .iter()
                .find(|statement| names(statement) != (false, false))
                .is_some_and(|statement| names(statement) == (false, true));
            if let Statement::Assign(target, value, _) = statement
                && matches!(&**value, Rvalue::Use(Operand::Copy(source)) if *source == Place::local(local))
                && target.path.is_empty()
                && reassigned
            {
                continue;
            }
            if let Statement::Assign(_, _, span) = statement
                && names(statement).0
                && *span != crate::source::Span::default()
            {
                return Some(*span);
            }
        }
        for next in successors(&data.terminator) {
            if !std::mem::replace(&mut seen[next], true) {
                pending.push_back((next, 0));
            }
        }
    }
    None
}
