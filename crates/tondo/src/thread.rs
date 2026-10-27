//! Branches that each way into them already decides, as inlining leaves
//! them: a function that returns `ok(value)` on one way and `err(...)` on
//! another, whose caller then tests which it got, or `a and b`, whose
//! short way is `false`. A short block that ends in such a test is copied
//! into each way that reaches it, and there what the way knows of the
//! tested value folds the test into a jump: the variant an enum local was
//! given and its payload, a constant, or the local a value was copied
//! from. A value read only by the tests it decided is then not made.
//!
//! Copies keep their block's scope of inlining, so traces still name
//! the function each line is in; a block is merged into the way before it
//! only when both are in the same one.

use crate::ast::BinOp;
use crate::mir::*;
use crate::types::Types;
use std::collections::HashMap;

/// The statements a block may hold to be copied into the ways into it.
const SHORT: usize = 4;

pub fn thread(bodies: &mut [Body], types: &Types) {
    crate::parallel::each_mut(bodies, |body| thread_one(body, types));
}

fn thread_one(body: &mut Body, types: &Types) {
    {
        // A binding that borrows writes through to what it borrows, which
        // may change under it: it is never known, and always written; nor
        // is what is lent known, which a binding may change.
        let mut aliases = vec![false; body.locals.len()];
        let mut lent = vec![false; body.locals.len()];
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            match statement {
                Statement::Lend(alias, place) => {
                    aliases[alias.0 as usize] = true;
                    lent[place.local.0 as usize] = true;
                }
                Statement::LendAt(alias, _) => aliases[alias.0 as usize] = true,
                _ => {}
            }
        }
        // What a fact about may fold something: a test, the enum whose
        // tag is read, a tag compared with a constant, or a payload read.
        let mut wanted = vec![false; body.locals.len()];
        for block in &body.blocks {
            if let Terminator::Branch(Operand::Copy(place), ..) = &block.terminator {
                wanted[place.local.0 as usize] = true;
            }
            for statement in &block.stmts {
                let Statement::Assign(_, value, _) = statement else { continue };
                match &**value {
                    Rvalue::Discriminant(place) => wanted[place.local.0 as usize] = true,
                    Rvalue::Binary(BinOp::Eq | BinOp::Ne, Operand::Copy(place), Operand::Const(_)) => {
                        wanted[place.local.0 as usize] = true
                    }
                    Rvalue::Use(Operand::Copy(place)) if matches!(place.path.first(), Some(Proj::Variant(..))) => {
                        wanted[place.local.0 as usize] = true
                    }
                    _ => {}
                }
            }
        }
        if !wanted.iter().any(|&w| w) {
            return;
        }
        let unknowable: Vec<bool> = aliases.iter().zip(&lent).zip(&wanted).map(|((a, l), w)| *a || *l || !*w).collect();
        for _ in 0..3 {
            let copied = duplicate(body);
            let folded = fold(body, &unknowable);
            if !copied && !folded {
                break;
            }
        }
        unused(body, types, &aliases);
    }
}

/// What a way knows of a local's value.
#[derive(Clone, PartialEq, Debug)]
enum Known {
    /// An enum's variant and its payload's components.
    Variant(u32, Vec<Operand>),
    Const(Const),
    /// The same value as another local.
    Copy(Local),
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Goto(to) => vec![to.0 as usize],
        Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
        _ => Vec::new(),
    }
}

fn whole(place: &Place) -> Option<Local> {
    place.path.is_empty().then_some(place.local)
}

