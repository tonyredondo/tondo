//! Where each value a function makes on the heap would go if the program
//! had regions: memory a frame takes by moving a pointer and gives back
//! whole when it ends, without letting go of each value. A value would go
//! in the region of the function that makes it when nothing keeps it past
//! the call; in one its caller gives when the function only returns it,
//! fresh, and the callers that take it keep it nowhere either; and it
//! stays on the heap otherwise, for the reason said. Nothing is built from
//! this yet: `tondo optimizations --kind region` says it, so that what
//! regions would reach is known before they are made.
//!
//! What may keep a value is what `stack` and `lends` say keeps one: being
//! stored in what the function was given, captured by a closure, sent,
//! given to a task, or passed to a call that keeps it; a call of the
//! program keeps a parameter when its own code does, which is found from
//! the callees up, and the runtime or C may keep anything.

use crate::hir::Callee;
use crate::mir::*;
use crate::source::Span;
use crate::types::Types;

static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asks for the verdicts, or no longer: `tondo optimizations --kind
/// region`, the only one that says them, as they cost a build time.
pub fn ask(asked: bool) {
    ASKED.store(asked, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the verdicts are asked for.
pub fn asked() -> bool {
    ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

/// What becomes of a value a function made.
#[derive(Clone, Debug, PartialEq)]
enum Fate {
    /// It is let go in the function.
    Here,
    /// The function returns it, and keeps it nowhere else.
    Returned,
    /// Something may keep it past the call: why, and where.
    Kept(String, Span),
}

/// What a function does with its parameters and its result.
#[derive(Clone)]
struct Summary {
    /// Whether it may keep each parameter past the call, or return it.
    keeps: Vec<bool>,
    /// Whether what it returns is only what it, or the fresh results of
    /// its calls, made: a value no one else holds.
    fresh: bool,
    /// Whether every call of it keeps its result nowhere, in the caller
    /// or, when the caller returns it, in the callers of that one.
    dropped: bool,
    /// When not, who keeps it and why, to say after "returns it, and ",
    /// and where.
    kept_by: Option<(String, Span)>,
}

/// One verdict, for the remark and for counting.
pub struct Verdict {
    pub function: String,
    pub span: Span,
    pub region: bool,
    pub text: String,
    /// The code that keeps the value, which the remark names after it.
    pub by: Option<Span>,
}

/// The verdicts for every place that makes a value on the heap, and, while
/// remarks are asked for, those remarks.
pub fn regions(bodies: &mut [Body], types: &Types) -> Vec<Verdict> {
    let summaries = summaries(bodies, types);
    let mut verdicts = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        for (local, span, what) in sites(body, types) {
            let name = &body.name;
            let (region, text, by) = match fate(body, types, local, &summaries, bodies) {
                Fate::Here => (true, format!("{what}would go in a region of `{name}`"), None),
                Fate::Returned if summaries[index].fresh && summaries[index].dropped => {
                    (true, format!("{what}would go in a region that the caller of `{name}` gives"), None)
                }
                Fate::Returned => match &summaries[index].kept_by {
                    Some((who, at)) => (
                        false,
                        format!("{what}stays on the heap: `{name}` returns it, and {who}"),
                        Some(*at).filter(|at| *at != Span::default()),
                    ),
                    None => {
                        (false, format!("{what}stays on the heap: `{name}` returns it with what it was given"), None)
                    }
                },
                Fate::Kept(why, at) => {
                    (false, format!("{what}stays on the heap: {why}"), Some(at).filter(|at| *at != Span::default()))
                }
            };
            let by = by.filter(|by| *by != span);
            verdicts.push(Verdict { function: body.name.clone(), span, region, text, by });
        }
    }
    if crate::mir_text::remarking() {
        for verdict in &verdicts {
            let body = bodies.iter_mut().find(|body| body.name == verdict.function);
            if let Some(body) = body {
                let text = verdict.text.clone();
                crate::mir_text::remark_by(body, verdict.span, "region", verdict.by, || text);
            }
        }
    }
    verdicts
}

/// The locals the function makes a value on the heap in, where, and what
/// it is in words: an array of values that are not all constants, a map,
/// a slice, a box, an array that grows, a text, and the arrays, texts and
/// other values the runtime gives.
fn sites(body: &Body, types: &Types) -> Vec<(Local, Span, &'static str)> {
    let mut found = Vec::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let Statement::Assign(place, rvalue, span) = statement else { continue };
        let what = match &**rvalue {
            Rvalue::Array(values, _) if values.iter().any(|value| matches!(value, Operand::Copy(_))) => "",
            Rvalue::MapOp(crate::hir::MapOp::New, ..) => "",
            Rvalue::Slice(..) => "",
            Rvalue::Concat(_) if !body.framed.contains(&place.local) => "",
            Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) => {
                let ty = body.place_ty(types, place);
                let boxed = operands.iter().any(|operand| match operand {
                    Operand::Copy(part) => types.is_boxed(ty, body.place_ty(types, part)),
                    Operand::Const(_) => false,
                });
                if !boxed {
                    continue;
                }
                ""
            }
            Rvalue::Mutate(
                crate::hir::Mutation::Push | crate::hir::Mutation::PushFront | crate::hir::Mutation::InsertAt,
                target,
                ..,
            ) => {
                found.push((target.local, *span, "grows an array that "));
                continue;
            }
            Rvalue::Mutate(crate::hir::Mutation::Insert, target, ..) => {
                found.push((target.local, *span, "grows a map that "));
                continue;
            }
            Rvalue::Call(Callee::Std(_), _) if types.needs_drop(body.place_ty(types, place)) => "",
            // An array or a text the runtime makes, as `join` or `concat`
            // does; not an element read out of one.
            Rvalue::ArrayOp(op, ..)
                if *op != crate::hir::ArrayOp::Get && types.needs_drop(body.place_ty(types, place)) =>
            {
                ""
            }
            // A text an operation makes, as `replace` or `toUpper` does.
            Rvalue::StrOp(..)
                if matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::String) =>
            {
                ""
            }
            _ => continue,
        };
        if place.path.is_empty() {
            found.push((place.local, *span, what));
        }
    }
    found
}

