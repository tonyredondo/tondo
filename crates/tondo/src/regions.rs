//! Regions: memory a frame takes by moving a pointer and gives back whole
//! when it ends, without letting go of each value. A value goes in the
//! region of the function that makes it when nothing keeps it past the
//! call, or in that of a turn of the innermost loop it does not outlive; in
//! one its caller gives when the function only returns it, fresh, and the
//! callers that take it keep it nowhere either, through a copy of the
//! function that takes its caller's region, made for the calls that let the
//! result go, as copies are made for closures; and it stays on the heap
//! otherwise, for the reason said. A region takes arrays, boxes and arrays
//! that grow by `push`, and a value in a region holds only values of a
//! region or values that count nothing, as the region goes whole. `tondo
//! optimizations --kind region` says where each place's values go.
//!
//! What may keep a value is what `stack` and `lends` say keeps one: being
//! stored in what the function was given, captured by a closure, sent,
//! given to a task, or passed to a call that keeps it; a call of the
//! program keeps a parameter when its own code does, which is found from
//! the callees up, and the runtime or C may keep anything.

use crate::hir::Callee;
use crate::mir::*;
use crate::source::Span;
use crate::types::{Handle, Ty, TyKind, Types};

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
    /// Whether it may keep each parameter past the call: store it, send it
    /// or let a task or a closure take it.
    keeps: Vec<bool>,
    /// Whether it may return each parameter, and keep it no other way: what
    /// a call of it gives may be what it was given, as a sort gives back
    /// an array of fewer than two elements.
    returns: Vec<bool>,
    /// Whether what it returns is only what it, or the fresh results of
    /// its calls, made: a value no one else holds.
    fresh: bool,
    /// Its calls the program's code makes by its name, and what each does
    /// with the result.
    calls: Vec<Call>,
    /// Why calls of it that are not seen may keep its result: those
    /// through a function value, or from outside the program's code.
    unseen: Option<&'static str>,
}

/// A call of a function by its name.
#[derive(Clone)]
struct Call {
    caller: usize,
    /// The call's statement, by block and position.
    at: (u32, u32),
    span: Span,
    /// The call as the caller's own code writes it: the call that brought
    /// the code inlined there, when it is in such code.
    written: Span,
    taken: Taken,
}

/// What a call does with the result it takes.
#[derive(Clone)]
enum Taken {
    /// It lets it go.
    Drops,
    /// It returns it, fresh, and the calls of the caller decide.
    Passes,
    /// It keeps it: who and why, to say after "returns it, and ", and where.
    Keeps(String, Span),
}

/// One verdict, for the remark and for counting.
pub struct Verdict {
    pub function: String,
    pub span: Span,
    pub region: bool,
    /// Whether it is in a region only for the calls that let it go.
    pub by_call: bool,
    /// Whether the region is the function's own.
    pub own: bool,
    pub text: String,
    /// The code that keeps the value, which the remark names after it.
    pub by: Option<crate::mir::By>,
}

/// What a call of a function of the program does with its result, for
/// counting: the caller, the callee, the call, and `drops`, `passes` or
/// `keeps`.
pub struct Taking {
    pub caller: String,
    pub callee: String,
    pub span: Span,
    pub taken: &'static str,
}

/// The regions of every function and copy, and where each place's values
/// go: the verdicts, and what each call does with the result of its
/// callee; while remarks are asked for, the verdicts as remarks.
pub fn regions(
    bodies: &mut Vec<Body>,
    types: &Types,
    sources: &crate::source::SourceMap,
    roots: &[usize],
    main: Option<usize>,
    clones: &[(usize, usize)],
) -> (Vec<Verdict>, Vec<Taking>) {
    lasting(bodies, roots, main);
    // What each function's statements pass on, and its places, on every
    // core: each function is read on its own.
    let read = |bodies: &[Body]| {
        let flows: Vec<(Flow, Vec<Site>)> =
            crate::parallel::map(bodies, 16, |body| (Flow::of(body, types), sites(body, types)));
        let (mut flows, all_sites): (Vec<Flow>, Vec<Vec<Site>>) = flows.into_iter().unzip();
        let summaries = summaries(bodies, types, roots, &flows, &all_sites);
        // What a call that may give back what it was given gives holds it,
        // for every reading of the flows from here on.
        for (flow, body) in flows.iter_mut().zip(bodies.iter()) {
            flow.through(body, &summaries);
        }
        (flows, all_sites, summaries)
    };
    let (mut flows, mut all_sites, mut summaries) = read(bodies);
    // A function that copies out what it returns reads anew, and so do its
    // callers, whose calls of it now give a fresh text.
    if copied_out(bodies, types, roots, &flows, &all_sites, &summaries) {
        (flows, all_sites, summaries) = read(bodies);
    }
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    // Each function's places and their verdicts, on every core.
    let indices: Vec<usize> = (0..bodies.len()).collect();
    let all: &[Body] = bodies;
    let judged: Vec<Vec<Verdict>> = crate::parallel::map(&indices, 16, |&index| {
        let (body, bodies, sites) = (&all[index], all, &all_sites[index]);
        let mut verdicts = Vec::with_capacity(sites.len());
        for site in sites {
            let (local, span, what) = (site.local, site.span, site.what);
            let name = &body.name;
            let mut by_call = false;
            let fate = fate(&flows[index], body, types, local, &summaries, bodies);
            let own = fate == Fate::Here;
            let (region, text, by) = match fate {
                Fate::Here => (true, format!("{what}goes in a region of `{name}`"), None),
                Fate::Returned if !summaries[index].fresh => {
                    (false, format!("{what}stays on the heap: `{name}` returns it with what it was given"), None)
                }
                Fate::Returned => {
                    let (mut drops, mut keeps) = (Vec::new(), Vec::new());
                    let mut seen = vec![false; bodies.len()];
                    resolve(index, &summaries, &mut seen, &mut drops, &mut keeps);
                    match (drops.is_empty(), keeps.first()) {
                        (false, None) => {
                            (true, format!("{what}goes in a region that the caller of `{name}` gives"), None)
                        }
                        (false, Some((who, at))) => {
                            by_call = true;
                            let mut lets: Vec<String> = Vec::new();
                            for (caller, at) in &drops {
                                let (_, line, _) = sources.position(at.start);
                                let said = format!("`{}` at {line}", bodies[*caller].name);
                                if !lets.contains(&said) {
                                    lets.push(said);
                                }
                            }
                            let more = lets.len().saturating_sub(3);
                            lets.truncate(3);
                            if more > 0 {
                                lets.push(format!("{more} more"));
                            }
                            let text = format!(
                                "{what}goes in a region that the caller of `{name}` gives, in a copy of it for the \
                                 calls that let it go: {}; {who}",
                                lets.join(", ")
                            );
                            (true, text, Some(*at).filter(|at| *at != Span::default()))
                        }
                        (true, Some((who, at))) => (
                            false,
                            format!("{what}stays on the heap: `{name}` returns it, and {who}"),
                            Some(*at).filter(|at| *at != Span::default()),
                        ),
                        (true, None) => (
                            false,
                            format!("{what}stays on the heap: `{name}` returns it, and no call of it lets it go"),
                            None,
                        ),
                    }
                }
                Fate::Kept(why, at) => {
                    (false, format!("{what}stays on the heap: {why}"), Some(at).filter(|at| *at != Span::default()))
                }
            };
            // A fate names its code by place alone: the remark looks for its copy.
            let by = by.filter(|by| *by != span).map(|by| (by, None));
            verdicts.push(Verdict { function: body.name.clone(), span, region, by_call, own, text, by });
        }
        // What a region holds must be the region's too, as nothing lets go
        // of it when the region goes.
        let fresh = |_: (u32, u32), callee: usize| summaries[callee].fresh;
        let mut region: Vec<bool> = verdicts.iter().map(|verdict| verdict.region).collect();
        for (at, (why, by)) in holding(&flows[index], body, types, &loans, sites, &mut region, &fresh) {
            let verdict = &mut verdicts[at];
            let what = sites[at].what;
            verdict.region = false;
            verdict.by_call = false;
            verdict.own = false;
            verdict.text = format!("{what}stays on the heap: {why}");
            verdict.by = Some(by).filter(|(by, _)| *by != verdict.span);
        }
        verdicts
    });
    // What the placement changed of the verdicts: a region a loop empties
    // each turn, or the heap, where no region takes the value.
    let mut judged = judged;
    let outcomes = place(bodies, types, &summaries, &all_sites, &judged, &flows, clones);
    // The texts an append may grow, by the statements of each function,
    // its copies' as its own.
    let growable = growable(bodies, types, &flows);
    for body in bodies.iter_mut() {
        let of = body.copy_of.map_or(usize::MAX, |of| of as usize);
        if let Some(growable) = growable.get(of) {
            body.growable = growable.clone();
        }
    }
    for (body, growable) in bodies.iter_mut().zip(growable) {
        body.growable = growable;
    }
    for (((verdicts, outcomes), sites), body) in judged.iter_mut().zip(&outcomes).zip(&all_sites).zip(bodies.iter()) {
        for ((verdict, outcome), site) in verdicts.iter_mut().zip(outcomes).zip(sites) {
            let what = site.what;
            // A loop by its line, and its file when it is another than the
            // one the line is told in.
            let told = told_at(body, site, sources);
            // A loop by the first of its names that is the program's code.
            let line = |named: &[Span]| {
                let at = named.iter().find(|&&at| !library(sources, at)).or(named.first()).copied().unwrap_or_default();
                let ((file, line, _), (here, ..)) = (sources.position(at.start), sources.position(told.start));
                match file == here {
                    true => format!("line {line}"),
                    false => format!("{file}:{line}"),
                }
            };
            match outcome {
                Outcome::Stands => continue,
                Outcome::Turn(at) => {
                    verdict.text = format!("{what}goes in the region of a turn of the loop at {}", line(at))
                }
                Outcome::EveryTurn(at) => {
                    verdict.region = false;
                    verdict.by_call = false;
                    verdict.text = format!(
                        "{what}stays on the heap: it is made in every turn of the loop at {}, and the region the \
                         caller of `{}` gives would keep each turn's",
                        line(at),
                        body.name
                    );
                }
                Outcome::Outlives(at) => {
                    verdict.region = false;
                    verdict.own = false;
                    verdict.text = format!("{what}stays on the heap: it outlives a turn of the loop at {}", line(at));
                }
                Outcome::Heap(why, by) => {
                    verdict.region = false;
                    verdict.by_call = false;
                    verdict.own = false;
                    verdict.text = format!("{what}stays on the heap: {why}");
                    verdict.by = *by;
                }
            }
            verdict.by = verdict.by.filter(|(by, _)| *by != Span::default() && *by != verdict.span);
        }
    }
    if crate::mir_text::remarking() {
        let library = |span: Span| library(sources, span);
        for (index, (body, verdicts)) in bodies.iter_mut().zip(&judged).enumerate() {
            // A place a region takes in every copy of it is not made on the
            // heap: what the frame said of it says why the frame does not
            // take it.
            let placed: Vec<Span> = verdicts
                .iter()
                .map(|verdict| verdict.span)
                .filter(|&span| {
                    verdicts.iter().filter(|v| v.span == span).all(|verdict| verdict.region && !verdict.by_call)
                })
                .collect();
            for remark in
                body.remarks.iter_mut().filter(|remark| remark.kind == "heap" && placed.contains(&remark.span))
            {
                if let Some(why) = remark.text.strip_prefix("is made on the heap: ") {
                    remark.text = format!("is not made in the frame: {why}");
                    remark.kind = "frame";
                }
            }
            for (site, verdict) in all_sites[index].iter().zip(verdicts) {
                // A text an append may grow says it has the heap's room.
                let text = match verdict.region && body.growable.contains(&site.at) {
                    true => format!("{}, with room an append grows into", verdict.text),
                    false => verdict.text.clone(),
                };
                let given = !library(verdict.span) || reaches_program(&flows[index], body, types, site, &library);
                let scope = body.scopes.get(site.at.0 as usize).copied().flatten();
                crate::mir_text::remark_by(body, verdict.span, "region", verdict.by, given, scope, || text);
            }
        }
        summed(bodies, &judged);
    }
    let verdicts: Vec<Verdict> = judged.into_iter().flatten().collect();
    let takings = summaries
        .iter()
        .enumerate()
        .flat_map(|(callee, summary)| summary.calls.iter().map(move |call| (callee, call)))
        .map(|(callee, call)| Taking {
            caller: bodies[call.caller].name.clone(),
            callee: bodies[callee].name.clone(),
            span: call.span,
            taken: match call.taken {
                Taken::Drops => "drops",
                Taken::Passes => "passes",
                Taken::Keeps(..) => "keeps",
            },
        })
        .collect();
    (verdicts, takings)
}

