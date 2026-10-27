//! Inlining of small functions into their callers, in the MIR of a build
//! that neither steps through code, counts it nor replaces it while it
//! runs.
//!
//! A call of a small function becomes the function's blocks: its parameters
//! are the caller's arguments, a loan the caller's place itself, and each
//! `return` assigns the result where the call did and goes on after it.
//! The MIR's ownership is explicit, so the copied code keeps it: a
//! parameter is borrowed, and the result is owned by where it goes.
//! Each block of inlined code knows the call it came from, so that a stack
//! trace still shows the inlined function's frame. A call that stays, of
//! a larger function or of the caller itself, makes the function's first
//! test before it when that test may return at once (`Guard`).

use crate::hir::Callee;
use crate::mir::*;
use crate::source::Span;
use std::collections::{HashMap, HashSet};

/// The most a function may cost, in statements and branches, to be
/// inlined.
const SMALL: usize = 30;
/// How many times calls in code just inlined are inlined in turn.
const ROUNDS: usize = 3;
/// The most statements a function's guard may have, its test and its way
/// out together, to be copied into its callers.
const GUARD: usize = 4;
/// The most statements a function may have to be copied for a `fn` value
/// it is passed, and how many such copies a program may have.
const SPECIALIZED: usize = 400;
const COPIES: usize = 256;

pub fn inline(bodies: &mut Vec<Body>) {
    inline_small(bodies);
    let guards: Vec<Option<Guard>> = bodies.iter().map(Guard::of).collect();
    if guards.iter().any(Option::is_some) {
        crate::parallel::each_mut(bodies, |body| {
            if let Some(guarded) = guard_calls(body, &guards) {
                *body = guarded;
            }
        });
    }
}