/// The locals that hold what `start` holds: that are it, a copy of it, or
/// that contain it, as an array it was pushed into or a record made of it,
/// as copies, parts or values made of it pass it on. An element read out of
/// the value itself is another value, which it does not hold; one read out
/// of what contains it may be it. What another operation gives, as the
/// runtime's `trim` that may give back the text it was lent, may be it when
/// it is of its type, and contain it when its type can refer to it.
fn holders(body: &Body, types: &Types, start: Local) -> Vec<bool> {
    let count = body.locals.len();
    let (mut is, mut contains) = (vec![false; count], vec![false; count]);
    is[start.0 as usize] = true;
    let counted = |local: Local| types.needs_drop(body.locals[local.0 as usize]);
    let held = types.repr(body.locals[start.0 as usize]);
    // Whether a value of a type may be it, and whether it may refer to it.
    let kin = |ty: crate::types::Ty| {
        (types.repr(ty) == held, types.reach(ty).into_iter().skip(1).any(|part| types.repr(part) == held))
    };
    let element =
        |place: &Place| place.path.iter().any(|projection| matches!(projection, Proj::Index(..) | Proj::InBounds(_)));
    let mut changed = true;
    while changed {
        changed = false;
        let mut mark = |set: &mut Vec<bool>, local: Local| {
            if !set[local.0 as usize] && counted(local) {
                set[local.0 as usize] = true;
                changed = true;
            }
        };
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    let into = place.local;
                    // What the value written may be or contain.
                    let (mut may_be, mut may_contain) = (false, false);
                    let mut read = |operand: &Operand, whole_value: bool| {
                        let Operand::Copy(from) = operand else { return };
                        let (it, inside) = (is[from.local.0 as usize], contains[from.local.0 as usize]);
                        if from.path.is_empty() {
                            may_be |= it && whole_value;
                            may_contain |= inside || (it && !whole_value);
                        } else if element(from) || !from.path.is_empty() {
                            // A part: of the value itself, another value; of
                            // what contains it, maybe the value.
                            may_be |= inside && whole_value;
                            may_contain |= inside;
                        }
                    };
                    match &**rvalue {
                        Rvalue::Use(operand) => read(operand, true),
                        Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _) => {
                            operands.iter().for_each(|operand| read(operand, false));
                        }
                        Rvalue::Concat(pieces) => {
                            for piece in pieces {
                                if let Piece::Taken(taken) = piece {
                                    read(&Operand::Copy(taken.clone()), true);
                                }
                            }
                        }
                        Rvalue::Mutate(_, target, operands, ..) => {
                            let (mut it, mut inside) = (false, false);
                            for operand in operands {
                                if let Operand::Copy(from) = operand {
                                    it |= is[from.local.0 as usize];
                                    inside |= contains[from.local.0 as usize];
                                }
                            }
                            if it || inside {
                                mark(&mut contains, target.local);
                            }
                        }
                        // A call of the program's keeps what it returns of
                        // what it is given, which `fate` sees.
                        Rvalue::Call(Callee::Fn(_), _) => {}
                        _ => {
                            let mut used = Vec::new();
                            rvalue_uses(rvalue, &mut used);
                            if used.iter().any(|&local| is[local.0 as usize] || contains[local.0 as usize]) {
                                let (same, refers) = kin(body.place_ty(types, place));
                                may_be |= same;
                                may_contain |= refers;
                            }
                        }
                    }
                    if !place.path.is_empty() {
                        // Stored in a part of `into`, which then contains it.
                        if may_be || may_contain {
                            mark(&mut contains, into);
                        }
                        continue;
                    }
                    if may_be {
                        mark(&mut is, into);
                    }
                    if may_contain {
                        mark(&mut contains, into);
                    }
                }
                Statement::Lend(alias, place) => {
                    if is[place.local.0 as usize] && place.path.is_empty() {
                        mark(&mut is, *alias);
                    }
                    if contains[place.local.0 as usize] || (is[place.local.0 as usize] && !place.path.is_empty()) {
                        mark(&mut contains, *alias);
                    }
                }
                _ => {}
            }
        }
    }
    (0..count).map(|index| is[index] || contains[index]).collect()
}

