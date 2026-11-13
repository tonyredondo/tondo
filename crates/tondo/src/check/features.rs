//! What each function needs of the processor beyond the build's level
//! (toolchain 7.8, rules 10 to 12): the features of the instructions of
//! `std.cpu.x64` and `std.cpu.arm64` in its body, wherever they are, and
//! those of the functions it calls outside a guard, worked out over the
//! whole program; and the errors of the functions that may need none.

use super::*;
use crate::instructions::Instruction;

/// What may bring features into a function: one of its instructions, or a
/// call, with the features the guards around it test and where it is.
#[derive(Clone, Copy)]
struct Source {
    brings: Brings,
    guards: u32,
    span: Span,
}

#[derive(Clone, Copy)]
enum Brings {
    Instruction(Instruction),
    Call(FnId),
}

impl Checker<'_> {
    /// Sets the `needs` of every function, and reports those that may need
    /// nothing but do: `main`, the tests, a `pub extern fn`, a method that
    /// implements a trait, and a function or closure used as a value.
    pub(super) fn check_features(&mut self, functions: &mut [hir::Function], main: Option<FnId>, closures: &[FnId]) {
        let level = crate::driver::checked_level();
        let beyond = |instruction| {
            crate::instructions::feature(instruction, level.arch).filter(|&feature| level.has(feature) != Some(true))
        };
        // Most programs use no instruction beyond their level.
        if !self.instructions_used.iter().any(|&instruction| beyond(instruction).is_some()) {
            return;
        }
        // The functions used as values, where first, and whether `spawn`
        // starts them.
        let mut values: HashMap<FnId, (Span, bool)> = HashMap::new();
        let sources: Vec<Vec<Source>> = functions
            .iter()
            .map(|function| {
                let mut found = Vec::new();
                sources(&function.body, 0, &mut found, &mut values, &mut HashSet::new());
                found
            })
            .collect();
        let own = |source: &Source| match source.brings {
            Brings::Instruction(instruction) => beyond(instruction).map_or(0, |feature| 1 << feature),
            Brings::Call(_) => 0,
        };
        let mut needs: Vec<u32> = sources.iter().map(|found| found.iter().map(own).fold(0, |a, b| a | b)).collect();
        // What a call brings is what the callee needs and its guards do not
        // test, until nothing changes.
        let mut changed = true;
        while changed {
            changed = false;
            for (id, found) in sources.iter().enumerate() {
                let mut need = needs[id];
                for source in found {
                    if let Brings::Call(callee) = source.brings {
                        need |= needs.get(callee.0 as usize).copied().unwrap_or(0) & !source.guards;
                    }
                }
                if need != needs[id] {
                    needs[id] = need;
                    changed = true;
                }
            }
        }
        for (function, need) in functions.iter_mut().zip(&needs) {
            function.needs = *need;
        }
        // Rule 12: where needing a feature is an error, and why.
        let mut forbidden: Vec<(FnId, String)> = Vec::new();
        if let Some(main) = main {
            forbidden.push((main, "`main`, or the top level of a script, cannot need one".into()));
        }
        for (id, function) in functions.iter().enumerate() {
            if function.test {
                forbidden.push((FnId(id as u32), "a test cannot need one".into()));
            }
        }
        for export in &self.exports {
            forbidden.push((export.function, "a `pub extern fn`, which C calls, cannot need one".into()));
        }
        for imp in self.impls.iter().filter(|imp| !imp.intrinsic) {
            let name = &self.traits[imp.trait_id.0 as usize].name;
            for &method in imp.methods.iter().flatten() {
                forbidden.push((method, format!("a method that implements `{name}` cannot need one")));
            }
        }
        for (&id, &(span, spawned)) in &values {
            let line = self.sources.position(span.start).1;
            let why = match spawned {
                true => format!("what `spawn` starts, as on line {line}, cannot need one"),
                false => format!("a function used as a value, as on line {line}, cannot need one"),
            };
            forbidden.push((id, why));
        }
        for &id in closures {
            forbidden.push((id, "a closure, which is a value, cannot need one".into()));
        }
        // What a source brings of the features, under its guards.
        let brought = |source: &Source| match source.brings {
            Brings::Instruction(instruction) => beyond(instruction).map_or(0, |feature| 1 << feature),
            Brings::Call(callee) => needs[callee.0 as usize] & !source.guards,
        };
        let mut reported = HashSet::new();
        for (id, why) in forbidden {
            if needs.get(id.0 as usize).is_none_or(|&need| need == 0) || !reported.insert(id) {
                continue;
            }
            // Each call or instruction that brings a feature in, with the
            // chain of calls to the instruction that needs it.
            for first in sources[id.0 as usize].iter().filter(|source| brought(source) != 0) {
                let feature = brought(first).trailing_zeros();
                // The shortest chain of calls to an instruction, as recursion
                // may lead back to where the chain started.
                let mut chain = vec![id];
                let instruction = match first.brings {
                    Brings::Instruction(instruction) => instruction,
                    Brings::Call(callee) => {
                        let mut from: HashMap<FnId, FnId> = HashMap::from([(callee, id)]);
                        let mut queue = std::collections::VecDeque::from([callee]);
                        let found = 'search: loop {
                            let Some(at) = queue.pop_front() else { break None };
                            for source in sources[at.0 as usize].iter().filter(|s| brought(s) & (1 << feature) != 0) {
                                match source.brings {
                                    Brings::Instruction(instruction) => break 'search Some((at, instruction)),
                                    Brings::Call(next) => {
                                        if next != id && !from.contains_key(&next) {
                                            from.insert(next, at);
                                            queue.push_back(next);
                                        }
                                    }
                                }
                            }
                        };
                        let Some((mut at, instruction)) = found else { continue };
                        let mut path = Vec::new();
                        while at != id {
                            path.push(at);
                            at = from[&at];
                        }
                        chain.extend(path.into_iter().rev());
                        instruction
                    }
                };
                let chain: Vec<String> =
                    chain.iter().map(|f| format!("`{}`", shown_name(&functions[f.0 as usize].name))).collect();
                let feature = crate::target::FEATURE_NAMES[feature as usize];
                let called = crate::instructions::name(instruction, level.arch);
                let message = match chain.len() {
                    1 => format!(
                        "`{called}` needs `{feature}` of the processor, and {why}: write it in a function of its own, and call that under `if cpu.has(cpu.Feature.{feature}) {{ ... }}`"
                    ),
                    _ => format!(
                        "this call needs `{feature}` of the processor, and {why}: {}, which uses `{called}`; put the call under `if cpu.has(cpu.Feature.{feature}) {{ ... }}`",
                        chain[0].clone() + " calls " + &chain[1..].join(", which calls ")
                    ),
                };
                self.diags.error("E1809", first.span, message);
            }
        }
    }
}

