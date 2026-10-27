//! What a function's MIR proves about its integers, as Go's `prove` pass
//! does: an index that stays within its array's length, and a sum, a
//! difference or a product that cannot overflow. A read or a write of
//! `xs[i]` it proves becomes `InBounds`, and an operation `Exact`, which
//! codegen compiles without their checks.
//!
//! The facts are bounds on differences, `a - b <= k`, between integer
//! locals, zero and the lengths of array locals, kept closed under
//! transitivity. The conditions of branches add them, assignments move
//! them, anything that may change a length forgets it, and at the head of
//! a loop those that do not hold on every turn are dropped.

use crate::hir::Callee;
use crate::mir::*;
use crate::types::{Ty, TyKind, Types};
use std::collections::HashMap;

/// Proves what each body's MIR shows, with the loans of the functions it
/// calls, which may change what they borrow.
pub fn prove(bodies: &mut [Body], types: &Types) {
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = bodies.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for part in bodies.chunks_mut(chunk) {
            let loans = &loans;
            scope.spawn(move || {
                for body in part {
                    let Some(relevant) = relevant(body, types) else { continue };
                    Prover { body, types, loans, relevant }.run();
                }
            });
        }
    });
}

/// The locals the facts follow: the indexes of arrays' elements, the
/// operands of integer arithmetic, and what those come from or are
/// compared with; none when a body has neither.
fn relevant(body: &mut Body, types: &Types) -> Option<Vec<bool>> {
    use crate::ast::BinOp;
    let ints: Vec<bool> = body.locals.iter().map(|&ty| types.repr(ty) == Ty::INT).collect();
    let arrays: Vec<bool> = body
        .locals
        .iter()
        .map(|&ty| matches!(types.kind(types.repr(ty)), TyKind::Array(_) | TyKind::Deque(_)))
        .collect();
    let mut relevant = vec![false; body.locals.len()];
    let mut any = false;
    let local = |operand: &Operand| match operand {
        Operand::Copy(Place { local, path }) if path.is_empty() => Some(local.0 as usize),
        _ => None,
    };
    for block in &mut body.blocks {
        for statement in &mut block.stmts {
            crate::inline::visit_statement(statement, &mut |place| {
                if let Some(&Proj::Index(index, _)) = place.path.first()
                    && arrays[place.local.0 as usize]
                    && ints[index.0 as usize]
                {
                    relevant[index.0 as usize] = true;
                    relevant[place.local.0 as usize] = true;
                    any = true;
                }
            });
            if let Statement::Assign(place, rvalue, _) = statement
                && place.path.is_empty()
                && ints[place.local.0 as usize]
                && let Rvalue::Binary(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem, a, b) = &**rvalue
            {
                any = true;
                for operand in [a, b] {
                    if let Some(local) = local(operand).filter(|&l| ints[l]) {
                        relevant[local] = true;
                    }
                }
            }
        }
    }
    if !any {
        return None;
    }
    // What they come from, and what they are compared with.
    let mut changed = true;
    while changed {
        changed = false;
        for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
            let Statement::Assign(place, rvalue, _) = statement else { continue };
            if !place.path.is_empty() {
                continue;
            }
            let target = relevant[place.local.0 as usize];
            let marked: Vec<usize> = match &**rvalue {
                Rvalue::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne, a, b) => {
                    match (local(a), local(b)) {
                        (Some(a), Some(b)) if relevant[a] && ints[b] => vec![b],
                        (Some(a), Some(b)) if relevant[b] && ints[a] => vec![a],
                        _ => Vec::new(),
                    }
                }
                Rvalue::Use(o) | Rvalue::Increment(o) | Rvalue::Length(o) if target => local(o).into_iter().collect(),
                Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b) if target => {
                    [a, b].into_iter().filter_map(local).filter(|&l| ints[l]).collect()
                }
                _ => Vec::new(),
            };
            for local in marked {
                if !relevant[local] && (ints[local] || arrays[local]) {
                    relevant[local] = true;
                    changed = true;
                }
            }
        }
    }
    Some(relevant)
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Node {
    Zero,
    Int(Local),
    Len(Local),
}