/// Sums, at each call that stays a call, what the places of the function
/// called and of those it calls make, each function once, as places in the
/// code rather than allocations of a run: how many make values on the heap,
/// in regions of their own, and in the region the caller gives. A native's
/// own allocations are not code the view reads, and a call through a
/// function value names no function.
fn summed(bodies: &mut [Body], judged: &[Vec<Verdict>]) {
    // A copy a region's caller calls is its function's.
    let originals: Vec<usize> =
        bodies.iter().enumerate().map(|(index, body)| body.copy_of.map_or(index, |of| of as usize)).collect();
    let original = |index: usize| originals[index];
    // Each place once, as the copies of an unrolled loop or of code
    // inlined twice are one place: on the heap if any copy is, else in a
    // region of its own if any is.
    let own: Vec<[usize; 3]> = judged
        .iter()
        .map(|verdicts| {
            let mut places: std::collections::HashMap<Span, usize> = std::collections::HashMap::new();
            for verdict in verdicts {
                let at = match (verdict.region, verdict.own) {
                    (false, _) => 0,
                    (true, true) => 1,
                    (true, false) => 2,
                };
                let held = places.entry(verdict.span).or_insert(at);
                *held = (*held).min(at);
            }
            let mut sum = [0; 3];
            for at in places.into_values() {
                sum[at] += 1;
            }
            sum
        })
        .collect();
    let calls: Vec<Vec<usize>> = bodies
        .iter()
        .map(|body| {
            let mut called: Vec<usize> = body
                .blocks
                .iter()
                .flat_map(|block| &block.stmts)
                .filter_map(|statement| match statement {
                    Statement::Assign(_, rvalue, _) => match &**rvalue {
                        Rvalue::Call(Callee::Fn(id), _) => Some(id.0 as usize),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            called.sort_unstable();
            called.dedup();
            called
        })
        .collect();
    let mut sums: std::collections::HashMap<usize, [usize; 3]> = std::collections::HashMap::new();
    for index in 0..bodies.len() {
        for at in 0..bodies[index].remarks.len() {
            let Some(callee) = bodies[index].remarks[at].callee else { continue };
            let callee = original(callee as usize);
            let sum = *sums.entry(callee).or_insert_with(|| {
                let (mut seen, mut pending, mut sum) = (vec![false; bodies.len()], vec![callee], [0; 3]);
                seen[callee] = true;
                while let Some(function) = pending.pop() {
                    if let Some(places) = own.get(function) {
                        for (total, places) in sum.iter_mut().zip(places) {
                            *total += places;
                        }
                    }
                    for &next in &calls[function] {
                        let next = original(next);
                        if !std::mem::replace(&mut seen[next], true) {
                            pending.push(next);
                        }
                    }
                }
                sum
            });
            if sum == [0; 3] {
                continue;
            }
            let remark = &mut bodies[index].remarks[at];
            remark.places = Some(sum);
            remark.text = format!("{}; {}", remark.text, places_text(sum));
        }
    }
}

/// How a call's sum of places reads, of those that are not none: "2 places
/// in it make values on the heap and 4 in regions of its own".
fn places_text([heap, own, given]: [usize; 3]) -> String {
    let wheres = [(heap, "on the heap"), (own, "in regions of its own"), (given, "in the region its caller gives")];
    let told: Vec<(usize, &str)> = wheres.into_iter().filter(|&(count, _)| count > 0).collect();
    let mut parts: Vec<String> = Vec::new();
    for (at, (count, place)) in told.iter().enumerate() {
        parts.push(match (at, count) {
            (0, 1) => format!("1 place in it makes values {place}"),
            (0, _) => format!("{count} places in it make values {place}"),
            _ => format!("{count} {place}"),
        });
    }
    match parts.len() {
        0 | 1 => parts.concat(),
        count => format!("{} and {}", parts[..count - 1].join(", "), parts[count - 1]),
    }
}

/// Copies out the text a function returns where it may be a part of a
/// collection the function made, as the word a count of words finds among
/// the pieces of a `split`, when the return is all that keeps that
/// collection from the function's region: the collection, and what holds
/// it, stay there, and the return costs one copy of the text. Whether any
/// function changed.
fn copied_out(
    bodies: &mut [Body],
    types: &Types,
    roots: &[usize],
    flows: &[Flow],
    all_sites: &[Vec<Site>],
    summaries: &[Summary],
) -> bool {
    let text = |ty: Ty| matches!(types.kind(types.repr(ty)), TyKind::String);
    // What holds texts a return may be one of: a text alone, or wrapped in
    // a result, is the copy itself.
    let collection = |ty: Ty| {
        matches!(types.kind(types.repr(ty)), TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_))
    };
    // The functions something still calls: one whose every call was
    // inlined runs as none.
    let mut called = vec![false; bodies.len()];
    for &root in roots {
        called[root] = true;
    }
    for statement in bodies.iter().flat_map(|body| &body.blocks).flat_map(|block| &block.stmts) {
        let Statement::Assign(_, rvalue, _) = statement else { continue };
        match &**rvalue {
            Rvalue::Call(Callee::Fn(id), _)
            | Rvalue::FnValue(id)
            | Rvalue::Callback(id, _)
            | Rvalue::Erase(_, id, _)
            | Rvalue::CallOsr(id, ..) => called[id.0 as usize] = true,
            _ => {}
        }
    }
    let copying: Vec<usize> = (0..bodies.len())
        .filter(|&index| {
            let body = &bodies[index];
            !summaries[index].fresh
                && called[index]
                && body.ret.is_some()
                && text(body.result)
                && all_sites[index].iter().any(|site| {
                    collection(body.locals[site.local.0 as usize])
                        && fate(&flows[index], body, types, site.local, summaries, bodies) == Fate::Returned
                })
        })
        .collect();
    for &index in &copying {
        let body = &mut bodies[index];
        let Some(ret) = body.ret else { continue };
        let copied = Local(body.locals.len() as u32);
        body.locals.push(body.result);
        for block in &mut body.blocks {
            let Terminator::Return(Operand::Copy(place)) = &block.terminator else { continue };
            if place.local != ret || !place.path.is_empty() {
                continue;
            }
            // Where the code says what it returns: the last write of the
            // result in the block. A literal it returns is the program's
            // own, which needs no copy.
            let last = block.stmts.iter().rev().find_map(|statement| match statement {
                Statement::Assign(place, rvalue, span) if place.local == ret => Some((rvalue, *span)),
                _ => None,
            });
            let span = last.map(|(_, span)| span).unwrap_or_default();
            let copy = match last {
                Some((rvalue, _)) if matches!(&**rvalue, Rvalue::Use(Operand::Const(_))) => (**rvalue).clone(),
                _ => Rvalue::StrOp(crate::hir::StrOp::CopyOut, vec![Operand::local(ret)], span),
            };
            block.stmts.push(Statement::Assign(Place::local(copied), Box::new(copy), span));
            block.terminator = Terminator::Return(Operand::local(copied));
        }
        body.ret = Some(copied);
    }
    !copying.is_empty()
}

/// The loop in every turn of which a copy makes the value of `site` for the
/// region its caller gives, which would keep each turn's until the caller's
/// ends (`every_turn_of`).
fn made_every_turn(flow: &Flow, body: &Body, types: &Types, lifetimes: &Lifetimes, site: &Site) -> Option<usize> {
    let parts = flow.called[site.local.0 as usize];
    every_turn_of(flow, body, types, lifetimes, site.local, site.at, parts, Some(site))
}

/// The loop in every turn of which the statement at `at` makes the value
/// of `local` again, so that a region that outlives the loop would keep
/// each turn's: none where it is made once a call, as on the way out of a
/// `?`, and none where the function's own region would take it for the
/// whole loop (`Lifetimes::region`), one array the loop only grows with
/// `push`, what only such arrays keep past the turns, or one text grown in
/// place, `x = "{x}..."`, each of which leaves at most its own size behind
/// it as it grows.
#[allow(clippy::too_many_arguments)]
fn every_turn_of(
    flow: &Flow,
    body: &Body,
    types: &Types,
    lifetimes: &Lifetimes,
    local: Local,
    at: (u32, u32),
    parts: bool,
    grows: Option<&Site>,
) -> Option<usize> {
    let k = lifetimes.every_turn(at.0 as usize)?;
    match lifetimes.region(flow, body, types, local, at, parts, grows) {
        Some(0) => None,
        _ => Some(k),
    }
}

/// What a build puts in regions today, of what a region takes now: in each
/// function, what the verdicts give its own region, in the region of the
/// function or of the innermost loop whose turn it does not outlive; in a
/// copy of a function whose result some calls let go, which takes its
/// caller's region, what it returns too. Each holds only what is put in a
/// region and the results of the calls given one. A copy's result is its
/// region's when all of it is, which depends on the copies it calls: until
/// none changes. The calls that let a result go then call the copy, giving
/// a region of theirs, and the copies' calls that return one, the region
/// they were given.
fn place(
    bodies: &mut Vec<Body>,
    types: &Types,
    summaries: &[Summary],
    all_sites: &[Vec<Site>],
    judged: &[Vec<Verdict>],
    flows: &[Flow],
    clones: &[(usize, usize)],
) -> Vec<Vec<Outcome>> {
    let count = bodies.len();
    let loans: &[Vec<bool>] = &bodies.iter().map(|body| body.loans.clone()).collect::<Vec<_>>();
    // What each call does with its result, by caller and statement.
    let mut taking: crate::hash::FastMap<(usize, (u32, u32)), bool> = crate::hash::FastMap::default();
    for summary in summaries {
        for call in &summary.calls {
            match call.taken {
                Taken::Drops => _ = taking.insert((call.caller, call.at), true),
                Taken::Passes => _ = taking.insert((call.caller, call.at), false),
                Taken::Keeps(..) => {}
            }
        }
    }
    // A copy for each function whose result a call lets go, through the
    // callers that return it. A function and its clone for wider vectors,
    // the body of its version for `x86-64-v3`, are copied together, so that
    // the copy has that version too, or neither is.
    let mut barred = vec![false; count];
    let copied = loop {
        let mut copied = vec![false; count];
        let mut changed = true;
        while changed {
            changed = false;
            for index in 0..count {
                if copied[index] || !summaries[index].fresh || barred[index] {
                    continue;
                }
                let lets_go = summaries[index].calls.iter().any(|call| {
                    matches!(call.taken, Taken::Drops) || matches!(call.taken, Taken::Passes) && copied[call.caller]
                });
                if lets_go {
                    copied[index] = true;
                    changed = true;
                }
            }
            for &(base, clone) in clones {
                if copied[base] != copied[clone] && summaries[base].fresh && summaries[clone].fresh {
                    copied[base] = true;
                    copied[clone] = true;
                    changed = true;
                }
            }
        }
        let apart: Vec<(usize, usize)> =
            clones.iter().copied().filter(|&(base, clone)| copied[base] != copied[clone]).collect();
        if apart.is_empty() {
            break copied;
        }
        for (base, clone) in apart {
            barred[base] = true;
            barred[clone] = true;
        }
    };
    // The calls of a copy that let its result go, by caller.
    let mut copy_calls: Vec<Vec<(u32, u32)>> = vec![Vec::new(); count];
    for (summary, _) in summaries.iter().zip(&copied).filter(|&(_, &copied)| copied) {
        for call in summary.calls.iter().filter(|call| matches!(call.taken, Taken::Drops)) {
            copy_calls[call.caller].push(call.at);
        }
    }
    // The lifetimes of the functions with something to put in a region;
    // where nothing of it is in a loop, everything goes in their own.
    let indices: Vec<usize> = (0..count).collect();
    let lifetimes: Vec<Lifetimes> = crate::parallel::map(&indices, 16, |&index| {
        let mut blocks = all_sites[index]
            .iter()
            .filter(|site| site.placed())
            .map(|site| site.at.0)
            .chain(copy_calls[index].iter().map(|at| at.0))
            .peekable();
        match blocks.peek() {
            Some(_) => Lifetimes::of(&bodies[index], blocks),
            None => Lifetimes::none(),
        }
    });
    // The region of each call of a copy that lets its result go, by
    // statement.
    let call_regions: Vec<crate::hash::FastMap<(u32, u32), u32>> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            let mut calls = copy_calls[index].clone();
            calls.sort_unstable();
            calls.dedup();
            calls
                .into_iter()
                .filter_map(|at| {
                    let Statement::Assign(place, ..) = &body.blocks[at.0 as usize].stmts[at.1 as usize] else {
                        return None;
                    };
                    let region = lifetimes[index].region(&flows[index], body, types, place.local, at, true, None)?;
                    Some((at, region))
                })
                .collect()
        })
        .collect();
    // Whether a copy's result is all its region's, from all and down.
    let mut whole = copied.clone();
    let mut own: Vec<Vec<((u32, u32), u32)>> = vec![Vec::new(); count];
    let mut copy_own: Vec<Vec<((u32, u32), u32)>> = vec![Vec::new(); count];
    let mut copy_given: Vec<Vec<(u32, u32)>> = vec![Vec::new(); count];
    let mut outcomes: Vec<Vec<Outcome>> = vec![Vec::new(); count];
    // Each function, and again those that call a copy whose result is no
    // longer all its region's.
    let mut queued = vec![true; count];
    let mut pending: Vec<usize> = (0..count).rev().collect();
    while let Some(index) = pending.pop() {
        queued[index] = false;
        let (body, sites) = (&bodies[index], &all_sites[index]);
        let verdicts = &judged[index];
        let regions: Vec<Option<u32>> = sites
            .iter()
            .zip(verdicts)
            .map(|(site, verdict)| match site.placed() && verdict.own {
                true => {
                    // What a site made has parts, followed as they are.
                    let parts = flows[index].called[site.local.0 as usize];
                    lifetimes[index].region(&flows[index], body, types, site.local, site.at, parts, Some(site))
                }
                false => None,
            })
            .collect();
        // Where what the verdicts give a region goes in the end.
        let loops = &lifetimes[index];
        let written = |k: usize| loops.named(body, k);
        // What a copy would make in every turn of a loop for the region its
        // caller gives stays on the heap, as that region would keep each
        // turn's until the caller's ends.
        let every_turn: Vec<Option<usize>> = sites
            .iter()
            .zip(verdicts)
            .map(|(site, verdict)| match copied[index] && site.placed() && verdict.region && !verdict.own {
                true => made_every_turn(&flows[index], body, types, loops, site),
                false => None,
            })
            .collect();
        let mut outcome: Vec<Outcome> = sites
            .iter()
            .zip(verdicts)
            .zip(&regions)
            .zip(&every_turn)
            .map(|(((site, verdict), region), every_turn)| match (verdict.region, verdict.own, site.refused, region) {
                (false, ..) => Outcome::Stands,
                (true, _, Some(why), _) => Outcome::Heap(why.to_string(), None),
                (true, true, None, Some(0)) => Outcome::Stands,
                (true, true, None, Some(region)) => Outcome::Turn(written(*region as usize - 1)),
                (true, true, None, None) => {
                    Outcome::Outlives(loops.loop_of(site.at.0 as usize).map(written).unwrap_or_default())
                }
                (true, false, None, _) => match (copied[index], every_turn) {
                    (true, Some(k)) => Outcome::EveryTurn(written(*k)),
                    (true, None) => Outcome::Stands,
                    (false, _) => Outcome::Heap(format!("no call of `{}` lets it go", body.name), None),
                },
            })
            .collect();
        let lets_go = |at: (u32, u32), callee: usize| whole[callee] && call_regions[index].contains_key(&at);
        let mut placed: Vec<bool> = regions.iter().map(Option::is_some).collect();
        for (at, (why, by)) in holding(&flows[index], body, types, loans, sites, &mut placed, &lets_go) {
            outcome[at] = Outcome::Heap(why, Some(by));
        }
        own[index] = sites
            .iter()
            .zip(&regions)
            .zip(&placed)
            .filter_map(|((site, region), placed)| region.filter(|_| *placed).map(|region| (site.at, region)))
            .collect();
        if !copied[index] {
            outcomes[index] = outcome;
            continue;
        }
        let gives = |at: (u32, u32), callee: usize| match taking.get(&(index, at)) {
            Some(true) => whole[callee] && call_regions[index].contains_key(&at),
            Some(false) => whole[callee],
            None => false,
        };
        let mut placed: Vec<bool> = sites
            .iter()
            .zip(verdicts)
            .zip(&regions)
            .zip(&every_turn)
            .map(|(((site, verdict), region), every_turn)| {
                site.placed() && verdict.region && (!verdict.own || region.is_some()) && every_turn.is_none()
            })
            .collect();
        for (at, (why, by)) in holding(&flows[index], body, types, loans, sites, &mut placed, &gives) {
            if !verdicts[at].own {
                outcome[at] = Outcome::Heap(why, Some(by));
            }
        }
        outcomes[index] = outcome;
        copy_own[index] = sites
            .iter()
            .zip(verdicts)
            .zip(&regions)
            .zip(&placed)
            .filter(|(((_, verdict), _), placed)| **placed && verdict.own)
            .filter_map(|(((site, _), region), _)| region.map(|region| (site.at, region)))
            .collect();
        copy_given[index] = sites
            .iter()
            .zip(verdicts)
            .zip(&placed)
            .filter(|((_, verdict), placed)| **placed && !verdict.own)
            .map(|((site, _), _)| site.at)
            .collect();
        let made = made(&flows[index], body, types, loans, sites, &placed, &gives);
        if whole[index] && !body.ret.is_some_and(|ret| made[ret.0 as usize]) {
            whole[index] = false;
            for call in &summaries[index].calls {
                if !std::mem::replace(&mut queued[call.caller], true) {
                    pending.push(call.caller);
                }
            }
        }
    }
    // The copies, after the functions, and the calls that call them.
    let mut copy_of = vec![None; count];
    for index in 0..count {
        if copied[index] {
            let mut copy = bodies[index].clone();
            copy.copy_of = Some(index as u32);
            copy.region = std::mem::take(&mut copy_own[index]);
            copy.given = std::mem::take(&mut copy_given[index]);
            bodies.push(copy);
            copy_of[index] = Some(bodies.len() - 1);
        }
    }
    let origin: Vec<usize> = (0..count).chain((0..count).filter(|&index| copy_of[index].is_some())).collect();
    for index in 0..bodies.len() {
        let of = origin[index];
        if index < count {
            bodies[index].region = std::mem::take(&mut own[index]);
        }
        bodies[index].loops = lifetimes[of].back_edges(&bodies[of]);
        let in_copy = index >= count;
        let mut calls = Vec::new();
        for (block, data) in bodies[index].blocks.iter_mut().enumerate() {
            for (at, statement) in data.stmts.iter_mut().enumerate() {
                let Statement::Assign(_, rvalue, _) = statement else { continue };
                let Rvalue::Call(Callee::Fn(id), _) = &mut **rvalue else { continue };
                let Some(copy) = copy_of[id.0 as usize] else { continue };
                let position = (block as u32, at as u32);
                let region = match taking.get(&(of, position)) {
                    Some(true) => match call_regions[of].get(&position) {
                        Some(&region) => Some(region),
                        None => continue,
                    },
                    Some(false) if in_copy => None,
                    _ => continue,
                };
                *id = crate::hir::FnId(copy as u32);
                calls.push((position, region));
            }
        }
        bodies[index].region_calls = calls;
    }
    outcomes
}

/// Where a value the verdicts give a region goes in the end.
#[derive(Clone)]
enum Outcome {
    /// Where its verdict says, in a region or on the heap.
    Stands,
    /// In the region of a turn of the loop named so (`Lifetimes::named`).
    Turn(Vec<Span>),
    /// On the heap, as it outlives a turn of the loop named so.
    Outlives(Vec<Span>),
    /// On the heap, as a copy makes it in every turn of the loop named so
    /// for the region its caller gives, which would keep each turn's.
    EveryTurn(Vec<Span>),
    /// On the heap, as no region takes it: why, and the code that keeps it.
    Heap(String, Option<crate::mir::By>),
}

/// Where a value a function lets go may go: its loops, the innermost
/// first, the locals live at each one's head and where it is left.
struct Lifetimes {
    loops: Vec<(usize, Vec<bool>)>,
    /// The locals live at each loop's head and where it is left for a block
    /// that may come back to it, as bits.
    live_head: Vec<Vec<u64>>,
    live_out: Vec<Vec<u64>>,
    /// The blocks from which each loop's head is reached again. A turn's
    /// region is emptied only as a turn goes back to the head, and left as
    /// the function returns, after the locals it held went: what is still
    /// held where the loop is left for a block that never comes back, as on
    /// the way out of a `?`, outlives no turn.
    back: Vec<Vec<bool>>,
    looped: Vec<bool>,
}

impl Lifetimes {
    /// Of a function with nothing to put in a region by loops.
    fn none() -> Lifetimes {
        Lifetimes {
            loops: Vec::new(),
            live_head: Vec::new(),
            live_out: Vec::new(),
            back: Vec::new(),
            looped: Vec::new(),
        }
    }

    /// Of a function that puts something in a region in `blocks`; what is
    /// live where is read only when one of them is in a loop.
    fn of(body: &Body, mut blocks: impl Iterator<Item = u32>) -> Lifetimes {
        let loops = loops(body);
        let mut looped = vec![false; body.blocks.len()];
        for (_, inside) in &loops {
            for (looped, &inside) in looped.iter_mut().zip(inside) {
                *looped |= inside;
            }
        }
        if !blocks.any(|block| looped[block as usize]) {
            return Lifetimes { loops, live_head: Vec::new(), live_out: Vec::new(), back: Vec::new(), looped };
        }
        let live = crate::mir::live_in_bits(&body.blocks, body.locals.len());
        let mut before = vec![Vec::new(); body.blocks.len()];
        for (from, data) in body.blocks.iter().enumerate() {
            for to in crate::hoist::targets(&data.terminator) {
                before[to].push(from);
            }
        }
        let (mut live_head, mut live_out, mut back) = (Vec::new(), Vec::new(), Vec::new());
        for (head, inside) in &loops {
            live_head.push(live[*head].clone());
            let mut reaches = vec![false; body.blocks.len()];
            let mut next = vec![*head];
            while let Some(block) = next.pop() {
                for &from in &before[block] {
                    if !std::mem::replace(&mut reaches[from], true) {
                        next.push(from);
                    }
                }
            }
            let mut out = vec![0u64; live[*head].len()];
            for (_, data) in body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]) {
                for to in crate::hoist::targets(&data.terminator).into_iter().filter(|&to| !inside[to] && reaches[to]) {
                    for (out, &live) in out.iter_mut().zip(&live[to]) {
                        *out |= live;
                    }
                }
            }
            live_out.push(out);
            back.push(reaches);
        }
        Lifetimes { loops, live_head, live_out, back, looped }
    }

    /// Where loop `k` is written (`written`), and then each call that
    /// brought the code of its head, from the innermost out: the view names
    /// it by the first that is the program's, as the `for` a `next()` of an
    /// iterator is inlined into.
    fn named(&self, body: &Body, k: usize) -> Vec<Span> {
        let mut named = vec![self.written(body, k)];
        let mut scope = body.scopes.get(self.loops[k].0).copied().flatten();
        while let Some(at) = scope {
            named.push(body.inlined[at as usize].call);
            scope = body.inlined[at as usize].parent;
        }
        named
    }

    /// The innermost loop `block` is in.
    fn loop_of(&self, block: usize) -> Option<usize> {
        self.loops.iter().position(|(_, inside)| inside[block])
    }

    /// The innermost loop whose head `block` comes back to, so that what it
    /// makes is made again in each turn; none when it is in no loop, or
    /// only on a way out that never comes back, as a `?`'s.
    fn every_turn(&self, block: usize) -> Option<usize> {
        if !self.looped.get(block).copied().unwrap_or(false) {
            return None;
        }
        // In a loop whose lifetimes were not read: each turn, to be safe.
        if self.back.len() != self.loops.len() {
            return self.loop_of(block);
        }
        (0..self.loops.len()).find(|&k| self.loops[k].1[block] && self.back[k][block])
    }

    /// Where loop `k` is written: the first statement of its head, as its
    /// range or its test; for a loop unrolling made, whose test is not
    /// written, that of the loop it copies, which has the same statements
    /// and is not around it nor in it; else its first statement written.
    fn written(&self, body: &Body, k: usize) -> Span {
        let placed = |statement: &Statement| match statement {
            Statement::Assign(_, _, span) if *span != Span::default() => Some(*span),
            _ => None,
        };
        let head = |k: usize| body.blocks[self.loops[k].0].stmts.iter().find_map(placed);
        let spans = |k: usize| {
            let inside = &self.loops[k].1;
            (0..body.blocks.len())
                .filter(move |&block| inside[block])
                .flat_map(|block| &body.blocks[block].stmts)
                .filter_map(placed)
        };
        let mine: std::collections::HashSet<Span> = spans(k).collect();
        let nested = |j: usize| self.loops[j].1[self.loops[k].0] || self.loops[k].1[self.loops[j].0];
        // A loop without a condition, `for { … }`, by where its `for` is: the
        // smallest of those written that holds all its statements, unless
        // it holds all of a loop around it too, whose `for` it is then.
        // Of loop `j`, the statements of the code its head is of, not of what
        // was inlined into it, which is written elsewhere.
        let own = |j: usize| {
            let inside = &self.loops[j].1;
            let scope = |block: usize| body.scopes.get(block).copied().flatten();
            let head = scope(self.loops[j].0);
            (0..body.blocks.len())
                .filter(move |&block| inside[block] && scope(block) == head)
                .flat_map(|block| &body.blocks[block].stmts)
                .filter_map(placed)
        };
        let holds = |outer: &Span, j: usize| own(j).all(|span| outer.start <= span.start && span.end <= outer.end);
        let written = |k: usize| {
            let found = body.loop_spans.iter().filter(|outer| own(k).next().is_some() && holds(outer, k));
            let smallest = found.min_by_key(|outer| outer.end - outer.start).copied()?;
            let around = |j: usize| j != k && self.loops[j].1[self.loops[k].0] && holds(&smallest, j);
            (!(0..self.loops.len()).any(around)).then_some(smallest)
        };
        written(k)
            .or_else(|| head(k))
            .or_else(|| {
                (0..self.loops.len())
                    .filter(|&j| j != k && !nested(j) && spans(j).any(|span| mine.contains(&span)))
                    .find_map(head)
            })
            .or_else(|| spans(k).next())
            .unwrap_or_default()
    }

    /// The region a value made in `block` would go in: the function's, 0,
    /// outside every loop; else that of the innermost loop whose turn it
    /// does not outlive, nothing that holds it live at the loop's head nor
    /// where the loop is left, `k + 1`; or the function's for an array that
    /// lives across the turns and grows (`bounded`); or none.
    #[allow(clippy::too_many_arguments)]
    fn region(
        &self,
        flow: &Flow,
        body: &Body,
        types: &Types,
        local: Local,
        at: (u32, u32),
        parts: bool,
        grows: Option<&Site>,
    ) -> Option<u32> {
        let block = at.0 as usize;
        if !self.looped.get(block).copied().unwrap_or(false) {
            return Some(0);
        }
        // In a loop of a function whose lifetimes were not read for it.
        if self.live_head.len() != self.loops.len() {
            return None;
        }
        let held = flow.holders(types, body, local, parts);
        let words = held.len().div_ceil(64);
        let mut bits = vec![0u64; words];
        for (index, _) in held.iter().enumerate().filter(|&(_, &held)| held) {
            bits[index / 64] |= 1 << (index % 64);
        }
        // The locals that hold it past a turn of loop `k`, of `holds`: live
        // at its head, or where it is left, and holding it there.
        let keepers = |k: usize, at_head: &[u64], at_exits: &[u64]| -> Vec<usize> {
            let (head, out) = (&self.live_head[k], &self.live_out[k]);
            (0..held.len())
                .filter(|&index| {
                    let (word, bit) = (index / 64, 1 << (index % 64));
                    (head[word] & at_head[word] | out[word] & at_exits[word]) & bit != 0
                })
                .collect()
        };
        let containing: Vec<usize> = (0..self.loops.len()).filter(|&k| self.loops[k].1[block]).collect();
        if let Some(&k) = containing.iter().find(|&&k| keepers(k, &bits, &bits).is_empty()) {
            return Some(k as u32 + 1);
        }
        if grows.is_some_and(|site| bounded(body, &self.looped, site)) {
            return Some(0);
        }
        // Where it is at each loop's head and exits, following the statements
        // from where it is made: a local a move zeroes, or that a whole value
        // of another writes over, holds it no more, as a tree's array once its
        // node took it.
        let (entry, exit, again) = held_from(body, &bits, local, at);
        let at = |k: usize| {
            let inside = &self.loops[k].1;
            let mut exits = vec![0u64; words];
            for (from, data) in body.blocks.iter().enumerate().filter(|&(from, _)| inside[from]) {
                if crate::hoist::targets(&data.terminator).into_iter().any(|to| !inside[to] && self.back[k][to]) {
                    exits.iter_mut().zip(&exit[from]).for_each(|(all, out)| *all |= out);
                }
            }
            (&entry[self.loops[k].0], exits)
        };
        if let Some(&k) = containing.iter().find(|&&k| {
            let (head, exits) = at(k);
            keepers(k, head, &exits).is_empty()
        }) {
            return Some(k as u32 + 1);
        }
        // What only arrays that each loop does nothing to but push keeps
        // past its turns lives as long as they do, and adds nothing to throw
        // away: the function's region, as a tree's children each turn makes.
        // So does a text that grows in place across the turns, `x =
        // "{x}..."`, held past them by its own local alone, which the loops
        // write whole only so: its room doubles, so the rooms it leaves add
        // up to no more than it ends with, as an array's that grows. Each
        // time it grows, nothing else may hold it but those arrays, which
        // keep the copy it then makes as they would on the heap: a value a
        // turn lets go of would leave a copy in the region at every turn.
        let appended = grows.is_some_and(|site| appends(body, site));
        let kept = |k: usize| {
            let (head, exits) = at(k);
            let inside = &self.loops[k].1;
            let pushed = |keeper: usize| only_pushed(body, types, inside, Local(keeper as u32));
            let alone = |keeper: usize| keeper == local.0 as usize || pushed(keeper);
            keepers(k, head, &exits).into_iter().all(|keeper| {
                pushed(keeper) || appended && keeper == local.0 as usize && only_appended(body, inside, local)
            }) && (!appended || (0..held.len()).filter(|&index| again[index / 64] & 1 << (index % 64) != 0).all(alone))
        };
        containing.into_iter().all(kept).then_some(0)
    }

    /// Each loop's head and the blocks that go back to it.
    fn back_edges(&self, body: &Body) -> Vec<(u32, Vec<u32>)> {
        self.loops
            .iter()
            .map(|(head, inside)| {
                let back = body
                    .blocks
                    .iter()
                    .enumerate()
                    .filter(|&(block, data)| inside[block] && crate::hoist::targets(&data.terminator).contains(head))
                    .map(|(block, _)| block as u32)
                    .collect();
                (*head as u32, back)
            })
            .collect()
    }
}

