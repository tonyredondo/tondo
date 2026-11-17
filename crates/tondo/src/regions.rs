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
    /// Whether it may keep each parameter past the call, or return it.
    keeps: Vec<bool>,
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
    pub by: Option<Span>,
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
    clones: &[(usize, usize)],
) -> (Vec<Verdict>, Vec<Taking>) {
    // What each function's statements pass on, and its places, on every
    // core: each function is read on its own.
    let flows: Vec<(Flow, Vec<Site>)> =
        crate::parallel::map(bodies, 16, |body| (Flow::of(body, types), sites(body, types)));
    let (flows, all_sites): (Vec<Flow>, Vec<Vec<Site>>) = flows.into_iter().unzip();
    let summaries = summaries(bodies, types, roots, &flows, &all_sites);
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
            let by = by.filter(|by| *by != span);
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
            verdict.by = Some(by).filter(|by| *by != verdict.span);
        }
        verdicts
    });
    // What the placement changed of the verdicts: a region a loop empties
    // each turn, or the heap, where no region takes the value.
    let mut judged = judged;
    let outcomes = place(bodies, types, &summaries, &all_sites, &judged, &flows, clones);
    for ((verdicts, outcomes), sites) in judged.iter_mut().zip(&outcomes).zip(&all_sites) {
        for ((verdict, outcome), site) in verdicts.iter_mut().zip(outcomes).zip(sites) {
            let what = site.what;
            // A loop by its line, and its file when it is another.
            let line = |at: &Span| {
                let ((file, line, _), (here, ..)) = (sources.position(at.start), sources.position(verdict.span.start));
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
            verdict.by = verdict.by.filter(|by| *by != Span::default() && *by != verdict.span);
        }
    }
    if crate::mir_text::remarking() {
        for (body, verdicts) in bodies.iter_mut().zip(&judged) {
            for verdict in verdicts {
                let text = verdict.text.clone();
                crate::mir_text::remark_by(body, verdict.span, "region", verdict.by, || text);
            }
        }
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
                true => lifetimes[index].region(&flows[index], body, types, site.local, site.at, false, Some(site)),
                false => None,
            })
            .collect();
        // Where what the verdicts give a region goes in the end.
        let loops = &lifetimes[index];
        let written = |k: usize| loops.written(body, k);
        let mut outcome: Vec<Outcome> = sites
            .iter()
            .zip(verdicts)
            .zip(&regions)
            .map(|((site, verdict), region)| match (verdict.region, verdict.own, site.refused, region) {
                (false, ..) => Outcome::Stands,
                (true, _, Some(why), _) => Outcome::Heap(why.to_string(), None),
                (true, true, None, Some(0)) => Outcome::Stands,
                (true, true, None, Some(region)) => Outcome::Turn(written(*region as usize - 1)),
                (true, true, None, None) => {
                    Outcome::Outlives(loops.loop_of(site.at.0 as usize).map(written).unwrap_or_default())
                }
                (true, false, None, _) => match copied[index] {
                    true => Outcome::Stands,
                    false => Outcome::Heap(format!("no call of `{}` lets it go", body.name), None),
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
            .map(|((site, verdict), region)| site.placed() && verdict.region && (!verdict.own || region.is_some()))
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
        let made = made(&flows[index], body, loans, sites, &placed, &gives);
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
    /// In the region of a turn of the loop written there.
    Turn(Span),
    /// On the heap, as it outlives a turn of the loop written there.
    Outlives(Span),
    /// On the heap, as no region takes it: why, and the code that keeps it.
    Heap(String, Option<Span>),
}

/// Where a value a function lets go may go: its loops, the innermost
/// first, the locals live at each one's head and where it is left.
struct Lifetimes {
    loops: Vec<(usize, Vec<bool>)>,
    /// The locals live at each loop's head and where it is left, as bits.
    live_head: Vec<Vec<u64>>,
    live_out: Vec<Vec<u64>>,
    looped: Vec<bool>,
}

impl Lifetimes {
    /// Of a function with nothing to put in a region by loops.
    fn none() -> Lifetimes {
        Lifetimes { loops: Vec::new(), live_head: Vec::new(), live_out: Vec::new(), looped: Vec::new() }
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
            return Lifetimes { loops, live_head: Vec::new(), live_out: Vec::new(), looped };
        }
        let live = crate::mir::live_in_bits(&body.blocks, body.locals.len());
        let (mut live_head, mut live_out) = (Vec::new(), Vec::new());
        for (head, inside) in &loops {
            live_head.push(live[*head].clone());
            let mut out = vec![0u64; live[*head].len()];
            for (_, data) in body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]) {
                for to in crate::hoist::targets(&data.terminator).into_iter().filter(|&to| !inside[to]) {
                    for (out, &live) in out.iter_mut().zip(&live[to]) {
                        *out |= live;
                    }
                }
            }
            live_out.push(out);
        }
        Lifetimes { loops, live_head, live_out, looped }
    }

    /// The innermost loop `block` is in.
    fn loop_of(&self, block: usize) -> Option<usize> {
        self.loops.iter().position(|(_, inside)| inside[block])
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
        head(k)
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
        let (entry, exit) = held_from(body, &bits, local, at);
        let at = |k: usize| {
            let inside = &self.loops[k].1;
            let mut exits = vec![0u64; words];
            for (from, data) in body.blocks.iter().enumerate().filter(|&(from, _)| inside[from]) {
                if crate::hoist::targets(&data.terminator).into_iter().any(|to| !inside[to]) {
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
        let kept = |k: usize| {
            let (head, exits) = at(k);
            let inside = &self.loops[k].1;
            keepers(k, head, &exits).into_iter().all(|keeper| only_pushed(body, types, inside, Local(keeper as u32)))
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
    site.what.starts_with("grows")
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
fn held_from(body: &Body, held: &[u64], local: Local, at: (u32, u32)) -> (Vec<Vec<u64>>, Vec<Vec<u64>>) {
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
    let mut pending = vec![at.0 as usize];
    let mut queued = vec![false; count];
    queued[at.0 as usize] = true;
    while let Some(block) = pending.pop() {
        queued[block] = false;
        let mut state = entry[block].clone();
        for (index, statement) in body.blocks[block].stmts.iter().enumerate() {
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
    (entry, exit)
}

/// Whether `local` is an array that the blocks `inside` a loop only push
/// to: given its value outside them, never written whole there, nor an
/// element of it, nor changed otherwise, nor lent, nor passed to a call.
fn only_pushed(body: &Body, types: &Types, inside: &[bool], local: Local) -> bool {
    let array = matches!(types.kind(types.repr(body.locals[local.0 as usize])), crate::types::TyKind::Array(_));
    let ours = |operand: &Operand| matches!(operand, Operand::Copy(place) if place.local == local);
    array
        && body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]).all(|(_, data)| {
            data.stmts.iter().all(|statement| match statement {
                Statement::Assign(place, rvalue, _) => {
                    place.local != local
                        && match &**rvalue {
                            Rvalue::Mutate(mutation, target, ..) if target.local == local => {
                                target.path.is_empty()
                                    && matches!(mutation, crate::hir::Mutation::Push | crate::hir::Mutation::Reserve)
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
) -> Vec<(usize, (String, Span))> {
    let counted = |place: &Place| match place.path.is_empty() {
        true => flow.counted[place.local.0 as usize],
        false => types.needs_drop(body.place_ty(types, place)),
    };
    let mut found = Vec::new();
    if !region.contains(&true) {
        return found;
    }
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
                | Rvalue::Array(..) => storing.push((at, statement)),
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
        let made = made(flow, body, loans, sites, region, call_made);
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
            let mut outside = (site.what.starts_with("grows") && !made[local.0 as usize]).then_some(span);
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
                    outside = Some(span);
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
                    outside = Some(*by);
                    break;
                }
                let stored: Vec<&Operand> = match &**rvalue {
                    Rvalue::Mutate(_, target, operands, ..) if is[target.local.0 as usize] => operands.iter().collect(),
                    // The elements of what it is given, which hold what a
                    // region made when it did.
                    Rvalue::AssignSlice(target, _, operand, _) if is[target.local.0 as usize] => vec![operand],
                    Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _)
                        if place.path.is_empty() && place.local == local && at_statement == site.at =>
                    {
                        operands.iter().collect()
                    }
                    Rvalue::Use(operand) if !place.path.is_empty() && is[place.local.0 as usize] => vec![operand],
                    Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) | Rvalue::Array(operands, _)
                        if !place.path.is_empty() && is[place.local.0 as usize] =>
                    {
                        operands.iter().collect()
                    }
                    _ => continue,
                };
                let elsewhere = stored.into_iter().find(
                    |operand| matches!(operand, Operand::Copy(from) if counted(from) && !made[from.local.0 as usize]),
                );
                if elsewhere.is_some() {
                    outside = Some(*by);
                    break;
                }
            }
            if let Some(by) = outside {
                region[at] = false;
                changed = true;
                found.push((at, ("it holds a value that its region did not make".to_string(), by)));
            }
        }
        if !changed {
            return found;
        }
    }
}

/// The locals whose values a region made, or that count nothing: each
/// value written whole into them is a place a region takes, a result of a
/// call `call_made` says a region makes, a constant, or a part or a copy of
/// such a value.
fn made(
    flow: &Flow,
    body: &Body,
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
                    Rvalue::Mutate(_, target, ..) => Some((target.local, taken(target.local))),
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
                    Some((place.local, false))
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
                    // The runtime reads what it is given; C and a function
                    // value may take anything as a loan.
                    Rvalue::Call(Callee::Std(_), _) => {}
                    Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) => args.iter().for_each(lend),
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

/// The places the function makes a value on the heap in: an array of
/// values that are not all constants, a map, a slice, a box, an array that
/// grows, a text, and the arrays, texts and other values the runtime gives.
fn sites(body: &Body, types: &Types) -> Vec<Site> {
    let mut found = Vec::new();
    let array =
        |place: &Place| matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::Array(_));
    const MAPS: Option<&str> = Some("regions do not take maps yet");
    const TEXTS: Option<&str> = Some("regions do not take texts yet");
    const RUNTIME: Option<&str> = Some("regions do not take what the runtime makes yet");
    for (at, statement) in positioned(body) {
        let Statement::Assign(place, rvalue, span) = statement else { continue };
        let refused = match &**rvalue {
            Rvalue::Array(values, _) if values.iter().any(|value| matches!(value, Operand::Copy(_))) => None,
            Rvalue::MapOp(crate::hir::MapOp::New, ..) => MAPS,
            Rvalue::Slice(..) => Some("regions do not take slices yet"),
            // A text interpolation makes, but `x = "{x}..."`, which grows
            // `x`'s own on the heap when nothing else holds it.
            Rvalue::Concat(pieces) if !body.framed.contains(&place.local) => {
                match pieces.iter().any(|piece| matches!(piece, Piece::Taken(_))) {
                    true => Some("a region does not grow a text in place yet"),
                    false => None,
                }
            }
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
                let refused = match (array(target), mutation) {
                    (true, crate::hir::Mutation::Push) => None,
                    (true, _) => Some("a region grows an array only at its end yet"),
                    (false, _) => Some("regions do not take deques yet"),
                };
                found.push(Site { local: target.local, span: *span, what: "grows an array that ", at, refused });
                continue;
            }
            // Room a loop is about to push into, which goes where its
            // pushes go.
            Rvalue::Mutate(crate::hir::Mutation::Reserve, target, ..) if array(target) => {
                found.push(Site { local: target.local, span: *span, what: "grows an array that ", at, refused: None });
                continue;
            }
            Rvalue::Mutate(crate::hir::Mutation::Insert, target, ..) => {
                found.push(Site { local: target.local, span: *span, what: "grows a map that ", at, refused: MAPS });
                continue;
            }
            Rvalue::Call(Callee::Std(_), _) if types.needs_drop(body.place_ty(types, place)) => RUNTIME,
            // An array or a text the runtime makes, as `join` or `concat`
            // does; not an element read out of one.
            Rvalue::ArrayOp(op, ..)
                if *op != crate::hir::ArrayOp::Get && types.needs_drop(body.place_ty(types, place)) =>
            {
                RUNTIME
            }
            // A text an operation makes, as `replace` or `toUpper` does.
            Rvalue::StrOp(..)
                if matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::String) =>
            {
                TEXTS
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
    /// What it reads, and whether through a path, a part of the local.
    from: Vec<(Local, bool)>,
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
    pub(crate) fn of(body: &Body, types: &Types) -> Flow {
        let mut reached: crate::hash::FastMap<Ty, (Ty, Vec<Ty>, bool)> = crate::hash::FastMap::default();
        let mut transfers = Vec::new();
        let copies = |operands: &[Operand]| -> Vec<(Local, bool)> {
            operands
                .iter()
                .filter_map(|operand| match operand {
                    Operand::Copy(place) => Some((place.local, !place.path.is_empty())),
                    Operand::Const(_) => None,
                })
                .collect()
        };
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    let (into, part) = (place.local, !place.path.is_empty());
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
                                    Some((shown.local, !shown.path.is_empty()))
                                }
                                _ => None,
                            };
                            let taken = pieces
                                .iter()
                                .filter_map(|piece| match piece {
                                    Piece::Taken(taken) => Some((taken.local, !taken.path.is_empty())),
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
                            });
                            continue;
                        }
                        // A call of the program's keeps what it returns of
                        // what it is given, which `fate` sees.
                        Rvalue::Call(Callee::Fn(_), _) => continue,
                        // A native that reads what it is given gives a value
                        // the runtime made, which holds none of it.
                        Rvalue::Call(Callee::Std(crate::hir::StdFn::Native(index)), _)
                            if crate::hir::NATIVES.get(*index as usize).is_some_and(|native| {
                                READING.contains(&native.symbol)
                                    && (!takes_texts(native) || READING_TEXTS.contains(&native.symbol))
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
                        | Rvalue::Slice(..)
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
                                    let anything = all.iter().any(|&part| {
                                        matches!(
                                            types.kind(part),
                                            TyKind::Fn(..) | TyKind::Handle(Handle::ReflectBox, _)
                                        )
                                    });
                                    let reach = all.into_iter().skip(1).map(|part| types.repr(part)).collect();
                                    (types.repr(ty), reach, anything)
                                })
                                .clone();
                            let how = How::Other { same, reach, anything };
                            (how, used.into_iter().map(|local| (local, false)).collect())
                        }
                    };
                    if !from.is_empty() {
                        transfers.push(Transfer { into, part, how, from });
                    }
                }
                Statement::Lend(alias, place) => {
                    transfers.push(Transfer {
                        into: *alias,
                        part: false,
                        how: How::Lent,
                        from: vec![(place.local, !place.path.is_empty())],
                    });
                    // What is written through the loan is written into what
                    // it lends: `row.values = [..]` in `for mut row in rows`
                    // stores the array in `rows`.
                    transfers.push(Transfer {
                        into: place.local,
                        part: !place.path.is_empty(),
                        how: How::Whole,
                        from: vec![(*alias, false)],
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
            for &(local, _) in &transfer.from {
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
        let mut pending: Vec<u32> = self.readers[start.0 as usize].clone();
        while let Some(index) = pending.pop() {
            let transfer = &self.transfers[index as usize];
            let (mut may_be, mut may_contain) = (false, false);
            for &(from, through) in &transfer.from {
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
            if grew {
                pending.extend(&self.readers[into]);
            }
        }
        (0..count).map(|index| is[index] || contains[index]).collect()
    }
}

/// What becomes of what `start` holds in `body`. A parameter and the
/// result of a call are followed through their parts too: a caller may
/// give a value of its region, and the copy of a function that makes its
/// result in a region the caller gives makes its parts there, places this
/// function does not see.
fn fate(flow: &Flow, body: &Body, types: &Types, start: Local, summaries: &[Summary], bodies: &[Body]) -> Fate {
    let held = flow.holders(types, body, start, flow.called[start.0 as usize]);
    fate_of(flow, body, &held, summaries, bodies)
}

/// What becomes of what the locals `held` hold.
fn fate_of(flow: &Flow, body: &Body, held: &[bool], summaries: &[Summary], bodies: &[Body]) -> Fate {
    let holds = |local: Local| held[local.0 as usize];
    let reads = |operand: &Operand| matches!(operand, Operand::Copy(place) if holds(place.local));
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
                        return Fate::Kept(runtime_why(std), *span);
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
                if body.ret == Some(place.local) {
                    returned = true;
                }
            }
            Statement::DeferPush(_, place) if holds(place.local) => return kept("a `defer` keeps it", Span::default()),
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
            Summary { keeps: vec![false; body.params.len()], fresh: body.ret.is_some(), calls: Vec::new(), unseen }
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
    // As many looks as 16 rounds over all of them, should it not settle.
    let mut looks = 16 * bodies.len();
    while let Some(index) = pending.pop() {
        queued[index] = false;
        if looks == 0 {
            break;
        }
        looks -= 1;
        let body = &bodies[index];
        {
            // Parameters, from what each holds.
            let keeps: Vec<bool> = body
                .params
                .iter()
                .map(|&param| {
                    types.needs_drop(body.locals[param.0 as usize])
                        && fate(&flows[index], body, types, param, &summaries, bodies) != Fate::Here
                })
                .collect();
            // The result: fresh when what reaches it is made here, or a
            // fresh result of a call.
            let fresh = body.ret.is_some_and(|ret| {
                let mut made = all_sites[index].clone();
                made.extend(fresh_calls(&flows[index], &summaries));
                made.sort_unstable_by_key(|local| local.0);
                made.dedup();
                let flow = &flows[index];
                let held: Vec<Vec<bool>> = made.iter().map(|&local| flow.holders(types, body, local, false)).collect();
                held.iter().any(|held| held[ret.0 as usize])
                    && only_made(body, types, ret, &held)
                    && made.iter().zip(&held).all(|(&local, held)| {
                        let fate = match flow.called[local.0 as usize] {
                            true => fate(flow, body, types, local, &summaries, bodies),
                            false => fate_of(flow, body, held, &summaries, bodies),
                        };
                        !matches!(fate, Fate::Kept(..))
                    })
            });
            if keeps != summaries[index].keeps || fresh != summaries[index].fresh {
                summaries[index].keeps = keeps;
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
    // callee returns what it makes.
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
                Fate::Returned if summaries[index].fresh => Taken::Passes,
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

/// Whether a native of the runtime can be given a text, which a region may
/// make: one is taken to keep it until its code is read and it is in
/// `READING_TEXTS`, which costs a region's text nothing but its place.
fn takes_texts(native: &crate::hir::Native) -> bool {
    use crate::hir::NativeTy::String;
    native.receiver.iter().chain(native.params).any(|kind| matches!(kind, String))
}

/// The natives that can be given a text and keep none of it past the call,
/// as their code shows. A native that takes a text is in this list, in
/// `KEEPING_TEXTS` or in `UNREAD_TEXTS`, and a test holds the table of
/// natives to them; one in either of the others is taken to keep it.
const READING_TEXTS: &[&str] = &[];

/// The natives that take a text whose code is not read yet, taken to keep
/// it until it is: each moves to `READING_TEXTS` or `KEEPING_TEXTS` with
/// its case in tests/cases/run/regions, and texts in regions are done when
/// this list is empty.
const UNREAD_TEXTS: &[&str] = &[
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
    "tondo_rt_http_respond_text",
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
    "tondo_rt_json_reader",
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
    "tondo_rt_text_is_int",
    "tondo_rt_text_is_normalized",
    "tondo_rt_text_lines",
    "tondo_rt_text_lower",
    "tondo_rt_text_normalize",
    "tondo_rt_text_number_symbols",
    "tondo_rt_text_parse_float",
    "tondo_rt_text_parse_int",
    "tondo_rt_text_plural_rules",
    "tondo_rt_text_repeat",
    "tondo_rt_text_scalar_at",
    "tondo_rt_text_split",
    "tondo_rt_text_trim",
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
    const KEEPING_TEXTS: &[&str] = &[];

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