/// The facts at a point: `a - b <= k` for each `(a, b) -> k`, closed.
#[derive(Clone, Default, PartialEq, Debug)]
struct Facts(HashMap<(Node, Node), i128>);

/// Bounds beyond these are no bounds: sums of them cannot overflow.
const LIMIT: i128 = 1 << 80;

impl Facts {
    /// The least `k` with `a - b <= k`, if any.
    fn bound(&self, a: Node, b: Node) -> Option<i128> {
        if a == b {
            return Some(0);
        }
        self.0.get(&(a, b)).copied()
    }

    fn nodes(&self) -> Vec<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        for node in self.0.keys().flat_map(|&(a, b)| [a, b]) {
            if !nodes.contains(&node) {
                nodes.push(node);
            }
        }
        nodes
    }

    /// Adds `a - b <= k` and what follows from it; false when the facts
    /// contradict each other, on a way the program never takes.
    fn add(&mut self, a: Node, b: Node, k: i128) -> bool {
        if k.abs() >= LIMIT {
            return true;
        }
        if self.bound(a, b).is_some_and(|old| old <= k) {
            return true;
        }
        if self.bound(b, a).is_some_and(|back| back + k < 0) {
            return false;
        }
        let mut nodes = self.nodes();
        for node in [a, b] {
            if !nodes.contains(&node) {
                nodes.push(node);
            }
        }
        // Every `x - a` and `b - y` known gives `x - y` through the new fact.
        let into: Vec<(Node, i128)> = nodes.iter().filter_map(|&x| Some((x, self.bound(x, a)?))).collect();
        let from: Vec<(Node, i128)> = nodes.iter().filter_map(|&y| Some((y, self.bound(b, y)?))).collect();
        for &(x, xa) in &into {
            for &(y, by) in &from {
                let k = xa + k + by;
                if x == y {
                    if k < 0 {
                        return false;
                    }
                    continue;
                }
                if k.abs() < LIMIT && self.bound(x, y).is_none_or(|old| k < old) {
                    self.0.insert((x, y), k);
                }
            }
        }
        true
    }

    fn forget(&mut self, node: Node) {
        self.0.retain(|&(a, b), _| a != node && b != node);
    }

    /// `node` grew by `by`: `node - x` grows, `x - node` shrinks.
    fn shift(&mut self, node: Node, by: i128) {
        let shifted: HashMap<(Node, Node), i128> = self
            .0
            .iter()
            .map(|(&(a, b), &k)| {
                let k = if a == node {
                    k + by
                } else if b == node {
                    k - by
                } else {
                    k
                };
                ((a, b), k)
            })
            .filter(|&(_, k)| k.abs() < LIMIT)
            .collect();
        self.0 = shifted;
    }

    /// What holds on both ways: the weaker bound of each fact both have.
    fn join(&self, other: &Facts) -> Facts {
        Facts(self.0.iter().filter_map(|(&key, &k)| Some((key, k.max(*other.0.get(&key)?)))).collect())
    }

    /// At a loop's head: the facts of `old` that the new turn kept.
    fn widen(&self, new: &Facts) -> Facts {
        Facts(
            self.0
                .iter()
                .filter(|&(key, &k)| new.0.get(key).is_some_and(|&n| n <= k))
                .map(|(&key, &k)| (key, k))
                .collect(),
        )
    }

    /// The values an integer may hold, `node + offset`, which as an `Int`
    /// is within 64 bits.
    fn range(&self, (node, offset): (Node, i128)) -> (i128, i128) {
        let (min, max) = (i64::MIN as i128, i64::MAX as i128);
        let low = self.bound(Node::Zero, node).map_or(min, |k| (-k).max(min)) + offset;
        let high = self.bound(node, Node::Zero).map_or(max, |k| k.min(max)) + offset;
        (low, high)
    }
}

struct Prover<'a> {
    body: &'a mut Body,
    types: &'a Types,
    loans: &'a [Vec<bool>],
    /// The locals the facts follow (`relevant`).
    relevant: Vec<bool>,
}

