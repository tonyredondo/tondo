//! Small counted loops, four turns at a time. A loop over a range of
//! `Int`s, as the lowering makes `for i in a..b`, whose body is short and
//! holds no loop, runs its body four times, one after another, while four
//! turns remain, and as before for the last ones: a turn then tests the
//! range and jumps a quarter of the times, and `prove`, which runs after,
//! sees each copy's index under the length that bounds the loop.
//!
//! `continue` in a copy goes to the loop's own step, and `break` out of
//! it, so a turn means what it meant.

use crate::ast::BinOp;
use crate::mir::*;
use crate::source::Span;
use crate::types::{Ty, Types};
use std::collections::HashMap;

/// The statements a body may have to be copied.
const SMALL: usize = 24;
const COPIES: usize = 4;

pub fn unroll(bodies: &mut [Body], types: &Types) {
    for body in bodies {
        let mut done = Vec::new();
        while let Some(found) = find(body, types, &done) {
            done.push(found.head);
            rewrite(body, &found);
        }
    }
}

/// A counted loop: its head tests `counter < last` and goes to `entry`
/// or out, its step adds one to the counter and goes back to the head.
struct Counted {
    head: usize,
    step: usize,
    entry: usize,
    counter: Local,
    last: Local,
    /// Its blocks but the head and the step.
    blocks: Vec<usize>,
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Goto(to) => vec![to.0 as usize],
        Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
        _ => Vec::new(),
    }
}

fn int(body: &Body, types: &Types, local: Local) -> bool {
    types.repr(body.locals[local.0 as usize]) == Ty::INT
}

fn local(operand: &Operand) -> Option<Local> {
    match operand {
        Operand::Copy(Place { local, path }) if path.is_empty() => Some(*local),
        _ => None,
    }
}

fn find(body: &Body, types: &Types, done: &[usize]) -> Option<Counted> {
    let blocks = &body.blocks;
    let mut predecessors = vec![Vec::new(); blocks.len()];
    for (block, data) in blocks.iter().enumerate() {
        for to in successors(&data.terminator) {
            predecessors[to].push(block);
        }
    }
    'steps: for (step, data) in blocks.iter().enumerate() {
        // The step: `counter = counter + 1` and back to the head.
        let (Terminator::Goto(head), [Statement::Assign(place, increment, _)]) = (&data.terminator, &data.stmts[..])
        else {
            continue;
        };
        let head = head.0 as usize;
        let Rvalue::Increment(counted) = &**increment else { continue };
        let counter = place.local;
        if !place.path.is_empty()
            || local(counted) != Some(counter)
            || done.contains(&head)
            || !int(body, types, counter)
        {
            continue;
        }
        // The head: `test = counter < last`, then the body or out.
        let (Terminator::Branch(test, entry, _), [Statement::Assign(tested, comparison, _)]) =
            (&blocks[head].terminator, &blocks[head].stmts[..])
        else {
            continue;
        };
        let Rvalue::Binary(BinOp::Lt, left, right) = &**comparison else { continue };
        let (Some(last), true) = (local(right), local(left) == Some(counter)) else { continue };
        if local(test) != Some(tested.local) || !int(body, types, last) {
            continue;
        }
        // The loop's blocks: those that reach the step without the head.
        let mut inside = vec![false; blocks.len()];
        let mut pending = vec![step];
        while let Some(block) = pending.pop() {
            if block == head || std::mem::replace(&mut inside[block], true) {
                continue;
            }
            pending.extend(&predecessors[block]);
        }
        let entry = entry.0 as usize;
        if !inside[entry] || predecessors[head].iter().filter(|&&p| inside[p]).count() != 1 {
            continue;
        }
        let loop_blocks: Vec<usize> = (0..blocks.len()).filter(|&b| inside[b] && b != step).collect();
        // Short, without a loop of its own, and its first statement takes
        // the counter.
        let statements: usize = loop_blocks.iter().map(|&b| blocks[b].stmts.len()).sum();
        if statements > SMALL
            || !matches!(blocks[entry].stmts.first(), Some(Statement::Assign(_, value, _)) if matches!(&**value, Rvalue::Use(o) if local(o) == Some(counter)))
        {
            continue;
        }
        if inner_loop(blocks, &loop_blocks, entry) {
            continue;
        }
        for &block in &loop_blocks {
            if successors(&blocks[block].terminator).contains(&head) {
                continue 'steps;
            }
            // Neither the counter nor the bound changes in the body.
            for statement in &blocks[block].stmts {
                let changed = match statement {
                    Statement::Assign(place, ..) | Statement::Lend(_, place) | Statement::Moved(place) => place.local,
                    Statement::Clear(local) => *local,
                    _ => continue,
                };
                if changed == counter || changed == last {
                    continue 'steps;
                }
            }
        }
        return Some(Counted { head, step, entry, counter, last, blocks: loop_blocks });
    }
    None
}

