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
pub(crate) fn nothing(types: &Types, ty: Ty) -> bool {
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
        if let Some(problem) = unassigned(body, types).or_else(|| moved_read(body)).or_else(|| taken_once(body)) {
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

/// Where a body reads a part of a local that a move zeroed: on every way on
/// from a `moved`, the local is written whole, released, or copied whole
/// into another that then is, as `old = x; x = new; release old` does,
/// before anything reads the part.
pub fn moved_read(body: &Body) -> Option<String> {
    for (block, data) in body.blocks.iter().enumerate() {
        for (at, statement) in data.stmts.iter().enumerate() {
            let Statement::Moved(moved) = statement else { continue };
            if let Some(problem) = read_after_move(body, block, at + 1, moved) {
                return Some(format!("{problem} after `moved` in b{block} zeroed it"));
            }
        }
    }
    None
}

/// Whether `a` and `b` may name the same memory: one is a part of the
/// other, or they are the same.
fn overlap(a: &Place, b: &Place) -> bool {
    a.local == b.local
        && a.path.iter().zip(&b.path).all(|pair| match pair {
            (Proj::Field(i), Proj::Field(j)) => i == j,
            (Proj::Variant(v, i), Proj::Variant(w, j)) => v != w || i == j,
            _ => true,
        })
}

fn read_after_move(body: &Body, block: usize, from: usize, moved: &Place) -> Option<String> {
    let mut seen: std::collections::HashSet<(usize, Local)> = std::collections::HashSet::new();
    let mut ways = vec![(block, from, moved.clone())];
    while let Some((block, from, mut place)) = ways.pop() {
        let data = &body.blocks[block];
        let mut done = false;
        for (index, statement) in data.stmts.iter().enumerate().skip(from) {
            // The local copied whole, and written whole next without being
            // read: its old value, with the part zeroed, is the copy's.
            if let Statement::Assign(target, value, _) = statement
                && let Rvalue::Use(Operand::Copy(source)) = &**value
                && source.local == place.local
                && source.path.is_empty()
                && target.path.is_empty()
                && target.local != place.local
            {
                let names = |s: &Statement| {
                    let (mut used, mut defined) = (Vec::new(), Vec::new());
                    statement_locals(s, &mut used, &mut defined);
                    (used.contains(&place.local), defined.contains(&place.local))
                };
                if data.stmts[index + 1..]
                    .iter()
                    .find(|s| names(s) != (false, false))
                    .is_some_and(|s| names(s) == (false, true))
                {
                    place = Place { local: target.local, path: place.path.clone() };
                    continue;
                }
            }
            // The moved part, or a part of it, copied into a whole local,
            // as `split` takes a record's old value field by field: the copy
            // holds the zero from then on, and is followed too.
            if let Statement::Assign(target, value, _) = statement
                && let Rvalue::Use(Operand::Copy(source)) = &**value
                && source.local == place.local
                && source.path.len() >= place.path.len()
                && source.path[..place.path.len()] == place.path[..]
                && target.path.is_empty()
            {
                if seen.insert((block, target.local)) {
                    ways.push((block, index + 1, Place::local(target.local)));
                }
                continue;
            }
            match statement {
                // Released whole, or by a part that holds the moved one.
                Statement::Release(released)
                    if released.local == place.local
                        && released.path.len() <= place.path.len()
                        && place.path[..released.path.len()] == released.path[..] =>
                {
                    done = true;
                    break;
                }
                Statement::Moved(again) if overlap(again, &place) => continue,
                Statement::Clear(local) if *local == place.local => {
                    done = true;
                    break;
                }
                Statement::Assign(target, _, _) if target.local == place.local && target.path.is_empty() => {
                    done = true;
                    break;
                }
                // Emptied whole, which gives it a value of its own again.
                Statement::Assign(_, value, _)
                    if matches!(&**value, Rvalue::Mutate(crate::hir::Mutation::Empty, target, ..)
                        if target.local == place.local && target.path.is_empty()) =>
                {
                    done = true;
                    break;
                }
                _ => {}
            }
            // A tag is not the payload moved out of it.
            if let Statement::Assign(target, value, _) = statement
                && let Rvalue::Discriminant(of) = &**value
                && of.local == place.local
                && of.path.len() < place.path.len()
                && place.path[..of.path.len()] == of.path[..]
                && !overlap(target, &place)
            {
                continue;
            }
            let mut read = false;
            crate::inline::visit_statement(&mut statement.clone(), &mut |seen_place: &mut Place| {
                read |= overlap(seen_place, &place);
            });
            // A write of the part, or of one that holds it, gives it a value
            // again.
            if let Statement::Assign(target, _, _) = statement
                && overlap(target, &place)
                && !read_besides_target(statement, &place)
            {
                done = true;
                break;
            }
            if read {
                return Some(format!("b{block} reads `l{}`", place.local.0));
            }
        }
        if done {
            continue;
        }
        let mut read = false;
        crate::inline::visit_terminator(&mut data.terminator.clone(), &mut |seen_place: &mut Place| {
            read |= overlap(seen_place, &place);
        });
        if read {
            return Some(format!("b{block}'s end reads `l{}`", place.local.0));
        }
        for next in successors(&data.terminator) {
            if seen.insert((next, place.local)) {
                ways.push((next, 0, place.clone()));
            }
        }
    }
    None
}

/// Whether an assignment reads `place` other than by writing it.
fn read_besides_target(statement: &Statement, place: &Place) -> bool {
    let Statement::Assign(_, value, _) = statement else { return false };
    let mut read = false;
    crate::inline::visit_rvalue(&mut (**value).clone(), &mut |seen_place: &mut Place| {
        read |= overlap(seen_place, place);
    });
    read
}

/// What a taken parameter holds at a point: the reference its caller gave,
/// or none, released or moved away; or, past a join of a way that holds it
/// and one that moved it, either of the two.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Taken {
    Owned,
    Released,
    Moved,
    /// The reference on one way, the zeros of a move on another. Releasing
    /// it lets go of the reference once, as two facts hold: every kind of
    /// build zeroes a moved place, a release build, a debug one, a live
    /// build's new code and an entry by OSR alike, as they share the one
    /// code generator (`Statement::Moved`); and every release finds
    /// nothing to let go of in zeros, the runtime's calls by testing for
    /// null and those written in place by testing the word, which a local
    /// released on a way that never assigned it rests on too. Anything
    /// else that names it reads zeros on one way.
    Either,
}

/// The state past a join of two ways, or none where one released the
/// parameter and the other may still hold it: whatever follows, a release
/// or none, is wrong on one of them.
fn joined(a: Taken, b: Taken) -> Option<Taken> {
    use Taken::*;
    match (a, b) {
        _ if a == b => Some(a),
        (Owned | Moved | Either, Owned | Moved | Either) => Some(Either),
        (Released, Moved) | (Moved, Released) => Some(Released),
        _ => None,
    }
}

/// Where a function lets go of a parameter it takes (`takes`) other than
/// once on every way out: it releases it twice, reads it once released or
/// where one way moved it, returns still holding it on a way, or comes to
/// one block having released it on one way and holding it on another.
pub fn taken_once(body: &Body) -> Option<String> {
    for (at, &param) in body.params.iter().enumerate() {
        if !body.takes.get(at).copied().unwrap_or(false) {
            continue;
        }
        let names = |statement: &Statement| {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            used.contains(&param) || defined.contains(&param)
        };
        // A copy of it, or of a part, that the parameter's every other
        // naming after, in its block or in any other, is a copy of a part
        // too: as the moves pass leaves a move it found released at once
        // (`drop_released_moves`), and `split` takes that copy field by
        // field. The copies hold its reference.
        let part_copy = |statement: &Statement| {
            matches!(statement, Statement::Assign(target, value, _)
                if target.local != param
                    && matches!(&**value, Rvalue::Use(Operand::Copy(source)) if source.local == param))
        };
        let handed = |block: usize, index: usize| {
            body.blocks.iter().enumerate().all(|(other, data)| {
                let statements = if other == block { &data.stmts[index + 1..] } else { &data.stmts[..] };
                let mut used = Vec::new();
                terminator_uses(&data.terminator, &mut used);
                !used.contains(&param) && statements.iter().all(|statement| !names(statement) || part_copy(statement))
            })
        };
        let mut entry: Vec<Option<Taken>> = vec![None; body.blocks.len()];
        entry[0] = Some(Taken::Owned);
        let mut pending = vec![0usize];
        while let Some(block) = pending.pop() {
            let mut state = entry[block].expect("a block reached");
            for (index, statement) in body.blocks[block].stmts.iter().enumerate() {
                if part_copy(statement) && state == Taken::Owned && handed(block, index) {
                    state = Taken::Moved;
                    continue;
                }
                let (mut used, mut defined) = (Vec::new(), Vec::new());
                statement_locals(statement, &mut used, &mut defined);
                state = match (statement, state) {
                    (Statement::Release(place), Taken::Owned | Taken::Either)
                        if place.local == param && place.path.is_empty() =>
                    {
                        Taken::Released
                    }
                    (Statement::Release(place), Taken::Released) if place.local == param => {
                        return Some(format!("releases its parameter `l{}` twice, in b{block}", param.0));
                    }
                    (Statement::Release(place), Taken::Moved) if place.local == param => Taken::Moved,
                    // A part held in the parameter's own room, which the
                    // move zeroed with the rest.
                    (Statement::Release(place), Taken::Either)
                        if place.local == param
                            && place.path.iter().all(|step| matches!(step, Proj::Field(_) | Proj::Variant(..))) =>
                    {
                        Taken::Either
                    }
                    (Statement::Moved(place), _) if place.local == param && place.path.is_empty() => Taken::Moved,
                    (Statement::Clear(local), _) if *local == param => Taken::Moved,
                    (_, Taken::Released) if used.contains(&param) => {
                        return Some(format!("reads its parameter `l{}` after releasing it, in b{block}", param.0));
                    }
                    (_, Taken::Either) if used.contains(&param) || defined.contains(&param) => {
                        return Some(format!(
                            "uses its parameter `l{}` in b{block}, where one way moved it and another holds it",
                            param.0
                        ));
                    }
                    (_, state) => state,
                };
            }
            let data = &body.blocks[block];
            let mut used = Vec::new();
            terminator_uses(&data.terminator, &mut used);
            if state == Taken::Either && used.contains(&param) {
                return Some(format!(
                    "uses its parameter `l{}` at the end of b{block}, where one way moved it and another holds it",
                    param.0
                ));
            }
            if matches!(data.terminator, Terminator::Return(_)) && matches!(state, Taken::Owned | Taken::Either) {
                return Some(format!("returns from b{block} still holding its parameter `l{}` ({state:?})", param.0));
            }
            for next in successors(&data.terminator) {
                let Some(before) = entry[next] else {
                    entry[next] = Some(state);
                    pending.push(next);
                    continue;
                };
                match joined(before, state) {
                    None => {
                        return Some(format!(
                            "comes to b{next} having released its parameter `l{}` on one way and holding it on \
                             another ({before:?}, {state:?})",
                            param.0
                        ));
                    }
                    Some(after) if after != before => {
                        entry[next] = Some(after);
                        pending.push(next);
                    }
                    Some(_) => {}
                }
            }
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

    /// The MIR of a function that takes an array, `p`, and is given a flag,
    /// with `blocks(p, flag, other)` for its code, `other` a local of the
    /// array's type.
    fn taking(blocks: impl FnOnce(Local, Local, Local) -> Vec<BlockData>) -> Body {
        static LOWERED: std::sync::OnceLock<Body> = std::sync::OnceLock::new();
        let lowered = LOWERED.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("tondo-verify-taking-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("main.to");
            let source = "fn given(values: Array[Int], flag: Bool): Int {\n    if flag { values.length() } else { 0 }\n}\n\n\
                fn main() {\n    let _ = given([1], true)\n}\n";
            std::fs::write(&path, source).unwrap();
            let analysis = crate::driver::analyze(&path).unwrap();
            let bodies = crate::lower::lower(analysis.program.as_ref().unwrap(), false, false, None);
            let _ = std::fs::remove_dir_all(&dir);
            bodies.into_iter().find(|body| body.name.ends_with("given")).unwrap()
        });
        let mut body = lowered.clone();
        let (p, flag) = (body.params[0], body.params[1]);
        let other = Local(body.locals.len() as u32);
        body.locals.push(body.locals[p.0 as usize]);
        body.takes = vec![true, false];
        body.blocks = blocks(p, flag, other);
        body
    }

    fn block(stmts: Vec<Statement>, terminator: Terminator) -> BlockData {
        BlockData { stmts, terminator }
    }

    fn goto(to: u32) -> Terminator {
        Terminator::Goto(BlockId(to))
    }

    fn branch(flag: Local, then: u32, otherwise: u32) -> Terminator {
        Terminator::Branch(Operand::local(flag), BlockId(then), BlockId(otherwise))
    }

    fn back() -> Terminator {
        Terminator::Return(Operand::Const(Const::Unit))
    }

    fn read(target: Local, source: Local) -> Statement {
        Statement::Assign(Place::local(target), Box::new(Rvalue::Use(Operand::local(source))), Default::default())
    }

    /// Two ways into a block, one that moved the parameter and one that
    /// holds it, then `after`, and the function returns.
    fn joined_after(after: impl FnOnce(Local, Local) -> Vec<Statement>) -> Option<String> {
        taken_once(&taking(|p, flag, other| {
            vec![
                block(vec![], branch(flag, 1, 2)),
                block(vec![read(other, p), Statement::Moved(Place::local(p))], goto(3)),
                block(vec![], goto(3)),
                block(after(p, other), back()),
            ]
        }))
    }

    /// Past a join of a way that moved the parameter and one that holds
    /// it, a release lets it go, as the move zeroed it on the one way; a
    /// read, or a return without the release, is wrong on one of them.
    #[test]
    fn a_join_of_a_moved_way_and_a_held_one_takes_one_release() {
        assert_eq!(joined_after(|p, _| vec![Statement::Release(Place::local(p))]), None);
        let read = joined_after(|p, other| vec![read(other, p), Statement::Release(Place::local(p))]);
        assert!(read.as_ref().is_some_and(|found| found.contains("where one way moved it")), "{read:?}");
        let kept = joined_after(|_, _| vec![]);
        assert!(kept.as_ref().is_some_and(|found| found.contains("still holding")), "{kept:?}");
        let twice = joined_after(|p, _| vec![Statement::Release(Place::local(p)), Statement::Release(Place::local(p))]);
        assert!(twice.as_ref().is_some_and(|found| found.contains("twice")), "{twice:?}");
    }

    /// A way that released the parameter joined with one that holds it,
    /// or with a join that may, leaks it or releases it twice, whatever
    /// follows, which is why the join is the error (here it ends nowhere,
    /// so that nothing after it is); joined with one that moved it, it is
    /// released.
    #[test]
    fn a_released_way_joins_only_one_that_holds_nothing() {
        let release = |p: Local| Statement::Release(Place::local(p));
        let held = taken_once(&taking(|p, flag, _| {
            vec![
                block(vec![], branch(flag, 1, 2)),
                block(vec![release(p)], goto(3)),
                block(vec![], goto(3)),
                block(vec![], Terminator::Unreachable),
            ]
        }));
        assert!(held.as_ref().is_some_and(|found| found.contains("having released")), "{held:?}");
        let either = taken_once(&taking(|p, flag, _| {
            vec![
                block(vec![], branch(flag, 1, 2)),
                block(vec![Statement::Moved(Place::local(p))], goto(3)),
                block(vec![], goto(3)),
                block(vec![], branch(flag, 4, 5)),
                block(vec![release(p)], goto(6)),
                block(vec![], goto(6)),
                block(vec![], Terminator::Unreachable),
            ]
        }));
        assert!(either.as_ref().is_some_and(|found| found.contains("having released")), "{either:?}");
        let moved = |after: fn(Local) -> Vec<Statement>| {
            taken_once(&taking(|p, flag, _| {
                vec![
                    block(vec![], branch(flag, 1, 2)),
                    block(vec![release(p)], goto(3)),
                    block(vec![Statement::Moved(Place::local(p))], goto(3)),
                    block(after(p), back()),
                ]
            }))
        };
        assert_eq!(moved(|_| vec![]), None);
        let twice = moved(|p| vec![Statement::Release(Place::local(p))]);
        assert!(twice.as_ref().is_some_and(|found| found.contains("twice")), "{twice:?}");
    }

    /// A loop's head is a join: held on the way in and moved on the way
    /// back, the loop's body may not read the parameter, but its way out
    /// releases it.
    #[test]
    fn a_loop_that_moves_its_parameter_joins_at_its_head() {
        let looped = |turn: fn(Local, Local) -> Vec<Statement>| {
            taken_once(&taking(|p, flag, other| {
                vec![
                    block(vec![], goto(1)),
                    block(vec![], branch(flag, 2, 3)),
                    block(turn(p, other), goto(1)),
                    block(vec![Statement::Release(Place::local(p))], back()),
                ]
            }))
        };
        assert_eq!(looped(|p, _| vec![Statement::Moved(Place::local(p))]), None);
        let read = looped(|p, other| vec![read(other, p), Statement::Moved(Place::local(p))]);
        assert!(read.as_ref().is_some_and(|found| found.contains("where one way moved it")), "{read:?}");
    }
}