/// A comparison a local holds, of two integers as `node + offset`.
type Test = (crate::ast::BinOp, (Node, i128), (Node, i128));

impl Prover<'_> {
    /// An `Int` local.
    fn typed_int(&self, local: Local) -> bool {
        self.types.repr(self.body.locals[local.0 as usize]) == Ty::INT
    }

    /// An `Int` local the facts follow.
    fn int(&self, local: Local) -> bool {
        self.relevant[local.0 as usize] && self.typed_int(local)
    }

    /// An array local whose length the facts follow.
    fn array(&self, local: Local) -> bool {
        let ty = self.types.repr(self.body.locals[local.0 as usize]);
        self.relevant[local.0 as usize] && matches!(self.types.kind(ty), TyKind::Array(_) | TyKind::Deque(_))
    }

    /// An integer operand as `node + offset`.
    fn value(&self, operand: &Operand) -> Option<(Node, i128)> {
        match operand {
            Operand::Const(Const::Int(value, ty)) if self.types.repr(*ty) == Ty::INT => Some((Node::Zero, *value)),
            Operand::Copy(Place { local, path }) if path.is_empty() && self.int(*local) => Some((Node::Int(*local), 0)),
            _ => None,
        }
    }

    fn run(mut self) {
        let count = self.body.blocks.len();
        if count == 0 {
            return;
        }
        let successors: Vec<Vec<usize>> = self.body.blocks.iter().map(|block| successors(&block.terminator)).collect();
        let (order, heads) = order(&successors);
        let mut entry: Vec<Option<Facts>> = vec![None; count];
        entry[0] = Some(Facts::default());
        let mut turns = vec![0u32; count];
        // What reaches each block from each of its predecessors.
        let mut reaching: Vec<HashMap<usize, Option<Facts>>> = vec![HashMap::new(); count];
        let mut changed = true;
        let mut rounds = 0;
        while changed {
            changed = false;
            rounds += 1;
            if rounds > 40 {
                return;
            }
            for &block in &order {
                if block != 0 {
                    let mut joined: Option<Facts> = None;
                    for facts in reaching[block].values().flatten() {
                        joined = Some(match joined {
                            None => facts.clone(),
                            Some(joined) => joined.join(facts),
                        });
                    }
                    let joined = match (&entry[block], joined) {
                        (Some(old), Some(new)) if heads[block] && turns[block] >= 2 => Some(old.widen(&new)),
                        (_, joined) => joined,
                    };
                    if joined == entry[block] && turns[block] > 0 {
                        continue;
                    }
                    turns[block] += 1;
                    entry[block] = joined;
                }
                let Some(facts) = entry[block].clone() else { continue };
                for (to, facts) in self.exits(block, facts, false) {
                    if reaching[to].get(&block) != Some(&facts) {
                        reaching[to].insert(block, facts);
                        changed = true;
                    }
                }
            }
        }
        for (block, facts) in entry.into_iter().enumerate() {
            if let Some(facts) = facts {
                self.exits(block, facts, true);
            }
        }
    }

    /// The facts on each way out of a block that starts with `facts`;
    /// rewriting what they prove when `rewrite` is set.
    fn exits(&mut self, block: usize, mut facts: Facts, rewrite: bool) -> Vec<(usize, Option<Facts>)> {
        let mut tests: HashMap<Local, Test> = HashMap::new();
        let mut statements = std::mem::take(&mut self.body.blocks[block].stmts);
        for statement in &mut statements {
            if rewrite {
                self.rewrite(statement, &facts);
            }
            self.step(statement, &mut facts, &mut tests);
        }
        self.body.blocks[block].stmts = statements;
        let terminator = &self.body.blocks[block].terminator;
        match terminator {
            Terminator::Goto(to) => vec![(to.0 as usize, Some(facts))],
            Terminator::Branch(condition, then, otherwise) => {
                let test = match condition {
                    Operand::Copy(Place { local, path }) if path.is_empty() => tests.get(local).copied(),
                    _ => None,
                };
                let (then, otherwise) = (then.0 as usize, otherwise.0 as usize);
                let Some((op, a, b)) = test else {
                    return vec![(then, Some(facts.clone())), (otherwise, Some(facts))];
                };
                let mut yes = facts.clone();
                let mut no = facts;
                let yes_holds = assume(&mut yes, op, a, b);
                let no_holds = assume(&mut no, negated(op), a, b);
                vec![(then, yes_holds.then_some(yes)), (otherwise, no_holds.then_some(no))]
            }
            _ => Vec::new(),
        }
    }

    /// Marks what the facts before a statement prove in it.
    fn rewrite(&self, statement: &mut Statement, facts: &Facts) {
        let within = |place: &mut Place| {
            if let Some(Proj::Index(index, _)) = place.path.first()
                && self.array(place.local)
                && self.int(*index)
                && facts.bound(Node::Zero, Node::Int(*index)).is_some_and(|k| k <= 0)
                && facts.bound(Node::Int(*index), Node::Len(place.local)).is_some_and(|k| k <= -1)
            {
                place.path[0] = Proj::InBounds(*index);
            }
        };
        let mut within = within;
        crate::inline::visit_statement(statement, &mut within);
        if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && self.typed_int(place.local)
            && let Rvalue::Binary(op @ (crate::ast::BinOp::Add | crate::ast::BinOp::Sub | crate::ast::BinOp::Mul), a, b) =
                &**rvalue
            && let (Some(a), Some(b)) = (self.value(a), self.value(b))
        {
            let (range_a, range_b) = (facts.range(a), facts.range(b));
            let (low, high) = arithmetic(*op, range_a, range_b);
            if low >= i64::MIN as i128 && high <= i64::MAX as i128 {
                exact(rvalue);
            }
        } else if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && self.typed_int(place.local)
            && let Rvalue::Binary(crate::ast::BinOp::Div | crate::ast::BinOp::Rem, a, b) = &**rvalue
            && let (Some(a), Some((Node::Zero, divisor))) = (self.value(a), self.value(b))
            && divisor > 0
            && (divisor as u128).is_power_of_two()
            && facts.range(a).0 >= 0
        {
            // A dividend not below zero by a power of two: a shift or a
            // mask, without the work signs take.
            exact(rvalue);
        }
    }

    /// The facts after a statement.
    fn step(&self, statement: &Statement, facts: &mut Facts, tests: &mut HashMap<Local, Test>) {
        let forget_local = |facts: &mut Facts, tests: &mut HashMap<Local, Test>, local: Local| {
            tests.remove(&local);
            if self.relevant[local.0 as usize] {
                facts.forget(Node::Int(local));
                facts.forget(Node::Len(local));
                tests.retain(|_, (_, (a, _), (b, _))| *a != Node::Int(local) && *b != Node::Int(local));
            }
        };
        match statement {
            Statement::Assign(place, rvalue, _) => {
                // What may change the length of an array or a local the
                // call borrows.
                self.calls(rvalue, facts, tests, &forget_local);
                if let Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) = &**rvalue {
                    facts.forget(Node::Len(target.local));
                }
                let element = matches!(place.path.first(), Some(Proj::Index(..) | Proj::InBounds(_)));
                if !place.path.is_empty() {
                    if !element {
                        facts.forget(Node::Len(place.local));
                    }
                    return;
                }
                let local = place.local;
                if !self.int(local) {
                    forget_local(facts, tests, local);
                    if let Rvalue::Binary(op, a, b) = &**rvalue
                        && matches!(
                            op,
                            crate::ast::BinOp::Lt
                                | crate::ast::BinOp::Le
                                | crate::ast::BinOp::Gt
                                | crate::ast::BinOp::Ge
                                | crate::ast::BinOp::Eq
                                | crate::ast::BinOp::Ne
                        )
                        && let (Some(a), Some(b)) = (self.value(a), self.value(b))
                    {
                        tests.insert(local, (*op, a, b));
                    }
                    return;
                }
                self.assign(local, rvalue, facts, tests, &forget_local);
            }
            // An element lent or moved keeps the array's length.
            Statement::Lend(_, place) | Statement::Moved(place) => match place.path.first() {
                None => forget_local(facts, tests, place.local),
                Some(Proj::Index(..) | Proj::InBounds(_)) => {}
                Some(_) => facts.forget(Node::Len(place.local)),
            },
            Statement::Clear(local) => forget_local(facts, tests, *local),
            Statement::OsrLoad(_) => facts.0.clear(),
            Statement::Retain(_)
            | Statement::Release(_)
            | Statement::DeferPush(..)
            | Statement::DeferPop
            | Statement::LendAt(..) => {}
        }
    }

    /// A call may change what it borrows: a local passed whole to one of
    /// the program's functions that takes a loan there, or an array passed
    /// whole to any other callee.
    fn calls(
        &self,
        rvalue: &Rvalue,
        facts: &mut Facts,
        tests: &mut HashMap<Local, Test>,
        forget_local: &impl Fn(&mut Facts, &mut HashMap<Local, Test>, Local),
    ) {
        let (callee, operands) = match rvalue {
            Rvalue::Call(callee, operands) => (Some(callee), operands),
            Rvalue::CallIndirect(_, operands) | Rvalue::CallOsr(_, operands, _) => (None, operands),
            _ => return,
        };
        for (index, operand) in operands.iter().enumerate() {
            let Operand::Copy(Place { local, path }) = operand else { continue };
            if !path.is_empty() {
                continue;
            }
            let lent = match callee {
                Some(Callee::Fn(id)) => {
                    self.loans.get(id.0 as usize).and_then(|loans| loans.get(index)).copied().unwrap_or(true)
                }
                _ => self.array(*local),
            };
            if lent {
                forget_local(facts, tests, *local);
            }
        }
    }

    /// The facts after an integer local takes a value.
    fn assign(
        &self,
        local: Local,
        rvalue: &Rvalue,
        facts: &mut Facts,
        tests: &mut HashMap<Local, Test>,
        forget_local: &impl Fn(&mut Facts, &mut HashMap<Local, Test>, Local),
    ) {
        use crate::ast::BinOp;
        let node = Node::Int(local);
        // `local = other + offset`.
        let moved = match rvalue {
            Rvalue::Use(operand) => self.value(operand),
            Rvalue::Increment(operand) => self.value(operand).map(|(n, o)| (n, o + 1)),
            Rvalue::Binary(BinOp::Add, a, b) | Rvalue::Exact(BinOp::Add, a, b) => {
                match (self.value(a), self.value(b)) {
                    (Some((n, o)), Some((Node::Zero, c))) | (Some((Node::Zero, c)), Some((n, o))) => Some((n, o + c)),
                    _ => None,
                }
            }
            Rvalue::Binary(BinOp::Sub, a, b) | Rvalue::Exact(BinOp::Sub, a, b) => {
                match (self.value(a), self.value(b)) {
                    (Some((n, o)), Some((Node::Zero, c))) => Some((n, o - c)),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some((other, offset)) = moved {
            if other == node {
                facts.shift(node, offset);
                tests.retain(|_, (_, (a, _), (b, _))| *a != node && *b != node);
                return;
            }
            forget_local(facts, tests, local);
            facts.add(node, other, offset);
            facts.add(other, node, -offset);
            return;
        }
        // Otherwise only the values it may hold.
        let range = match rvalue {
            Rvalue::Length(Operand::Copy(Place { local: array, path })) if path.is_empty() && self.array(*array) => {
                forget_local(facts, tests, local);
                let length = Node::Len(*array);
                facts.add(Node::Zero, length, 0);
                facts.add(length, Node::Zero, i64::MAX as i128);
                facts.add(node, length, 0);
                facts.add(length, node, 0);
                return;
            }
            Rvalue::Binary(op, a, b) | Rvalue::Exact(op, a, b) => match (self.value(a), self.value(b)) {
                (Some(a), Some(b)) => match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => Some(arithmetic(*op, facts.range(a), facts.range(b))),
                    // A remainder by a positive constant is under it, and
                    // not negative when the dividend is not.
                    BinOp::Rem if b.0 == Node::Zero && b.1 > 0 => {
                        let low = if facts.range(a).0 >= 0 { 0 } else { -(b.1 - 1) };
                        Some((low, b.1 - 1))
                    }
                    // A quotient by one grows with the dividend.
                    BinOp::Div if b.0 == Node::Zero && b.1 > 0 => {
                        let (low, high) = facts.range(a);
                        Some((low / b.1, high / b.1))
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        forget_local(facts, tests, local);
        if let Some((low, high)) = range {
            facts.add(Node::Zero, node, -low);
            facts.add(node, Node::Zero, high);
        }
    }
}

/// `Exact` in place of the `Binary` it was.
fn exact(rvalue: &mut Rvalue) {
    let Rvalue::Binary(op, a, b) = std::mem::replace(rvalue, Rvalue::Cancelled) else { unreachable!() };
    *rvalue = Rvalue::Exact(op, a, b);
}

/// The range of `a op b` for `a` and `b` in theirs.
fn arithmetic(op: crate::ast::BinOp, (a_low, a_high): (i128, i128), (b_low, b_high): (i128, i128)) -> (i128, i128) {
    use crate::ast::BinOp;
    let clamp = |value: i128| value.clamp(-LIMIT, LIMIT);
    match op {
        BinOp::Add => (clamp(a_low + b_low), clamp(a_high + b_high)),
        BinOp::Sub => (clamp(a_low - b_high), clamp(a_high - b_low)),
        _ => {
            // Bounds at the limit stand for none: so does a product of one.
            if [a_low, a_high, b_low, b_high].iter().any(|v| v.abs() >= LIMIT) {
                return (-LIMIT, LIMIT);
            }
            let products = [(a_low, b_low), (a_low, b_high), (a_high, b_low), (a_high, b_high)]
                .map(|(a, b)| clamp(a.checked_mul(b).unwrap_or(LIMIT * a.signum() * b.signum())));
            (*products.iter().min().unwrap(), *products.iter().max().unwrap())
        }
    }
}

/// Adds what `a op b` says; false when it cannot hold.
fn assume(facts: &mut Facts, op: crate::ast::BinOp, (a, a_offset): (Node, i128), (b, b_offset): (Node, i128)) -> bool {
    use crate::ast::BinOp;
    // `a + ao - (b + bo) <= k` is `a - b <= k - ao + bo`.
    let shift = b_offset - a_offset;
    match op {
        BinOp::Lt => facts.add(a, b, -1 + shift),
        BinOp::Le => facts.add(a, b, shift),
        BinOp::Gt => facts.add(b, a, -1 - shift),
        BinOp::Ge => facts.add(b, a, -shift),
        BinOp::Eq => facts.add(a, b, shift) && facts.add(b, a, -shift),
        _ => true,
    }
}

fn negated(op: crate::ast::BinOp) -> crate::ast::BinOp {
    use crate::ast::BinOp;
    match op {
        BinOp::Lt => BinOp::Ge,
        BinOp::Le => BinOp::Gt,
        BinOp::Gt => BinOp::Le,
        BinOp::Ge => BinOp::Lt,
        BinOp::Eq => BinOp::Ne,
        _ => BinOp::Eq,
    }
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Goto(to) => vec![to.0 as usize],
        Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
        _ => Vec::new(),
    }
}

/// The blocks in reverse postorder from the entry, and which are the
/// heads of loops: the targets of edges back to a block being visited.
fn order(successors: &[Vec<usize>]) -> (Vec<usize>, Vec<bool>) {
    let count = successors.len();
    let (mut state, mut heads, mut post) = (vec![0u8; count], vec![false; count], Vec::new());
    let mut stack = vec![(0usize, 0usize)];
    state[0] = 1;
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        if let Some(&to) = successors[block].get(*next) {
            *next += 1;
            match state[to] {
                0 => {
                    state[to] = 1;
                    stack.push((to, 0));
                }
                1 => heads[to] = true,
                _ => {}
            }
        } else {
            state[block] = 2;
            post.push(block);
            stack.pop();
        }
    }
    post.reverse();
    (post, heads)
}