/// The statements of each function, of those that make a value in a region,
/// whose text an append may later grow in place, `x = "{x}..."`: what
/// they make reaches the start of one, in the function, in a function it
/// is given to, or in a caller that takes what the function returns. They
/// make it with the room the heap's block would hold, so that it grows by
/// the heap's steps and allocates as often as it would on the heap; every
/// other text has the room it asks for, which no append grows into. What
/// may hold the text at any time counts, and its parts: where the analysis
/// cannot tell, a text has the room.
fn growable(bodies: &[Body], types: &Types, flows: &[Flow]) -> Vec<Vec<(u32, u32)>> {
    let count = flows.len();
    // The locals each function appends to in place, and its calls by name
    // with the locals each gives.
    let appended: Vec<Vec<bool>> = bodies[..count]
        .iter()
        .map(|body| {
            let mut appended = vec![false; body.locals.len()];
            for (_, statement) in positioned(body) {
                if let Statement::Assign(_, rvalue, _) = statement
                    && let Rvalue::Concat(pieces) = &**rvalue
                    && let Some(Piece::Taken(taken)) = pieces.first()
                {
                    appended[taken.local.0 as usize] = true;
                }
            }
            appended
        })
        .collect();
    type Calls = Vec<(usize, Vec<Option<Local>>)>;
    let calls: Vec<Calls> = bodies[..count]
        .iter()
        .map(|body| {
            positioned(body)
                .filter_map(|(_, statement)| match statement {
                    Statement::Assign(_, rvalue, _) => match &**rvalue {
                        Rvalue::Call(Callee::Fn(id), args) if (id.0 as usize) < count => {
                            let given = args
                                .iter()
                                .map(|arg| match arg {
                                    Operand::Copy(place) => Some(place.local),
                                    Operand::Const(_) => None,
                                })
                                .collect();
                            Some((id.0 as usize, given))
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        })
        .collect();
    // Whether what `held` says hold reaches an append in function `index`:
    // there, given to a function whose parameter does, or returned to a
    // caller that appends to it.
    let reaches = |index: usize, held: &[bool], params: &[Vec<bool>], result: &[bool]| {
        let ours = |local: Local| held.get(local.0 as usize).copied().unwrap_or(false);
        held.iter().zip(&appended[index]).any(|(&held, &appended)| held && appended)
            || calls[index].iter().any(|(callee, given)| {
                given.iter().zip(&params[*callee]).any(|(given, &appends)| appends && given.is_some_and(ours))
            })
            || bodies[index].ret.is_some_and(|ret| result[index] && ours(ret))
    };
    // What each parameter, and what each function returns, reaches: from
    // none, until nothing changes.
    let mut params: Vec<Vec<bool>> = bodies[..count].iter().map(|body| vec![false; body.params.len()]).collect();
    let mut result = vec![false; count];
    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..count {
            let (body, flow) = (&bodies[index], &flows[index]);
            for (at, &param) in body.params.iter().enumerate() {
                if !params[index][at] && reaches(index, &flow.holders(types, body, param, true), &params, &result) {
                    params[index][at] = true;
                    changed = true;
                }
            }
            for &(_, _, local, callee, _) in &flow.results {
                if !result[callee] && reaches(index, &flow.holders(types, body, local, true), &params, &result) {
                    result[callee] = true;
                    changed = true;
                }
            }
        }
    }
    // The statements that make a value in a region, of the function and of
    // its copies, whose value reaches one.
    (0..count)
        .map(|index| {
            let (body, flow) = (&bodies[index], &flows[index]);
            let mut made: Vec<(u32, u32)> = body.region.iter().map(|&(at, _)| at).collect();
            for copy in bodies[count..].iter().filter(|copy| copy.copy_of == Some(index as u32)) {
                made.extend(&copy.given);
                made.extend(copy.region.iter().map(|&(at, _)| at));
            }
            made.sort_unstable();
            made.dedup();
            made.into_iter()
                .filter(|&(block, at)| {
                    let statement = body.blocks.get(block as usize).and_then(|data| data.stmts.get(at as usize));
                    match statement {
                        Some(Statement::Assign(place, rvalue, _)) if place.path.is_empty() => {
                            !matches!(&**rvalue, Rvalue::Concat(pieces) if matches!(pieces.first(), Some(Piece::Taken(_))))
                                && reaches(index, &flow.holders(types, body, place.local, true), &params, &result)
                        }
                        _ => false,
                    }
                })
                .collect()
        })
        .collect()
}

/// The loops of a body, the innermost first: each one's head and its
/// blocks, the head and those that reach an edge back to it without passing
/// it: a loop inside another is a loop of its own, as a turn of the outer
/// one is many of the inner's, though the blocks on a cycle through either
/// head are the same.
fn loops(body: &Body) -> Vec<(usize, Vec<bool>)> {
    let count = body.blocks.len();
    let successors: Vec<Vec<usize>> = body.blocks.iter().map(|data| crate::hoist::targets(&data.terminator)).collect();
    let mut predecessors = vec![Vec::new(); count];
    for (block, to) in successors.iter().enumerate() {
        for &to in to {
            predecessors[to].push(block);
        }
    }
    // The edges back, to a block on the path a walk from the entry is on.
    let (mut state, mut back) = (vec![0u8; count], Vec::new());
    let mut stack = vec![(0usize, 0usize)];
    if count > 0 {
        state[0] = 1;
    }
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        match successors[block].get(*next).copied() {
            Some(to) => {
                *next += 1;
                match state[to] {
                    0 => {
                        state[to] = 1;
                        stack.push((to, 0));
                    }
                    1 => back.push((block, to)),
                    _ => {}
                }
            }
            None => {
                state[block] = 2;
                stack.pop();
            }
        }
    }
    let mut loops: Vec<(usize, Vec<bool>)> = Vec::new();
    for (from, head) in back {
        let at = match loops.iter().position(|(other, _)| *other == head) {
            Some(at) => at,
            None => {
                let mut inside = vec![false; count];
                inside[head] = true;
                loops.push((head, inside));
                loops.len() - 1
            }
        };
        let inside = &mut loops[at].1;
        let mut pending = vec![from];
        while let Some(block) = pending.pop() {
            if !std::mem::replace(&mut inside[block], true) {
                pending.extend(&predecessors[block]);
            }
        }
    }
    loops.sort_by_key(|(_, inside)| inside.iter().filter(|&&inside| inside).count());
    loops
}

/// Whether what a place makes in the function's own region stays within
/// what the function does once: a place in a loop makes a value in each
/// turn, which would gather in the region until the function returns, but
/// for an array that lives across the turns and grows, given its value
/// outside any loop, whose old room adds up to no more than what it ends
/// with.
fn bounded(body: &Body, looped: &[bool], site: &Site) -> bool {
    if !looped[site.at.0 as usize] {
        return true;
    }
    (site.what.starts_with("grows") || site.what.starts_with("changes"))
        && body.blocks.iter().enumerate().filter(|&(block, _)| looped[block]).all(|(_, data)| {
            data.stmts.iter().all(|statement| {
                !matches!(statement, Statement::Assign(place, ..) if place.local == site.local && place.path.is_empty())
            })
        })
}

/// The locals of `held`, as bits, that may hold what `local` takes at
/// `at`, at the start and at the end of each block, from `at` on: a
/// statement that reads one of them passes it on to what it writes and to
/// every local it reads, which a call or a change may write through, and
/// what is written through a loan to what it lends; a local a move or
/// `clear` zeroes, or that a whole value of none of them writes over, holds
/// it no more.
fn held_from(body: &Body, held: &[u64], local: Local, at: (u32, u32)) -> (Vec<Vec<u64>>, Vec<Vec<u64>>, Vec<u64>) {
    let words = held.len();
    let has = |set: &[u64], l: Local| set[l.0 as usize / 64] & (1 << (l.0 % 64)) != 0;
    let set = |set: &mut [u64], l: Local, on: bool| {
        let (word, bit) = (l.0 as usize / 64, 1u64 << (l.0 % 64));
        if on && held[word] & bit != 0 {
            set[word] |= bit;
        } else if !on {
            set[word] &= !bit;
        }
    };
    let lends: Vec<(Local, Local)> = positioned(body)
        .filter_map(|(_, statement)| match statement {
            Statement::Lend(alias, place) => Some((*alias, place.local)),
            _ => None,
        })
        .collect();
    let step = |state: &mut Vec<u64>, statement: &Statement| {
        match statement {
            Statement::Assign(place, rvalue, _) => {
                let mut used = Vec::new();
                rvalue_uses(rvalue, &mut used);
                let from = used.iter().any(|&l| has(state, l));
                if from {
                    used.iter().for_each(|&l| set(state, l, true));
                }
                match (from, place.path.is_empty()) {
                    (true, _) => set(state, place.local, true),
                    (false, true) => set(state, place.local, false),
                    (false, false) => {}
                }
            }
            Statement::Moved(place) if place.path.is_empty() => set(state, place.local, false),
            Statement::Clear(cleared) => set(state, *cleared, false),
            Statement::Lend(alias, place) if has(state, place.local) => set(state, *alias, true),
            _ => {}
        }
        // A loan of a loan passes it on as far as it goes.
        for _ in 0..lends.len() {
            let before = state.clone();
            for &(alias, lent) in &lends {
                if has(state, alias) {
                    set(state, lent, true);
                }
            }
            if *state == before {
                break;
            }
        }
    };
    let count = body.blocks.len();
    let (mut entry, mut exit) = (vec![vec![0u64; words]; count], vec![vec![0u64; words]; count]);
    let mut again = vec![0u64; words];
    let mut pending = vec![at.0 as usize];
    let mut queued = vec![false; count];
    queued[at.0 as usize] = true;
    while let Some(block) = pending.pop() {
        queued[block] = false;
        let mut state = entry[block].clone();
        for (index, statement) in body.blocks[block].stmts.iter().enumerate() {
            if (block as u32, index as u32) == at {
                again.iter_mut().zip(&state).for_each(|(into, from)| *into |= from);
            }
            step(&mut state, statement);
            if (block as u32, index as u32) == at {
                set(&mut state, local, true);
            }
        }
        exit[block].clone_from(&state);
        for to in crate::hoist::targets(&body.blocks[block].terminator) {
            let before = entry[to].clone();
            entry[to].iter_mut().zip(&state).for_each(|(into, from)| *into |= from);
            if entry[to] != before && !std::mem::replace(&mut queued[to], true) {
                pending.push(to);
            }
        }
    }
    (entry, exit, again)
}

/// Whether a site's statement grows its local's own text in place, `x =
/// "{x}..."`.
fn appends(body: &Body, site: &Site) -> bool {
    let statement = body.blocks.get(site.at.0 as usize).and_then(|block| block.stmts.get(site.at.1 as usize));
    matches!(statement, Some(Statement::Assign(place, rvalue, _))
        if place.local == site.local && place.path.is_empty() && taken_first(rvalue, site.local))
}

/// Whether an rvalue appends to the text of `local`, taken whole.
fn taken_first(rvalue: &Rvalue, local: Local) -> bool {
    matches!(rvalue, Rvalue::Concat(pieces)
        if matches!(pieces.first(), Some(Piece::Taken(taken)) if taken.local == local && taken.path.is_empty()))
}

/// Whether the blocks `inside` a loop write the text of `local` whole only
/// by appending to it, and move, clear and lend it nowhere.
fn only_appended(body: &Body, inside: &[bool], local: Local) -> bool {
    body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]).all(|(_, data)| {
        data.stmts.iter().all(|statement| match statement {
            Statement::Assign(place, rvalue, _) => place.local != local || taken_first(rvalue, local),
            Statement::Moved(place) | Statement::Lend(_, place) => place.local != local,
            Statement::Clear(cleared) => *cleared != local,
            _ => true,
        })
    })
}

/// Whether `local` is an array or a deque that the blocks `inside` a loop
/// only push to, or empty to fill again: given its value outside them,
/// never written whole there, nor an element of it, nor changed otherwise,
/// nor lent, nor passed to a call.
fn only_pushed(body: &Body, types: &Types, inside: &[bool], local: Local) -> bool {
    let array = matches!(
        types.kind(types.repr(body.locals[local.0 as usize])),
        crate::types::TyKind::Array(_) | crate::types::TyKind::Deque(_)
    );
    let ours = |operand: &Operand| matches!(operand, Operand::Copy(place) if place.local == local);
    array
        && body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]).all(|(_, data)| {
            data.stmts.iter().all(|statement| match statement {
                Statement::Assign(place, rvalue, _) => {
                    place.local != local
                        && match &**rvalue {
                            Rvalue::Mutate(mutation, target, ..) if target.local == local => {
                                target.path.is_empty()
                                    && match mutation {
                                        crate::hir::Mutation::Push
                                        | crate::hir::Mutation::PushFront
                                        | crate::hir::Mutation::Reserve => true,
                                        // Emptied, to be filled again or
                                        // by `clear()`, where its
                                        // function's region does not last
                                        // long.
                                        crate::hir::Mutation::Empty => body.lasting.is_none(),
                                        crate::hir::Mutation::Clear => body.lasting.is_none(),
                                        _ => false,
                                    }
                            }
                            Rvalue::AssignSlice(target, ..) => target.local != local,
                            Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) => !args.iter().any(ours),
                            _ => true,
                        }
                }
                Statement::Lend(_, place) => place.local != local,
                _ => true,
            })
        })
}