/// Whether the blocks of a loop's body, from its entry, hold a loop.
fn inner_loop(blocks: &[BlockData], body: &[usize], entry: usize) -> bool {
    // 0 unseen, 1 on the way being walked, 2 done.
    let mut state: HashMap<usize, u8> = body.iter().map(|&block| (block, 0)).collect();
    let mut stack = vec![(entry, 0usize)];
    state.insert(entry, 1);
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        let ways = successors(&blocks[block].terminator);
        let Some(&to) = ways.get(*next) else {
            state.insert(block, 2);
            stack.pop();
            continue;
        };
        *next += 1;
        match state.get(&to) {
            Some(0) => {
                state.insert(to, 1);
                stack.push((to, 0));
            }
            Some(1) => return true,
            _ => {}
        }
    }
    false
}

fn rewrite(body: &mut Body, found: &Counted) {
    let scope = body.scopes.get(found.head).copied().flatten();
    let fresh = |body: &mut Body, data: BlockData| {
        body.blocks.push(data);
        body.scopes.resize(body.blocks.len() - 1, None);
        body.scopes.push(scope);
        BlockId(body.blocks.len() as u32 - 1)
    };
    let local = |body: &mut Body, ty: Ty| {
        body.locals.push(ty);
        body.names.push(None);
        Local(body.locals.len() as u32 - 1)
    };
    let (big, bound, fits) = (local(body, Ty::BOOL), local(body, Ty::INT), local(body, Ty::BOOL));
    let unrolled_head = BlockId(body.blocks.len() as u32 + 2);
    let original = BlockId(found.head as u32);
    let span = Span::default();
    let operand = |local: Local| Operand::local(local);
    let int = |value: i128| Operand::Const(Const::Int(value, Ty::INT));
    // Before the loop: `bound = last - 3` when it fits, or only the loop
    // as it was.
    let at_least = Rvalue::Binary(BinOp::Ge, operand(found.last), int(i64::MIN as i128 + COPIES as i128 - 1));
    let before = fresh(
        body,
        BlockData {
            stmts: vec![Statement::Assign(Place::local(big), Box::new(at_least), span)],
            terminator: Terminator::Branch(operand(big), BlockId(body.blocks.len() as u32 + 1), original),
        },
    );
    let less = Rvalue::Exact(BinOp::Sub, operand(found.last), int(COPIES as i128 - 1));
    fresh(
        body,
        BlockData {
            stmts: vec![Statement::Assign(Place::local(bound), Box::new(less), span)],
            terminator: Terminator::Goto(unrolled_head),
        },
    );
    // The head of the turns of four: `counter < bound`.
    let test = Rvalue::Binary(BinOp::Lt, operand(found.counter), operand(bound));
    let placeholder = BlockData { stmts: Vec::new(), terminator: Terminator::Unreachable };
    let head = fresh(body, placeholder.clone());
    assert_eq!(head, unrolled_head);
    // The copies, each followed by a step to the next, the last back to
    // the head.
    let mut entries = Vec::new();
    let mut maps = Vec::new();
    for _ in 0..COPIES {
        let map: HashMap<usize, BlockId> =
            found.blocks.iter().map(|&block| (block, fresh(body, placeholder.clone()))).collect();
        // Each copy of a block comes from the call its block's code does.
        for (&block, &new) in &map {
            body.scopes[new.0 as usize] = body.scopes.get(block).copied().flatten();
        }
        entries.push(map[&found.entry]);
        maps.push(map);
    }
    let steps: Vec<BlockId> = (0..COPIES).map(|_| fresh(body, placeholder.clone())).collect();
    body.blocks[head.0 as usize] = BlockData {
        stmts: vec![Statement::Assign(Place::local(fits), Box::new(test), span)],
        terminator: Terminator::Branch(operand(fits), entries[0], original),
    };
    for (copy, map) in maps.iter().enumerate() {
        let target = |to: BlockId| match to.0 as usize {
            to if to == found.step => steps[copy],
            to => map.get(&to).copied().unwrap_or(BlockId(to as u32)),
        };
        for (&block, &new) in map {
            let mut data = body.blocks[block].clone();
            data.terminator = match data.terminator {
                Terminator::Goto(to) => Terminator::Goto(target(to)),
                Terminator::Branch(condition, then, otherwise) => {
                    Terminator::Branch(condition, target(then), target(otherwise))
                }
                other => other,
            };
            body.blocks[new.0 as usize] = data;
        }
        let next = if copy + 1 < COPIES { entries[copy + 1] } else { head };
        body.blocks[steps[copy].0 as usize] = BlockData {
            stmts: vec![Statement::Assign(
                Place::local(found.counter),
                Box::new(Rvalue::Increment(operand(found.counter))),
                span,
            )],
            terminator: Terminator::Goto(next),
        };
    }
    // What entered the loop enters it through the test of the bound.
    let count = body.blocks.len();
    for block in 0..count {
        if block == found.step || block >= before.0 as usize {
            continue;
        }
        let terminator = &mut body.blocks[block].terminator;
        match terminator {
            Terminator::Goto(to) if *to == original => *to = before,
            Terminator::Branch(_, then, otherwise) => {
                for to in [then, otherwise] {
                    if *to == original {
                        *to = before;
                    }
                }
            }
            _ => {}
        }
    }
}