/// What becomes of what `start` holds in `body`.
fn fate(body: &Body, types: &Types, start: Local, summaries: &[Summary], bodies: &[Body]) -> Fate {
    let held = holders(body, types, start);
    let holds = |local: Local| held[local.0 as usize];
    let reads = |operand: &Operand| matches!(operand, Operand::Copy(place) if holds(place.local));
    let outside = outside(body);
    let given = |local: Local| outside[local.0 as usize] != Outside::No;
    let where_given = |local: Local| match outside[local.0 as usize] {
        Outside::Lock => "it is stored in a lock's state",
        _ => "it is stored in what the function was given",
    };
    // The closures `spawn` runs, which hold what is given to the task.
    let spawned: Vec<Local> = body
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .filter_map(|statement| match statement {
            Statement::Assign(_, rvalue, _) => match &**rvalue {
                Rvalue::Spawn(Operand::Copy(closure), _) => Some(closure.local),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let mut returned = body.ret.is_some_and(holds);
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let kept = |why: &str, span: Span| Fate::Kept(why.to_string(), span);
        match statement {
            Statement::Assign(place, rvalue, span) => {
                let mut used = Vec::new();
                rvalue_uses(rvalue, &mut used);
                if !used.iter().any(|&local| holds(local)) {
                    continue;
                }
                if given(place.local) {
                    return kept(where_given(place.local), *span);
                }
                if place.path.iter().any(|projection| matches!(projection, Proj::Deref)) {
                    return kept("it is stored in a `Ref`", *span);
                }
                match &**rvalue {
                    Rvalue::Call(Callee::Fn(id), args) => {
                        let summary = &summaries[id.0 as usize];
                        for (at, arg) in args.iter().enumerate() {
                            if reads(arg) && summary.keeps.get(at).copied().unwrap_or(true) {
                                let name = &bodies[id.0 as usize].name;
                                return Fate::Kept(format!("it is passed to `{name}`, which keeps it"), *span);
                            }
                        }
                    }
                    Rvalue::Call(Callee::Std(std), args) if args.iter().any(reads) && runtime_keeps(std) => {
                        return kept("it is passed to the runtime, which may keep it", *span);
                    }
                    Rvalue::Call(Callee::Std(_), _) => {}
                    Rvalue::Call(..) => return kept("it is passed to C, which may keep it", *span),
                    Rvalue::CallIndirect(..) => {
                        return kept("it is passed to a function value, which may keep it", *span);
                    }
                    Rvalue::Erase(..) if spawned.contains(&place.local) => return kept("a task takes it", *span),
                    Rvalue::Erase(..) => return kept("a closure captures it", *span),
                    Rvalue::Spawn(..) => return kept("a task takes it", *span),
                    Rvalue::TaskOp(op, ..) => {
                        let why = match op {
                            crate::hir::TaskOp::SyncNew => "it is stored in a lock's state",
                            crate::hir::TaskOp::RefNew | crate::hir::TaskOp::WeakNew => "it is stored in a `Ref`",
                            crate::hir::TaskOp::Send | crate::hir::TaskOp::Offer | crate::hir::TaskOp::TryOffer => {
                                "it is sent"
                            }
                            _ => "a task's operation takes it",
                        };
                        return kept(why, *span);
                    }
                    Rvalue::Select(..) => return kept("it is sent", *span),
                    Rvalue::Mutate(_, target, ..) if given(target.local) => {
                        return kept(where_given(target.local), *span);
                    }
                    _ => {}
                }
                if body.ret == Some(place.local) {
                    returned = true;
                }
            }
            Statement::DeferPush(_, place) if holds(place.local) => return kept("a `defer` keeps it", Span::default()),
            _ => {}
        }
    }
    for block in &body.blocks {
        if let Terminator::Return(operand) = &block.terminator
            && reads(operand)
        {
            returned = true;
        }
    }
    if returned { Fate::Returned } else { Fate::Here }
}

/// Each function's summary, from the callees up, until none changes: a
/// parameter is kept when its value may be; a result is fresh when only
/// what the function made reaches it; and it is dropped when no call
/// keeps it.
fn summaries(bodies: &[Body], types: &Types) -> Vec<Summary> {
    // Whether the program's code calls each function by its name, and
    // whether it calls it through a value too, as those calls are not seen.
    let (mut called, mut valued) = (vec![false; bodies.len()], vec![false; bodies.len()]);
    for statement in bodies.iter().flat_map(|body| &body.blocks).flat_map(|block| &block.stmts) {
        let (id, value) = match statement {
            Statement::Assign(_, rvalue, _) => match &**rvalue {
                Rvalue::Call(Callee::Fn(id), _) => (id, false),
                Rvalue::FnValue(id) | Rvalue::Callback(id, _) | Rvalue::Erase(_, id, _) | Rvalue::CallOsr(id, ..) => {
                    (id, true)
                }
                _ => continue,
            },
            Statement::DeferPush(id, _) => (id, true),
            _ => continue,
        };
        if let Some(seen) = if value { &mut valued } else { &mut called }.get_mut(id.0 as usize) {
            *seen = true;
        }
    }
    let mut summaries: Vec<Summary> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            let unseen = match (valued[index], called[index]) {
                (true, _) => Some("what calls it through a function value may keep it"),
                (false, false) => {
                    Some("no call of it is left in the program's code: each was inlined, or it is called from outside")
                }
                (false, true) => None,
            };
            Summary {
                keeps: vec![false; body.params.len()],
                fresh: body.ret.is_some(),
                dropped: unseen.is_none(),
                kept_by: unseen.map(|why| (why.to_string(), Span::default())),
            }
        })
        .collect();
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 16 {
        changed = false;
        rounds += 1;
        for (index, body) in bodies.iter().enumerate() {
            // Parameters, from what each holds.
            let keeps: Vec<bool> = body
                .params
                .iter()
                .map(|&param| {
                    types.needs_drop(body.locals[param.0 as usize])
                        && fate(body, types, param, &summaries, bodies) != Fate::Here
                })
                .collect();
            // The result: fresh when what reaches it is made here, or a
            // fresh result of a call.
            let fresh = body.ret.is_some_and(|ret| {
                let mut made = sites(body, types).into_iter().map(|(local, ..)| local).collect::<Vec<_>>();
                made.extend(fresh_calls(body, &summaries));
                let reached = made.iter().any(|&local| holders(body, types, local)[ret.0 as usize]);
                reached
                    && only_made(body, types, ret, &made)
                    && made.iter().all(|&local| !matches!(fate(body, types, local, &summaries, bodies), Fate::Kept(..)))
            });
            if keeps != summaries[index].keeps || fresh != summaries[index].fresh {
                summaries[index].keeps = keeps;
                summaries[index].fresh = fresh;
                changed = true;
            }
        }
        // Whether the callers keep each fresh result.
        for (index, body) in bodies.iter().enumerate() {
            for (local, callee) in call_results(body) {
                if !summaries[callee].dropped {
                    continue;
                }
                let kept_by = match fate(body, types, local, &summaries, bodies) {
                    Fate::Here => None,
                    Fate::Returned if summaries[index].fresh && summaries[index].dropped => None,
                    Fate::Returned => Some(match &summaries[index].kept_by {
                        Some(kept_by) => kept_by.clone(),
                        None => (format!("`{}` returns it with what it was given", body.name), Span::default()),
                    }),
                    Fate::Kept(why, at) => Some((format!("`{}` keeps it: {why}", body.name), at)),
                };
                if let Some(kept_by) = kept_by {
                    summaries[callee].dropped = false;
                    summaries[callee].kept_by = Some(kept_by);
                    changed = true;
                }
            }
        }
    }
    summaries
}