/// The places a region would take that hold a value it did not make, why,
/// and where they take it, until none is left: a value of a region holds
/// only values of a region, its own or one that outlives it, or values that
/// count nothing, as when the region goes it goes whole, letting go of
/// nothing its values hold. A value the function was given, or that a call
/// keeps a count of, would stay held, and leak.
fn holding(
    flow: &Flow,
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    sites: &[Site],
    region: &mut [bool],
    call_made: &dyn Fn((u32, u32), usize) -> bool,
) -> Vec<(usize, (String, crate::mir::By))> {
    let counted = |place: &Place| match place.path.is_empty() {
        true => flow.counted[place.local.0 as usize],
        false => types.needs_drop(body.place_ty(types, place)),
    };
    let mut found = Vec::new();
    if !region.contains(&true) {
        return found;
    }
    let map = |local: Local| matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::Map(..));
    // The copies of a local, whole, by the local copied.
    let mut copies: Vec<(u32, u32)> = Vec::new();
    // The statements that may store into a value or change it inside.
    let mut storing: Vec<((u32, u32), &Statement)> = Vec::new();
    for (at, statement) in positioned(body) {
        match statement {
            Statement::Assign(place, rvalue, _) => match &**rvalue {
                Rvalue::Use(Operand::Copy(from)) if place.path.is_empty() && from.path.is_empty() => {
                    copies.push((from.local.0, place.local.0))
                }
                Rvalue::Use(_) if !place.path.is_empty() => storing.push((at, statement)),
                Rvalue::Mutate(..)
                | Rvalue::AssignSlice(..)
                | Rvalue::Call(Callee::Fn(_), _)
                | Rvalue::Aggregate(..)
                | Rvalue::Variant(..)
                | Rvalue::Array(..)
                | Rvalue::Erase(..)
                | Rvalue::ArrayOp(..)
                | Rvalue::Slice(..) => storing.push((at, statement)),
                _ if place.path.len() > 1 => storing.push((at, statement)),
                _ => {}
            },
            Statement::Lend(alias, place) if place.path.is_empty() => copies.push((place.local.0, alias.0)),
            Statement::Lend(..) => storing.push((at, statement)),
            _ => {}
        }
    }
    copies.sort_unstable();
    loop {
        if !region.contains(&true) {
            return found;
        }
        let made = made(flow, body, types, loans, sites, region, call_made);
        let mut changed = false;
        for (at, site) in sites.iter().enumerate() {
            let (local, span) = (site.local, site.span);
            if !region[at] {
                continue;
            }
            // The locals that are the value: its copies, whole.
            let mut is = vec![false; body.locals.len()];
            is[local.0 as usize] = true;
            let mut pending = vec![local.0];
            while let Some(from) = pending.pop() {
                let first = copies.partition_point(|&(copied, _)| copied < from);
                for &(_, into) in copies[first..].iter().take_while(|&&(copied, _)| copied == from) {
                    if !std::mem::replace(&mut is[into as usize], true) {
                        pending.push(into);
                    }
                }
            }
            // What is stored in it: by the statement that makes it, an
            // element or a field written, or what grows it; and an array
            // that grows here holds what it held before, which a region
            // must have made too.
            // Where the code that keeps it is, with its inlined scope.
            let scope = |block: u32| body.scopes.get(block as usize).copied().flatten();
            let grows = site.what.starts_with("grows") || site.what.starts_with("changes");
            let mut outside = (grows && !made[local.0 as usize]).then(|| match unchanged(body, types, local) {
                Some((why, by, block)) => (by, scope(block), Some(why)),
                None => (span, scope(site.at.0), None),
            });
            for &(at_statement, statement) in &storing {
                if outside.is_some() {
                    break;
                }
                // A loan of one of its parts makes that part unique first,
                // which copies a shared one onto the heap, as a change does.
                if let Statement::Lend(_, lent) = statement
                    && is[lent.local.0 as usize]
                    && !lent.path.is_empty()
                {
                    outside = Some((span, scope(site.at.0), None));
                    break;
                }
                let Statement::Assign(place, rvalue, by) = statement else { continue };
                // A change inside one of its elements or parts copies that
                // element first when it is shared, as one of the program's
                // data is, onto the heap, and leaves the copy in it.
                let inside = |place: &Place| {
                    is[place.local.0 as usize]
                        && place.path.len() > 1
                        && place.path.iter().any(|p| matches!(p, Proj::Index(..) | Proj::InBounds(_) | Proj::Field(_)))
                        && !(entry_value(place) && map(place.local))
                };
                // As does a slice written back into a part, or a part a call
                // takes as a loan, through which it writes.
                let part = |operand: &Operand| matches!(operand, Operand::Copy(place) if is[place.local.0 as usize] && !place.path.is_empty());
                let deep = inside(place)
                    || match &**rvalue {
                        Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => {
                            is[target.local.0 as usize] && !target.path.is_empty()
                        }
                        Rvalue::Call(Callee::Fn(id), args) => {
                            args.iter().zip(&loans[id.0 as usize]).any(|(arg, &loan)| loan && part(arg))
                        }
                        _ => false,
                    };
                if deep {
                    outside = Some((*by, scope(at_statement.0), None));
                    break;
                }
                let stored: Vec<&Operand> = match &**rvalue {
                    Rvalue::Mutate(_, target, operands, ..) if is[target.local.0 as usize] => operands.iter().collect(),
                    // The elements of what it is given, which hold what a
                    // region made when it did, when they count.
                    Rvalue::AssignSlice(target, _, operand, _) if is[target.local.0 as usize] => {
                        let counts = matches!(operand, Operand::Copy(from)
                            if matches!(types.kind(types.repr(body.place_ty(types, from))),
                                TyKind::Array(element) | TyKind::Deque(element) if types.needs_drop(*element)));
                        if counts { vec![operand] } else { Vec::new() }
                    }
                    Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _)
                        if place.path.is_empty() && place.local == local && at_statement == site.at =>
                    {
                        operands.iter().collect()
                    }
                    // A closure's captures, which its region lets go of with
                    // it: the region must have made them.
                    Rvalue::Erase(captures, ..)
                        if place.path.is_empty() && place.local == local && at_statement == site.at =>
                    {
                        vec![captures]
                    }
                    // The arrays whose elements an array the runtime makes
                    // takes, when they count: the region must have made them.
                    Rvalue::ArrayOp(op, operands, _)
                        if op.makes_in_region() && place.local == local && at_statement == site.at =>
                    {
                        let counts = |operand: &&Operand| match operand {
                            Operand::Copy(from) => match types.kind(types.repr(body.place_ty(types, from))) {
                                TyKind::Array(element) | TyKind::Deque(element) => types.needs_drop(*element),
                                _ => false,
                            },
                            Operand::Const(_) => false,
                        };
                        operands.iter().filter(counts).collect()
                    }
                    // So does a slice of an array: its elements are the
                    // array's, which the region must have made when they
                    // count.
                    Rvalue::Slice(operand, ..) if place.local == local && at_statement == site.at => {
                        let counts = matches!(operand, Operand::Copy(from)
                            if matches!(types.kind(types.repr(body.place_ty(types, from))),
                                TyKind::Array(element) if types.needs_drop(*element)));
                        if counts { vec![operand] } else { Vec::new() }
                    }
                    Rvalue::Use(operand) if !place.path.is_empty() && is[place.local.0 as usize] => vec![operand],
                    Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _)
                        if !place.path.is_empty() && is[place.local.0 as usize] =>
                    {
                        operands.iter().collect()
                    }
                    _ => continue,
                };
                let elsewhere = stored.iter().find(
                    |operand| matches!(operand, Operand::Copy(from) if counted(from) && !made[from.local.0 as usize]),
                );
                if elsewhere.is_some() {
                    outside = Some((*by, scope(at_statement.0), None));
                    break;
                }
                // An object of the runtime, whose last reference must
                // drop it where it would on the heap: a region's value is
                // let go of all at once, and would never drop it.
                let object = stored.iter().find(
                    |operand| matches!(operand, Operand::Copy(from) if holds_object(types, body.place_ty(types, from))),
                );
                if object.is_some() {
                    let why = "it holds an object of the runtime, which a region's value would never let go of";
                    outside = Some((*by, scope(at_statement.0), Some(why.to_string())));
                    break;
                }
            }
            if let Some((by, scope, why)) = outside {
                region[at] = false;
                changed = true;
                let why = why.unwrap_or_else(|| "it holds a value that its region did not make".to_string());
                found.push((at, (why, (by, scope))));
            }
        }
        if !changed {
            return found;
        }
    }
}

/// Marks the functions whose region may last as long as the program or a
/// task, and why: `main`, the exports, which code outside the program
/// calls, and the bodies of the tasks the program starts.
fn lasting(bodies: &mut [Body], roots: &[usize], main: Option<usize>) {
    let mut tasks = Vec::new();
    for body in bodies.iter() {
        let made = |local: Local| {
            body.blocks.iter().flat_map(|block| &block.stmts).find_map(|statement| match statement {
                Statement::Assign(place, rvalue, _) if place.path.is_empty() && place.local == local => match &**rvalue
                {
                    Rvalue::Erase(_, id, _) | Rvalue::FnValue(id) => Some(id.0 as usize),
                    _ => None,
                },
                _ => None,
            })
        };
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            if let Statement::Assign(_, rvalue, _) = statement
                && let Rvalue::Spawn(Operand::Copy(task), _) = &**rvalue
                && task.path.is_empty()
                && let Some(id) = made(task.local)
            {
                tasks.push(id);
            }
        }
    }
    for id in tasks {
        let name = &bodies[id].name;
        bodies[id].lasting =
            Some(format!("it is emptied in `{name}`, the body of a task, whose region lasts as long as the task"));
    }
    for &root in roots {
        let name = &bodies[root].name;
        bodies[root].lasting = Some(match Some(root) == main {
            true => format!("it is emptied in `{name}`, whose region lasts as long as the program"),
            false => format!(
                "it is emptied in `{name}`, which code outside the program calls, and whose region may last as long \
                 as the program"
            ),
        });
    }
}

/// Of a map or an array a region would grow, the first change of it that a
/// region does not take yet, which may copy it onto the heap when it is
/// shared, as it is the heap's: why, the code, and its block.
fn unchanged(body: &Body, types: &Types, local: Local) -> Option<(String, Span, u32)> {
    let map = matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::Map(..) | TyKind::Set(_));
    let deque = matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::Deque(_));
    positioned(body).find_map(|((block, _), statement)| {
        let Statement::Assign(place, rvalue, span) = statement else { return None };
        let taken = match &**rvalue {
            Rvalue::AssignSlice(target, ..) if target.local == local => !map,
            Rvalue::Mutate(crate::hir::Mutation::Empty, target, ..) if target.local == local => {
                return body.lasting.as_ref().map(|why| (why.clone(), *span, block));
            }
            Rvalue::Mutate(crate::hir::Mutation::Clear, target, ..) if target.local == local => {
                return body.lasting.as_ref().map(|why| (why.clone(), *span, block));
            }
            Rvalue::Mutate(mutation, target, ..) if target.local == local => matches!(
                mutation,
                crate::hir::Mutation::Push
                    | crate::hir::Mutation::PushFront
                    | crate::hir::Mutation::Reserve
                    | crate::hir::Mutation::Pop
                    | crate::hir::Mutation::PopFront
                    | crate::hir::Mutation::Insert
                    | crate::hir::Mutation::Remove
            ),
            _ if place.local == local && !place.path.is_empty() => match map {
                true => entry_value(place),
                false => element(place),
            },
            _ => return None,
        };
        let set = matches!(types.kind(types.repr(body.locals[local.0 as usize])), TyKind::Set(_));
        let why = match (map, deque) {
            (true, _) if set => {
                "a region changes a set only by its insertions, its removals, `x = Set[T]()` and `x.clear()` yet"
            }
            (true, _) => {
                "a region changes a map only by its insertions, its removals, `x = [:]`, `x.clear()` and the values \
                 of its entries yet"
            }
            (_, true) => {
                "a region changes a deque only by `push`, `pushFront`, `pop`, `popFront`, `x = Deque[T]()`, \
                 `x.clear()` and the writes of its elements yet"
            }
            _ => {
                "a region changes an array only by `push`, `pop`, `x = []`, `x.clear()` and the writes of its \
                 elements yet"
            }
        };
        (!taken).then(|| (why.to_string(), *span, block))
    })
}

/// The locals whose values a region made, or that count nothing: each
/// value written whole into them is a place a region takes, a result of a
/// call `call_made` says a region makes, a constant, or a part or a copy of
/// such a value.
fn made(
    flow: &Flow,
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    sites: &[Site],
    region: &[bool],
    call_made: &dyn Fn((u32, u32), usize) -> bool,
) -> Vec<bool> {
    let counted = |local: Local| flow.counted[local.0 as usize];
    let mut made = vec![true; body.locals.len()];
    for &param in &body.params {
        made[param.0 as usize] = !counted(param);
    }
    debug_assert!(sites.windows(2).all(|pair| pair[0].at < pair[1].at));
    let mut changed = true;
    while changed {
        changed = false;
        // The site of each statement, as both are in the same order.
        let mut next = 0;
        for (at, statement) in positioned(body) {
            while sites.get(next).is_some_and(|site| site.at < at) {
                next += 1;
            }
            let here = sites.get(next).filter(|site| site.at == at);
            let taken = |local: Local| here.is_some_and(|site| site.local == local && region[next]);
            // What may make a local hold an array of the heap: a change of
            // it, which copies it first when it is shared and may grow it,
            // unless a region grows it; and a write of one of its elements,
            // which copies it first too.
            let changed_here = match statement {
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    // The emptying of an array to fill again, which empties
                    // it in place or gives it the constant empty array, as
                    // `[]` does, and copies nothing onto the heap.
                    Rvalue::Mutate(crate::hir::Mutation::Empty, ..) if body.lasting.is_none() => None,
                    // So does `clear()`, which empties a collection in place,
                    // keeping its room, or gives it the constant empty one.
                    Rvalue::Mutate(crate::hir::Mutation::Clear, target, ..)
                        if body.lasting.is_none()
                            && matches!(
                                types.kind(types.repr(body.locals[target.local.0 as usize])),
                                TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_)
                            ) =>
                    {
                        None
                    }
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => {
                        Some((target.local, taken(target.local)))
                    }
                    _ => None,
                },
                _ => None,
            }
            .or(match statement {
                Statement::Assign(place, ..)
                    if place
                        .path
                        .iter()
                        .any(|projection| matches!(projection, Proj::Index(..) | Proj::InBounds(_))) =>
                {
                    Some((place.local, taken(place.local)))
                }
                _ => None,
            });
            if let Some((target, kept)) = changed_here
                && !kept
                && made[target.0 as usize]
                && counted(target)
            {
                made[target.0 as usize] = false;
                changed = true;
            }
            // A call that takes a local as a loan may leave anything in it.
            if let Statement::Assign(_, rvalue, _) = statement {
                let mut lend = |arg: &Operand| {
                    if let Operand::Copy(place) = arg
                        && made[place.local.0 as usize]
                        && counted(place.local)
                    {
                        made[place.local.0 as usize] = false;
                        changed = true;
                    }
                };
                match &**rvalue {
                    Rvalue::Call(Callee::Fn(id), args) => {
                        args.iter().zip(&loans[id.0 as usize]).filter(|&(_, &loan)| loan).for_each(|(arg, _)| lend(arg))
                    }
                    // The runtime reads what it is given; C may take anything
                    // as a loan, and a function value what its type lends.
                    Rvalue::Call(Callee::Std(_), _) => {}
                    Rvalue::CallIndirect(callee, args) => {
                        let modes = match callee {
                            Operand::Copy(place) => match types.kind(types.repr(body.place_ty(types, place))) {
                                TyKind::Fn(_, _, _, modes, _) => Some(modes.clone()),
                                _ => None,
                            },
                            Operand::Const(_) => None,
                        };
                        for (at, arg) in args.iter().enumerate() {
                            let lent = modes.as_ref().is_none_or(|modes| {
                                modes
                                    .get(at)
                                    .is_none_or(|mode| matches!(mode, crate::ast::Mode::Mut | crate::ast::Mode::Var))
                            });
                            if lent {
                                lend(arg);
                            }
                        }
                    }
                    Rvalue::Call(_, args) => args.iter().for_each(lend),
                    _ => {}
                }
            }
            let (into, from_region) = match statement {
                Statement::Assign(place, rvalue, _) if place.path.is_empty() && counted(place.local) => {
                    let from = |operand: &Operand| match operand {
                        Operand::Copy(from) => made[from.local.0 as usize],
                        Operand::Const(_) => true,
                    };
                    // A place that makes a value on the heap, unless a
                    // region takes it, whatever its parts are.
                    let site = here.is_some_and(|site| site.local == place.local);
                    let from_region = taken(place.local)
                        || !site
                            && match &**rvalue {
                                Rvalue::Use(operand) => from(operand),
                                Rvalue::Call(Callee::Fn(id), _) => call_made(at, id.0 as usize),
                                Rvalue::Aggregate(operands)
                                | Rvalue::Variant(_, operands)
                                | Rvalue::Array(operands, _) => operands.iter().all(from),
                                // The runtime's empty map, and the program's empty
                                // array or deque, which count nothing.
                                Rvalue::MapOp(crate::hir::MapOp::New, ..)
                                | Rvalue::ArrayOp(crate::hir::ArrayOp::New, ..) => true,
                                _ => false,
                            };
                    (place.local, from_region)
                }
                Statement::Lend(alias, place) => (*alias, made[place.local.0 as usize]),
                Statement::LendAt(alias, _) => (*alias, false),
                _ => continue,
            };
            if !from_region && made[into.0 as usize] {
                made[into.0 as usize] = false;
                changed = true;
            }
        }
    }
    made
}

/// What the calls of a function that returns what it makes do with it: the
/// calls that let it go and those that keep it, through the callers that
/// return it in turn, each once.
fn resolve(
    index: usize,
    summaries: &[Summary],
    seen: &mut [bool],
    drops: &mut Vec<(usize, Span)>,
    keeps: &mut Vec<(String, Span)>,
) {
    if std::mem::replace(&mut seen[index], true) {
        return;
    }
    if let Some(why) = summaries[index].unseen {
        keeps.push((why.to_string(), Span::default()));
    }
    for call in &summaries[index].calls {
        match &call.taken {
            Taken::Drops => drops.push((call.caller, call.written)),
            Taken::Keeps(who, at) => keeps.push((who.clone(), *at)),
            Taken::Passes => resolve(call.caller, summaries, seen, drops, keeps),
        }
    }
}

/// The statements of a body with their block and position.
fn positioned(body: &Body) -> impl Iterator<Item = ((u32, u32), &Statement)> {
    body.blocks.iter().enumerate().flat_map(|(block, data)| {
        data.stmts.iter().enumerate().map(move |(index, statement)| ((block as u32, index as u32), statement))
    })
}

/// A place that makes a value on the heap.
struct Site {
    /// The local that holds the value.
    local: Local,
    span: Span,
    /// What it is in words, before "goes" or "stays".
    what: &'static str,
    /// Its statement, by block and position.
    at: (u32, u32),
    /// Why a region does not take it yet, when it does not: one takes an
    /// array, a box and an array that grows by `push`.
    refused: Option<&'static str>,
}

impl Site {
    fn placed(&self) -> bool {
        self.refused.is_none()
    }
}

/// Whether a place is the standard library's code or a dependency's.
fn library(sources: &crate::source::SourceMap, span: Span) -> bool {
    let file = sources.file_of(span.start);
    file.dependency || file.logical.starts_with("<std>/")
}

/// Where the line of a place is told: at its place, or, of the library's
/// code copied into `body`, at the program's call of the copy it is of.
fn told_at(body: &Body, site: &Site, sources: &crate::source::SourceMap) -> Span {
    if !library(sources, site.span) {
        return site.span;
    }
    let mut scope = body.scopes.get(site.at.0 as usize).copied().flatten();
    while let Some(at) = scope {
        let Some(inlined) = body.inlined.get(at as usize) else { break };
        if !library(sources, inlined.call) {
            return inlined.call;
        }
        scope = inlined.parent;
    }
    site.span
}

/// Whether what a place of the library's code copied into `body` makes
/// reaches the program's own code: a statement of the program's holds or
/// reads it, or the function returns it.
fn reaches_program(flow: &Flow, body: &Body, types: &Types, site: &Site, library: &impl Fn(Span) -> bool) -> bool {
    let held = flow.holders(types, body, site.local, flow.called[site.local.0 as usize]);
    if body.ret.is_some_and(|ret| held[ret.0 as usize]) {
        return true;
    }
    body.blocks.iter().flat_map(|block| &block.stmts).any(|statement| {
        let Statement::Assign(place, rvalue, span) = statement else { return false };
        // What the inliner moves between its copies is no one's code.
        if library(*span) || *span == Span::default() {
            return false;
        }
        let mut used = vec![place.local];
        rvalue_uses(rvalue, &mut used);
        used.iter().any(|local| held[local.0 as usize])
    })
}