/// A statement that only computes a value, which a copy may repeat and an
/// unused one may drop: not a read of an element by an index it checks,
/// which may panic.
fn pure(statement: &Statement) -> bool {
    let checked =
        |place: &Place| place.path.iter().any(|projection| matches!(projection, Proj::Index(..) | Proj::Deref));
    let read = |operand: &Operand| matches!(operand, Operand::Copy(place) if checked(place));
    match statement {
        Statement::Assign(place, value, _) => {
            let checked = match &**value {
                Rvalue::Use(operand) => read(operand),
                Rvalue::Variant(_, operands) | Rvalue::Aggregate(operands) => operands.iter().any(read),
                Rvalue::Discriminant(place) => checked(place),
                Rvalue::Binary(_, a, b) => read(a) || read(b),
                _ => false,
            };
            place.path.is_empty()
                && !checked
                && matches!(
                    &**value,
                    Rvalue::Use(_)
                        | Rvalue::Variant(..)
                        | Rvalue::Aggregate(_)
                        | Rvalue::Discriminant(_)
                        | Rvalue::Binary(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, ..)
                )
        }
        _ => false,
    }
}

/// The blocks in reverse postorder, each one's number in it.
fn order(body: &Body) -> Vec<usize> {
    let count = body.blocks.len();
    let mut number = vec![usize::MAX; count];
    let (mut seen, mut post) = (vec![false; count], Vec::new());
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        let ways = successors(&body.blocks[block].terminator);
        if let Some(&to) = ways.get(*next) {
            *next += 1;
            if !seen[to] {
                seen[to] = true;
                stack.push((to, 0));
            }
        } else {
            post.push(block);
            stack.pop();
        }
    }
    for (index, &block) in post.iter().rev().enumerate() {
        number[block] = index;
    }
    number
}

/// Copies each short block that ends in a test into the ways into it that
/// end in a jump to it, but a loop's head: the copy is merged into the way
/// when both are in one scope, or follows it as a block of its own.
fn duplicate(body: &mut Body) -> bool {
    let number = order(body);
    let mut predecessors = vec![Vec::new(); body.blocks.len()];
    for (block, data) in body.blocks.iter().enumerate() {
        if number[block] == usize::MAX {
            continue;
        }
        for to in successors(&data.terminator) {
            predecessors[to].push(block);
        }
    }
    let mut changed = false;
    // Blocks a copy jumps to have ways into them this round does not know.
    let mut stale = vec![false; body.blocks.len()];
    for target in 0..body.blocks.len() {
        let ways = predecessors[target].clone();
        if stale[target]
            || ways.len() < 2
            || ways.iter().any(|&way| number[way] >= number[target])
            || !ways
                .iter()
                .all(|&way| matches!(body.blocks[way].terminator, Terminator::Goto(to) if to.0 as usize == target))
        {
            continue;
        }
        // The block and those only it jumps to, down to a test.
        let mut chain = vec![target];
        let mut statements = 0;
        let ends = loop {
            let block = *chain.last().unwrap();
            let data = &body.blocks[block];
            statements += data.stmts.len();
            if statements > SHORT || !data.stmts.iter().all(pure) {
                break false;
            }
            match data.terminator {
                Terminator::Branch(..) => break true,
                Terminator::Goto(next) => {
                    let next = next.0 as usize;
                    // A block this round made is one it does not know.
                    if chain.len() == SHORT
                        || next >= stale.len()
                        || stale[next]
                        || predecessors[next].len() != 1
                        || number[next] <= number[block]
                    {
                        break false;
                    }
                    chain.push(next);
                }
                _ => break false,
            }
        };
        if !ends {
            continue;
        }
        let test = *chain.last().unwrap();
        for to in successors(&body.blocks[test].terminator) {
            stale[to] = true;
        }
        for way in ways {
            // Each block of the chain in turn: merged into the one before
            // when both are in one scope, or after it as its own.
            let mut current = way;
            for &block in &chain {
                let copy = body.blocks[block].clone();
                let scope = body.scopes.get(block).copied().flatten();
                if body.scopes.get(current).copied().flatten() == scope {
                    let into = &mut body.blocks[current];
                    into.stmts.extend(copy.stmts);
                    into.terminator = copy.terminator;
                } else {
                    body.blocks.push(copy);
                    body.scopes.resize(body.blocks.len() - 1, None);
                    body.scopes.push(scope);
                    let new = body.blocks.len() - 1;
                    body.blocks[current].terminator = Terminator::Goto(BlockId(new as u32));
                    current = new;
                }
            }
        }
        for &block in &chain {
            body.blocks[block] = BlockData { stmts: Vec::new(), terminator: Terminator::Unreachable };
            stale[block] = true;
        }
        changed = true;
    }
    changed
}

