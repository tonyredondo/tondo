//! Where each value a function makes on the heap would go if the program
//! had regions: memory a frame takes by moving a pointer and gives back
//! whole when it ends, without letting go of each value. A value would go
//! in the region of the function that makes it when nothing keeps it past
//! the call; in one its caller gives when the function only returns it,
//! fresh, and the callers that take it keep it nowhere either; and it
//! stays on the heap otherwise, for the reason said. When some calls let
//! the result go and others keep it, it would go in a region in a copy of
//! the function for the first, as copies are made for closures, and the
//! verdict names both. Nothing is built from this yet: `tondo
//! optimizations --kind region` says it, so that what regions would reach
//! is known before they are made.
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

/// Whether a build puts values in regions: while regions are made,
/// `TONDO_REGIONS_BUILD=1` in the environment of a build.
pub fn built() -> bool {
    std::env::var_os("TONDO_REGIONS_BUILD").is_some()
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

/// The verdicts for every place that makes a value on the heap, and what
/// each call does with the result of its callee; while remarks are asked
/// for, the verdicts as remarks.
pub fn regions(
    bodies: &mut Vec<Body>,
    types: &Types,
    sources: &crate::source::SourceMap,
    roots: &[usize],
) -> (Vec<Verdict>, Vec<Taking>) {
    let summaries = summaries(bodies, types, roots);
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    let mut verdicts = Vec::new();
    // Each function's places and where its verdicts start.
    let (mut all_sites, mut starts) = (Vec::new(), Vec::new());
    for (index, body) in bodies.iter().enumerate() {
        let sites = sites(body, types);
        let start = verdicts.len();
        for site in &sites {
            let (local, span, what) = (site.local, site.span, site.what);
            let name = &body.name;
            let mut by_call = false;
            let fate = fate(body, types, local, &summaries, bodies);
            let own = fate == Fate::Here;
            let (region, text, by) = match fate {
                Fate::Here => (true, format!("{what}would go in a region of `{name}`"), None),
                Fate::Returned if !summaries[index].fresh => {
                    (false, format!("{what}stays on the heap: `{name}` returns it with what it was given"), None)
                }
                Fate::Returned => {
                    let (mut drops, mut keeps) = (Vec::new(), Vec::new());
                    let mut seen = vec![false; bodies.len()];
                    resolve(index, &summaries, &mut seen, &mut drops, &mut keeps);
                    match (drops.is_empty(), keeps.first()) {
                        (false, None) => {
                            (true, format!("{what}would go in a region that the caller of `{name}` gives"), None)
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
                                "{what}would go in a region that the caller of `{name}` gives, in a copy of it for the \
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
        let mut region: Vec<bool> = verdicts[start..].iter().map(|verdict| verdict.region).collect();
        for (at, (why, by)) in holding(body, types, &loans, &sites, &mut region, &fresh) {
            let verdict = &mut verdicts[start + at];
            let what = sites[at].what;
            verdict.region = false;
            verdict.by_call = false;
            verdict.own = false;
            verdict.text = format!("{what}stays on the heap: {why}");
            verdict.by = Some(by).filter(|by| *by != verdict.span);
        }
        all_sites.push(sites);
        starts.push(start);
    }
    if built() {
        place(bodies, types, &summaries, &loans, &all_sites, &starts, &verdicts);
    }
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
    if crate::mir_text::remarking() {
        for verdict in &verdicts {
            let body = bodies.iter_mut().find(|body| body.name == verdict.function);
            if let Some(body) = body {
                let text = verdict.text.clone();
                crate::mir_text::remark_by(body, verdict.span, "region", verdict.by, || text);
            }
        }
    }
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
    loans: &[Vec<bool>],
    all_sites: &[Vec<Site>],
    starts: &[usize],
    verdicts: &[Verdict],
) {
    let count = bodies.len();
    // What each call does with its result, by caller and statement.
    let mut taking: std::collections::HashMap<(usize, (u32, u32)), bool> = std::collections::HashMap::new();
    for summary in summaries {
        for call in &summary.calls {
            match call.taken {
                Taken::Drops => _ = taking.insert((call.caller, call.at), true),
                Taken::Passes => _ = taking.insert((call.caller, call.at), false),
                Taken::Keeps(..) => {}
            }
        }
    }
    // While regions are made: only the functions whose names hold
    // TONDO_REGIONS_ONLY take part, to find one that goes wrong.
    let only = std::env::var("TONDO_REGIONS_ONLY").ok();
    let takes_part: Vec<bool> = bodies
        .iter()
        .map(|body| only.as_ref().is_none_or(|only| only.split(',').any(|only| body.name.contains(only))))
        .collect();
    // A copy for each function whose result a call lets go, through the
    // callers that return it.
    let mut copied = vec![false; count];
    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..count {
            if copied[index] || !summaries[index].fresh || !takes_part[index] {
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
    }
    let lifetimes: Vec<Lifetimes> = bodies.iter().map(Lifetimes::of).collect();
    // The region of each call that lets its result go, by statement.
    let call_regions: Vec<std::collections::HashMap<(u32, u32), u32>> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            call_results(body)
                .into_iter()
                .filter(|&(block, at, ..)| taking.get(&(index, (block as u32, at as u32))) == Some(&true))
                .filter_map(|(block, at, local, ..)| {
                    let region = lifetimes[index].region(body, types, local, block, true, None)?;
                    Some(((block as u32, at as u32), region))
                })
                .collect()
        })
        .collect();
    // Whether a copy's result is all its region's, from all and down.
    let mut whole = copied.clone();
    let mut own: Vec<Vec<((u32, u32), u32)>> = vec![Vec::new(); count];
    let mut copy_own: Vec<Vec<((u32, u32), u32)>> = vec![Vec::new(); count];
    let mut copy_given: Vec<Vec<(u32, u32)>> = vec![Vec::new(); count];
    loop {
        let mut shrunk = false;
        for index in 0..count {
            let (body, sites) = (&bodies[index], &all_sites[index]);
            let verdicts = &verdicts[starts[index]..starts[index] + sites.len()];
            let regions: Vec<Option<u32>> = sites
                .iter()
                .zip(verdicts)
                .map(|(site, verdict)| match takes_part[index] && site.placed && verdict.own {
                    true => lifetimes[index].region(body, types, site.local, site.at.0 as usize, false, Some(site)),
                    false => None,
                })
                .collect();
            let lets_go = |at: (u32, u32), callee: usize| whole[callee] && call_regions[index].contains_key(&at);
            let mut placed: Vec<bool> = regions.iter().map(Option::is_some).collect();
            holding(body, types, loans, sites, &mut placed, &lets_go);
            own[index] = sites
                .iter()
                .zip(&regions)
                .zip(&placed)
                .filter_map(|((site, region), placed)| region.filter(|_| *placed).map(|region| (site.at, region)))
                .collect();
            if !copied[index] {
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
                .map(|((site, verdict), region)| site.placed && verdict.region && (!verdict.own || region.is_some()))
                .collect();
            holding(body, types, loans, sites, &mut placed, &gives);
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
            let made = made(body, types, loans, sites, &placed, &gives);
            if whole[index] && !body.ret.is_some_and(|ret| made[ret.0 as usize]) {
                whole[index] = false;
                shrunk = true;
            }
        }
        if !shrunk {
            break;
        }
    }
    // The copies, after the functions, and the calls that call them.
    let mut copy_of = vec![None; count];
    for index in 0..count {
        if copied[index] {
            let mut copy = bodies[index].clone();
            copy.takes_region = true;
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
}

/// Where a value a function lets go may go: its loops, the innermost
/// first, the locals live at each one's head and where it is left.
struct Lifetimes {
    loops: Vec<(usize, Vec<bool>)>,
    live_head: Vec<Vec<bool>>,
    live_out: Vec<Vec<bool>>,
    looped: Vec<bool>,
}

impl Lifetimes {
    fn of(body: &Body) -> Lifetimes {
        let loops = crate::hoist::loops(body);
        let count = body.locals.len();
        let live: Vec<Vec<bool>> = crate::mir::live_in(&body.blocks, count)
            .into_iter()
            .map(|locals| {
                let mut set = vec![false; count];
                for local in locals {
                    set[local.0 as usize] = true;
                }
                set
            })
            .collect();
        let mut looped = vec![false; body.blocks.len()];
        let (mut live_head, mut live_out) = (Vec::new(), Vec::new());
        for (head, inside) in &loops {
            live_head.push(live[*head].clone());
            let mut out = vec![false; count];
            for (block, data) in body.blocks.iter().enumerate().filter(|&(block, _)| inside[block]) {
                looped[block] = true;
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

    /// The region a value made in `block` would go in: the function's, 0,
    /// outside every loop; else that of the innermost loop whose turn it
    /// does not outlive, nothing that holds it live at the loop's head nor
    /// where the loop is left, `k + 1`; or the function's for an array that
    /// lives across the turns and grows (`bounded`); or none.
    fn region(
        &self,
        body: &Body,
        types: &Types,
        local: Local,
        block: usize,
        parts: bool,
        grows: Option<&Site>,
    ) -> Option<u32> {
        if !self.looped[block] {
            return Some(0);
        }
        let held = holders(body, types, local, parts);
        let outlives = |live: &[bool]| held.iter().zip(live).any(|(&held, &live)| held && live);
        for (k, (_, inside)) in self.loops.iter().enumerate() {
            if inside[block] && !outlives(&self.live_head[k]) && !outlives(&self.live_out[k]) {
                return Some(k as u32 + 1);
            }
        }
        grows.filter(|site| bounded(body, &self.looped, site)).map(|_| 0)
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

/// The places a region would take that hold a value it did not make, why,
/// and where they take it, until none is left: a value of a region holds
/// only values of a region, its own or one that outlives it, or values that
/// count nothing, as when the region goes it goes whole, letting go of
/// nothing its values hold. A value the function was given, or that a call
/// keeps a count of, would stay held, and leak.
fn holding(
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    sites: &[Site],
    region: &mut [bool],
    call_made: &dyn Fn((u32, u32), usize) -> bool,
) -> Vec<(usize, (String, Span))> {
    let counted = |place: &Place| types.needs_drop(body.place_ty(types, place));
    let mut found = Vec::new();
    loop {
        let made = made(body, types, loans, sites, region, call_made);
        let mut changed = false;
        for (at, site) in sites.iter().enumerate() {
            let (local, span) = (site.local, site.span);
            if !region[at] {
                continue;
            }
            // The locals that are the value: its copies, whole.
            let mut is = vec![false; body.locals.len()];
            is[local.0 as usize] = true;
            let mut grew = true;
            while grew {
                grew = false;
                for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
                    let into = match statement {
                        Statement::Assign(place, rvalue, _) if place.path.is_empty() => match &**rvalue {
                            Rvalue::Use(Operand::Copy(from)) if from.path.is_empty() && is[from.local.0 as usize] => {
                                place.local
                            }
                            _ => continue,
                        },
                        Statement::Lend(alias, place) if place.path.is_empty() && is[place.local.0 as usize] => *alias,
                        _ => continue,
                    };
                    if !is[into.0 as usize] {
                        is[into.0 as usize] = true;
                        grew = true;
                    }
                }
            }
            // What is stored in it: by the statement that makes it, an
            // element or a field written, or what grows it; and an array
            // that grows here holds what it held before, which a region
            // must have made too.
            let mut outside = (site.what.starts_with("grows") && !made[local.0 as usize]).then_some(span);
            for (at_statement, statement) in positioned(body) {
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
    body: &Body,
    types: &Types,
    loans: &[Vec<bool>],
    sites: &[Site],
    region: &[bool],
    call_made: &dyn Fn((u32, u32), usize) -> bool,
) -> Vec<bool> {
    let counted = |local: Local| types.needs_drop(body.locals[local.0 as usize]);
    let mut made = vec![true; body.locals.len()];
    for &param in &body.params {
        made[param.0 as usize] = !counted(param);
    }
    let taken = |local: Local, at: (u32, u32)| {
        sites.iter().zip(region).any(|(site, &region)| region && site.local == local && site.at == at)
    };
    let mut changed = true;
    while changed {
        changed = false;
        for (at, statement) in positioned(body) {
            // What may make a local hold an array of the heap: a change of
            // it, which copies it first when it is shared and may grow it,
            // unless a region grows it; and a write of one of its elements,
            // which copies it first too.
            let changed_here = match statement {
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    Rvalue::Mutate(_, target, ..) => Some((target.local, taken(target.local, at))),
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
                let lent: Vec<Local> = match &**rvalue {
                    Rvalue::Call(Callee::Fn(id), args) => args
                        .iter()
                        .zip(&loans[id.0 as usize])
                        .filter(|&(_, &loan)| loan)
                        .filter_map(|(arg, _)| match arg {
                            Operand::Copy(place) => Some(place.local),
                            Operand::Const(_) => None,
                        })
                        .collect(),
                    // The runtime reads what it is given; C and a function
                    // value may take anything as a loan.
                    Rvalue::Call(Callee::Std(_), _) => Vec::new(),
                    Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) => args
                        .iter()
                        .filter_map(|arg| match arg {
                            Operand::Copy(place) => Some(place.local),
                            Operand::Const(_) => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for local in lent {
                    if made[local.0 as usize] && counted(local) {
                        made[local.0 as usize] = false;
                        changed = true;
                    }
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
                    let site = sites.iter().any(|site| site.local == place.local && site.at == at);
                    let from_region = taken(place.local, at)
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
    /// What it is in words, before "would go" or "stays".
    what: &'static str,
    /// Its statement, by block and position.
    at: (u32, u32),
    /// Whether a region can take it today: an array, a box, an array that
    /// grows by `push`.
    placed: bool,
}

/// The places the function makes a value on the heap in: an array of
/// values that are not all constants, a map, a slice, a box, an array that
/// grows, a text, and the arrays, texts and other values the runtime gives.
fn sites(body: &Body, types: &Types) -> Vec<Site> {
    let mut found = Vec::new();
    let array =
        |place: &Place| matches!(types.kind(types.repr(body.place_ty(types, place))), crate::types::TyKind::Array(_));
    for (at, statement) in positioned(body) {
        let Statement::Assign(place, rvalue, span) = statement else { continue };
        let mut placed = false;
        let what = match &**rvalue {
            Rvalue::Array(values, _) if values.iter().any(|value| matches!(value, Operand::Copy(_))) => {
                placed = true;
                ""
            }
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
                placed = true;
                ""
            }
            Rvalue::Mutate(
                mutation @ (crate::hir::Mutation::Push
                | crate::hir::Mutation::PushFront
                | crate::hir::Mutation::InsertAt),
                target,
                ..,
            ) => {
                let placed = *mutation == crate::hir::Mutation::Push && array(target);
                found.push(Site { local: target.local, span: *span, what: "grows an array that ", at, placed });
                continue;
            }
            // Room a loop is about to push into, which goes where its
            // pushes go.
            Rvalue::Mutate(crate::hir::Mutation::Reserve, target, ..) if array(target) => {
                found.push(Site { local: target.local, span: *span, what: "grows an array that ", at, placed: true });
                continue;
            }
            Rvalue::Mutate(crate::hir::Mutation::Insert, target, ..) => {
                found.push(Site { local: target.local, span: *span, what: "grows a map that ", at, placed: false });
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
            found.push(Site { local: place.local, span: *span, what, at, placed });
        }
    }
    found
}

/// The locals that hold what `start` holds: that are it, a copy of it, or
/// that contain it, as an array it was pushed into or a record made of it,
/// as copies, parts or values made of it pass it on. A part read out of the
/// value itself is another value, which it does not hold, unless `parts`
/// says its parts are its memory; one read out of what contains it may be
/// it. What another operation gives, as the
/// runtime's `trim` that may give back the text it was lent, may be it when
/// it is of its type, and contain it when its type can refer to it.
fn holders(body: &Body, types: &Types, start: Local, parts: bool) -> Vec<bool> {
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
                            // A part: of the value itself, its memory; of
                            // what contains it, maybe the value.
                            may_be |= inside && whole_value;
                            may_contain |= inside || (it && parts);
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

/// What becomes of what `start` holds in `body`. A parameter and the
/// result of a call are followed through their parts too: a caller may
/// give a value of its region, and the copy of a function that makes its
/// result in a region the caller gives makes its parts there, places this
/// function does not see.
fn fate(body: &Body, types: &Types, start: Local, summaries: &[Summary], bodies: &[Body]) -> Fate {
    let called = body.params.contains(&start)
        || body.blocks.iter().flat_map(|block| &block.stmts).any(|statement| {
            matches!(statement, Statement::Assign(place, rvalue, _)
                if place.local == start && place.path.is_empty() && matches!(**rvalue, Rvalue::Call(Callee::Fn(_), _)))
        });
    let held = holders(body, types, start, called);
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
/// parameter is kept when its value may be, and a result is fresh when
/// only what the function made reaches it; then what each call does with
/// the result it takes.
fn summaries(bodies: &[Body], types: &Types, roots: &[usize]) -> Vec<Summary> {
    // The functions that run: those code outside the program's calls, those
    // called through a value, whose calls are not seen, and what they call
    // by name in turn. A function inlined in every caller runs no more.
    let count = bodies.len();
    let (mut valued, mut callees) = (vec![false; count], vec![Vec::new(); count]);
    for (index, body) in bodies.iter().enumerate() {
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            let id = match statement {
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    Rvalue::Call(Callee::Fn(id), _) => {
                        callees[index].push(id.0 as usize);
                        continue;
                    }
                    Rvalue::FnValue(id)
                    | Rvalue::Callback(id, _)
                    | Rvalue::Erase(_, id, _)
                    | Rvalue::CallOsr(id, ..) => id,
                    _ => continue,
                },
                Statement::DeferPush(id, _) => id,
                _ => continue,
            };
            if let Some(valued) = valued.get_mut(id.0 as usize) {
                *valued = true;
            }
        }
    }
    let mut live = vec![false; count];
    let mut pending: Vec<usize> = roots.iter().copied().chain((0..count).filter(|&index| valued[index])).collect();
    while let Some(index) = pending.pop() {
        if index < count && !std::mem::replace(&mut live[index], true) {
            pending.extend(&callees[index]);
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
                let mut made = sites(body, types).into_iter().map(|site| site.local).collect::<Vec<_>>();
                made.extend(fresh_calls(body, &summaries));
                let reached = made.iter().any(|&local| holders(body, types, local, false)[ret.0 as usize]);
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
    }
    // What each call that runs does with the result it takes.
    for (index, body) in bodies.iter().enumerate().filter(|&(index, _)| live[index]) {
        for (block, statement, local, callee, span) in call_results(body) {
            let mut written = span;
            let mut scope = body.scopes.get(block).copied().flatten();
            while let Some(at) = scope {
                written = body.inlined[at as usize].call;
                scope = body.inlined[at as usize].parent;
            }
            let taken = match fate(body, types, local, &summaries, bodies) {
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

/// The locals that take the result of a call of the program's, with the
/// block, the callee and the call.
fn call_results(body: &Body) -> Vec<(usize, usize, Local, usize, Span)> {
    let mut found = Vec::new();
    for (block, data) in body.blocks.iter().enumerate() {
        for (at, statement) in data.stmts.iter().enumerate() {
            if let Statement::Assign(place, rvalue, span) = statement
                && place.path.is_empty()
                && let Rvalue::Call(Callee::Fn(id), _) = &**rvalue
            {
                found.push((block, at, place.local, id.0 as usize, *span));
            }
        }
    }
    found
}

/// The locals that take a fresh result of a call.
fn fresh_calls(body: &Body, summaries: &[Summary]) -> Vec<Local> {
    call_results(body)
        .into_iter()
        .filter(|&(_, _, _, callee, _)| summaries[callee].fresh)
        .map(|(_, _, local, ..)| local)
        .collect()
}

/// Whether every value written whole into `ret`, or into a local that
/// reaches it, comes from what the function made, or holds no count.
fn only_made(body: &Body, types: &Types, ret: Local, made: &[Local]) -> bool {
    let mut allowed = vec![false; body.locals.len()];
    for &local in made {
        for (index, held) in holders(body, types, local, false).into_iter().enumerate() {
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