/// Whether an operation of numbers gives a text, which the runtime makes in
/// the region it is given.
fn numbers_text(op: crate::numbers::NumOp) -> bool {
    matches!(op, crate::numbers::NumOp::ToString | crate::numbers::NumOp::ToFixed)
}

/// Whether a text operation always makes a new text, which the runtime
/// makes in the region it is given.
fn makes_text(op: crate::hir::StrOp) -> bool {
    use crate::hir::StrOp::*;
    matches!(op, Replace | LowerAscii | UpperAscii | FromChars | CopyOut)
}

/// Whether a place is the value of an entry of the collection its local
/// holds, written in place, `m.in_bounds(p).field(1)`: the entry is the
/// collection's own memory, which a change of its value leaves where it is.
fn entry_value(place: &Place) -> bool {
    matches!(place.path.as_slice(), [Proj::Index(..) | Proj::InBounds(_), Proj::Field(1)])
}

/// Whether a place is an element of the array its local holds, written in
/// place whole, `a[i] = x`.
fn element(place: &Place) -> bool {
    matches!(place.path.as_slice(), [Proj::Index(..) | Proj::InBounds(_)])
}

/// The places the function makes a value on the heap in: an array of
/// values that are not all constants, a map, a slice, a box, an array that
/// grows, a text, and the arrays, texts and other values the runtime gives.
fn sites(body: &Body, types: &Types) -> Vec<Site> {
    let mut found = Vec::new();
    let array =
        |place: &Place| matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::Array(_));
    let deque =
        |place: &Place| matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::Deque(_));
    const RUNTIME: Option<&str> = Some("regions do not take what the runtime makes yet");
    let kind = |local: Local| types.kind(types.repr(body.locals[local.0 as usize])).clone();
    for (at, statement) in positioned(body) {
        let Statement::Assign(place, rvalue, span) = statement else { continue };
        // A change of an entry of a map in place, as `m[k, default: 0] += 1`
        // makes, or of an element of an array, `a[i] = x`: the collection,
        // made unique first when it is shared, goes where its place goes, as
        // one an insertion or a push grows.
        let what = match kind(place.local) {
            crate::types::TyKind::Map(..) if entry_value(place) => "changes a map that ",
            crate::types::TyKind::Array(_) if element(place) => "changes an array that ",
            crate::types::TyKind::Deque(_) if element(place) => "changes a deque that ",
            _ => "",
        };
        if !what.is_empty() {
            found.push(Site { local: place.local, span: *span, what, at, refused: None });
            continue;
        }
        let refused = match &**rvalue {
            Rvalue::Array(values, _) if values.iter().any(|value| matches!(value, Operand::Copy(_))) => None,
            // The empty map is the runtime's, which nothing makes: the
            // first insertion makes the map, where its place goes.
            Rvalue::MapOp(crate::hir::MapOp::New, ..) => continue,
            // A slice, which copies its part, in the region of its place.
            Rvalue::Slice(..) => None,
            // A function's value a closure makes, which the frame does not
            // keep: as a box, where its holder's place goes.
            Rvalue::Erase(..) if !body.framed.contains(&place.local) => None,
            // A text interpolation makes, `x = "{x}..."` too, which grows
            // `x`'s own text in the region, its room doubling.
            Rvalue::Concat(_) if !body.framed.contains(&place.local) => None,
            Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) => {
                let ty = body.place_ty(types, place);
                let boxed = operands.iter().any(|operand| match operand {
                    Operand::Copy(part) => types.is_boxed(ty, body.place_ty(types, part)),
                    Operand::Const(_) => false,
                });
                if !boxed {
                    continue;
                }
                None
            }
            Rvalue::Mutate(
                mutation @ (crate::hir::Mutation::Push
                | crate::hir::Mutation::PushFront
                | crate::hir::Mutation::InsertAt),
                target,
                ..,
            ) => {
                // A deque grows at either end, in the region of its place.
                let (what, refused) = match (array(target), mutation) {
                    (true, crate::hir::Mutation::Push) => ("grows an array that ", None),
                    (true, _) => ("grows an array that ", Some("a region grows an array only at its end yet")),
                    (false, crate::hir::Mutation::InsertAt) => {
                        ("grows a deque that ", Some("a region grows a deque only at its ends yet"))
                    }
                    (false, _) => ("grows a deque that ", None),
                };
                found.push(Site { local: target.local, span: *span, what, at, refused });
                continue;
            }
            // Room a loop is about to push into, which goes where its
            // pushes go.
            Rvalue::Mutate(crate::hir::Mutation::Reserve, target, ..) if array(target) => {
                found.push(Site { local: target.local, span: *span, what: "grows an array that ", at, refused: None });
                continue;
            }
            Rvalue::Mutate(crate::hir::Mutation::Insert, target, ..) => {
                found.push(Site { local: target.local, span: *span, what: "grows a map that ", at, refused: None });
                continue;
            }
            // A removal, which takes nothing in, or a slice written back, as
            // `sort()` and `reverse()` do: the collection, made unique first
            // when it is shared, goes where its place goes, as one an
            // insertion or a push grows.
            Rvalue::AssignSlice(target, ..)
            | Rvalue::Mutate(crate::hir::Mutation::Pop | crate::hir::Mutation::PopFront, target, ..)
                if deque(target) && target.path.is_empty() =>
            {
                found.push(Site { local: target.local, span: *span, what: "changes a deque that ", at, refused: None });
                continue;
            }
            Rvalue::AssignSlice(target, ..) | Rvalue::Mutate(crate::hir::Mutation::Pop, target, ..)
                if array(target) && target.path.is_empty() =>
            {
                found.push(Site {
                    local: target.local,
                    span: *span,
                    what: "changes an array that ",
                    at,
                    refused: None,
                });
                continue;
            }
            Rvalue::Mutate(crate::hir::Mutation::Remove, target, ..)
                if target.path.is_empty()
                    && matches!(kind(target.local), crate::types::TyKind::Map(..) | crate::types::TyKind::Set(_)) =>
            {
                found.push(Site { local: target.local, span: *span, what: "changes a map that ", at, refused: None });
                continue;
            }
            // What a native makes in the region it is given, or else what
            // the runtime makes on the heap.
            Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                if crate::hir::NATIVES.get(*index as usize).is_some_and(crate::hir::Native::makes_in_region) =>
            {
                None
            }
            // A text the runtime keeps for good, which nothing makes.
            Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                if crate::hir::NATIVES.get(*index as usize).is_some_and(crate::hir::Native::gives_kept) =>
            {
                continue;
            }
            // What an operation of the library makes in the region it is
            // given, as `path.join` or `fs.readText` does, and the text of
            // a number.
            Rvalue::Call(Callee::Std(function), _) if function.makes_in_region() => None,
            Rvalue::Call(Callee::Std(crate::hir::StdFn::Number(op)), _) if numbers_text(*op) => None,
            // A spare the runtime keeps, which costs the heap nothing.
            Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                if crate::hir::NATIVES.get(*index as usize).is_some_and(|native| {
                    matches!(native.result, crate::hir::NativeTy::JsonReader | crate::hir::NativeTy::JsonWriter)
                }) =>
            {
                Some("the runtime gives a spare JSON reader or writer it keeps, which costs the heap no allocation")
            }
            Rvalue::Call(Callee::Std(_), _) if types.needs_drop(body.place_ty(types, place)) => RUNTIME,
            // The text `join` makes, which its builder makes in a region.
            Rvalue::ArrayOp(crate::hir::ArrayOp::Join, ..) => None,
            // An empty array, the program's data, which the first change
            // makes one of its own, where its growth says.
            Rvalue::ArrayOp(crate::hir::ArrayOp::New, ..) => continue,
            // An array the runtime makes, as `concat` does, in the region it
            // is given.
            Rvalue::ArrayOp(op, ..) if op.makes_in_region() => None,
            // The text `slice(start, end)` copies, the success of its result,
            // in the region it is given.
            Rvalue::StrOp(crate::hir::StrOp::SliceChecked, ..) => None,
            // The text a function returns, copied out (`copied_out`).
            Rvalue::StrOp(crate::hir::StrOp::CopyOut, ..) => {
                let what = "is copied out at its return, and ";
                found.push(Site { local: place.local, span: *span, what, at, refused: None });
                continue;
            }
            // Not an element read out of one.
            Rvalue::ArrayOp(op, ..)
                if *op != crate::hir::ArrayOp::Get && types.needs_drop(body.place_ty(types, place)) =>
            {
                RUNTIME
            }
            // A text an operation makes, as `replace` or `toUpper` does: in
            // a region when the runtime makes it there. The text a loop reads
            // in place (`InPlace`) is the same text, which makes nothing.
            Rvalue::StrOp(op, ..)
                if makes_text(*op)
                    && matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::String) =>
            {
                None
            }
            _ => continue,
        };
        if place.path.is_empty() {
            found.push(Site { local: place.local, span: *span, what: "", at, refused });
        }
    }
    found
}

/// How values pass from local to local in a function, read once from its
/// statements: what `holders` follows from a value, visiting only what it
/// reaches; and what `fate` asks of every value.
pub(crate) struct Flow {
    transfers: Vec<Transfer>,
    /// The transfers that read each local.
    readers: Vec<Vec<u32>>,
    counted: Vec<bool>,
    /// Whether each local is, or leads to, what outlives the call.
    outside: Vec<Outside>,
    /// The closures `spawn` runs.
    spawned: Vec<Local>,
    /// Whether each local is a parameter or takes a call's result, which
    /// `fate` follows through its parts.
    called: Vec<bool>,
    /// Whether each local takes the result, `T ! E`, of an operation of the
    /// library that makes its value in a region, whose region holds only
    /// the success: the error the runtime gives is made of none of it.
    succeeds: Vec<bool>,
    /// The statements that read each local, as an assignment does, or put
    /// it in a `defer`, by their index in the body's order.
    users: Vec<Vec<u32>>,
    /// Each statement's block and position, by that index.
    at: Vec<(u32, u32)>,
    /// The functions of the program it calls by name, and those it takes
    /// as a value, whose calls are not seen.
    callees: Vec<usize>,
    valued: Vec<usize>,
    /// The locals that take the result of a call of the program's, with
    /// the block, the statement, the callee and the call.
    results: Vec<(usize, usize, Local, usize, Span)>,
}

/// What one statement passes to a local.
struct Transfer {
    into: Local,
    /// Whether it writes a part of `into`, which then contains it.
    part: bool,
    how: How,
    /// What it reads, whether through a path, a part of the local, and the
    /// variant of a union the path reads first, as the error a `?` returns.
    from: Vec<(Local, bool, Option<u32>)>,
    /// The variant of a union it makes `into`, which holds what it reads
    /// in that variant alone.
    variant: Option<u32>,
}

/// A variant's bit in a set of them, all of them past 64.
fn variant_bit(variant: u32) -> u64 {
    1u64.checked_shl(variant).unwrap_or(u64::MAX)
}

/// The variant of a union a place reads first, if it reads one.
fn variant_read(place: &Place) -> Option<u32> {
    match place.path.first() {
        Some(Proj::Variant(variant, _)) => Some(*variant),
        _ => None,
    }
}

/// How a statement passes what it reads.
enum How {
    /// A copy or a part, the value written whole.
    Whole,
    /// Into a value made of what it reads.
    Made,
    /// What grows: what it reads is put in it.
    Grows,
    /// What another operation gives: the value when it is of its type, or
    /// what contains it when its type can refer to it, or `anything` can: a
    /// function value or a `reflect.Value`'s box, as `reflect.valueOf`
    /// makes, may hold a value of any type.
    Other { same: Ty, reach: Vec<Ty>, anything: bool },
    /// A loan of a place.
    Lent,
}

impl Flow {
    /// Carries what each call of the program's is given into what it gives,
    /// when `summaries` says the callee may give it back: the result may be
    /// it, or contain it, so that what holds the result holds it too.
    fn through(&mut self, body: &Body, summaries: &[Summary]) {
        for &(block, at, result, callee, _) in &self.results {
            let returns = &summaries[callee].returns;
            let Some(Statement::Assign(_, rvalue, _)) = body.blocks[block].stmts.get(at) else { continue };
            let Rvalue::Call(Callee::Fn(_), args) = &**rvalue else { continue };
            let from: Vec<(Local, bool, Option<u32>)> = args
                .iter()
                .zip(returns)
                .filter(|&(_, &returns)| returns)
                .filter_map(|(arg, _)| match arg {
                    Operand::Copy(place) => Some((place.local, !place.path.is_empty(), variant_read(place))),
                    Operand::Const(_) => None,
                })
                .collect();
            if from.is_empty() {
                continue;
            }
            for how in [How::Whole, How::Grows] {
                let index = self.transfers.len() as u32;
                for &(local, ..) in &from {
                    self.readers[local.0 as usize].push(index);
                }
                self.transfers.push(Transfer { into: result, part: false, how, from: from.clone(), variant: None });
            }
        }
    }

