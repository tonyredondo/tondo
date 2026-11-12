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
use crate::source::Span;
use crate::types::{Ty, TyKind, Types};
use std::collections::HashMap;

/// Proves what each body's MIR shows, with the loans of the functions it
/// calls, which may change what they borrow.
pub fn prove(bodies: &mut [Body], types: &Types) {
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    crate::parallel::each_mut(bodies, |body| {
        if let Some(relevant) = relevant(body, types) {
            Prover { body, types, loans: &loans, relevant }.run();
        }
        if crate::mir_text::remarking() {
            kept(body, types);
        }
    });
}

/// Remarks on the checks a body keeps in its innermost loops, where they
/// cost the most, and on its products by a constant that are checked by
/// the range of the other factor (`tondo optimizations`).
fn kept(body: &mut Body, types: &Types) {
    use crate::ast::BinOp;
    let tight = crate::mir::inner_loops(&body.blocks, true);
    let mut remarks: Vec<(Span, &'static str, &'static str)> = Vec::new();
    for (index, block) in body.blocks.iter_mut().enumerate() {
        let looping = tight.get(index).copied().unwrap_or(false);
        for statement in &mut block.stmts {
            if looping {
                crate::inline::visit_statement(statement, &mut |place| {
                    if let Some(Proj::Index(_, span)) = place.path.first() {
                        remarks.push((*span, "checked", "keeps its bounds check, in a loop"));
                    }
                });
            }
            let Statement::Assign(place, rvalue, span) = statement else { continue };
            let Rvalue::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), a, b) = &**rvalue else { continue };
            let Some(int) = place.path.is_empty().then(|| types.as_int(body.locals[place.local.0 as usize])).flatten()
            else {
                continue;
            };
            let constant = match (a, b) {
                (_, Operand::Const(Const::Int(c, _))) | (Operand::Const(Const::Int(c, _)), _) => Some(*c),
                _ => None,
            };
            if *op == BinOp::Mul && constant.is_some_and(|c| crate::codegen::by_range(c, int.bits())) {
                remarks.push((*span, "arithmetic", "is checked by the range of the other factor, not by the product"));
            } else if looping {
                remarks.push((*span, "checked", "keeps its overflow check, in a loop"));
            }
        }
    }
    for (span, kind, text) in remarks {
        crate::mir_text::remark(body, span, kind, || text.to_string());
    }
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
    let mut tested = vec![false; body.locals.len()];
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
            // An `Int` tested against a constant, which the facts may decide
            // when it is a copy of one they follow.
            if let Statement::Assign(_, rvalue, _) = statement
                && let Rvalue::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne, a, b) =
                    &**rvalue
                && let Some(local) = match (a, b) {
                    (_, Operand::Const(Const::Int(..))) => local(a),
                    (Operand::Const(Const::Int(..)), _) => local(b),
                    _ => None,
                }
                && ints[local]
            {
                tested[local] = true;
            }
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
    // The counters: locals that step by a constant, as a loop's.
    let mut counters = vec![false; body.locals.len()];
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && ints[place.local.0 as usize]
            && let Rvalue::Increment(o) | Rvalue::Binary(BinOp::Add | BinOp::Sub, o, Operand::Const(_)) = &**rvalue
            && local(o) == Some(place.local.0 as usize)
        {
            counters[place.local.0 as usize] = true;
            any |= tested.iter().any(|&t| t);
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
                // A tested copy of a local followed or of a counter, or of
                // one more or less.
                Rvalue::Use(o)
                | Rvalue::Increment(o)
                | Rvalue::Binary(BinOp::Add | BinOp::Sub, o, Operand::Const(_))
                    if tested[place.local.0 as usize]
                        && local(o).is_some_and(|source| relevant[source] || counters[source]) =>
                {
                    [Some(place.local.0 as usize), local(o)].into_iter().flatten().collect()
                }
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
        // Every `x - a` and `b - y` known gives `x - y` through the new fact.
        let (mut into, mut from) = (vec![(a, 0)], vec![(b, 0)]);
        for (&(x, y), &known) in &self.0 {
            if y == a {
                into.push((x, known));
            }
            if x == b {
                from.push((y, known));
            }
        }
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
        let Some((mut entry, reaching)) = self.fixpoint(&order, &heads, &HashMap::new()) else { return };
        // What a loop's counter bounds of the locals it only adds to, which
        // a second round holds at the loop's head.
        let pins = self.accumulators(&entry, &reaching, &successors, &heads);
        if !pins.is_empty()
            && let Some((pinned, _)) = self.fixpoint(&order, &heads, &pins)
        {
            entry = pinned;
        }
        for (block, facts) in entry.into_iter().enumerate() {
            if let Some(facts) = facts {
                self.exits(block, facts, true);
            }
        }
    }

    /// The facts at the start of each block, and what reaches each block
    /// from each of its predecessors, with `pins` held at the heads of
    /// loops; none when they do not settle.
    #[allow(clippy::type_complexity)]
    fn fixpoint(
        &mut self,
        order: &[usize],
        heads: &[bool],
        pins: &HashMap<usize, Vec<(Node, Node, i128)>>,
    ) -> Option<(Vec<Option<Facts>>, Vec<HashMap<usize, Option<Facts>>>)> {
        let count = self.body.blocks.len();
        let mut entry: Vec<Option<Facts>> = vec![None; count];
        entry[0] = Some(Facts::default());
        let mut turns = vec![0u32; count];
        let mut reaching: Vec<HashMap<usize, Option<Facts>>> = vec![HashMap::new(); count];
        let mut changed = true;
        let mut rounds = 0;
        while changed {
            changed = false;
            rounds += 1;
            if rounds > 40 {
                return None;
            }
            for &block in order {
                if block != 0 {
                    let mut joined: Option<Facts> = None;
                    for facts in reaching[block].values().flatten() {
                        joined = Some(match joined {
                            None => facts.clone(),
                            Some(joined) => joined.join(facts),
                        });
                    }
                    let mut joined = match (&entry[block], joined) {
                        (Some(old), Some(new)) if heads[block] && turns[block] >= 2 => Some(old.widen(&new)),
                        (_, joined) => joined,
                    };
                    if let (Some(facts), Some(pinned)) = (&mut joined, pins.get(&block)) {
                        for &(a, b, k) in pinned {
                            facts.add(a, b, k);
                        }
                    }
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
        Some((entry, reaching))
    }

    /// Bounds at the heads of loops for the locals they only add to. A
    /// counter whose additions on every way around sum to at least one
    /// step bounds how often the head is reached, between its bounds
    /// there; a local whose only writes in the loop add a bounded amount,
    /// at most so many times on any way from the head, then stays within
    /// what it held on entry and that many additions of the most and the
    /// least: facts true at every arrival at the head, which the first
    /// round's widening drops as the local grows.
    fn accumulators(
        &mut self,
        entry: &[Option<Facts>],
        reaching: &[HashMap<usize, Option<Facts>>],
        successors: &[Vec<usize>],
        heads: &[bool],
    ) -> HashMap<usize, Vec<(Node, Node, i128)>> {
        use crate::ast::BinOp;
        let count = successors.len();
        let mut predecessors = vec![Vec::new(); count];
        for (from, tos) in successors.iter().enumerate() {
            for &to in tos {
                predecessors[to].push(from);
            }
        }
        // A loop's blocks: its head, and the blocks it rules, which only
        // ways through the head reach, that reach an edge back to it without
        // passing it. A loop around this one goes back to the head through
        // blocks the entry reaches without it, which are not this loop's.
        let blocks_of = |head: usize| -> Vec<bool> {
            let reach = |from: usize, stop: Option<usize>| {
                let mut reached = vec![false; count];
                let mut stack = vec![from];
                reached[from] = true;
                while let Some(block) = stack.pop() {
                    for &to in &successors[block] {
                        if !reached[to] && Some(to) != stop {
                            reached[to] = true;
                            stack.push(to);
                        }
                    }
                }
                reached
            };
            let reached = reach(head, None);
            let outside = if head == 0 { vec![false; count] } else { reach(0, Some(head)) };
            let ruled = |block: usize| reached[block] && !outside[block];
            let mut inside = vec![false; count];
            inside[head] = true;
            let mut stack: Vec<usize> = predecessors[head].iter().copied().filter(|&from| ruled(from)).collect();
            for &from in &stack {
                inside[from] = true;
            }
            while let Some(block) = stack.pop() {
                for &from in &predecessors[block] {
                    if ruled(from) && !inside[from] {
                        inside[from] = true;
                        stack.push(from);
                    }
                }
            }
            inside
        };
        #[derive(Clone, Copy)]
        enum Write {
            Step(i128),
            Add(usize, usize, bool),
            Other,
        }
        let mut pins: HashMap<usize, Vec<(Node, Node, i128)>> = HashMap::new();
        let noting = crate::mir_text::noting(&self.body.name);
        let mut notes = Vec::new();
        for head in 0..count {
            if !heads[head] || entry[head].is_none() {
                continue;
            }
            // What reaches the head, joined once the facts settled without
            // the widening: a bound that grew on each turn until a test
            // held it, as a counter's under its limit, holds there too.
            let Some(at_head) = reaching[head].values().flatten().fold(None, |joined: Option<Facts>, facts| {
                Some(joined.map_or_else(|| facts.clone(), |j| j.join(facts)))
            }) else {
                continue;
            };
            pins.entry(head).or_default().extend(at_head.0.iter().map(|(&(a, b), &k)| (a, b, k)));
            let inside = blocks_of(head);
            // A way back to the head must start inside: what reaches it
            // only from outside is no loop of this head.
            let back: Vec<usize> = predecessors[head].iter().copied().filter(|&from| inside[from]).collect();
            if back.is_empty() {
                continue;
            }
            // The loops inside this one, whose turns no count bounds, and
            // their edges back to their heads.
            let mut nested = vec![false; count];
            let mut turning: std::collections::HashSet<(usize, usize)> = Default::default();
            for other in (0..count).filter(|&other| other != head && inside[other] && heads[other]) {
                let within = blocks_of(other);
                for (block, &is) in within.iter().enumerate() {
                    nested[block] |= is && inside[block];
                }
                for &from in predecessors[other].iter().filter(|&&from| within[from]) {
                    turning.insert((from, other));
                }
            }
            // The ways through the loop once, without edges back to its head
            // or to the head of a loop inside it, in an order where each
            // block comes after those before it.
            let edge =
                |from: usize, to: usize| inside[from] && inside[to] && to != head && !turning.contains(&(from, to));
            let mut ins = vec![0usize; count];
            for (from, tos) in successors.iter().enumerate() {
                for &to in tos {
                    if edge(from, to) {
                        ins[to] += 1;
                    }
                }
            }
            let mut topological = vec![head];
            let mut next = 0;
            while next < topological.len() {
                let from = topological[next];
                next += 1;
                for &to in &successors[from] {
                    if edge(from, to) {
                        ins[to] -= 1;
                        if ins[to] == 0 {
                            topological.push(to);
                        }
                    }
                }
            }
            if topological.len() != inside.iter().filter(|&&within| within).count() {
                if noting {
                    notes.push(format!(
                        "prove: loop at b{head}: its blocks go round other than through b{head}: no sums bounded"
                    ));
                }
                continue;
            }
            // Each local's writes in the loop, by block.
            let mut writes: HashMap<Local, Vec<(usize, Write)>> = HashMap::new();
            let mut all_other = false;
            for block in (0..count).filter(|&block| inside[block]) {
                for (at, statement) in self.body.blocks[block].stmts.iter().enumerate() {
                    if matches!(statement, Statement::OsrLoad(_)) {
                        all_other = true;
                    }
                    let (mut used, mut defined) = (Vec::new(), Vec::new());
                    statement_locals(statement, &mut used, &mut defined);
                    let kind = |local: Local| -> Write {
                        let Statement::Assign(place, rvalue, _) = statement else { return Write::Other };
                        if !place.path.is_empty() || place.local != local || !self.int(local) {
                            return Write::Other;
                        }
                        let me = Some((Node::Int(local), 0));
                        match &**rvalue {
                            Rvalue::Increment(o) if self.value(o) == me => Write::Step(1),
                            Rvalue::Binary(BinOp::Add, a, b) | Rvalue::Exact(BinOp::Add, a, b) => {
                                match (self.value(a), self.value(b)) {
                                    (x, Some((Node::Zero, c))) | (Some((Node::Zero, c)), x) if x == me => {
                                        Write::Step(c)
                                    }
                                    (x, Some((n, _))) if x == me && n != Node::Int(local) => {
                                        Write::Add(block, at, false)
                                    }
                                    (Some((n, _)), x) if x == me && n != Node::Int(local) => {
                                        Write::Add(block, at, false)
                                    }
                                    _ => Write::Other,
                                }
                            }
                            Rvalue::Binary(BinOp::Sub, a, b) | Rvalue::Exact(BinOp::Sub, a, b) => {
                                match (self.value(a), self.value(b)) {
                                    (x, Some((Node::Zero, c))) if x == me => Write::Step(-c),
                                    (x, Some((n, _))) if x == me && n != Node::Int(local) => {
                                        Write::Add(block, at, true)
                                    }
                                    _ => Write::Other,
                                }
                            }
                            _ => Write::Other,
                        }
                    };
                    for local in defined {
                        writes.entry(local).or_default().push((block, kind(local)));
                    }
                    // A call may write a local it is given, by a loan.
                    if let Statement::Assign(_, rvalue, _) = statement
                        && let Rvalue::Call(_, operands)
                        | Rvalue::CallIndirect(_, operands)
                        | Rvalue::CallOsr(_, operands, _) = &**rvalue
                    {
                        for operand in operands {
                            if let Operand::Copy(place) = operand {
                                writes.entry(place.local).or_default().push((block, Write::Other));
                            }
                        }
                    }
                }
            }
            if all_other {
                continue;
            }
            // The fewest turns' worth of steps of each counter.
            let mut turns: Option<i128> = None;
            for (&local, list) in &writes {
                if list.iter().any(|&(block, write)| nested[block] || !matches!(write, Write::Step(_))) {
                    continue;
                }
                let (Some(high), Some(low)) =
                    (at_head.bound(Node::Int(local), Node::Zero), at_head.bound(Node::Zero, Node::Int(local)))
                else {
                    continue;
                };
                let step = |block: usize| -> i128 {
                    list.iter()
                        .filter(|&&(b, _)| b == block)
                        .map(|&(_, w)| if let Write::Step(k) = w { k } else { 0 })
                        .sum()
                };
                let mut least = vec![None::<i128>; count];
                least[head] = Some(step(head));
                for &block in &topological {
                    let Some(here) = least[block] else { continue };
                    for &to in &successors[block] {
                        if edge(block, to) {
                            let through = here + step(to);
                            least[to] = Some(least[to].map_or(through, |old: i128| old.min(through)));
                        }
                    }
                }
                let Some(each) =
                    back.iter().map(|&from| least[from]).collect::<Option<Vec<_>>>().and_then(|l| l.into_iter().min())
                else {
                    continue;
                };
                if each < 1 {
                    continue;
                }
                let span = (high + low).max(0);
                let arrivals = span / each + 1;
                if noting {
                    notes.push(format!(
                        "prove: loop at b{head}: l{} steps {each} a turn at least: {arrivals} turns at most",
                        local.0
                    ));
                }
                turns = Some(turns.map_or(arrivals, |old| old.min(arrivals)));
            }
            let Some(arrivals) = turns else {
                if noting {
                    notes.push(format!(
                        "prove: loop at b{head}: no counter within bounds steps each turn: no sums bounded"
                    ));
                }
                continue;
            };
            for (&local, list) in &writes {
                if list.iter().any(|&(block, write)| nested[block] || matches!(write, Write::Other))
                    || !list.iter().any(|&(_, write)| matches!(write, Write::Add(..)))
                {
                    continue;
                }
                // What each write adds: a step its constant, an addition the
                // values its operand may hold there.
                let mut most = 0i128;
                let mut least = 0i128;
                let mut bounded = true;
                for &(_, write) in list {
                    let (low, high) = match write {
                        Write::Step(k) => (k, k),
                        Write::Add(block, at, subtracts) => {
                            let Some(mut facts) = entry[block].clone() else { continue };
                            let mut tests = HashMap::new();
                            for statement in &self.body.blocks[block].stmts[..at] {
                                self.step(statement, &mut facts, &mut tests);
                            }
                            let Statement::Assign(_, rvalue, _) = &self.body.blocks[block].stmts[at] else { continue };
                            let (Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b)) = &**rvalue else { continue };
                            let me = Some((Node::Int(local), 0));
                            let other = if self.value(a) == me { b } else { a };
                            let Some(value) = self.value(other) else {
                                bounded = false;
                                break;
                            };
                            let (low, high) = facts.range(value);
                            if subtracts { (-high, -low) } else { (low, high) }
                        }
                        Write::Other => unreachable!("skipped above"),
                    };
                    if low.abs() >= LIMIT / 4 || high.abs() >= LIMIT / 4 {
                        bounded = false;
                        break;
                    }
                    most = most.max(high);
                    least = least.min(low);
                }
                if !bounded {
                    if noting {
                        notes.push(format!("prove: loop at b{head}: l{} adds what no bound holds", local.0));
                    }
                    continue;
                }
                // The most writes on any way from the head.
                let written = |block: usize| list.iter().filter(|&&(b, _)| b == block).count() as i128;
                let mut many = vec![None::<i128>; count];
                many[head] = Some(written(head));
                for &block in &topological {
                    let Some(here) = many[block] else { continue };
                    for &to in &successors[block] {
                        if edge(block, to) {
                            let through = here + written(to);
                            many[to] = Some(many[to].map_or(through, |old: i128| old.max(through)));
                        }
                    }
                }
                let Some(each) = many.iter().flatten().copied().max() else { continue };
                // What it holds on entry, from outside the loop.
                let (mut low, mut high) = (i128::MAX, i128::MIN);
                for (&from, facts) in &reaching[head] {
                    if inside[from] {
                        continue;
                    }
                    let Some(facts) = facts else { continue };
                    let (l, h) = facts.range((Node::Int(local), 0));
                    low = low.min(l);
                    high = high.max(h);
                }
                if low > high || low <= i64::MIN as i128 || high >= i64::MAX as i128 {
                    if noting {
                        notes.push(format!("prove: loop at b{head}: l{} enters with no bound", local.0));
                    }
                    continue;
                }
                let additions = arrivals.saturating_mul(each);
                let low = low.saturating_add(additions.saturating_mul(least));
                let high = high.saturating_add(additions.saturating_mul(most));
                if low.abs() >= LIMIT || high.abs() >= LIMIT {
                    continue;
                }
                if noting {
                    notes.push(format!("prove: loop at b{head}: l{} stays within {low}..={high}", local.0));
                }
                let pinned = pins.entry(head).or_default();
                pinned.push((Node::Zero, Node::Int(local), -low));
                pinned.push((Node::Int(local), Node::Zero, high));
            }
        }
        self.body.notes.append(&mut notes);
        pins
    }

    /// The facts on each way out of a block that starts with `facts`;
    /// rewriting what they prove when `rewrite` is set.
    fn exits(&mut self, block: usize, mut facts: Facts, rewrite: bool) -> Vec<(usize, Option<Facts>)> {
        let mut tests: HashMap<Local, Test> = HashMap::new();
        let mut statements = std::mem::take(&mut self.body.blocks[block].stmts);
        let mut remarks = Vec::new();
        for statement in &mut statements {
            if rewrite {
                self.rewrite(statement, &facts, &mut remarks);
            }
            self.step(statement, &mut facts, &mut tests);
        }
        self.body.blocks[block].stmts = statements;
        for (span, kind, text) in remarks {
            crate::mir_text::remark(self.body, span, kind, || text.to_string());
        }
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
                // A test the facts decide is a jump: the way it never takes
                // goes, and what only that way reached.
                if rewrite && yes_holds != no_holds {
                    let (taken, never) = if yes_holds { (then, otherwise) } else { (otherwise, then) };
                    self.body.blocks[block].terminator = Terminator::Goto(BlockId(taken as u32));
                    crate::mir_text::note(self.body, "prove", || {
                        format!("b{block}'s test always goes to b{taken}: no way to b{never}")
                    });
                }
                vec![(then, yes_holds.then_some(yes)), (otherwise, no_holds.then_some(no))]
            }
            _ => Vec::new(),
        }
    }

    /// Marks what the facts before a statement prove in it.
    fn rewrite(&self, statement: &mut Statement, facts: &Facts, remarks: &mut Vec<(Span, &'static str, &'static str)>) {
        let within = |place: &mut Place| {
            if let Some(Proj::Index(index, span)) = place.path.first()
                && self.array(place.local)
                && self.int(*index)
                && facts.bound(Node::Zero, Node::Int(*index)).is_some_and(|k| k <= 0)
                && facts.bound(Node::Int(*index), Node::Len(place.local)).is_some_and(|k| k <= -1)
            {
                remarks.push((*span, "unchecked", "stays within its array: no bounds check"));
                place.path[0] = Proj::InBounds(*index);
            }
        };
        let mut within = within;
        crate::inline::visit_statement(statement, &mut within);
        let span = match statement {
            Statement::Assign(_, _, span) => *span,
            _ => Span::default(),
        };
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
                // Arithmetic of constants says nothing.
                if low < high {
                    remarks.push((span, "unchecked", "cannot overflow: no check"));
                }
            }
        } else if let Statement::Assign(place, rvalue, _) = statement
            && place.path.is_empty()
            && self.typed_int(place.local)
            && let Rvalue::Binary(crate::ast::BinOp::Div | crate::ast::BinOp::Rem, a, b) = &**rvalue
            && let (Some(a), Some((Node::Zero, divisor))) = (self.value(a), self.value(b))
            && divisor > 0
            && facts.range(a).0 >= 0
        {
            // A dividend not below zero by a constant above it: unsigned,
            // without the work signs take, a shift or a mask by a power of
            // two and a multiplication by another.
            exact(rvalue);
            if facts.range(a).0 < facts.range(a).1 {
                let said = if (divisor as u128).is_power_of_two() {
                    "is a shift or a mask: its dividend is not negative"
                } else {
                    "divides without signs: its dividend is not negative"
                };
                remarks.push((span, "arithmetic", said));
            }
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
                if let Rvalue::Mutate(mutation, target, ..) = &**rvalue
                    && *mutation != crate::hir::Mutation::Reserve
                {
                    facts.forget(Node::Len(target.local));
                }
                if let Rvalue::AssignSlice(target, ..) = &**rvalue {
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
            | Statement::LendAt(..)
            | Statement::Failing(..) => {}
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
        let constant = |operand: &Operand| match self.value(operand) {
            Some((Node::Zero, c)) => Some(c),
            _ => None,
        };
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
            // A checked product by a constant went on, so it fits, a
            // multiple of the constant: `3 * n` is below `Int.max`, and `3 * n
            // + 1` fits, whatever `n` is, an element or a capture too.
            Rvalue::Binary(BinOp::Mul, a, b) if constant(a).or(constant(b)).is_some_and(|c| c.abs() >= 2) => {
                let c = constant(a).or(constant(b)).map_or(1, i128::abs);
                let (low, high) = match (self.value(a), self.value(b)) {
                    (Some(a), Some(b)) => arithmetic(BinOp::Mul, facts.range(a), facts.range(b)),
                    _ => (-LIMIT, LIMIT),
                };
                let (min, max) = (i64::MIN as i128, i64::MAX as i128);
                Some((low.max(min / c * c), high.min(max / c * c)))
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