/// The locals that take the result of a call of the program's, with its
/// callee.
fn call_results(body: &Body) -> Vec<(Local, usize)> {
    let mut found = Vec::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && let Rvalue::Call(Callee::Fn(id), _) = &**rvalue
        {
            found.push((place.local, id.0 as usize));
        }
    }
    found
}

/// The locals that take a fresh result of a call.
fn fresh_calls(body: &Body, summaries: &[Summary]) -> Vec<Local> {
    call_results(body).into_iter().filter(|&(_, callee)| summaries[callee].fresh).map(|(local, _)| local).collect()
}

/// Whether every value written whole into `ret`, or into a local that
/// reaches it, comes from what the function made, or holds no count.
fn only_made(body: &Body, types: &Types, ret: Local, made: &[Local]) -> bool {
    let mut allowed = vec![false; body.locals.len()];
    for &local in made {
        for (index, held) in holders(body, types, local).into_iter().enumerate() {
            allowed[index] |= held;
        }
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let Statement::Assign(place, rvalue, _) = statement else { continue };
        if place.local != ret {
            continue;
        }
        let mut used = Vec::new();
        rvalue_uses(rvalue, &mut used);
        let counted = |local: Local| types.needs_drop(body.locals[local.0 as usize]);
        if used.iter().any(|&local| !allowed[local.0 as usize] && body.params.contains(&local) && counted(local)) {
            return false;
        }
    }
    true
}