    pub(crate) fn of(body: &Body, types: &Types) -> Flow {
        let mut reached: crate::hash::FastMap<Ty, (Ty, Vec<Ty>, bool)> = crate::hash::FastMap::default();
        let mut transfers = Vec::new();
        let copies = |operands: &[Operand]| -> Vec<(Local, bool, Option<u32>)> {
            operands
                .iter()
                .filter_map(|operand| match operand {
                    Operand::Copy(place) => Some((place.local, !place.path.is_empty(), variant_read(place))),
                    Operand::Const(_) => None,
                })
                .collect()
        };
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    let (into, part) = (place.local, !place.path.is_empty());
                    let variant = match &**rvalue {
                        Rvalue::Variant(variant, _) => Some(*variant),
                        // The array the runtime makes is the success of its
                        // result, when it has one, as a slice's does.
                        Rvalue::ArrayOp(op, ..)
                            if op.makes_in_region()
                                && matches!(
                                    types.kind(types.repr(body.place_ty(types, place))),
                                    TyKind::Result(..)
                                ) =>
                        {
                            Some(0)
                        }
                        Rvalue::StrOp(crate::hir::StrOp::SliceChecked, ..) => Some(0),
                        _ => None,
                    };
                    let (how, from) = match &**rvalue {
                        Rvalue::Use(operand) => (How::Whole, copies(std::slice::from_ref(operand))),
                        Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _) => {
                            (How::Made, copies(operands))
                        }
                        // What an interpolation takes over, and the text it
                        // shows when it shows one alone, which it gives as it
                        // is (`concat` in codegen): it copies the rest.
                        Rvalue::Concat(pieces) => {
                            let alone = match pieces.as_slice() {
                                [Piece::Value(Operand::Copy(shown), ty)] if types.repr(*ty) == Ty::STRING => {
                                    Some((shown.local, !shown.path.is_empty(), variant_read(shown)))
                                }
                                _ => None,
                            };
                            let taken = pieces
                                .iter()
                                .filter_map(|piece| match piece {
                                    Piece::Taken(taken) => {
                                        Some((taken.local, !taken.path.is_empty(), variant_read(taken)))
                                    }
                                    _ => None,
                                })
                                .chain(alone)
                                .collect();
                            (How::Whole, taken)
                        }
                        Rvalue::Mutate(_, target, operands, ..) => {
                            transfers.push(Transfer {
                                into: target.local,
                                part: true,
                                how: How::Grows,
                                from: copies(operands),
                                variant: None,
                            });
                            continue;
                        }
                        // A slice written in place puts the elements of
                        // what it is given in what it names, as growing does.
                        Rvalue::AssignSlice(target, _, operand, _) => {
                            transfers.push(Transfer {
                                into: target.local,
                                part: true,
                                how: How::Grows,
                                from: copies(std::slice::from_ref(operand)),
                                variant: None,
                            });
                            continue;
                        }
                        // A call of the program's keeps what it returns of
                        // what it is given, which `fate` sees.
                        Rvalue::Call(Callee::Fn(_), _) => continue,
                        // A text operation that makes a new text gives one
                        // that holds none of what it is given, as a slice of
                        // a text does.
                        Rvalue::StrOp(op, ..) if makes_text(*op) || *op == crate::hir::StrOp::SliceChecked => continue,
                        Rvalue::Slice(..)
                            if matches!(types.kind(types.repr(body.place_ty(types, place))), TyKind::String) =>
                        {
                            continue;
                        }
                        // A slice of an array, a new array of its elements,
                        // which holds none of it but what they hold.
                        Rvalue::Slice(operand, ..) => {
                            let elements = copies(std::slice::from_ref(operand))
                                .into_iter()
                                .map(|(local, _, variant)| (local, true, variant));
                            (How::Made, elements.collect())
                        }
                        Rvalue::ArrayOp(crate::hir::ArrayOp::Join, ..) => continue,
                        // A new array of the elements of those it is given,
                        // which holds none of them but what they hold.
                        Rvalue::ArrayOp(op, operands, _) if op.makes_in_region() => {
                            let elements =
                                copies(operands).into_iter().map(|(local, _, variant)| (local, true, variant));
                            (How::Made, elements.collect())
                        }
                        Rvalue::Call(Callee::Std(function), _) if function.makes_in_region() => continue,
                        Rvalue::Call(Callee::Std(crate::hir::StdFn::Number(op)), _) if numbers_text(*op) => continue,
                        // A native that reads what it is given gives a value
                        // the runtime made, which holds none of it: one that
                        // reads arrays says so of all it gives; one that
                        // reads only texts, of a value of the runtime, as
                        // a text it gives may be one it was given.
                        Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                            if crate::hir::NATIVES.get(*index as usize).is_some_and(|native| {
                                use crate::hir::NativeTy::{BytePointer, Object, Owned, Regex};
                                let texts_read = !takes_texts(native) || READING_TEXTS.contains(&native.symbol);
                                let made = matches!(native.result, Object | Owned | Regex | BytePointer);
                                native.makes_in_region()
                                    || native.gives_kept()
                                    || texts_read
                                        && (READING.contains(&native.symbol)
                                            || made && takes_texts(native) && !takes_from_regions(native))
                            }) =>
                        {
                            continue;
                        }
                        // What any other operation gives: the value, or what
                        // holds it, as its type can be or refer to it.
                        Rvalue::Unary(..)
                        | Rvalue::Binary(..)
                        | Rvalue::Increment(_)
                        | Rvalue::Exact(..)
                        | Rvalue::Call(..)
                        | Rvalue::FnValue(_)
                        | Rvalue::ExternFn(_)
                        | Rvalue::Callback(..)
                        | Rvalue::Layout(..)
                        | Rvalue::Reflect(..)
                        | Rvalue::Fixed(..)
                        | Rvalue::CallIndirect(..)
                        | Rvalue::Erase(..)
                        | Rvalue::Spawn(..)
                        | Rvalue::TestMembers(_)
                        | Rvalue::TaskOp(..)
                        | Rvalue::Select(..)
                        | Rvalue::Cancelled
                        | Rvalue::Unwinding
                        | Rvalue::Restart(_)
                        | Rvalue::Transfer(_)
                        | Rvalue::CallOsr(..)
                        | Rvalue::OsrPoint
                        | Rvalue::Discriminant(_)
                        | Rvalue::Convert(_)
                        | Rvalue::ArrayOp(..)
                        | Rvalue::CheckRegion(..)
                        | Rvalue::Length(_)
                        | Rvalue::StrOp(..)
                        | Rvalue::Pointer(..)
                        | Rvalue::MapOp(..) => {
                            let mut used = Vec::new();
                            rvalue_uses(rvalue, &mut used);
                            let ty = body.place_ty(types, place);
                            let (same, reach, anything) = reached
                                .entry(ty)
                                .or_insert_with(|| {
                                    let all = types.reach(ty);
                                    // A function value, a reflected value or an
                                    // object of the runtime may hold anything
                                    // it was given.
                                    let anything = all.iter().any(|&part| match types.kind(part) {
                                        TyKind::Fn(..) | TyKind::Handle(Handle::ReflectBox, _) => true,
                                        TyKind::Handle(handle, _) => runtime_object(*handle),
                                        _ => false,
                                    });
                                    let reach = all.into_iter().skip(1).map(|part| types.repr(part)).collect();
                                    (types.repr(ty), reach, anything)
                                })
                                .clone();
                            let how = How::Other { same, reach, anything };
                            (how, used.into_iter().map(|local| (local, false, None)).collect())
                        }
                    };
                    if !from.is_empty() {
                        transfers.push(Transfer { into, part, how, from, variant });
                    }
                }
                Statement::Lend(alias, place) => {
                    transfers.push(Transfer {
                        into: *alias,
                        part: false,
                        how: How::Lent,
                        from: vec![(place.local, !place.path.is_empty(), variant_read(place))],
                        variant: None,
                    });
                    // What is written through the loan is written into what
                    // it lends: `row.values = [..]` in `for mut row in rows`
                    // stores the array in `rows`.
                    transfers.push(Transfer {
                        into: place.local,
                        part: !place.path.is_empty(),
                        how: How::Whole,
                        from: vec![(*alias, false, None)],
                        variant: None,
                    });
                }
                // Counts, cleanups, zeroings, and a lock's state, which
                // `outside` follows: none passes a value on.
                Statement::Retain(_)
                | Statement::Release(_)
                | Statement::DeferPush(..)
                | Statement::DeferPop
                | Statement::Clear(_)
                | Statement::Moved(_)
                | Statement::LendAt(..)
                | Statement::OsrLoad(_)
                | Statement::Failing(..) => {}
            }
        }
        let mut readers = vec![Vec::new(); body.locals.len()];
        for (index, transfer) in transfers.iter().enumerate() {
            for &(local, ..) in &transfer.from {
                if readers[local.0 as usize].last() != Some(&(index as u32)) {
                    readers[local.0 as usize].push(index as u32);
                }
            }
        }
        let counted = body.locals.iter().map(|&ty| types.needs_drop(ty)).collect();
        let spawned = body
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
        let mut called = vec![false; body.locals.len()];
        let mut succeeds = vec![false; body.locals.len()];
        for &param in &body.params {
            called[param.0 as usize] = true;
        }
        let (mut users, mut at) = (vec![Vec::new(); body.locals.len()], Vec::new());
        let (mut callees, mut valued, mut results) = (Vec::new(), Vec::new(), Vec::new());
        let mut used = Vec::new();
        for (position, statement) in positioned(body) {
            let index = at.len() as u32;
            at.push(position);
            used.clear();
            match statement {
                Statement::Assign(place, rvalue, span) => {
                    rvalue_uses(rvalue, &mut used);
                    match &**rvalue {
                        Rvalue::Call(Callee::Fn(id), _) => {
                            callees.push(id.0 as usize);
                            if place.path.is_empty() {
                                called[place.local.0 as usize] = true;
                                let (block, at) = (position.0 as usize, position.1 as usize);
                                results.push((block, at, place.local, id.0 as usize, *span));
                            }
                        }
                        // What a native makes in a region, its parts too, as
                        // the texts of an array `split` gives: followed
                        // through its parts as a call's result is.
                        Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                            if place.path.is_empty()
                                && crate::hir::NATIVES
                                    .get(*index as usize)
                                    .is_some_and(crate::hir::Native::makes_in_region) =>
                        {
                            called[place.local.0 as usize] = true;
                        }
                        // And what an operation of the library makes so, as
                        // the names `fs.list` gives.
                        Rvalue::Call(Callee::Std(function), _)
                            if place.path.is_empty() && function.makes_in_region() =>
                        {
                            called[place.local.0 as usize] = true;
                            let ty = body.place_ty(types, place);
                            succeeds[place.local.0 as usize] = matches!(types.kind(types.repr(ty)), TyKind::Result(..));
                        }
                        // An array the runtime makes as the success of a
                        // result, as a slice's, which is a part of it, followed
                        // as a call's result is: its error, out of bounds,
                        // holds none of it.
                        Rvalue::ArrayOp(op, ..) if place.path.is_empty() && op.makes_in_region() => {
                            let ty = body.place_ty(types, place);
                            let result = matches!(types.kind(types.repr(ty)), TyKind::Result(..));
                            called[place.local.0 as usize] |= result;
                            succeeds[place.local.0 as usize] = result;
                        }
                        // And the text a slice of one copies, so.
                        Rvalue::StrOp(crate::hir::StrOp::SliceChecked, ..) if place.path.is_empty() => {
                            called[place.local.0 as usize] = true;
                            succeeds[place.local.0 as usize] = true;
                        }
                        Rvalue::FnValue(id)
                        | Rvalue::Callback(id, _)
                        | Rvalue::Erase(_, id, _)
                        | Rvalue::CallOsr(id, ..) => valued.push(id.0 as usize),
                        _ => {}
                    }
                }
                Statement::DeferPush(id, place) => {
                    used.push(place.local);
                    valued.push(id.0 as usize);
                }
                _ => {}
            }
            for local in &used {
                let users: &mut Vec<u32> = &mut users[local.0 as usize];
                if users.last() != Some(&index) {
                    users.push(index);
                }
            }
        }
        Flow {
            transfers,
            readers,
            counted,
            outside: outside(body),
            spawned,
            called,
            succeeds,
            users,
            at,
            callees,
            valued,
            results,
        }
    }

    /// The locals that hold what `start` holds: that are it, a copy of it,
    /// or that contain it, as an array it was pushed into or a record made
    /// of it, as copies, parts or values made of it pass it on. A part read
    /// out of the value itself is another value, which it does not hold,
    /// unless `parts` says its parts are its memory; one read out of what
    /// contains it may be it. What another operation gives, as the
    /// runtime's `trim` that may give back the text it was lent, may be it
    /// when it is of its type, and contain it when its type can refer to it.
    fn holders(&self, types: &Types, body: &Body, start: Local, parts: bool) -> Vec<bool> {
        self.holders_in(types, body, start, parts, self.start_variants(start)).0
    }

    /// The variants of `start`, a union, that hold what it makes: the
    /// success of an operation of the library's result, or all.
    fn start_variants(&self, start: Local) -> u64 {
        if self.succeeds[start.0 as usize] { variant_bit(0) } else { u64::MAX }
    }

    /// The locals that hold what `start` holds in the variants `held_in` of
    /// it, a union, and the variants of each that may hold it.
    fn holders_in(&self, types: &Types, body: &Body, start: Local, parts: bool, held_in: u64) -> (Vec<bool>, Vec<u64>) {
        let count = self.readers.len();
        let (mut is, mut contains) = (vec![false; count], vec![false; count]);
        is[start.0 as usize] = true;
        let held = types.repr(body.locals[start.0 as usize]);
        // With its parts, what it is made of: a value that holds one of
        // them holds the memory they share, as a field of a call's result.
        let made_of: Vec<Ty> = match parts {
            true => types.reach(held).into_iter().map(|part| types.repr(part)).collect(),
            false => Vec::new(),
        };
        // The variants of each local, a union, that may be or hold it: the
        // one a transfer made it, or all. The success alone of what an
        // operation of the library made, so that the error a `?` returns
        // takes none of the text it read.
        let mut variants = vec![0u64; count];
        variants[start.0 as usize] = held_in;
        let mut pending: Vec<u32> = self.readers[start.0 as usize].clone();
        while let Some(index) = pending.pop() {
            let transfer = &self.transfers[index as usize];
            let (mut may_be, mut may_contain) = (false, false);
            for &(from, through, read) in &transfer.from {
                if read.is_some_and(|read| variants[from.0 as usize] & variant_bit(read) == 0) {
                    continue;
                }
                let (it, inside) = (is[from.0 as usize], contains[from.0 as usize]);
                match (&transfer.how, through) {
                    (How::Whole, false) => {
                        may_be |= it;
                        may_contain |= inside;
                    }
                    (How::Whole, true) => {
                        may_be |= inside;
                        may_contain |= inside || (it && parts);
                    }
                    (How::Made, false) => may_contain |= inside || it,
                    (How::Made, true) => may_contain |= inside || (it && parts),
                    (How::Grows, _) => may_contain |= it || inside,
                    (How::Other { same, reach, anything }, _) => {
                        if it || inside {
                            may_be |= *same == held;
                            may_contain |= *anything
                                || reach.contains(&held)
                                || reach.iter().chain([same]).any(|ty| made_of.contains(ty));
                        }
                    }
                    (How::Lent, false) => {
                        may_be |= it;
                        may_contain |= inside;
                    }
                    (How::Lent, true) => may_contain |= inside || it,
                }
            }
            if transfer.part {
                may_contain |= may_be;
                may_be = false;
            }
            let into = transfer.into.0 as usize;
            if !self.counted[into] {
                continue;
            }
            let mut grew = false;
            if may_be && !is[into] {
                is[into] = true;
                grew = true;
            }
            if may_contain && !contains[into] {
                contains[into] = true;
                grew = true;
            }
            let made = match transfer.variant {
                Some(variant) if !transfer.part => variant_bit(variant),
                _ => u64::MAX,
            };
            if (may_be || may_contain) && variants[into] | made != variants[into] {
                variants[into] |= made;
                grew = true;
            }
            if grew {
                pending.extend(&self.readers[into]);
            }
        }
        ((0..count).map(|index| is[index] || contains[index]).collect(), variants)
    }
}

/// What becomes of what `start` holds in `body`. A parameter and the
/// result of a call are followed through their parts too: a caller may
/// give a value of its region, and the copy of a function that makes its
/// result in a region the caller gives makes its parts there, places this
/// function does not see.
fn fate(flow: &Flow, body: &Body, types: &Types, start: Local, summaries: &[Summary], bodies: &[Body]) -> Fate {
    fate_from(flow, body, types, start, None, summaries, bodies)
}

/// What becomes of the value a parameter was given: as `fate`, but a
/// parameter changed in place, as `mut self` is, still holds only what it
/// was given, which stays where it was.
fn given_fate(flow: &Flow, body: &Body, types: &Types, param: Local, summaries: &[Summary], bodies: &[Body]) -> Fate {
    fate_from(flow, body, types, param, Some(param), summaries, bodies)
}

fn fate_from(
    flow: &Flow,
    body: &Body,
    types: &Types,
    start: Local,
    param: Option<Local>,
    summaries: &[Summary],
    bodies: &[Body],
) -> Fate {
    let parts = flow.called[start.0 as usize];
    let (mut held, mut variants) = flow.holders_in(types, body, start, parts, flow.start_variants(start));
    through(flow, body, types, &mut held, &mut variants, summaries);
    fate_of(flow, body, types, param, &held, &variants, summaries, bodies)
}

/// Adds to the locals `held` that may hold a value the results of the
/// calls given it that may return it, `summaries` says, and what holds
/// them in turn: a call that only gives back what it was given keeps none
/// of it, and its result is followed instead.
fn through(flow: &Flow, body: &Body, types: &Types, held: &mut [bool], variants: &mut [u64], summaries: &[Summary]) {
    loop {
        let mut grown = false;
        for &(block, at, result, callee, _) in &flow.results {
            if held[result.0 as usize] || !summaries[callee].returns.contains(&true) {
                continue;
            }
            let Some(Statement::Assign(_, rvalue, _)) = body.blocks[block].stmts.get(at) else { continue };
            let Rvalue::Call(Callee::Fn(_), args) = &**rvalue else { continue };
            let given = args
                .iter()
                .zip(&summaries[callee].returns)
                .any(|(arg, &returns)| returns && matches!(arg, Operand::Copy(place) if held[place.local.0 as usize]));
            if !given {
                continue;
            }
            let parts = flow.called[result.0 as usize];
            let (more, more_variants) = flow.holders_in(types, body, result, parts, flow.start_variants(result));
            for ((held, variants), (more, more_variants)) in
                held.iter_mut().zip(variants.iter_mut()).zip(more.iter().zip(&more_variants))
            {
                *held |= more;
                *variants |= more_variants;
            }
            grown = true;
        }
        if !grown {
            return;
        }
    }
}

/// Whether a call's result, `T ! E`, leaves the function only by its
/// error: what a `?` passes on, which the function that made the result
/// may have made in the region it made the success in.
fn returned_by_error(
    flow: &Flow,
    body: &Body,
    types: &Types,
    start: Local,
    summaries: &[Summary],
    bodies: &[Body],
) -> bool {
    let (mut held, mut variants) = flow.holders_in(types, body, start, true, variant_bit(0));
    through(flow, body, types, &mut held, &mut variants, summaries);
    !matches!(fate_of(flow, body, types, None, &held, &variants, summaries, bodies), Fate::Returned)
}

/// What becomes of what the locals `held` hold; with `param`, the value
/// that parameter was given (`given_fate`).
#[allow(clippy::too_many_arguments)]
fn fate_of(
    flow: &Flow,
    body: &Body,
    types: &Types,
    param: Option<Local>,
    held: &[bool],
    variants: &[u64],
    summaries: &[Summary],
    bodies: &[Body],
) -> Fate {
    let holds = |local: Local| held[local.0 as usize];
    // A place that reads what it holds: not a variant of a union that holds
    // none of it, as the error a `?` returns, nor a part that counts
    // nothing, as a number read out of an array, which can carry none of it.
    let passes = |place: &Place| {
        holds(place.local)
            && variant_read(place).is_none_or(|variant| variants[place.local.0 as usize] & variant_bit(variant) != 0)
            && (place.path.is_empty() || types.needs_drop(body.place_ty(types, place)))
    };
    let reads = |operand: &Operand| matches!(operand, Operand::Copy(place) if passes(place));
    let outside = &flow.outside;
    let given = |local: Local| outside[local.0 as usize] != Outside::No;
    let where_given = |local: Local| match outside[local.0 as usize] {
        Outside::Lock => "it is stored in a lock's state",
        _ => "it is stored in what the function was given",
    };
    // The closures `spawn` runs, which hold what is given to the task.
    let spawned = &flow.spawned;
    let mut returned = body.ret.is_some_and(holds);
    // The statements that read what it holds, in order.
    let mut reading: Vec<u32> = Vec::new();
    for (local, _) in held.iter().enumerate().filter(|&(_, &held)| held) {
        reading.extend(&flow.users[local]);
    }
    reading.sort_unstable();
    reading.dedup();
    for index in reading {
        let (block, at) = flow.at[index as usize];
        let statement = &body.blocks[block as usize].stmts[at as usize];
        let kept = |why: &str, span: Span| Fate::Kept(why.to_string(), span);
        match statement {
            Statement::Assign(place, rvalue, span) => {
                let mut read = false;
                crate::mir::rvalue_places(rvalue, &mut |place| read |= passes(place));
                if !read && !holds(place.local) {
                    continue;
                }
                if given(place.local) && param != Some(place.local) {
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
                    // An object of the runtime given to the runtime, which
                    // may keep it unless its code was read for it.
                    Rvalue::Call(Callee::Std(std), args)
                        if !reads_objects(std)
                            && args.iter().any(|arg| {
                                reads(arg)
                                    && matches!(arg, Operand::Copy(place)
                                        if holds_object(types, body.place_ty(types, place)))
                            }) =>
                    {
                        return Fate::Kept(keeps_objects_why(std), *span);
                    }
                    Rvalue::Call(Callee::Std(std), args) if args.iter().any(reads) && runtime_keeps(std) => {
                        return Fate::Kept(runtime_why(std), *span);
                    }
                    Rvalue::Call(Callee::Std(_), _) => {}
                    Rvalue::Call(..) => return kept("it is passed to C, which may keep it", *span),
                    // A function value that is called keeps nothing of
                    // itself: what it captured, a closure captured for good
                    // (`Erase`), and only what it is given may stay.
                    Rvalue::CallIndirect(_, args) if args.iter().any(reads) => {
                        return kept("it is passed to a function value, which may keep it", *span);
                    }
                    Rvalue::CallIndirect(..) => {}
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
                    Rvalue::TestMembers(..) => return kept("a task takes it", *span),
                    Rvalue::CallOsr(..) => return kept("a live build's call takes it", *span),
                    // Memory a raw pointer reaches, which nothing here
                    // follows.
                    Rvalue::Pointer(crate::hir::PointerOp::Write, ..) => {
                        return kept("it is written through a raw pointer", *span);
                    }
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) if given(target.local) => {
                        return kept(where_given(target.local), *span);
                    }
                    // What grows, or has a slice written, is followed as
                    // what it holds (`Flow`).
                    Rvalue::Mutate(..) | Rvalue::AssignSlice(..) => {}
                    // These keep nothing of what they read but in what they
                    // give, which `Flow` follows by its type; a C array is a
                    // copy of the elements, either way.
                    Rvalue::Fixed(..)
                    | Rvalue::Use(_)
                    | Rvalue::Unary(..)
                    | Rvalue::Binary(..)
                    | Rvalue::Increment(_)
                    | Rvalue::Exact(..)
                    | Rvalue::FnValue(_)
                    | Rvalue::ExternFn(_)
                    | Rvalue::Callback(..)
                    | Rvalue::Layout(..)
                    | Rvalue::Reflect(..)
                    | Rvalue::Cancelled
                    | Rvalue::Unwinding
                    | Rvalue::Restart(_)
                    | Rvalue::Transfer(_)
                    | Rvalue::OsrPoint
                    | Rvalue::Concat(_)
                    | Rvalue::Aggregate(_)
                    | Rvalue::Variant(..)
                    | Rvalue::Discriminant(_)
                    | Rvalue::Convert(_)
                    | Rvalue::Array(..)
                    | Rvalue::ArrayOp(..)
                    | Rvalue::Slice(..)
                    | Rvalue::CheckRegion(..)
                    | Rvalue::Length(_)
                    | Rvalue::StrOp(..)
                    | Rvalue::Pointer(..)
                    | Rvalue::MapOp(..) => {}
                }
                // A new text an operation makes of it holds none of it, as
                // the one a return copies out.
                let fresh = matches!(&**rvalue, Rvalue::StrOp(op, ..) if makes_text(*op));
                if body.ret == Some(place.local) && !fresh {
                    returned = true;
                }
            }
            Statement::DeferPush(_, place) if passes(place) => return kept("a `defer` keeps it", Span::default()),
            // Counts, cleanups that hold nothing of it, zeroings, and what
            // `outside` and `Flow` follow.
            Statement::DeferPush(..)
            | Statement::DeferPop
            | Statement::Retain(_)
            | Statement::Release(_)
            | Statement::Lend(..)
            | Statement::LendAt(..)
            | Statement::Clear(_)
            | Statement::Moved(_)
            | Statement::OsrLoad(_)
            | Statement::Failing(..) => {}
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
/// parameter is kept when its value may be, and a result is fresh when
/// only what the function made reaches it; then what each call does with
/// the result it takes.
fn summaries(bodies: &[Body], types: &Types, roots: &[usize], flows: &[Flow], sites: &[Vec<Site>]) -> Vec<Summary> {
    // The functions that run: those code outside the program's calls, those
    // called through a value, whose calls are not seen, and what they call
    // by name in turn. A function inlined in every caller runs no more.
    let count = bodies.len();
    let mut valued = vec![false; count];
    for &id in flows.iter().flat_map(|flow| &flow.valued) {
        if let Some(valued) = valued.get_mut(id) {
            *valued = true;
        }
    }
    let mut live = vec![false; count];
    let mut pending: Vec<usize> = roots.iter().copied().chain((0..count).filter(|&index| valued[index])).collect();
    while let Some(index) = pending.pop() {
        if index < count && !std::mem::replace(&mut live[index], true) {
            pending.extend(&flows[index].callees);
        }
    }
    let mut summaries: Vec<Summary> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            let unseen = match () {
                _ if valued[index] => Some("what calls it through a function value may keep it"),
                _ if roots.contains(&index) => Some("it is called from outside the program's code"),
                _ if !live[index] => Some("no call of it is left in the program's code: each was inlined"),
                _ => None,
            };
            let params = vec![false; body.params.len()];
            Summary { keeps: params.clone(), returns: params, fresh: body.ret.is_some(), calls: Vec::new(), unseen }
        })
        .collect();
    // The callers of each function, which a change of its summary may
    // change: each function is looked at again only then.
    let mut callers: Vec<Vec<usize>> = vec![Vec::new(); bodies.len()];
    for (index, flow) in flows.iter().enumerate() {
        for &callee in &flow.callees {
            if callers[callee].last() != Some(&index) {
                callers[callee].push(index);
            }
        }
    }
    let all_sites: Vec<Vec<Local>> = sites.iter().map(|sites| sites.iter().map(|site| site.local).collect()).collect();
    let mut queued = vec![true; bodies.len()];
    let mut pending: Vec<usize> = (0..bodies.len()).rev().collect();
    // As many looks as 16 rounds over all of them, should it not settle;
    // the summaries start from what lets the most go in regions, so that
    // one that did not settle says nothing goes there.
    let mut looks = 16 * bodies.len();
    while let Some(index) = pending.pop() {
        queued[index] = false;
        if looks == 0 {
            for summary in &mut summaries {
                summary.keeps.iter_mut().for_each(|keeps| *keeps = true);
                summary.returns.iter_mut().for_each(|returns| *returns = false);
                summary.fresh = false;
            }
            break;
        }
        looks -= 1;
        let body = &bodies[index];
        {
            // Parameters, from what each holds: kept, or only returned.
            let fates: Vec<Fate> = body
                .params
                .iter()
                .map(|&param| match types.needs_drop(body.locals[param.0 as usize]) {
                    true => given_fate(&flows[index], body, types, param, &summaries, bodies),
                    false => Fate::Here,
                })
                .collect();
            let keeps: Vec<bool> = fates.iter().map(|fate| matches!(fate, Fate::Kept(..))).collect();
            let returns: Vec<bool> = fates.iter().map(|fate| *fate == Fate::Returned).collect();
            // The result: fresh when what reaches it is made here, or a
            // fresh result of a call.
            let fresh = body.ret.is_some_and(|ret| {
                let mut made = all_sites[index].clone();
                made.extend(fresh_calls(&flows[index], &summaries));
                made.sort_unstable_by_key(|local| local.0);
                made.dedup();
                let flow = &flows[index];
                let (held, variants): (Vec<Vec<bool>>, Vec<Vec<u64>>) = made
                    .iter()
                    .map(|&local| {
                        let (mut held, mut variants) =
                            flow.holders_in(types, body, local, false, flow.start_variants(local));
                        through(flow, body, types, &mut held, &mut variants, &summaries);
                        (held, variants)
                    })
                    .unzip();
                held.iter().any(|held| held[ret.0 as usize])
                    && only_made(body, types, ret, &held)
                    && made.iter().zip(held.iter().zip(&variants)).all(|(&local, (held, variants))| {
                        let fate = match flow.called[local.0 as usize] {
                            true => fate(flow, body, types, local, &summaries, bodies),
                            false => fate_of(flow, body, types, None, held, variants, &summaries, bodies),
                        };
                        !matches!(fate, Fate::Kept(..))
                    })
            });
            if keeps != summaries[index].keeps || returns != summaries[index].returns || fresh != summaries[index].fresh
            {
                summaries[index].keeps = keeps;
                summaries[index].returns = returns;
                summaries[index].fresh = fresh;
                for &caller in &callers[index] {
                    if !std::mem::replace(&mut queued[caller], true) {
                        pending.push(caller);
                    }
                }
            }
        }
    }
    // What each call that runs does with the result it takes, when the
    // callee returns what it makes. The loops of a caller, read once, where
    // a call may pass on the region its caller gives.
    let mut loops: Vec<Option<Lifetimes>> = (0..bodies.len()).map(|_| None).collect();
    for (index, body) in bodies.iter().enumerate().filter(|&(index, _)| live[index]) {
        for &(block, statement, local, callee, span) in &flows[index].results {
            if !summaries[callee].fresh {
                continue;
            }
            let mut written = span;
            let mut scope = body.scopes.get(block).copied().flatten();
            while let Some(at) = scope {
                written = body.inlined[at as usize].call;
                scope = body.inlined[at as usize].parent;
            }
            let taken = match fate(&flows[index], body, types, local, &summaries, bodies) {
                Fate::Here => Taken::Drops,
                // A call in every turn of a loop that passed on the region
                // its caller gives would leave each turn's result there.
                Fate::Returned if summaries[index].fresh => {
                    let lifetimes = loops[index].get_or_insert_with(|| {
                        Lifetimes::of(body, flows[index].results.iter().map(|&(block, ..)| block as u32))
                    });
                    let at = (block as u32, statement as u32);
                    match every_turn_of(&flows[index], body, types, lifetimes, local, at, true, None) {
                        Some(k) => Taken::Keeps(
                            format!(
                                "`{}` calls it in every turn of a loop, where the region its caller gives would keep \
                                 each turn's",
                                body.name
                            ),
                            lifetimes.written(body, k),
                        ),
                        None => Taken::Passes,
                    }
                }
                Fate::Returned if returned_by_error(&flows[index], body, types, local, &summaries, bodies) => {
                    let callee = &bodies[callee].name;
                    let why = format!(
                        "`{}` passes on the error of `{callee}`, which `{callee}` may make in the region of what it returns",
                        body.name
                    );
                    Taken::Keeps(why, Span::default())
                }
                Fate::Returned => {
                    Taken::Keeps(format!("`{}` returns it with what it was given", body.name), Span::default())
                }
                Fate::Kept(why, at) => Taken::Keeps(format!("`{}` keeps it: {why}", body.name), at),
            };
            let at = (block as u32, statement as u32);
            summaries[callee].calls.push(Call { caller: index, at, span, written, taken });
        }
    }
    summaries
}