/// Folds what each way knows into the tests and reads it reaches.
fn fold(body: &mut Body, aliases: &[bool]) -> bool {
    let count = body.blocks.len();
    let number = order(body);
    let mut blocks: Vec<usize> = (0..count).filter(|&b| number[b] != usize::MAX).collect();
    blocks.sort_by_key(|&b| number[b]);
    // What each block knows at its start: the same on every way into it.
    let mut entry: Vec<Option<HashMap<Local, Known>>> = vec![None; count];
    entry[0] = Some(HashMap::new());
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 20 {
        changed = false;
        rounds += 1;
        for &block in &blocks {
            let Some(mut known) = entry[block].clone() else { continue };
            for statement in &body.blocks[block].stmts {
                learn(statement, &mut known, aliases);
            }
            for to in successors(&body.blocks[block].terminator) {
                let merged = match &entry[to] {
                    None => known.clone(),
                    Some(old) => {
                        old.iter().filter(|(l, k)| known.get(l) == Some(k)).map(|(l, k)| (*l, k.clone())).collect()
                    }
                };
                if entry[to].as_ref() != Some(&merged) {
                    entry[to] = Some(merged);
                    changed = true;
                }
            }
        }
    }
    let mut folded = false;
    for &block in &blocks {
        let Some(mut known) = entry[block].clone() else { continue };
        let mut statements = std::mem::take(&mut body.blocks[block].stmts);
        for statement in &mut statements {
            folded |= rewrite(statement, &known, &body.locals);
            learn(statement, &mut known, aliases);
        }
        body.blocks[block].stmts = statements;
        let terminator = &mut body.blocks[block].terminator;
        if let Terminator::Branch(Operand::Copy(place), then, otherwise) = terminator
            && let Some(local) = whole(place)
        {
            match known.get(&local) {
                Some(Known::Const(Const::Bool(value))) => {
                    *terminator = Terminator::Goto(if *value { *then } else { *otherwise });
                    folded = true;
                }
                Some(Known::Copy(other)) => {
                    *place = Place::local(*other);
                    folded = true;
                }
                _ => {}
            }
        }
    }
    folded
}

/// What a statement teaches, and what it makes stale.
fn learn(statement: &Statement, known: &mut HashMap<Local, Known>, aliases: &[bool]) {
    let (mut used, mut defined) = (Vec::new(), Vec::new());
    statement_locals(statement, &mut used, &mut defined);
    // A loan of a local may change it, as a call it is passed to.
    if let Statement::Assign(_, value, _) = statement
        && matches!(&**value, Rvalue::Call(..) | Rvalue::CallIndirect(..) | Rvalue::CallOsr(..) | Rvalue::Mutate(..))
    {
        defined.extend(used.iter().copied());
    }
    if let Statement::Lend(_, place) | Statement::Moved(place) = statement {
        defined.push(place.local);
    }
    if let Statement::Assign(place, ..) = statement
        && !place.path.is_empty()
    {
        defined.push(place.local);
    }
    for local in &defined {
        known.remove(local);
        known.retain(|_, fact| !mentions(fact, *local));
    }
    let Statement::Assign(place, value, _) = statement else { return };
    let Some(local) = whole(place) else { return };
    if aliases[local.0 as usize] {
        return;
    }
    let fact = match &**value {
        Rvalue::Variant(variant, components) => Some(Known::Variant(*variant, components.clone())),
        Rvalue::Use(Operand::Const(constant)) => Some(Known::Const(constant.clone())),
        Rvalue::Use(Operand::Copy(source)) => {
            whole(source).filter(|&s| s != local && !aliases[s.0 as usize]).map(Known::Copy)
        }
        _ => None,
    };
    let stale = |fact: &Known| match fact {
        Known::Variant(_, components) => components.iter().any(|operand| {
            let mut used = Vec::new();
            operand_uses(operand, &mut used);
            used.iter().any(|local| aliases[local.0 as usize])
        }),
        Known::Copy(other) => aliases[other.0 as usize],
        Known::Const(_) => false,
    };
    if let Some(fact) = fact.filter(|fact| !mentions(fact, local) && !stale(fact)) {
        known.insert(local, fact);
    }
}