/// Whether a function of the runtime may keep what it is given: one that
/// puts it somewhere, by its name, as inserting, sending or setting does.
/// The others read what they are lent, as the program's calls do.
fn runtime_keeps(std: &crate::hir::StdFn) -> bool {
    let crate::hir::StdFn::Native(index) = std else { return false };
    let Some(native) = crate::hir::NATIVES.get(*index as usize) else { return true };
    const PUTS: [&str; 12] =
        ["insert", "push", "send", "set", "store", "put", "spawn", "register", "append", "add", "enqueue", "offer"];
    let name = native.name.to_ascii_lowercase();
    PUTS.iter().any(|word| name.contains(word))
}

/// Whether a local is, or leads to, what outlives the call.
#[derive(Clone, Copy, PartialEq)]
enum Outside {
    No,
    /// A parameter, or what is lent out of one.
    Given,
    /// The state a lock's guard lends.
    Lock,
}

/// The locals that are, or lead to, what outlives the call: the parameters,
/// the state a lock's guard lends, and what is lent out of either.
fn outside(body: &Body) -> Vec<Outside> {
    let mut outside = vec![Outside::No; body.locals.len()];
    for &param in &body.params {
        outside[param.0 as usize] = Outside::Given;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            let (alias, kind) = match statement {
                Statement::LendAt(alias, _) => (*alias, Outside::Lock),
                Statement::Lend(alias, place) if outside[place.local.0 as usize] != Outside::No => {
                    (*alias, outside[place.local.0 as usize])
                }
                _ => continue,
            };
            if outside[alias.0 as usize] == Outside::No {
                outside[alias.0 as usize] = kind;
                changed = true;
            }
        }
    }
    outside
}