/// The name of a function as its code calls it: `x64.depositBits`, not
/// `std.cpu.x64.depositBits`.
fn shown_name(name: &str) -> &str {
    name.strip_prefix("std.cpu.").unwrap_or(name)
}

/// The features a condition tests when it is a guard (rule 11): `cpu.has`
/// of a variant written there, or several joined with `and`.
fn guarded(condition: &hir::Expr) -> Option<u32> {
    match &condition.kind {
        hir::ExprKind::Call(hir::Callee::Std(StdFn::CpuHas), _, args) => match &args[0].kind {
            hir::ExprKind::Variant(feature, fields) if fields.is_empty() => Some(1 << feature),
            _ => None,
        },
        hir::ExprKind::Binary(BinOp::And, left, right) => Some(guarded(left)? | guarded(right)?),
        _ => None,
    }
}

/// What may bring features into a block, under `guards`, and the functions
/// it uses as values. A guard's block is walked first, with its features;
/// `seen` keeps what it walked from being walked again without them.
fn sources(
    block: &hir::Block,
    guards: u32,
    found: &mut Vec<Source>,
    values: &mut HashMap<FnId, (Span, bool)>,
    seen: &mut HashSet<*const hir::Expr>,
) {
    super::reach::each_expr(block, &mut |e| {
        if !seen.insert(e as *const hir::Expr) {
            return;
        }
        let brings = match &e.kind {
            hir::ExprKind::If(condition, then, _) if let Some(tested) = guarded(condition) => {
                sources(then, guards | tested, found, values, seen);
                return;
            }
            hir::ExprKind::Call(hir::Callee::Std(StdFn::Cpu(instruction)), _, _) => Brings::Instruction(*instruction),
            hir::ExprKind::Call(hir::Callee::Fn(callee), _, _) => Brings::Call(*callee),
            hir::ExprKind::Spawn(started) if let hir::ExprKind::Erase(_, id) = started.kind => {
                values.entry(id).or_insert((e.span, true));
                return;
            }
            hir::ExprKind::FnValue(id, _) | hir::ExprKind::Erase(_, id) | hir::ExprKind::Callback(_, id) => {
                values.entry(*id).or_insert((e.span, false));
                return;
            }
            _ => return,
        };
        found.push(Source { brings, guards, span: e.span });
    });
}