fn mentions(fact: &Known, local: Local) -> bool {
    match fact {
        Known::Variant(_, components) => components.iter().any(|operand| {
            let mut used = Vec::new();
            operand_uses(operand, &mut used);
            used.contains(&local)
        }),
        Known::Const(_) => false,
        Known::Copy(other) => *other == local,
    }
}

/// Folds what is known into a statement: a known variant's tag and the
/// components of its payload, and tests of known constants.
fn rewrite(statement: &mut Statement, known: &HashMap<Local, Known>, locals: &[crate::types::Ty]) -> bool {
    let mut folded = false;
    let Statement::Assign(place, value, _) = statement else { return false };
    match &mut **value {
        Rvalue::Discriminant(of) => {
            if let Some(local) = whole(of)
                && let Some(Known::Variant(variant, _)) = known.get(&local)
            {
                // The tag, of the type the local that takes it has.
                let ty = locals[place.local.0 as usize];
                **value = Rvalue::Use(Operand::Const(Const::Int(*variant as i128, ty)));
                folded = true;
            }
        }
        Rvalue::Binary(op @ (BinOp::Eq | BinOp::Ne), left, right) => {
            if let Operand::Copy(tag) = left
                && let Some(local) = whole(tag)
                && let Some(Known::Const(Const::Int(value_known, _))) = known.get(&local)
                && let Operand::Const(Const::Int(compared, _)) = right
            {
                let equal = value_known == compared;
                **value = Rvalue::Use(Operand::Const(Const::Bool(if *op == BinOp::Eq { equal } else { !equal })));
                folded = true;
            }
        }
        _ => {}
    }
    // Reads of a known variant's payload are its components: what the
    // statement computes, not where it writes.
    crate::inline::visit_rvalue(value, &mut |place: &mut Place| {
        if let [Proj::Variant(variant, index), rest @ ..] = &place.path[..]
            && let Some(Known::Variant(known_variant, components)) = known.get(&place.local)
            && known_variant == variant
            && let Some(Operand::Copy(source)) = components.get(*index as usize)
        {
            let mut path = source.path.clone();
            path.extend_from_slice(rest);
            *place = Place { local: source.local, path };
            folded = true;
        }
    });
    folded
}

/// Drops the values made for nothing: a local nothing reads, without
/// references to count, loses the statements that only compute it.
fn unused(body: &mut Body, types: &Types, aliases: &[bool]) {
    loop {
        let mut read = vec![false; body.locals.len()];
        for block in &body.blocks {
            for statement in &block.stmts {
                let (mut used, mut defined) = (Vec::new(), Vec::new());
                statement_locals(statement, &mut used, &mut defined);
                for local in used {
                    read[local.0 as usize] = true;
                }
            }
            let mut used = Vec::new();
            terminator_uses(&block.terminator, &mut used);
            for local in used {
                read[local.0 as usize] = true;
            }
        }
        for local in body.params.iter().chain(&body.ret) {
            read[local.0 as usize] = true;
        }
        for (local, &alias) in aliases.iter().enumerate() {
            read[local] |= alias;
        }
        let mut dropped = false;
        for block in &mut body.blocks {
            block.stmts.retain(|statement| {
                let keep = !pure(statement)
                    || match statement {
                        Statement::Assign(place, ..) => {
                            read[place.local.0 as usize] || types.needs_drop(body.locals[place.local.0 as usize])
                        }
                        _ => true,
                    };
                dropped |= !keep;
                keep
            });
        }
        if !dropped {
            break;
        }
    }
}