/// The locals that take a fresh result of a call.
fn fresh_calls(flow: &Flow, summaries: &[Summary]) -> Vec<Local> {
    flow.results
        .iter()
        .filter(|&&(_, _, _, callee, _)| summaries[callee].fresh)
        .map(|&(_, _, local, ..)| local)
        .collect()
}

/// Whether every value written whole into `ret`, or into a local that
/// reaches it, comes from what the function made, or holds no count.
fn only_made(body: &Body, types: &Types, ret: Local, held: &[Vec<bool>]) -> bool {
    let mut allowed = vec![false; body.locals.len()];
    for held in held {
        for (allowed, &held) in allowed.iter_mut().zip(held) {
            *allowed |= held;
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

/// Whether a function of the runtime may keep what it is given past the
/// call. A native of the runtime can be given a value a region makes only
/// in a parameter of such a kind, an array or a reflect box; one that has
/// none keeps none, and one that has is in `READING`, read from its code,
/// or may keep it. The rest of the runtime's calls are told one by one.
fn runtime_keeps(std: &crate::hir::StdFn) -> bool {
    use crate::hir::StdFn;
    match std {
        StdFn::Native(index) => match crate::hir::NATIVES.get(*index as usize) {
            Some(native) => {
                takes_from_regions(native) && !READING.contains(&native.symbol)
                    || takes_texts(native) && !READING_TEXTS.contains(&native.symbol)
            }
            None => true,
        },
        // A detached task takes its closure; a context keeps its value for
        // the task and those it starts; a panic carries its values to the
        // `recover` that stops it.
        StdFn::SpawnDetached
        | StdFn::ContextSet(..)
        | StdFn::ContextRestore
        | StdFn::PanicUnwind(_)
        | StdFn::PanicHeld => true,
        // These copy what they are given, or read it during the call, as
        // their code does: a file's or a stream's bytes, a command's
        // arguments and input, an attached artifact, a snapshot.
        StdFn::ConsolePrint
        | StdFn::ConsolePrintln
        | StdFn::ConsoleFlush
        | StdFn::ConsoleEprint
        | StdFn::ConsoleEprintln
        | StdFn::ConsoleReadLine
        | StdFn::ConsoleStream(_)
        | StdFn::ConsoleSet(_)
        | StdFn::Io(_)
        | StdFn::Env(_)
        | StdFn::Process(_)
        | StdFn::CpuHas
        | StdFn::Cpu(_)
        | StdFn::Number(_)
        | StdFn::Simd(_)
        | StdFn::TimeSleep
        | StdFn::TimeNow
        | StdFn::TimeUnix
        | StdFn::TimeUtcNow
        | StdFn::TimeUtc
        | StdFn::Fs(_)
        | StdFn::Path(_)
        | StdFn::TestLog
        | StdFn::TestTags
        | StdFn::TestWhere
        | StdFn::WithinEnter
        | StdFn::WithinLeave
        | StdFn::WithinCatch
        | StdFn::ContextGet(_)
        | StdFn::ContextField(..)
        | StdFn::RecoverEnter
        | StdFn::RecoverEnd
        | StdFn::RecoverLeave
        | StdFn::RecoverCatch
        | StdFn::UnwindHold
        | StdFn::UnwindResume
        | StdFn::DeferEnd
        | StdFn::CancelReturn
        | StdFn::TestAttach
        | StdFn::TestSnapshot
        | StdFn::ClockBegin
        | StdFn::ClockEnd
        | StdFn::ClockSettle
        | StdFn::ClockAdvance
        | StdFn::BenchStart
        | StdFn::BenchNext
        | StdFn::BenchEnd
        | StdFn::Keep
        | StdFn::AllocationsStart
        | StdFn::AllocationsSince => false,
    }
}

/// Why a call of the runtime keeps what it is given: a native whose code is
/// not read yet is named as one.
fn runtime_why(std: &crate::hir::StdFn) -> String {
    match std {
        crate::hir::StdFn::Native(index) => match crate::hir::NATIVES.get(*index as usize) {
            Some(native) if UNREAD_TEXTS.contains(&native.symbol) => {
                format!("it is passed to `{}.{}`, a native of the runtime not read yet", native.module, native.name)
            }
            _ => "it is passed to the runtime, which may keep it".to_string(),
        },
        _ => "it is passed to the runtime, which may keep it".to_string(),
    }
}

/// Whether a native of the runtime keeps none of the texts it is given past
/// the call, but in what it returns, as its code was read.
pub(crate) fn reads_texts(native: &crate::hir::Native) -> bool {
    takes_texts(native) && READING_TEXTS.contains(&native.symbol)
}

/// Whether a native of the runtime can be given a text, which a region may
/// make: one is taken to keep it until its code is read and it is in
/// `READING_TEXTS`, which costs a region's text nothing but its place.
fn takes_texts(native: &crate::hir::Native) -> bool {
    use crate::hir::NativeTy::String;
    native.receiver.iter().chain(native.params).any(|kind| matches!(kind, String))
}

/// Whether a handle is an object of the runtime that a region may make: a
/// stream, a buffer, a regex, a value of the runtime; not shared state,
/// a task or a channel, which others hold.
fn runtime_object(handle: Handle) -> bool {
    matches!(
        handle,
        Handle::File
            | Handle::Writer
            | Handle::Reader
            | Handle::Buffer
            | Handle::Command
            | Handle::Pipeline
            | Handle::Process
            | Handle::JsonReader
            | Handle::JsonWriter
            | Handle::Regex
            | Handle::Native
            | Handle::Owned
    )
}

/// Whether a value of the type may hold an object of the runtime.
fn holds_object(types: &Types, ty: Ty) -> bool {
    types
        .reach(ty)
        .into_iter()
        .any(|part| matches!(types.kind(part), TyKind::Handle(handle, _) if runtime_object(*handle)))
}

/// Whether an operation of the runtime keeps none of the objects it is
/// given past the call but in what it gives: the operations of a stream
/// and of a file, which read and write it during the call, and the natives
/// of `READING_OBJECTS`. Any other may keep one, as `console.setInput`
/// keeps the stream it is given.
fn reads_objects(std: &crate::hir::StdFn) -> bool {
    use crate::hir::StdFn;
    match std {
        StdFn::Io(_) | StdFn::Fs(_) => true,
        StdFn::Native(index) => {
            crate::hir::NATIVES.get(*index as usize).is_some_and(|native| READING_OBJECTS.contains(&native.symbol))
        }
        _ => false,
    }
}

/// Why an object given to an operation of the runtime stays on the heap.
fn keeps_objects_why(std: &crate::hir::StdFn) -> String {
    match std {
        crate::hir::StdFn::Native(index) => match crate::hir::NATIVES.get(*index as usize) {
            Some(native) => {
                format!("it is passed to `{}.{}`, which may keep the objects it is given", native.module, native.name)
            }
            None => "it is passed to the runtime, which may keep it".to_string(),
        },
        _ => "it is passed to the runtime, which may keep it".to_string(),
    }
}

/// The natives that can be given an object of the runtime and keep none of
/// it past the call but in what they give, as their code shows: the
/// methods of a regex, of a JSON, YAML, TOML, protobuf, MessagePack or
/// CBOR reader or writer, of an image, a hasher and an unpacked archive
/// read it or write it during the call, an image's on a thread of the
/// runtime the call waits for. An object given to any other stays on the
/// heap.
const READING_OBJECTS: &[&str] = &[
    "tondo_rt_cbor_bytes",
    "tondo_rt_cbor_fail",
    "tondo_rt_cbor_failure",
    "tondo_rt_cbor_field",
    "tondo_rt_cbor_finish",
    "tondo_rt_cbor_float",
    "tondo_rt_cbor_int",
    "tondo_rt_cbor_next",
    "tondo_rt_cbor_skip",
    "tondo_rt_cbor_text",
    "tondo_rt_cbor_where",
    "tondo_rt_cbor_write_bool",
    "tondo_rt_cbor_write_bytes",
    "tondo_rt_cbor_write_close",
    "tondo_rt_cbor_write_datetime",
    "tondo_rt_cbor_write_float",
    "tondo_rt_cbor_write_int",
    "tondo_rt_cbor_write_key",
    "tondo_rt_cbor_write_null",
    "tondo_rt_cbor_write_number",
    "tondo_rt_cbor_write_open",
    "tondo_rt_cbor_write_text",
    "tondo_rt_hasher_finish",
    "tondo_rt_hasher_update",
    "tondo_rt_image_adjust",
    "tondo_rt_image_blur",
    "tondo_rt_image_change",
    "tondo_rt_image_colors",
    "tondo_rt_image_encode",
    "tondo_rt_image_failure",
    "tondo_rt_image_overlay",
    "tondo_rt_image_pixel",
    "tondo_rt_image_pixels",
    "tondo_rt_image_size",
    "tondo_rt_image_tone",
    "tondo_rt_json_fail",
    "tondo_rt_json_field",
    "tondo_rt_json_finish",
    "tondo_rt_json_integer",
    "tondo_rt_json_next",
    "tondo_rt_json_next_event",
    "tondo_rt_json_raw",
    "tondo_rt_json_several",
    "tondo_rt_json_skip",
    "tondo_rt_json_text",
    "tondo_rt_json_where",
    "tondo_rt_json_write_bracket",
    "tondo_rt_json_write_float",
    "tondo_rt_json_write_int",
    "tondo_rt_json_write_key",
    "tondo_rt_json_write_number",
    "tondo_rt_json_write_raw",
    "tondo_rt_json_write_string",
    "tondo_rt_json_write_word",
    "tondo_rt_json_writer_error",
    "tondo_rt_msgpack_bytes",
    "tondo_rt_msgpack_fail",
    "tondo_rt_msgpack_failure",
    "tondo_rt_msgpack_field",
    "tondo_rt_msgpack_finish",
    "tondo_rt_msgpack_float",
    "tondo_rt_msgpack_flush",
    "tondo_rt_msgpack_int",
    "tondo_rt_msgpack_next",
    "tondo_rt_msgpack_skip",
    "tondo_rt_msgpack_text",
    "tondo_rt_msgpack_where",
    "tondo_rt_msgpack_write_bool",
    "tondo_rt_msgpack_write_bytes",
    "tondo_rt_msgpack_write_close",
    "tondo_rt_msgpack_write_datetime",
    "tondo_rt_msgpack_write_float",
    "tondo_rt_msgpack_write_header",
    "tondo_rt_msgpack_write_int",
    "tondo_rt_msgpack_write_key",
    "tondo_rt_msgpack_write_null",
    "tondo_rt_msgpack_write_number",
    "tondo_rt_msgpack_write_open",
    "tondo_rt_msgpack_write_text",
    "tondo_rt_pb_begin",
    "tondo_rt_pb_deterministic",
    "tondo_rt_pb_end",
    "tondo_rt_pb_enter",
    "tondo_rt_pb_error",
    "tondo_rt_pb_fail",
    "tondo_rt_pb_failed",
    "tondo_rt_pb_finish",
    "tondo_rt_pb_is_packed",
    "tondo_rt_pb_more",
    "tondo_rt_pb_next",
    "tondo_rt_pb_offset",
    "tondo_rt_pb_raw",
    "tondo_rt_pb_read_bytes",
    "tondo_rt_pb_read_fixed32",
    "tondo_rt_pb_read_fixed64",
    "tondo_rt_pb_read_text",
    "tondo_rt_pb_read_varint",
    "tondo_rt_pb_read_zigzag",
    "tondo_rt_pb_skip",
    "tondo_rt_pb_write_bytes",
    "tondo_rt_pb_write_fixed32",
    "tondo_rt_pb_write_fixed64",
    "tondo_rt_pb_write_group",
    "tondo_rt_pb_write_raw",
    "tondo_rt_pb_write_text",
    "tondo_rt_pb_write_varint",
    "tondo_rt_pb_write_zigzag",
    "tondo_rt_regex_capture_count",
    "tondo_rt_regex_capture_index",
    "tondo_rt_regex_capture_name",
    "tondo_rt_regex_check_input",
    "tondo_rt_regex_find",
    "tondo_rt_regex_is_match",
    "tondo_rt_regex_pattern",
    "tondo_rt_regex_replace",
    "tondo_rt_regex_slice",
    "tondo_rt_regex_whole_match",
    "tondo_rt_text_collate",
    "tondo_rt_toml_fail",
    "tondo_rt_toml_failure",
    "tondo_rt_toml_field",
    "tondo_rt_toml_finish",
    "tondo_rt_toml_float",
    "tondo_rt_toml_int",
    "tondo_rt_toml_next",
    "tondo_rt_toml_skip",
    "tondo_rt_toml_text",
    "tondo_rt_toml_where",
    "tondo_rt_toml_write_bool",
    "tondo_rt_toml_write_close",
    "tondo_rt_toml_write_datetime",
    "tondo_rt_toml_write_float",
    "tondo_rt_toml_write_int",
    "tondo_rt_toml_write_key",
    "tondo_rt_toml_write_null",
    "tondo_rt_toml_write_number",
    "tondo_rt_toml_write_open",
    "tondo_rt_toml_write_text",
    "tondo_rt_unpack_failed",
    "tondo_rt_unpacked",
    "tondo_rt_yaml_documents",
    "tondo_rt_yaml_fail",
    "tondo_rt_yaml_field",
    "tondo_rt_yaml_finish",
    "tondo_rt_yaml_float",
    "tondo_rt_yaml_int",
    "tondo_rt_yaml_next",
    "tondo_rt_yaml_next_document",
    "tondo_rt_yaml_outline_error",
    "tondo_rt_yaml_outline_numbers",
    "tondo_rt_yaml_outline_paths",
    "tondo_rt_yaml_outline_properties",
    "tondo_rt_yaml_skip",
    "tondo_rt_yaml_tag",
    "tondo_rt_yaml_text",
    "tondo_rt_yaml_where",
    "tondo_rt_yaml_write_bool",
    "tondo_rt_yaml_write_close",
    "tondo_rt_yaml_write_float",
    "tondo_rt_yaml_write_int",
    "tondo_rt_yaml_write_key",
    "tondo_rt_yaml_write_null",
    "tondo_rt_yaml_write_number",
    "tondo_rt_yaml_write_open",
    "tondo_rt_yaml_write_tag",
    "tondo_rt_yaml_write_text",
];

/// The natives that give an object of the runtime a region does not make
/// yet, which stays on the heap: each kind waits for its code to be read
/// for one, as the natives of `MAKING_IN_REGION` that give one were. The
/// runtime's spare JSON readers and writers cost the heap no allocation,
/// which one of a region would. A test holds this list and that one to
/// the table of natives.
#[cfg(test)]
const OBJECTS_ON_THE_HEAP: &[&str] = &[
    "tondo_rt_contexts_capture",
    "tondo_rt_counter",
    "tondo_rt_files_link",
    "tondo_rt_files_read_symlink",
    "tondo_rt_files_real_path",
    "tondo_rt_files_set_modified",
    "tondo_rt_files_set_permissions",
    "tondo_rt_files_space",
    "tondo_rt_files_stat",
    "tondo_rt_files_symlink",
    "tondo_rt_files_temporary",
    "tondo_rt_files_walk",
    "tondo_rt_files_watch",
    "tondo_rt_grpc_channel",
    "tondo_rt_grpc_frames",
    "tondo_rt_grpc_frames_of",
    "tondo_rt_grpc_open",
    "tondo_rt_grpc_signal",
    "tondo_rt_grpc_slot",
    "tondo_rt_grpc_start",
    "tondo_rt_grpc_take",
    "tondo_rt_http_accept",
    "tondo_rt_http_accept_websocket",
    "tondo_rt_http_body",
    "tondo_rt_http_body_bytes",
    "tondo_rt_http_body_empty",
    "tondo_rt_http_body_form",
    "tondo_rt_http_body_stream",
    "tondo_rt_http_client",
    "tondo_rt_http_connection_next",
    "tondo_rt_http_form",
    "tondo_rt_http_form_next",
    "tondo_rt_http_listen",
    "tondo_rt_http_listen_unix",
    "tondo_rt_http_request_body",
    "tondo_rt_http_request_form",
    "tondo_rt_http_request_form_of",
    "tondo_rt_http_send",
    "tondo_rt_http_trailers",
    "tondo_rt_http_ws_connect",
    "tondo_rt_http_ws_receive",
    "tondo_rt_json_reader",
    "tondo_rt_json_stream_reader",
    "tondo_rt_json_stream_writer",
    "tondo_rt_json_writer",
    "tondo_rt_library_open",
    "tondo_rt_library_program",
    "tondo_rt_msgpack_stream_writer",
    "tondo_rt_net_accept",
    "tondo_rt_net_bind",
    "tondo_rt_net_bind_unix",
    "tondo_rt_net_connect",
    "tondo_rt_net_connect_unix",
    "tondo_rt_net_listen",
    "tondo_rt_net_listen_unix",
    "tondo_rt_net_reader",
    "tondo_rt_net_receive",
    "tondo_rt_net_resolve",
    "tondo_rt_net_start_tls",
    "tondo_rt_net_writer",
    "tondo_rt_os_descriptor",
    "tondo_rt_os_mapping",
    "tondo_rt_pack_writer",
    "tondo_rt_pipe",
    "tondo_rt_pipe_reader",
    "tondo_rt_pipe_writer",
    "tondo_rt_process_alone",
    "tondo_rt_profile_cpu_period",
    "tondo_rt_profile_cpu_stop",
    "tondo_rt_profile_memory",
    "tondo_rt_profile_since",
    "tondo_rt_profile_tasks",
    "tondo_rt_profile_waits",
    "tondo_rt_region_new",
    "tondo_rt_sqlite_open",
    "tondo_rt_tee",
    "tondo_rt_terminal_keys",
    "tondo_rt_terminal_take",
    "tondo_rt_unpack_reader",
];

/// The natives that can be given a text and keep none of it past the call,
/// as their code shows. A native that takes a text is in this list, in
/// `KEEPING_TEXTS` or in `UNREAD_TEXTS`, and a test holds the table of
/// natives to them; one in either of the others is taken to keep it.
const READING_TEXTS: &[&str] = &[
    "tondo_rt_base64_check",
    "tondo_rt_base64_decode",
    "tondo_rt_cbor_fail",
    "tondo_rt_cbor_write_datetime",
    "tondo_rt_cbor_write_key",
    "tondo_rt_cbor_write_number",
    "tondo_rt_cbor_write_text",
    "tondo_rt_crypto_hash_password",
    "tondo_rt_crypto_verify_password",
    "tondo_rt_csv_check",
    "tondo_rt_csv_fields",
    "tondo_rt_files_link",
    "tondo_rt_files_read_symlink",
    "tondo_rt_files_real_path",
    "tondo_rt_files_set_modified",
    "tondo_rt_files_set_permissions",
    "tondo_rt_files_space",
    "tondo_rt_files_stat",
    "tondo_rt_files_symlink",
    "tondo_rt_files_temporary",
    "tondo_rt_files_walk",
    "tondo_rt_fuzz_begin",
    "tondo_rt_fuzz_encoded",
    "tondo_rt_fuzz_invalid",
    "tondo_rt_fuzz_seed",
    "tondo_rt_grpc_channel",
    "tondo_rt_grpc_close",
    "tondo_rt_grpc_frames",
    "tondo_rt_grpc_frames_of",
    "tondo_rt_grpc_open",
    "tondo_rt_grpc_start",
    "tondo_rt_hex_check",
    "tondo_rt_hex_decode",
    "tondo_rt_html_escape",
    "tondo_rt_html_unescape",
    "tondo_rt_http_body_bytes",
    "tondo_rt_http_body_stream",
    "tondo_rt_http_client",
    "tondo_rt_http_form_bytes",
    "tondo_rt_http_form_file",
    "tondo_rt_http_form_stream",
    "tondo_rt_http_form_text",
    "tondo_rt_http_header",
    "tondo_rt_http_listen",
    "tondo_rt_http_listen_unix",
    "tondo_rt_http_media_type",
    "tondo_rt_http_request_header",
    "tondo_rt_http_respond_bytes",
    "tondo_rt_http_respond_file",
    "tondo_rt_http_save_body",
    "tondo_rt_http_send",
    "tondo_rt_http_trailers_set",
    "tondo_rt_http_upload_save",
    "tondo_rt_http_ws_connect",
    "tondo_rt_http_ws_send",
    "tondo_rt_json_fail",
    "tondo_rt_json_is_int",
    "tondo_rt_json_parse_float",
    "tondo_rt_json_parse_int",
    "tondo_rt_json_write_key",
    "tondo_rt_json_write_number",
    "tondo_rt_json_write_raw",
    "tondo_rt_json_write_string",
    "tondo_rt_library_file_name",
    "tondo_rt_library_open",
    "tondo_rt_library_symbol",
    "tondo_rt_memory_c_string",
    "tondo_rt_meta_bytes",
    "tondo_rt_meta_respond",
    "tondo_rt_metric",
    "tondo_rt_model_report",
    "tondo_rt_msgpack_fail",
    "tondo_rt_msgpack_write_datetime",
    "tondo_rt_msgpack_write_key",
    "tondo_rt_msgpack_write_number",
    "tondo_rt_msgpack_write_text",
    "tondo_rt_net_bind",
    "tondo_rt_net_bind_unix",
    "tondo_rt_net_canonical_ip",
    "tondo_rt_net_connect",
    "tondo_rt_net_connect_unix",
    "tondo_rt_net_contains",
    "tondo_rt_net_ip",
    "tondo_rt_net_ip_kind",
    "tondo_rt_net_listen",
    "tondo_rt_net_listen_unix",
    "tondo_rt_net_network",
    "tondo_rt_net_resolve",
    "tondo_rt_net_send_to",
    "tondo_rt_net_send_to_path",
    "tondo_rt_net_start_tls",
    "tondo_rt_net_write",
    "tondo_rt_panic_bool",
    "tondo_rt_panic_code",
    "tondo_rt_panic_float",
    "tondo_rt_panic_int",
    "tondo_rt_panic_name",
    "tondo_rt_panic_report",
    "tondo_rt_panic_restage",
    "tondo_rt_panic_text",
    "tondo_rt_path_key",
    "tondo_rt_path_steps",
    "tondo_rt_pb_fail",
    "tondo_rt_pb_write_text",
    "tondo_rt_process_which",
    "tondo_rt_reflect_index",
    "tondo_rt_reflect_quote",
    "tondo_rt_regex_capture_index",
    "tondo_rt_regex_check_input",
    "tondo_rt_regex_compile",
    "tondo_rt_regex_escape",
    "tondo_rt_regex_find",
    "tondo_rt_regex_is_match",
    "tondo_rt_regex_replace",
    "tondo_rt_regex_slice",
    "tondo_rt_regex_whole_match",
    "tondo_rt_region_show",
    "tondo_rt_sqlite_open",
    "tondo_rt_sqlite_run",
    "tondo_rt_template_parse",
    "tondo_rt_text_between",
    "tondo_rt_text_byte_at",
    "tondo_rt_text_collate",
    "tondo_rt_text_collator",
    "tondo_rt_text_count",
    "tondo_rt_text_currency_digits",
    "tondo_rt_text_currency_symbol",
    "tondo_rt_text_find_in",
    "tondo_rt_text_find_last",
    "tondo_rt_text_is_float",
    "tondo_rt_text_is_normalized",
    "tondo_rt_text_lines",
    "tondo_rt_text_lower",
    "tondo_rt_text_normalize",
    "tondo_rt_text_number_symbols",
    "tondo_rt_text_parse_float",
    "tondo_rt_text_plural_rules",
    "tondo_rt_text_repeat",
    "tondo_rt_text_scalar_at",
    "tondo_rt_text_split",
    "tondo_rt_text_trim",
    "tondo_rt_text_untrimmed",
    "tondo_rt_text_upper",
    "tondo_rt_text_words",
    "tondo_rt_time_names",
    "tondo_rt_tls_certificates_serving",
    "tondo_rt_tls_certificates_set",
    "tondo_rt_toml_fail",
    "tondo_rt_toml_parse",
    "tondo_rt_toml_write_datetime",
    "tondo_rt_toml_write_key",
    "tondo_rt_toml_write_number",
    "tondo_rt_toml_write_text",
    "tondo_rt_url_check",
    "tondo_rt_url_decode",
    "tondo_rt_url_encode",
    "tondo_rt_yaml_fail",
    "tondo_rt_yaml_outline",
    "tondo_rt_yaml_parse",
    "tondo_rt_yaml_write_key",
    "tondo_rt_yaml_write_number",
    "tondo_rt_yaml_write_tag",
    "tondo_rt_yaml_write_text",
    "tondo_rt_zone_abbreviation",
    "tondo_rt_zone_known",
    "tondo_rt_zone_offset",
    "tondo_rt_zone_resolve",
];

/// The natives that take a text whose code is not read yet, taken to keep
/// it until it is. Every one was read: a native added that takes a text is
/// read with it, into `READING_TEXTS` or `KEEPING_TEXTS` with its case, or
/// waits here, keeping what it is given, until it is.
const UNREAD_TEXTS: &[&str] = &[];

/// Whether a native of the runtime can be given a value a region makes:
/// an array, or a reflect box, as its receiver or a parameter.
fn takes_from_regions(native: &crate::hir::Native) -> bool {
    use crate::hir::NativeTy::*;
    native
        .receiver
        .iter()
        .chain(native.params)
        .any(|kind| matches!(kind, Bytes | Ints | Floats | Floats32 | Strings | ReflectBox))
}

/// The natives that can be given a value a region makes and keep none of
/// it past the call, as their code shows: each copies what it keeps, as a
/// reader's or a body's bytes, or reads it only during the call, and what
/// it gives is a value it made. A native
/// of such a kind that is not here may keep it, and a test holds this list
/// to the table of natives.
const READING: &[&str] = &[
    "tondo_rt_base64_encode",
    "tondo_rt_cbor_field",
    "tondo_rt_cbor_parse",
    "tondo_rt_cbor_write_bytes",
    "tondo_rt_crc32",
    "tondo_rt_crypto_hkdf",
    "tondo_rt_crypto_open",
    "tondo_rt_crypto_pbkdf2",
    "tondo_rt_crypto_public",
    "tondo_rt_crypto_seal",
    "tondo_rt_crypto_sign",
    "tondo_rt_crypto_verify",
    "tondo_rt_deflate",
    "tondo_rt_files_watch",
    "tondo_rt_grpc_frames_of",
    "tondo_rt_grpc_open",
    "tondo_rt_grpc_send",
    "tondo_rt_grpc_start",
    "tondo_rt_gzip",
    "tondo_rt_hash_digest",
    "tondo_rt_hasher",
    "tondo_rt_hasher_update",
    "tondo_rt_hex_encode",
    "tondo_rt_http_body_bytes",
    "tondo_rt_http_form_bytes",
    "tondo_rt_http_request_form_of",
    "tondo_rt_http_respond",
    "tondo_rt_http_respond_bytes",
    "tondo_rt_http_respond_file",
    "tondo_rt_http_respond_stream",
    "tondo_rt_http_respond_text",
    "tondo_rt_http_respond_trailed",
    "tondo_rt_http_send",
    "tondo_rt_http_windows_1252",
    "tondo_rt_http_ws_connect",
    "tondo_rt_http_ws_send",
    "tondo_rt_image_exif",
    "tondo_rt_image_from_pixels",
    "tondo_rt_image_info",
    "tondo_rt_image_read",
    "tondo_rt_inflate",
    "tondo_rt_json_field",
    "tondo_rt_labels_push",
    "tondo_rt_math_cycled_f32",
    "tondo_rt_math_cycled_f64",
    "tondo_rt_math_dot_at_f32",
    "tondo_rt_math_dot_at_f64",
    "tondo_rt_math_dot_f32",
    "tondo_rt_math_dot_f64",
    "tondo_rt_math_sum_f32",
    "tondo_rt_math_sum_f64",
    "tondo_rt_memory_from_c_chars",
    "tondo_rt_memory_store",
    "tondo_rt_msgpack_field",
    "tondo_rt_msgpack_parse",
    "tondo_rt_msgpack_write_bytes",
    "tondo_rt_net_send_to",
    "tondo_rt_net_send_to_path",
    "tondo_rt_net_write_bytes",
    "tondo_rt_pb_reader",
    "tondo_rt_pb_write_bytes",
    "tondo_rt_pb_write_raw",
    "tondo_rt_reflect_name",
    "tondo_rt_sqlite_run",
    "tondo_rt_tls_certificates",
    "tondo_rt_toml_field",
    "tondo_rt_unpack",
    "tondo_rt_yaml_field",
    "tondo_rt_zone_resolve",
    "tondo_rt_zstd",
];

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

#[cfg(test)]
mod natives {
    use super::*;

    /// The natives that can be given a value a region makes and may keep it
    /// past the call, as their code shows, which `runtime_keeps` takes as
    /// any native of such a kind not in `READING`: each such native is in
    /// one list or the other.
    const KEEPING: &[&str] = &[];

    /// The natives that can be given a text and may keep it past the call,
    /// as their code shows, which `runtime_keeps` takes as any native that
    /// takes a text and is not in `READING_TEXTS`.
    const KEEPING_TEXTS: &[&str] = &["tondo_rt_http_respond_text", "tondo_rt_json_reader"];

    /// Every native that can be given a value a region makes says whether
    /// it keeps it, as its code was read, and the lists name no other: a
    /// native added with such a parameter fails here until it is read.
    #[test]
    fn every_native_that_takes_from_regions_is_read() {
        let taking: Vec<&str> = crate::hir::NATIVES
            .iter()
            .filter(|native| takes_from_regions(native))
            .map(|native| native.symbol)
            .collect();
        let unread: Vec<&&str> =
            taking.iter().filter(|symbol| !READING.contains(symbol) && !KEEPING.contains(symbol)).collect();
        assert!(
            unread.is_empty(),
            "{unread:?} can be given a value a region makes: read its code and put it in `READING` if it copies \
             what it keeps or reads it only during the call, else in `KEEPING`"
        );
        let stale: Vec<&&str> = READING.iter().chain(KEEPING).filter(|symbol| !taking.contains(symbol)).collect();
        assert!(stale.is_empty(), "{stale:?} take nothing a region makes: take them out of the lists");
    }

    /// Every native that takes a text is in one of the three lists of
    /// texts, and in one only, and the lists name no other.
    #[test]
    fn every_native_that_gives_an_object_has_a_place() {
        use crate::hir::NativeTy::*;
        let giving: Vec<&crate::hir::Native> = crate::hir::NATIVES
            .iter()
            .filter(|native| {
                matches!(
                    native.result,
                    Regex | Object | Owned | Command | Pipeline | IoReader | IoWriter | JsonReader | JsonWriter
                )
            })
            .collect();
        let placeless: Vec<&str> = giving
            .iter()
            .filter(|native| native.makes_in_region() == OBJECTS_ON_THE_HEAP.contains(&native.symbol))
            .map(|native| native.symbol)
            .collect();
        assert!(
            placeless.is_empty(),
            "{placeless:?} give an object of the runtime: put each in `OBJECTS_ON_THE_HEAP`, or read its code and \
             put it in `MAKING_IN_REGION` if what it gives can be made in a region, and in neither both"
        );
        let stale: Vec<&&str> = OBJECTS_ON_THE_HEAP
            .iter()
            .filter(|symbol| !giving.iter().any(|native| native.symbol == **symbol))
            .collect();
        assert!(stale.is_empty(), "{stale:?} give no object: take them out of `OBJECTS_ON_THE_HEAP`");
    }

    #[test]
    fn every_native_that_takes_a_text_has_a_word() {
        let taking: Vec<&str> =
            crate::hir::NATIVES.iter().filter(|native| takes_texts(native)).map(|native| native.symbol).collect();
        let lists = [READING_TEXTS, KEEPING_TEXTS, UNREAD_TEXTS];
        let wordless: Vec<&&str> =
            taking.iter().filter(|symbol| !lists.iter().any(|list| list.contains(symbol))).collect();
        assert!(
            wordless.is_empty(),
            "{wordless:?} take a text: put each in `UNREAD_TEXTS`, or read its code and put it in `READING_TEXTS` if \
             it copies the text or reads it only during the call, else in `KEEPING_TEXTS`"
        );
        let twice: Vec<&&str> =
            taking.iter().filter(|symbol| lists.iter().filter(|list| list.contains(symbol)).count() > 1).collect();
        assert!(twice.is_empty(), "{twice:?} are in more than one list of texts");
        let stale: Vec<&&str> = lists.iter().flat_map(|list| list.iter()).filter(|s| !taking.contains(s)).collect();
        assert!(stale.is_empty(), "{stale:?} take no text: take them out of the lists of texts");
    }
}
