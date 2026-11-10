//! Checks the MIR that lowering and each pass of a release build leave, in
//! a debug build of `tondo`, as the tests run it: no local is read on a
//! path that has not assigned it, which the generated code would read as
//! zeros. A local starts as zeros, so releasing one that no path assigned
//! releases nothing, and reading one whose only value is zeros, as `Unit`
//! or `!Never`, reads that value: those are not reads here.

use crate::mir::*;
use crate::types::{Ty, TyKind, Types};

/// Whether the only value of a type is zeros: what holds nothing, and a
/// result that cannot fail of it, which a function that gives nothing
/// gives where one is expected.
fn nothing(types: &Types, ty: Ty) -> bool {
    match types.kind(types.repr(ty)) {
        TyKind::Unit | TyKind::Never | TyKind::Error => true,
        TyKind::Result(ok, error) => nothing(types, *ok) && types.repr(*error) == Ty::NEVER,
        TyKind::Tuple(items) => items.iter().all(|&item| nothing(types, item)),
        _ => false,
    }
}

/// Panics, in a debug build of `tondo`, at the first body that reads a
/// local before every path to the read assigns it, after `stage`, with
/// its MIR.
pub fn verify(stage: &str, bodies: &[Body], types: &Types, sources: &crate::source::SourceMap) {
    if !cfg!(debug_assertions) {
        return;
    }
    for body in bodies {
        if let Some(problem) = unassigned(body, types) {
            let text = crate::mir_text::text(body, types, sources);
            panic!("the MIR of `{}` after {stage} {problem}\n{text}", body.name);
        }
    }
}

/// A set of locals, one bit each.
#[derive(Clone, PartialEq)]
struct Set(Vec<u64>);

impl Set {
    fn new(count: usize, all: bool) -> Set {
        Set(vec![if all { u64::MAX } else { 0 }; count.div_ceil(64)])
    }

    fn has(&self, local: Local) -> bool {
        self.0[local.0 as usize / 64] >> (local.0 % 64) & 1 == 1
    }

    fn add(&mut self, local: Local) {
        self.0[local.0 as usize / 64] |= 1 << (local.0 % 64);
    }

    fn meet(&mut self, other: &Set) {
        self.0.iter_mut().zip(&other.0).for_each(|(a, b)| *a &= b);
    }
}

/// What a statement reads and assigns, for this check.
fn effects(statement: &Statement) -> (Vec<Local>, Vec<Local>) {
    let (mut used, mut defined) = (Vec::new(), Vec::new());
    statement_locals(statement, &mut used, &mut defined);
    if matches!(statement, Statement::Release(_)) {
        used.clear();
    }
    // A `select` writes a case's local when the case completes, and the
    // code after it reads it only there.
    if let Statement::Assign(_, rvalue, _) = statement
        && let Rvalue::Select(cases, ..) = &**rvalue
    {
        used.retain(|local| !cases.iter().any(|case| case.out == *local));
        defined.extend(cases.iter().map(|case| case.out));
    }
    (used, defined)
}

/// Where a body first reads a local some path to the read has not
/// assigned, in the order of its blocks.
pub fn unassigned(body: &Body, types: &Types) -> Option<String> {
    let (count, blocks) = (body.locals.len(), body.blocks.len());
    if blocks == 0 {
        return None;
    }
    let empty: Vec<bool> = body.locals.iter().map(|&ty| nothing(types, ty)).collect();
    let successors = |block: usize| match &body.blocks[block].terminator {
        Terminator::Goto(to) => vec![to.0 as usize],
        Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
        _ => Vec::new(),
    };
    // What every path to the start of each block assigns; a block no path
    // reaches has none.
    let mut entry: Vec<Option<Set>> = vec![None; blocks];
    let mut start = Set::new(count, false);
    body.params.iter().chain(&body.osr).for_each(|&local| start.add(local));
    entry[0] = Some(start);
    let step = |statement: &Statement, set: &mut Set| {
        // A live build's frame takes every local from the old code.
        if matches!(statement, Statement::OsrLoad(_)) {
            *set = Set::new(count, true);
        }
        effects(statement).1.into_iter().for_each(|local| set.add(local));
    };
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks {
            let Some(mut set) = entry[block].clone() else { continue };
            body.blocks[block].stmts.iter().for_each(|statement| step(statement, &mut set));
            for to in successors(block) {
                let met = match &entry[to] {
                    None => set.clone(),
                    Some(old) => {
                        let mut met = old.clone();
                        met.meet(&set);
                        met
                    }
                };
                if entry[to].as_ref() != Some(&met) {
                    entry[to] = Some(met);
                    changed = true;
                }
            }
        }
    }
    let unread = |set: &Set, used: &[Local]| used.iter().copied().find(|&l| !set.has(l) && !empty[l.0 as usize]);
    for (block, set) in entry.into_iter().enumerate() {
        let Some(mut set) = set else { continue };
        for (index, statement) in body.blocks[block].stmts.iter().enumerate() {
            if !matches!(statement, Statement::OsrLoad(_))
                && let Some(local) = unread(&set, &effects(statement).0)
            {
                return Some(format!("reads l{} in b{block}[{index}] before every path to it assigns it", local.0));
            }
            step(statement, &mut set);
        }
        let mut used = Vec::new();
        terminator_uses(&body.blocks[block].terminator, &mut used);
        if let Some(local) = unread(&set, &used) {
            return Some(format!("reads l{} at the end of b{block} before every path to it assigns it", local.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program's MIR passes, and fails once the assignment of a local
    /// read later is taken out.
    #[test]
    fn finds_a_read_no_path_assigns() {
        let dir = std::env::temp_dir().join(format!("tondo-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.to");
        let source = "fn twice(n: Int): Int {\n    n * 2\n}\n\nfn main() {\n    let total = twice(21)\n    \
            if total > 40 {\n        panic(\"{total}\")\n    }\n}\n";
        std::fs::write(&path, source).unwrap();
        let analysis = crate::driver::analyze(&path).unwrap();
        let program = analysis.program.as_ref().unwrap();
        let mut bodies = crate::lower::lower(program, false, false, None);
        let main = bodies.iter_mut().find(|body| body.name == "main").unwrap();
        assert_eq!(unassigned(main, &program.types), None);
        let block = main.blocks.iter_mut().find(|block| {
            block.stmts.iter().any(
                |s| matches!(s, Statement::Assign(p, r, _) if p.path.is_empty() && matches!(**r, Rvalue::Call(..))),
            )
        });
        let block = block.unwrap();
        block.stmts.retain(
            |s| !matches!(s, Statement::Assign(p, r, _) if p.path.is_empty() && matches!(**r, Rvalue::Call(..))),
        );
        let found = unassigned(main, &program.types).expect("the read of what the call gave");
        assert!(found.contains("before every path to it assigns it"), "{found}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