fn inline_small(bodies: &mut Vec<Body>) {
    let mut copies = HashMap::new();
    for _ in 0..ROUNDS {
        specialize(bodies, &mut copies);
        let shapes: Vec<Shape> = bodies.iter().map(Shape::of).collect();
        for body in bodies.iter_mut() {
            let made = made(body);
            call_bodies(body, &made, &shapes);
        }
        let small: Vec<Option<Body>> = bodies.iter().map(|body| small(body).then(|| body.clone())).collect();
        if small.iter().all(Option::is_none) {
            return;
        }
        let indices: Vec<usize> = (0..bodies.len()).collect();
        let inlined = crate::parallel::map(&indices, 16, |&index| inline_into(&bodies[index], index, &small));
        let mut changed = false;
        for (body, inlined) in bodies.iter_mut().zip(inlined) {
            if let Some(inlined) = inlined {
                *body = inlined;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

/// What a call of a function's body needs to know of it: how many
/// parameters it takes, and the type of the first, which a closure's
/// captures are, lent by the `fn` value that holds them.
struct Shape {
    params: usize,
    captures: Option<crate::types::Ty>,
}

impl Shape {
    fn of(body: &Body) -> Shape {
        let first = body.params.first();
        Shape { params: body.params.len(), captures: first.map(|param| body.locals[param.0 as usize]) }
    }
}

/// A `fn` value a local holds: the function, and whether it is a
/// closure's body, which takes its captures first.
type Made = HashMap<Local, (crate::hir::FnId, bool)>;

/// The locals the function assigns a `fn` value it makes, of a closure or
/// of a named function, once, and never another.
fn made(body: &Body) -> Made {
    let mut definitions: HashMap<Local, usize> = HashMap::new();
    let mut made: HashMap<Local, (crate::hir::FnId, bool)> = HashMap::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Assign(place, rvalue, _) if place.path.is_empty() => {
                *definitions.entry(place.local).or_default() += 1;
                match &**rvalue {
                    Rvalue::Erase(_, id, _) => {
                        made.insert(place.local, (*id, true));
                    }
                    Rvalue::FnValue(id) => {
                        made.insert(place.local, (*id, false));
                    }
                    _ => {}
                }
            }
            Statement::Clear(local) | Statement::Lend(local, _) | Statement::LendAt(local, _) => {
                *definitions.entry(*local).or_default() += 2;
            }
            _ => {}
        }
    }
    made.retain(|local, _| definitions.get(local) == Some(&1));
    made
}

/// Calls through the `fn` values of `made` as calls of their bodies, which
/// can be inlined: a closure's takes its captures, which the value holds.
fn call_bodies(body: &mut Body, made: &Made, shapes: &[Shape]) {
    if made.is_empty() {
        return;
    }
    for statement in body.blocks.iter_mut().flat_map(|block| &mut block.stmts) {
        let Statement::Assign(_, rvalue, _) = statement else { continue };
        let Rvalue::CallIndirect(Operand::Copy(callee), arguments) = &**rvalue else { continue };
        let Some(&(id, closure)) = made.get(&callee.local).filter(|_| callee.path.is_empty()) else { continue };
        let shape = &shapes[id.0 as usize];
        let mut direct = Vec::with_capacity(arguments.len() + 1);
        match (closure, shape.captures) {
            (true, Some(captures)) if shape.params == arguments.len() + 1 => {
                let held = Place { local: callee.local, path: vec![Proj::Captures(captures)] };
                direct.push(Operand::Copy(held));
            }
            (false, _) if shape.params == arguments.len() => {}
            _ => continue,
        }
        direct.extend(arguments.iter().cloned());
        **rvalue = Rvalue::Call(Callee::Fn(id), direct);
    }
}

/// Calls that pass a `fn` value the caller made to a function too large
/// to inline, which calls it: calls of a copy of the function where those
/// calls are of the value's body, as Go's generics make `slices.Sort` of
/// `cmp.Less` for a type. `copies` keeps each copy, by the function, the
/// parameter and the value's function, for the calls that pass the same.
fn specialize(bodies: &mut Vec<Body>, copies: &mut HashMap<(usize, usize, u32, bool), u32>) {
    let count = bodies.len();
    for index in 0..count {
        let made = made(&bodies[index]);
        if made.is_empty() {
            continue;
        }
        let mut changes = Vec::new();
        for (block, statements) in bodies[index].blocks.iter().enumerate() {
            for (at, statement) in statements.stmts.iter().enumerate() {
                let Statement::Assign(_, rvalue, _) = statement else { continue };
                let Rvalue::Call(Callee::Fn(callee), arguments) = &**rvalue else { continue };
                let target = &bodies[callee.0 as usize];
                if small(target) || cost(target) > SPECIALIZED {
                    continue;
                }
                let passed = arguments.iter().enumerate().find_map(|(param, argument)| match argument {
                    Operand::Copy(place) if place.path.is_empty() => Some((param, *made.get(&place.local)?)),
                    _ => None,
                });
                let Some((param, (value, closure))) = passed else { continue };
                let local = target.params[param];
                if loan(target, param) || defined(target).contains(&local) || !calls(target, local) {
                    continue;
                }
                changes.push((block, at, callee.0 as usize, param, value, closure));
            }
        }
        for (block, at, callee, param, value, closure) in changes {
            let key = (callee, param, value.0, closure);
            if !copies.contains_key(&key) && copies.len() == COPIES {
                continue;
            }
            let copy = *copies.entry(key).or_insert_with(|| {
                let mut copy = bodies[callee].clone();
                let shapes: Vec<Shape> = bodies.iter().map(Shape::of).collect();
                let made = HashMap::from([(copy.params[param], (value, closure))]);
                call_bodies(&mut copy, &made, &shapes);
                bodies.push(copy);
                bodies.len() as u32 - 1
            });
            let Statement::Assign(_, rvalue, _) = &mut bodies[index].blocks[block].stmts[at] else { unreachable!() };
            let Rvalue::Call(callee, _) = &mut **rvalue else { unreachable!() };
            *callee = Callee::Fn(crate::hir::FnId(copy));
        }
    }
}

/// Whether a function calls the `fn` value a local holds.
fn calls(body: &Body, local: Local) -> bool {
    body.blocks.iter().flat_map(|block| &block.stmts).any(|statement| {
        matches!(statement, Statement::Assign(_, rvalue, _)
            if matches!(&**rvalue, Rvalue::CallIndirect(Operand::Copy(place), _) if place.local == local && place.path.is_empty()))
    })
}

/// A function's statements and branches, but the way a panic unwinds:
/// its checks and the cleanups they lead to cost nothing until one does.
fn cost(body: &Body) -> usize {
    let warm = warm(body);
    body.blocks.iter().zip(warm).filter(|(_, warm)| *warm).map(|(block, _)| block_cost(block)).sum()
}

fn block_cost(block: &BlockData) -> usize {
    match checked(block) {
        Some(_) => block.stmts.len() - 1,
        None => {
            block.stmts.len() + usize::from(!matches!(block.terminator, Terminator::Goto(_) | Terminator::Return(_)))
        }
    }
}

/// Where the check that ends the block unwinds to, if one does.
fn checked(block: &BlockData) -> Option<BlockId> {
    block.stmts.len().checked_sub(2).and_then(|at| unwinds(block, at))
}

/// The blocks a call runs unless a panic unwinds it (15.9).
fn warm(body: &Body) -> Vec<bool> {
    let mut warm = vec![false; body.blocks.len()];
    let mut pending = vec![0];
    while let Some(block) = pending.pop() {
        if std::mem::replace(&mut warm[block], true) {
            continue;
        }
        let data = &body.blocks[block];
        match (&data.terminator, checked(data)) {
            (Terminator::Branch(_, _, next), Some(_)) => pending.push(next.0 as usize),
            (Terminator::Branch(_, then, otherwise), None) => pending.extend([then.0 as usize, otherwise.0 as usize]),
            (Terminator::Goto(next), _) => pending.push(next.0 as usize),
            _ => {}
        }
    }
    warm
}

/// Whether a function is worth inlining and can be: it is small,
/// registers no `defer`, and uses its loans only as places. A call of
/// itself it makes is not inlined, and each round inlines one level more.
fn small(body: &Body) -> bool {
    if body.osr.is_some() || !body.points.is_empty() {
        return false;
    }
    let cost = self::cost(body);
    for block in &body.blocks {
        for statement in &block.stmts {
            match statement {
                Statement::DeferPush(..) | Statement::DeferPop | Statement::OsrLoad(_) => return false,
                Statement::Assign(_, rvalue, _) => match &**rvalue {
                    Rvalue::CallOsr(..)
                    | Rvalue::Restart(_)
                    | Rvalue::Transfer(_)
                    | Rvalue::OsrPoint
                    | Rvalue::TestMembers(_) => return false,
                    _ => {}
                },
                _ => {}
            }
        }
    }
    if cost > SMALL {
        return false;
    }
    // A loan becomes the place lent, which cannot be an index or defined.
    let mut named = defined(body);
    let mut copy = body.blocks.clone();
    for block in &mut copy {
        let mut indexes = |place: &mut Place| {
            for projection in &place.path {
                if let Proj::Index(index, _) | Proj::InBounds(index) = projection {
                    named.insert(*index);
                }
            }
        };
        block.stmts.iter_mut().for_each(|statement| visit_statement(statement, &mut indexes));
        visit_terminator(&mut block.terminator, &mut indexes);
    }
    body.params.iter().enumerate().all(|(index, param)| !loan(body, index) || !named.contains(param))
}

/// Whether a parameter is a loan; a closure's body lists none for its
/// captures.
fn loan(body: &Body, index: usize) -> bool {
    body.loans.get(index).copied().unwrap_or(false)
}

/// The locals a body defines whole.
fn defined(body: &Body) -> HashSet<Local> {
    let mut named = HashSet::new();
    for block in &body.blocks {
        for statement in &block.stmts {
            match statement {
                Statement::Lend(local, _) | Statement::Clear(local) | Statement::LendAt(local, _) => {
                    named.insert(*local);
                }
                Statement::Assign(place, rvalue, _) => {
                    if place.path.is_empty() {
                        named.insert(place.local);
                    }
                    if let Rvalue::Select(cases, ..) = &**rvalue {
                        named.extend(cases.iter().map(|case| case.out));
                    }
                }
                _ => {}
            }
        }
    }
    named
}

/// The caller with its calls of small functions inlined, unless it has
/// none.
fn inline_into(caller: &Body, index: usize, small: &[Option<Body>]) -> Option<Body> {
    let callee_of = |statement: &Statement| match statement {
        Statement::Assign(_, rvalue, _) => match &**rvalue {
            Rvalue::Call(Callee::Fn(id), _) if id.0 as usize != index => small[id.0 as usize].as_ref(),
            _ => None,
        },
        _ => None,
    };
    if !caller.blocks.iter().flat_map(|block| &block.stmts).any(|statement| callee_of(statement).is_some()) {
        return None;
    }
    let mut body = caller.clone();
    let mut blocks: Vec<BlockData> = Vec::new();
    let mut scopes: Vec<Option<u32>> = Vec::new();
    // Where each block of the caller starts now, and the blocks that end
    // as one of the caller's did, whose targets are its old numbers.
    let mut starts = Vec::with_capacity(caller.blocks.len());
    let mut ends = Vec::new();
    // The blocks that check for a panic as a call did, and those of the
    // callee that unwound to its caller, whose unwinding is the caller's
    // old number.
    let mut checks = Vec::new();
    let mut unwound = Vec::new();
    for (index, block) in caller.blocks.iter().enumerate() {
        let quiet = quieted(block, &callee_of);
        let block = quiet.as_ref().unwrap_or(block);
        starts.push(BlockId(blocks.len() as u32));
        let scope = caller.scopes.get(index).copied().flatten();
        let mut current = Vec::new();
        // A call inlined unwinds to the caller's way itself: the check
        // after it goes, and so does the check for a cancellation after
        // that when the code inlined cannot wait.
        let mut then = None;
        for (at, statement) in block.stmts.iter().enumerate() {
            if then.is_some() {
                break;
            }
            let Some(callee) = callee_of(statement) else {
                current.push(statement.clone());
                continue;
            };
            let Statement::Assign(destination, rvalue, span) = statement else { unreachable!() };
            let Rvalue::Call(_, arguments) = &**rvalue else { unreachable!() };
            let mut assigns = Vec::new();
            let rename = bind(&mut body, callee, arguments, &mut assigns, *span, &|_| true);
            // A parameter that takes an argument read by an index may
            // panic: it unwinds as the call would have (15.9).
            let unwind = unwinds(block, at);
            for assign in assigns {
                let checked = unwind.is_some() && reads_index(&assign);
                current.push(assign);
                if let (true, Some(unwind)) = (checked, unwind) {
                    let asked = Local(body.locals.len() as u32);
                    body.locals.push(crate::types::Ty::BOOL);
                    body.names.push(None);
                    body.origins.push(Origin::Made(Vec::new(), 0));
                    current.push(Statement::Assign(Place::local(asked), Box::new(Rvalue::Unwinding), Span::default()));
                    checks.push(blocks.len());
                    let next = BlockId(blocks.len() as u32 + 1);
                    let terminator = Terminator::Branch(Operand::local(asked), unwind, next);
                    blocks.push(BlockData { stmts: std::mem::take(&mut current), terminator });
                    scopes.push(scope);
                }
            }
            let entry = BlockId(blocks.len() as u32 + 1);
            blocks.push(BlockData { stmts: std::mem::take(&mut current), terminator: Terminator::Goto(entry) });
            scopes.push(scope);
            // The call, and the calls inlined into the callee, inside it.
            let call = body.inlined.len() as u32;
            body.inlined.push(Inlined { name: callee.name.clone(), span: callee.span, call: *span, parent: scope });
            let base = body.inlined.len() as u32;
            body.inlined.extend(callee.inlined.iter().map(|inlined| Inlined {
                parent: Some(inlined.parent.map_or(call, |parent| base + parent)),
                ..inlined.clone()
            }));
            let after = BlockId(entry.0 + callee.blocks.len() as u32);
            for (at, callee_block) in callee.blocks.iter().enumerate() {
                let inner = callee.scopes.get(at).copied().flatten();
                scopes.push(Some(inner.map_or(call, |inner| base + inner)));
                let mut copied = callee_block.clone();
                for statement in &mut copied.stmts {
                    rename.statement(statement);
                }
                copied.terminator = match copied.terminator {
                    Terminator::Return(_) if unwind.is_some() && resumes(callee_block) => {
                        unwound.push((blocks.len(), unwind.unwrap_or(BlockId(0))));
                        Terminator::Goto(BlockId(0))
                    }
                    Terminator::Return(mut value) => {
                        rename.operand(&mut value);
                        // Of no line: it is neither the callee's nor the call's.
                        if !matches!(value, Operand::Const(Const::Unit)) {
                            let result = Box::new(Rvalue::Use(value));
                            copied.stmts.push(Statement::Assign(destination.clone(), result, Span::default()));
                        }
                        Terminator::Goto(after)
                    }
                    Terminator::Goto(target) => Terminator::Goto(BlockId(entry.0 + target.0)),
                    Terminator::Branch(mut condition, then, otherwise) => {
                        rename.operand(&mut condition);
                        Terminator::Branch(condition, BlockId(entry.0 + then.0), BlockId(entry.0 + otherwise.0))
                    }
                    mut terminator => {
                        visit_terminator(&mut terminator, &mut |place| rename.place(place));
                        terminator
                    }
                };
                blocks.push(copied);
            }
            if let (Some(_), Terminator::Branch(_, _, next)) = (unwind, &block.terminator) {
                then = Some(past_cancellation(&caller.blocks, *next, callee));
            }
        }
        ends.push(blocks.len());
        let terminator = then.map_or_else(|| block.terminator.clone(), Terminator::Goto);
        blocks.push(BlockData { stmts: current, terminator });
        scopes.push(scope);
    }
    for check in checks {
        if let Terminator::Branch(condition, unwind, entry) = &blocks[check].terminator {
            blocks[check].terminator = Terminator::Branch(condition.clone(), starts[unwind.0 as usize], *entry);
        }
    }
    for (block, unwind) in unwound {
        blocks[block].terminator = Terminator::Goto(starts[unwind.0 as usize]);
    }
    for end in ends {
        let at = |target: BlockId| starts[target.0 as usize];
        blocks[end].terminator = match &blocks[end].terminator {
            Terminator::Goto(target) => Terminator::Goto(at(*target)),
            Terminator::Branch(condition, then, otherwise) => {
                Terminator::Branch(condition.clone(), at(*then), at(*otherwise))
            }
            other => other.clone(),
        };
    }
    body.blocks = blocks;
    body.scopes = scopes;
    // What coverage and live builds keep is of the code before inlining,
    // which they do not inline.
    body.starts.clear();
    body.ways.clear();
    Some(body)
}

/// Where a guard's test goes for the rest of the function.
const REST: BlockId = BlockId(u32::MAX);

/// A function's guard, when it starts with a test of its parameters that
/// may return at once, as `if n < 2 { return n }`: its entry, whose test
/// goes to `REST` for the rest, and the blocks from the test to that
/// return, which only compute; and the locals they name. A caller makes
/// the test itself, as GCC splits a function's head from its body, and
/// calls the function only for the rest: a call less where the test
/// returns, as for half the calls of a recursion, and a test more where
/// it does not.
struct Guard {
    body: Body,
    named: HashSet<Local>,
}

impl Guard {
    fn of(body: &Body) -> Option<Guard> {
        if body.osr.is_some() || !body.points.is_empty() {
            return None;
        }
        // The test, and the blocks that only compute on the way to it.
        let (mut test, mut before) = (Vec::new(), Vec::new());
        let mut at = BlockId(0);
        let (condition, then, otherwise) = loop {
            let block = body.blocks.get(at.0 as usize)?;
            if before.contains(&at) || !block.stmts.iter().all(crate::thread::pure) {
                return None;
            }
            before.push(at);
            test.extend(block.stmts.iter().cloned());
            match &block.terminator {
                Terminator::Goto(next) => at = *next,
                Terminator::Branch(condition, then, otherwise) => break (condition, then, otherwise),
                _ => return None,
            }
        };
        // The blocks from a way of the test to a return, if they only
        // compute.
        let way_out = |start: BlockId| {
            let mut path: Vec<BlockId> = Vec::new();
            let mut at = start;
            loop {
                let block = &body.blocks[at.0 as usize];
                if before.contains(&at) || path.contains(&at) || !block.stmts.iter().all(crate::thread::pure) {
                    return None;
                }
                path.push(at);
                match block.terminator {
                    Terminator::Goto(next) => at = next,
                    Terminator::Return(_) => return Some(path),
                    _ => return None,
                }
            }
        };
        let path = way_out(*then).or_else(|| way_out(*otherwise))?;
        if test.len() + path.iter().map(|at| body.blocks[at.0 as usize].stmts.len()).sum::<usize>() > GUARD {
            return None;
        }
        let new = |target: BlockId| match path.iter().position(|&at| at == target) {
            Some(index) => BlockId(index as u32 + 1),
            None => REST,
        };
        let mut blocks = vec![BlockData {
            stmts: test,
            terminator: Terminator::Branch(condition.clone(), new(*then), new(*otherwise)),
        }];
        for at in &path {
            let mut block = body.blocks[at.0 as usize].clone();
            if let Terminator::Goto(next) = block.terminator {
                block.terminator = Terminator::Goto(new(next));
            }
            blocks.push(block);
        }
        let (mut named, mut defined) = (Vec::new(), Vec::new());
        for block in &blocks {
            block.stmts.iter().for_each(|statement| statement_locals(statement, &mut named, &mut defined));
            terminator_uses(&block.terminator, &mut named);
        }
        named.extend(defined);
        Some(Guard { body: Body { blocks, ..body.clone() }, named: named.into_iter().collect() })
    }
}

/// The caller with the guards of the functions it calls before the calls,
/// unless it calls none that has one.
fn guard_calls(caller: &Body, guards: &[Option<Guard>]) -> Option<Body> {
    let guard_of = |statement: &Statement| match statement {
        Statement::Assign(_, rvalue, _) => match &**rvalue {
            Rvalue::Call(Callee::Fn(id), _) => guards[id.0 as usize].as_ref(),
            _ => None,
        },
        _ => None,
    };
    if !caller.blocks.iter().flat_map(|block| &block.stmts).any(|statement| guard_of(statement).is_some()) {
        return None;
    }
    let mut body = caller.clone();
    let (mut blocks, mut scopes): (Vec<BlockData>, Vec<Option<u32>>) = (Vec::new(), Vec::new());
    let mut starts = Vec::with_capacity(caller.blocks.len());
    let mut ends = Vec::new();
    // The calls that keep the check for a panic after them (15.9), whose
    // unwinding is the caller's old number.
    let mut checks = Vec::new();
    for (index, block) in caller.blocks.iter().enumerate() {
        starts.push(BlockId(blocks.len() as u32));
        let scope = caller.scopes.get(index).copied().flatten();
        let mut current = Vec::new();
        let mut resume = None;
        for (at, statement) in block.stmts.iter().enumerate() {
            if resume.is_some() {
                break;
            }
            let Some(guard) = guard_of(statement) else {
                current.push(statement.clone());
                continue;
            };
            let Statement::Assign(destination, rvalue, span) = statement else { unreachable!() };
            let Rvalue::Call(_, arguments) = &**rvalue else { unreachable!() };
            // Arguments read by an index, where a panic may unwind: the
            // call stays as it is (15.9).
            if indexed(arguments) && unwinds(block, at).is_some() {
                current.push(statement.clone());
                continue;
            }
            let checked = unwinds(block, at).map(|unwind| (unwind, block.stmts[at + 1].clone()));
            let rename =
                bind(&mut body, &guard.body, arguments, &mut current, *span, &|local| guard.named.contains(&local));
            // The test, then the way out, then the call, then the rest of
            // the caller's block.
            let test = &guard.body.blocks[0];
            current.extend(test.stmts.iter().map(|statement| {
                let mut statement = statement.clone();
                rename.statement(&mut statement);
                statement
            }));
            let first = blocks.len() as u32;
            let call = BlockId(first + guard.body.blocks.len() as u32);
            let after = BlockId(call.0 + 1);
            let at = |target: BlockId| if target == REST { call } else { BlockId(first + target.0) };
            let Terminator::Branch(mut condition, then, otherwise) = test.terminator.clone() else { unreachable!() };
            rename.operand(&mut condition);
            blocks.push(BlockData {
                stmts: std::mem::take(&mut current),
                terminator: Terminator::Branch(condition, at(then), at(otherwise)),
            });
            for out in &guard.body.blocks[1..] {
                let mut copied = out.clone();
                for statement in &mut copied.stmts {
                    rename.statement(statement);
                }
                copied.terminator = match copied.terminator {
                    Terminator::Return(mut value) => {
                        rename.operand(&mut value);
                        if !matches!(value, Operand::Const(Const::Unit)) {
                            let result = Box::new(Rvalue::Use(value));
                            copied.stmts.push(Statement::Assign(destination.clone(), result, Span::default()));
                        }
                        Terminator::Goto(after)
                    }
                    Terminator::Goto(target) => Terminator::Goto(at(target)),
                    _ => unreachable!(),
                };
                blocks.push(copied);
            }
            // The call keeps the check after it, where a panic it unwinds
            // from goes on; the way out cannot panic.
            match (checked, &block.terminator) {
                (Some((unwind, check)), Terminator::Branch(Operand::Copy(asked), _, next)) => {
                    checks.push((blocks.len(), unwind));
                    let terminator = Terminator::Branch(Operand::Copy(asked.clone()), unwind, after);
                    blocks.push(BlockData { stmts: vec![statement.clone(), check], terminator });
                    resume = Some(*next);
                }
                _ => blocks.push(BlockData { stmts: vec![statement.clone()], terminator: Terminator::Goto(after) }),
            }
            scopes.resize(blocks.len(), scope);
        }
        ends.push(blocks.len());
        let terminator = resume.map_or_else(|| block.terminator.clone(), Terminator::Goto);
        blocks.push(BlockData { stmts: current, terminator });
        scopes.push(scope);
    }
    for (check, unwind) in checks {
        if let Terminator::Branch(condition, _, after) = &blocks[check].terminator {
            blocks[check].terminator = Terminator::Branch(condition.clone(), starts[unwind.0 as usize], *after);
        }
    }
    for end in ends {
        let at = |target: BlockId| starts[target.0 as usize];
        blocks[end].terminator = match &blocks[end].terminator {
            Terminator::Goto(target) => Terminator::Goto(at(*target)),
            Terminator::Branch(condition, then, otherwise) => {
                Terminator::Branch(condition.clone(), at(*then), at(*otherwise))
            }
            other => other.clone(),
        };
    }
    body.blocks = blocks;
    body.scopes = scopes;
    body.starts.clear();
    body.ways.clear();
    Some(body)
}

/// The block without the check for cancellation after its last call, when
/// that call is inlined and the code inlined cannot wait: only a wait
/// lets a task be cancelled, and none is left in the call to check after.
fn quieted<'b>(block: &BlockData, callee_of: &impl Fn(&Statement) -> Option<&'b Body>) -> Option<BlockData> {
    let [.., call, Statement::Assign(asked, rvalue, _), Statement::Assign(is, compare, _)] = &block.stmts[..] else {
        return None;
    };
    let Terminator::Branch(Operand::Copy(condition), _, next) = &block.terminator else { return None };
    let checks = matches!(**rvalue, Rvalue::Cancelled)
        && matches!(&**compare, Rvalue::Binary(crate::ast::BinOp::Eq, Operand::Copy(read), _) if *read == *asked)
        && *condition == *is;
    if !checks || callee_of(call).is_none_or(waits) {
        return None;
    }
    let mut quiet = block.clone();
    quiet.stmts.truncate(block.stmts.len() - 2);
    quiet.terminator = Terminator::Goto(*next);
    Some(quiet)
}

/// Whether a block ends the way a panic unwinds a function, to its caller.
fn resumes(block: &BlockData) -> bool {
    block.stmts.iter().any(|statement| {
        matches!(statement, Statement::Assign(_, rvalue, _)
            if matches!(**rvalue, Rvalue::Call(Callee::Std(crate::hir::StdFn::UnwindResume), _)))
    })
}

/// Where the code after an inlined call goes on: `next`, or past the
/// check for a cancellation there when the code inlined cannot wait.
fn past_cancellation(blocks: &[BlockData], next: BlockId, callee: &Body) -> BlockId {
    let block = &blocks[next.0 as usize];
    let [Statement::Assign(asked, rvalue, _), Statement::Assign(is, compare, _)] = &block.stmts[..] else {
        return next;
    };
    let Terminator::Branch(Operand::Copy(condition), _, after) = &block.terminator else { return next };
    let checks = matches!(**rvalue, Rvalue::Cancelled)
        && matches!(&**compare, Rvalue::Binary(crate::ast::BinOp::Eq, Operand::Copy(read), _) if *read == *asked)
        && *condition == *is;
    if !checks || waits(callee) {
        return next;
    }
    *after
}

/// Whether a function may wait, and so be cancelled there.
fn waits(callee: &Body) -> bool {
    callee
        .blocks
        .iter()
        .flat_map(|b| &b.stmts)
        .any(|s| matches!(s, Statement::Assign(_, r, _) if matches!(**r, Rvalue::Cancelled)))
}

/// How the callee's locals are the caller's: a parameter is the argument,
/// a local of the caller, when the argument is one, and a loan the place
/// lent; any other local is a new one of the caller, a parameter assigned
/// its argument.
struct Rename {
    locals: HashMap<Local, Local>,
    places: HashMap<Local, Place>,
}

fn bind(
    body: &mut Body,
    callee: &Body,
    arguments: &[Operand],
    into: &mut Vec<Statement>,
    span: Span,
    keep: &dyn Fn(Local) -> bool,
) -> Rename {
    let mut rename = Rename { locals: HashMap::new(), places: HashMap::new() };
    let defined = defined(callee);
    for (index, (&param, argument)) in callee.params.iter().zip(arguments).enumerate() {
        match argument {
            Operand::Copy(place) if loan(callee, index) => {
                rename.places.insert(param, place.clone());
            }
            Operand::Copy(place) if place.path.is_empty() && !defined.contains(&param) => {
                rename.locals.insert(param, place.local);
            }
            _ => {}
        }
    }
    for (index, &ty) in callee.locals.iter().enumerate() {
        let local = Local(index as u32);
        if rename.locals.contains_key(&local) || rename.places.contains_key(&local) || !keep(local) {
            continue;
        }
        rename.locals.insert(local, Local(body.locals.len() as u32));
        body.locals.push(ty);
        body.names.push(None);
        body.origins.push(Origin::Made(Vec::new(), 0));
    }
    for (param, argument) in callee.params.iter().zip(arguments) {
        let Some(&local) = rename.locals.get(param) else { continue };
        // A parameter that is the argument's local needs no copy.
        if !matches!(argument, Operand::Copy(place) if place.path.is_empty() && place.local == local) {
            into.push(Statement::Assign(Place::local(local), Box::new(Rvalue::Use(argument.clone())), span));
        }
    }
    rename
}

impl Rename {
    fn local(&self, local: Local) -> Local {
        self.locals[&local]
    }

    fn place(&self, place: &mut Place) {
        for projection in &mut place.path {
            if let Proj::Index(index, _) | Proj::InBounds(index) = projection {
                *index = self.local(*index);
            }
        }
        match self.places.get(&place.local) {
            Some(lent) => {
                let mut path = lent.path.clone();
                path.append(&mut place.path);
                *place = Place { local: lent.local, path };
            }
            None => place.local = self.local(place.local),
        }
    }

    fn operand(&self, operand: &mut Operand) {
        if let Operand::Copy(place) = operand {
            self.place(place);
        }
    }

    fn statement(&self, statement: &mut Statement) {
        match statement {
            Statement::Lend(local, _) | Statement::Clear(local) | Statement::LendAt(local, _) => {
                *local = self.local(*local)
            }
            Statement::Assign(_, rvalue, _) => {
                if let Rvalue::Select(cases, ..) = &mut **rvalue {
                    for case in cases {
                        case.out = self.local(case.out);
                    }
                }
            }
            _ => {}
        }
        visit_statement(statement, &mut |place| self.place(place));
    }
}

/// Every place a statement names, operands' included.
pub(crate) fn visit_statement(statement: &mut Statement, f: &mut impl FnMut(&mut Place)) {
    match statement {
        Statement::Assign(place, rvalue, _) => {
            f(place);
            visit_rvalue(rvalue, f);
        }
        Statement::Retain(place) | Statement::Release(place) | Statement::DeferPush(_, place) => f(place),
        Statement::Lend(_, place) | Statement::Moved(place) => f(place),
        Statement::LendAt(_, operand) => visit_operand(operand, f),
        Statement::Clear(_) | Statement::DeferPop | Statement::OsrLoad(_) => {}
    }
}

pub(crate) fn visit_terminator(terminator: &mut Terminator, f: &mut impl FnMut(&mut Place)) {
    match terminator {
        Terminator::Branch(operand, ..) | Terminator::Return(operand) => visit_operand(operand, f),
        Terminator::Panic { message: Some(operand), .. } => visit_operand(operand, f),
        _ => {}
    }
}

fn visit_operand(operand: &mut Operand, f: &mut impl FnMut(&mut Place)) {
    if let Operand::Copy(place) = operand {
        f(place);
    }
}

fn visit_operands(operands: &mut [Operand], f: &mut impl FnMut(&mut Place)) {
    operands.iter_mut().for_each(|operand| visit_operand(operand, f));
}

fn visit_bounds(bounds: &mut [Option<Operand>; 3], f: &mut impl FnMut(&mut Place)) {
    bounds.iter_mut().flatten().for_each(|operand| visit_operand(operand, f));
}

pub(crate) fn visit_rvalue(rvalue: &mut Rvalue, f: &mut impl FnMut(&mut Place)) {
    match rvalue {
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::Increment(o)
        | Rvalue::Convert(o)
        | Rvalue::Length(o)
        | Rvalue::Fixed(o, _)
        | Rvalue::Erase(o, ..)
        | Rvalue::Spawn(o, _) => visit_operand(o, f),
        Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b) => {
            visit_operand(a, f);
            visit_operand(b, f);
        }
        Rvalue::Call(_, operands)
        | Rvalue::Array(operands, _)
        | Rvalue::Aggregate(operands)
        | Rvalue::Variant(_, operands)
        | Rvalue::Reflect(_, _, operands, _)
        | Rvalue::TaskOp(_, operands, ..)
        | Rvalue::ArrayOp(_, operands, _)
        | Rvalue::StrOp(_, operands, _)
        | Rvalue::Pointer(_, operands, _)
        | Rvalue::MapOp(_, operands, _) => visit_operands(operands, f),
        Rvalue::CallIndirect(callee, operands) => {
            visit_operand(callee, f);
            visit_operands(operands, f);
        }
        Rvalue::CallOsr(_, operands, state) => {
            visit_operands(operands, f);
            visit_operand(state, f);
        }
        Rvalue::TestMembers(members) => members.iter_mut().for_each(|(_, o, _)| visit_operand(o, f)),
        Rvalue::Select(cases, ..) => cases.iter_mut().for_each(|case| visit_operands(&mut case.operands, f)),
        Rvalue::Concat(pieces) => pieces.iter_mut().for_each(|piece| match piece {
            Piece::Value(o, _) => visit_operand(o, f),
            Piece::Taken(place) => f(place),
            Piece::Text(_) => {}
        }),
        Rvalue::Discriminant(place) => f(place),
        Rvalue::Slice(o, bounds, _) => {
            visit_operand(o, f);
            visit_bounds(bounds, f);
        }
        Rvalue::AssignSlice(place, bounds, o, _) => {
            f(place);
            visit_bounds(bounds, f);
            visit_operand(o, f);
        }
        Rvalue::CheckRegion(o, bounds, overlap, _) => {
            visit_operand(o, f);
            visit_bounds(bounds, f);
            match overlap {
                Overlap::Region(bounds) => visit_bounds(bounds, f),
                Overlap::Index(o) => visit_operand(o, f),
            }
        }
        Rvalue::Mutate(_, place, operands, ..) => {
            f(place);
            visit_operands(operands, f);
        }
        Rvalue::FnValue(_)
        | Rvalue::ExternFn(_)
        | Rvalue::Callback(..)
        | Rvalue::Layout(..)
        | Rvalue::Cancelled
        | Rvalue::Unwinding
        | Rvalue::Restart(_)
        | Rvalue::Transfer(_)
        | Rvalue::OsrPoint => {}
    }
}

/// Where the check that follows the statement at `at` unwinds a panic to,
/// when one does: `asked = Unwinding` and a branch on it end the block
/// (15.9).
fn unwinds(block: &BlockData, at: usize) -> Option<BlockId> {
    let Some(Statement::Assign(asked, rvalue, _)) = block.stmts.get(at + 1) else { return None };
    if !matches!(**rvalue, Rvalue::Unwinding) || at + 2 != block.stmts.len() {
        return None;
    }
    match &block.terminator {
        Terminator::Branch(Operand::Copy(condition), unwind, _) if condition == asked => Some(*unwind),
        _ => None,
    }
}

/// Whether arguments read an element by an index, which may panic.
fn indexed(arguments: &[Operand]) -> bool {
    arguments.iter().any(
        |argument| matches!(argument, Operand::Copy(place) if place.path.iter().any(|p| matches!(p, Proj::Index(..)))),
    )
}

/// Whether a parameter's assignment reads its argument by an index.
fn reads_index(statement: &Statement) -> bool {
    matches!(statement, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Use(argument) if indexed(std::slice::from_ref(argument))))
}
