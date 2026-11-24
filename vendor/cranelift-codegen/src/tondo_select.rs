//! Tondo: a branch whose two ways compute a few values, without failing,
//! touching memory or calling anything, chooses between those values with
//! `select`, which is `cmov` on x86-64 and `csel` on arm64, rather than
//! jumping to one way or the other: a jump the processor guesses wrong costs
//! it the work of a dozen instructions or more, and one that depends on the
//! data, as `n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 }`, is guessed
//! wrong often. A way may check what it computes, with a branch to a cold
//! block, as an overflow check does: the check stays, and fails when its way
//! would have run and its condition holds, as before.
//!
//! What a way computes runs whichever way the branch goes, so a way is short
//! and its values are integers, which a `select` chooses without a jump.

use crate::cursor::{Cursor, FuncCursor};
use crate::flowgraph::ControlFlowGraph;
use crate::ir::condcodes::{CondCode, IntCC};
use crate::ir::{Block, BlockArg, Function, Inst, InstBuilder, InstructionData, Opcode, Value, ValueDef};
use alloc::vec::Vec;
use cranelift_entity::SecondaryMap;

/// The most instructions, besides constants, that one way may compute, and
/// both together.
const WAY: usize = 6;
const BOTH: usize = 10;

/// The most blocks a way may run, each ending in a check or a jump on.
const BLOCKS: usize = 24;

/// One way out of a branch: the blocks it runs, single-predecessor block
/// parameters folded along the way, the checks they make, and the block where
/// the ways meet with the arguments this way gives it.
struct Way {
    blocks: Vec<Block>,
    /// Single-predecessor block parameters replaced by their sole incoming value.
    aliases: Vec<(Value, Value)>,
    /// Each check's branch, and whether it fails when its condition is not
    /// zero, rather than when it is.
    checks: Vec<(Inst, bool)>,
    join: Block,
    args: Vec<BlockArg>,
    work: usize,
}

/// An empty way paired with a single checked addition or subtraction on an
/// accumulator (`if cond { acc += delta }`), converted via a zero-addend
/// `select` so the overflow check stays unconditional without `band`.
struct CheckedAccumulate {
    in_taken: bool,
    inst: Inst,
    acc: Value,
    delta: Value,
}

/// Makes the branches it can choices; whether it made any. The control flow
/// graph is what it was before.
pub(crate) fn run(func: &mut Function, cfg: &ControlFlowGraph) -> bool {
    let mut uses = Uses::default();
    let mut touched: SecondaryMap<Block, bool> = SecondaryMap::new();
    let mut changed = false;
    let blocks: Vec<Block> = func.layout.blocks().collect();
    for block in blocks {
        if touched[block] || func.layout.is_cold(block) {
            continue;
        }
        let Some(branch) = func.layout.last_inst(block) else { continue };
        let InstructionData::Brif { arg: condition, blocks: [then, otherwise], .. } = func.dfg.insts[branch] else {
            continue;
        };
        let pool = &func.dfg.value_lists;
        let (then_block, then_args): (Block, Vec<BlockArg>) = (then.block(pool), then.args(pool).collect());
        let (else_block, else_args): (Block, Vec<BlockArg>) = (otherwise.block(pool), otherwise.args(pool).collect());
        if then_block == else_block {
            continue;
        }
        let (Some(taken), Some(other)) = (
            follow(func, cfg, &uses, block, then_block, then_args),
            follow(func, cfg, &uses, block, else_block, else_args),
        ) else {
            continue;
        };
        if taken.blocks.iter().chain(&other.blocks).chain([&taken.join]).any(|&b| touched[b]) {
            continue;
        }
        if let Some(accum) = checked_accumulate(func, &uses, block, &taken, &other) {
            for &b in taken.blocks.iter().chain(&other.blocks).chain([&block, &taken.join]) {
                touched[b] = true;
            }
            choose_checked_accumulate(func, branch, condition, &taken, &other, accum);
            uses.clear();
            changed = true;
            continue;
        }
        if !convertible(func, block, &taken, &other) {
            continue;
        }
        for &b in taken.blocks.iter().chain(&other.blocks).chain([&block, &taken.join]) {
            touched[b] = true;
        }
        choose(func, branch, condition, &taken, &other);
        uses.clear();
        changed = true;
    }
    if changed {
        func.dfg.resolve_all_aliases();
    }
    changed
}

fn resolve_way_value(aliases: &[(Value, Value)], mut value: Value) -> Value {
    while let Some(&(_, target)) = aliases.iter().find(|&&(param, _)| param == value) {
        value = target;
    }
    value
}

fn record_params(func: &Function, block: Block, args: &[BlockArg], aliases: &mut Vec<(Value, Value)>) -> bool {
    let params = func.dfg.block_params(block);
    if params.len() != args.len() {
        return false;
    }
    for (&param, &arg) in params.iter().zip(args) {
        let BlockArg::Value(value) = arg else { return false };
        let resolved = resolve_way_value(aliases, value);
        if resolved == param {
            return false;
        }
        aliases.push((param, resolved));
    }
    true
}

/// The way that starts at `start`, which `from` branches to with `args`.
fn follow(
    func: &Function,
    cfg: &ControlFlowGraph,
    uses: &Uses,
    from: Block,
    start: Block,
    args: Vec<BlockArg>,
) -> Option<Way> {
    // A block reached from elsewhere too, or cold, is where the ways meet:
    // this one runs nothing on its way there.
    if cfg.pred_iter(start).count() != 1 || func.layout.is_cold(start) {
        let (join, args) = forwarded(func, cfg, uses, start, args);
        return Some(Way { blocks: Vec::new(), aliases: Vec::new(), checks: Vec::new(), join, args, work: 0 });
    }
    let pool = &func.dfg.value_lists;
    let mut way = Way { blocks: Vec::new(), aliases: Vec::new(), checks: Vec::new(), join: start, args: Vec::new(), work: 0 };
    if !func.dfg.block_params(start).is_empty() && !record_params(func, start, &args, &mut way.aliases) {
        let (join, args) = forwarded(func, cfg, uses, start, args);
        return Some(Way { blocks: Vec::new(), aliases: Vec::new(), checks: Vec::new(), join, args, work: 0 });
    }
    let mut block = start;
    loop {
        if block == from || way.blocks.len() == BLOCKS || way.blocks.contains(&block) {
            return None;
        }
        way.blocks.push(block);
        let last = func.layout.last_inst(block)?;
        for inst in func.layout.block_insts(block).take_while(|&inst| inst != last) {
            way.work += cost(func, inst)?;
        }
        match func.dfg.insts[last] {
            InstructionData::Jump { destination, .. } => {
                let next = destination.block(pool);
                let dest_args: Vec<BlockArg> = destination.args(pool).collect();
                // On to a block only this way reaches, folding single-predecessor
                // block parameters that egraph left behind after pruning dead edges.
                if cfg.pred_iter(next).count() == 1
                    && !func.layout.is_cold(next)
                    && !way.blocks.contains(&next)
                    && next != from
                {
                    let saved = way.aliases.len();
                    if record_params(func, next, &dest_args, &mut way.aliases) {
                        block = next;
                        continue;
                    }
                    way.aliases.truncate(saved);
                }
                let resolved: Vec<BlockArg> = dest_args
                    .into_iter()
                    .map(|arg| match arg {
                        BlockArg::Value(v) => BlockArg::Value(resolve_way_value(&way.aliases, v)),
                        other => other,
                    })
                    .collect();
                (way.join, way.args) = forwarded(func, cfg, uses, next, resolved);
                return Some(way);
            }
            InstructionData::Brif { blocks: [then, otherwise], .. } => {
                let (to_then, to_otherwise) = (then.block(pool), otherwise.block(pool));
                // A check: one way to a cold block, which it never comes back
                // from, and the other on.
                let (fails_when_set, next, next_call) =
                    match (func.layout.is_cold(to_then), func.layout.is_cold(to_otherwise)) {
                        (true, false) => (true, to_otherwise, otherwise),
                        (false, true) => (false, to_then, then),
                        _ => return None,
                    };
                if next_call.len(pool) != 0
                    || !func.dfg.block_params(next).is_empty()
                    || cfg.pred_iter(next).count() != 1
                {
                    return None;
                }
                way.checks.push((last, fails_when_set));
                way.work += 1;
                block = next;
            }
            _ => return None,
        }
    }
}

/// Where a way that goes to `block` with `args` goes on to, past blocks
/// that only pass values on, as MIR leaves where it copied a branch into the
/// ways of another: their latent sites aside, a jump with their parameters
/// or values from before them, which come before the branch too.
fn forwarded(
    func: &Function,
    cfg: &ControlFlowGraph,
    uses: &Uses,
    mut block: Block,
    mut args: Vec<BlockArg>,
) -> (Block, Vec<BlockArg>) {
    let pool = &func.dfg.value_lists;
    for _ in 0..8 {
        if func.layout.is_cold(block) {
            break;
        }
        // Step through a single-predecessor chain of empty/site-only zero-arg
        // jumps from `block` (such as a loop header trampoline `b3(p) -> b4 ->
        // b5 -> b6 -> b8(p)`) to find the jump that forwards `block`'s params.
        let mut cur = block;
        let mut steps = 0;
        let dest = loop {
            let Some(last) = func.layout.last_inst(cur) else { break None };
            let InstructionData::Jump { destination, .. } = func.dfg.insts[last] else { break None };
            if func.layout.is_cold(cur)
                || !func
                    .layout
                    .block_insts(cur)
                    .take_while(|&inst| inst != last)
                    .all(|inst| site(func, inst) || dead_pure(func, uses, inst))
            {
                break None;
            }
            let next = destination.block(pool);
            if next == cur || next == block {
                break None;
            }
            if destination.len(pool) == 0
                && func.dfg.block_params(next).is_empty()
                && cfg.pred_iter(next).count() == 1
                && steps < 8
            {
                cur = next;
                steps += 1;
                continue;
            }
            break Some((last, destination));
        };
        let Some((last, destination)) = dest else { break };
        if !only_passed(func, uses, block, last) {
            break;
        }
        let params = func.dfg.block_params(block);
        let next = destination.block(pool);
        let passed: Vec<BlockArg> = destination
            .args(pool)
            .map(|arg| match arg {
                BlockArg::Value(value) => match params.iter().position(|&param| param == value) {
                    Some(index) => args[index],
                    None => arg,
                },
                other => other,
            })
            .collect();
        (block, args) = (next, passed);
    }
    (block, args)
}

/// Whether the parameters of `block` are used only by its last instruction,
/// which passes them on: a way that goes past it then needs none of them.
fn only_passed(func: &Function, uses: &Uses, block: Block, last: Inst) -> bool {
    let params = func.dfg.block_params(block);
    if params.is_empty() {
        return true;
    }
    let uses = uses.counts(func);
    params.iter().all(|&param| {
        let passed = func.dfg.inst_values(last).filter(|&value| value == param).count() as u32;
        uses[param] == passed
    })
}

/// Whether `inst` is pure and none of its results are used anywhere in `func`,
/// such as unused constants left behind by e-graph lowering.
fn dead_pure(func: &Function, uses: &Uses, inst: Inst) -> bool {
    let results = func.dfg.inst_results(inst);
    if results.is_empty() || cost(func, inst).is_none() {
        return false;
    }
    let uses = uses.counts(func);
    results.iter().all(|&v| uses[v] == 0)
}

/// How many times each value of a function is used, counted once a round
/// needs it and reset whenever a branch is converted.
#[derive(Default)]
struct Uses(core::cell::OnceCell<SecondaryMap<Value, u32>>);

impl Uses {
    fn counts(&self, func: &Function) -> &SecondaryMap<Value, u32> {
        self.0.get_or_init(|| {
            let mut uses: SecondaryMap<Value, u32> = SecondaryMap::new();
            for block in func.layout.blocks() {
                for inst in func.layout.block_insts(block) {
                    for value in func.dfg.inst_values(inst) {
                        uses[value] += 1;
                    }
                }
            }
            uses
        })
    }

    fn clear(&mut self) {
        self.0.take();
    }
}

/// What running an instruction whichever way a branch goes costs, in
/// instructions besides constants; `None` when it may not run so: it does
/// more than compute a value, may fail, or is slow enough that running it for
/// nothing costs more than a jump.
fn cost(func: &Function, inst: Inst) -> Option<usize> {
    let opcode = func.dfg.insts[inst].opcode();
    if site(func, inst) {
        return Some(0);
    }
    match opcode {
        Opcode::Iconst | Opcode::F32const | Opcode::F64const => return Some(0),
        // By a constant that is not zero, nor -1 when signed, a division
        // cannot fail: a shift by a power of two, or a multiplication.
        Opcode::Udiv | Opcode::Urem | Opcode::Sdiv | Opcode::Srem => {
            let divisor = constant(func, func.dfg.inst_args(inst)[1])?;
            let signed = matches!(opcode, Opcode::Sdiv | Opcode::Srem);
            return match divisor {
                0 => None,
                -1 if signed => None,
                _ if divisor > 0 && (divisor as u64).is_power_of_two() => Some(1),
                _ => Some(3),
            };
        }
        Opcode::Fdiv | Opcode::Sqrt => return None,
        _ => {}
    }
    let fails_or_acts = opcode.is_branch()
        || opcode.is_terminator()
        || opcode.is_call()
        || opcode.is_return()
        || opcode.can_trap()
        || opcode.can_load()
        || opcode.can_store()
        || opcode.other_side_effects();
    (!fails_or_acts).then_some(1)
}

/// Whether an instruction is a call of a patchable function with nothing to
/// pass or give, which does nothing until a tool patches it in, as Tondo's
/// latent sites for logpoints at the start of a line: a way made part of a
/// choice has none, as its line may not run, as a loop that calls nothing has
/// none either.
fn site(func: &Function, inst: Inst) -> bool {
    match func.dfg.insts[inst] {
        InstructionData::Call { func_ref, .. } => {
            func.dfg.ext_funcs[func_ref].patchable
                && func.dfg.inst_args(inst).is_empty()
                && func.dfg.inst_results(inst).is_empty()
        }
        _ => false,
    }
}

/// The value of an integer constant.
fn constant(func: &Function, value: Value) -> Option<i64> {
    let ValueDef::Result(inst, 0) = func.dfg.value_def(value) else { return None };
    match func.dfg.insts[inst] {
        InstructionData::UnaryImm { opcode: Opcode::Iconst, imm } => {
            // As its type holds it: `iconst.i32 -1` is 0xffff_ffff.
            let bits = func.dfg.value_type(value).bits();
            let raw = imm.bits();
            Some(if bits >= 64 { raw } else { (raw << (64 - bits)) >> (64 - bits) })
        }
        _ => None,
    }
}

/// Whether the two ways of the branch ending `block` meet with few enough
/// instructions between them, and arguments a `select` can choose.
fn convertible(func: &Function, block: Block, taken: &Way, other: &Way) -> bool {
    if taken.join != other.join
        || taken.join == block
        || taken.blocks.contains(&taken.join)
        || other.blocks.contains(&taken.join)
        || taken.work > WAY
        || other.work > WAY
        || taken.work + other.work > BOTH
        || taken.args.len() != other.args.len()
        // A way that runs nothing beside one that checks what it computes
        // is most often a count that a rare case adds to, which the
        // processor guesses right, and whose check would cost as much again.
        || (taken.work == 0 && !other.checks.is_empty())
        || (other.work == 0 && !taken.checks.is_empty())
    {
        return false;
    }
    taken.args.iter().zip(&other.args).all(|(a, b)| match (a, b) {
        _ if a == b => true,
        (BlockArg::Value(a), BlockArg::Value(b)) => {
            let ty = func.dfg.value_type(*a);
            ty.is_int() && ty.bits() <= 64 && ty == func.dfg.value_type(*b)
        }
        _ => false,
    })
}

/// Whether two values (resolved through each way's folded single-predecessor
/// block params) compute the exact same pure expression, so duplicate work
/// like `pos + 1` in both arms need not be selected or computed twice.
fn same_pure_value(
    func: &Function,
    a_aliases: &[(Value, Value)],
    a: Value,
    b_aliases: &[(Value, Value)],
    b: Value,
    depth: usize,
) -> bool {
    let a = resolve_way_value(a_aliases, a);
    let b = resolve_way_value(b_aliases, b);
    if a == b {
        return true;
    }
    if depth == 0 || func.dfg.value_type(a) != func.dfg.value_type(b) {
        return false;
    }
    let (ValueDef::Result(ia, 0), ValueDef::Result(ib, 0)) = (func.dfg.value_def(a), func.dfg.value_def(b)) else {
        return false;
    };
    if cost(func, ia).is_none() || cost(func, ib).is_none() {
        return false;
    }
    match (func.dfg.insts[ia], func.dfg.insts[ib]) {
        (
            InstructionData::UnaryImm { opcode: op_a, imm: imm_a },
            InstructionData::UnaryImm { opcode: op_b, imm: imm_b },
        ) => op_a == op_b && imm_a == imm_b,
        (
            InstructionData::Unary { opcode: op_a, arg: arg_a },
            InstructionData::Unary { opcode: op_b, arg: arg_b },
        ) => op_a == op_b && same_pure_value(func, a_aliases, arg_a, b_aliases, arg_b, depth - 1),
        (
            InstructionData::Binary { opcode: op_a, args: [la, ra] },
            InstructionData::Binary { opcode: op_b, args: [lb, rb] },
        ) if op_a == op_b => {
            (same_pure_value(func, a_aliases, la, b_aliases, lb, depth - 1)
                && same_pure_value(func, a_aliases, ra, b_aliases, rb, depth - 1))
                || (matches!(op_a, Opcode::Iadd | Opcode::Imul | Opcode::Band | Opcode::Bor | Opcode::Bxor)
                    && same_pure_value(func, a_aliases, la, b_aliases, rb, depth - 1)
                    && same_pure_value(func, a_aliases, ra, b_aliases, lb, depth - 1))
        }
        (
            InstructionData::BinaryImm8 { opcode: op_a, arg: arg_a, imm: imm_a },
            InstructionData::BinaryImm8 { opcode: op_b, arg: arg_b, imm: imm_b },
        ) => op_a == op_b && imm_a == imm_b && same_pure_value(func, a_aliases, arg_a, b_aliases, arg_b, depth - 1),
        _ => false,
    }
}

/// Recognizes `if cond { acc += delta }` (or `-=`) where one way has no
/// checks and the other has a single checked add/sub and its overflow check,
/// while all other join arguments match (directly or via identical pure ops).
fn checked_accumulate(
    func: &Function,
    uses: &Uses,
    block: Block,
    taken: &Way,
    other: &Way,
) -> Option<CheckedAccumulate> {
    if taken.join != other.join
        || taken.join == block
        || taken.blocks.contains(&taken.join)
        || other.blocks.contains(&taken.join)
        || taken.args.len() != other.args.len()
    {
        return None;
    }
    let (in_taken, active, empty) = if other.checks.is_empty() && taken.checks.len() == 1 && taken.work <= WAY {
        (true, taken, other)
    } else if taken.checks.is_empty() && other.checks.len() == 1 && other.work <= WAY {
        (false, other, taken)
    } else {
        return None;
    };
    let (check_inst, true) = active.checks[0] else { return None };
    let InstructionData::Brif { arg: check_arg, .. } = func.dfg.insts[check_inst] else { return None };
    let mut accum_inst = None;
    for &b in &active.blocks {
        let last = func.layout.last_inst(b)?;
        for inst in func.layout.block_insts(b).take_while(|&inst| inst != last) {
            if site(func, inst)
                || func.dfg.insts[inst].opcode() == Opcode::Iconst
                || dead_pure(func, uses, inst)
            {
                continue;
            }
            if matches!(
                func.dfg.insts[inst].opcode(),
                Opcode::SaddOverflow | Opcode::SsubOverflow | Opcode::UaddOverflow | Opcode::UsubOverflow
            ) {
                if accum_inst.replace(inst).is_some() {
                    return None;
                }
            }
        }
    }
    let inst = accum_inst?;
    let opcode = func.dfg.insts[inst].opcode();
    let results = func.dfg.inst_results(inst);
    if results.len() != 2 {
        return None;
    }
    let (v_res, v_ovf) = (results[0], results[1]);
    if resolve_way_value(&active.aliases, check_arg) != v_ovf {
        return None;
    }
    // No other instruction moved before `branch` may depend on `inst`'s results,
    // since `inst` will be placed after `select(cond, delta, 0)`.
    for &b in &active.blocks {
        let last = func.layout.last_inst(b)?;
        for other_inst in func.layout.block_insts(b).take_while(|&i| i != last) {
            if other_inst == inst {
                continue;
            }
            if func.dfg.inst_values(other_inst).any(|v| {
                let v = resolve_way_value(&active.aliases, v);
                v == v_res || v == v_ovf
            }) {
                return None;
            }
        }
    }
    let mut diff = None;
    for (idx, (&a, &e)) in active.args.iter().zip(&empty.args).enumerate() {
        let same = match (a, e) {
            _ if a == e => true,
            (BlockArg::Value(va), BlockArg::Value(ve)) => {
                same_pure_value(func, &active.aliases, va, &empty.aliases, ve, 3)
            }
            _ => false,
        };
        if same {
            continue;
        }
        if diff.replace((idx, a, e)).is_some() {
            return None;
        }
    }
    let (_, BlockArg::Value(active_val), BlockArg::Value(acc)) = diff? else { return None };
    if resolve_way_value(&active.aliases, active_val) != v_res {
        return None;
    }
    let acc = resolve_way_value(&empty.aliases, acc);
    // `acc` must come from before the branch (not computed inside `empty`).
    if let ValueDef::Result(acc_inst, _) = func.dfg.value_def(acc)
        && let Some(acc_block) = func.layout.inst_block(acc_inst)
        && empty.blocks.contains(&acc_block)
    {
        return None;
    }
    let ty = func.dfg.value_type(acc);
    if !ty.is_int() || ty.bits() > 64 || ty != func.dfg.value_type(v_res) {
        return None;
    }
    let args = func.dfg.inst_args(inst);
    let lhs = resolve_way_value(&active.aliases, args[0]);
    let rhs = resolve_way_value(&active.aliases, args[1]);
    let delta = match opcode {
        Opcode::SaddOverflow | Opcode::UaddOverflow => {
            if lhs == acc {
                rhs
            } else if rhs == acc {
                lhs
            } else {
                return None;
            }
        }
        Opcode::SsubOverflow | Opcode::UsubOverflow => {
            if lhs == acc {
                rhs
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(CheckedAccumulate { in_taken, inst, acc, delta })
}

fn apply_aliases(func: &mut Function, taken: &Way, other: &Way) {
    for way in [taken, other] {
        for &(param, replacement) in way.aliases.iter().rev() {
            let target = func.dfg.resolve_aliases(replacement);
            func.dfg.remove_block_param(param);
            func.dfg.change_to_alias(param, target);
        }
    }
}

/// Replaces `if cond { acc += delta }` (with overflow check) by selecting
/// `delta` or `0` and adding unconditionally: `acc + 0` never overflows, so
/// the overflow check stays unconditional and branchless.
fn choose_checked_accumulate(
    func: &mut Function,
    branch: Inst,
    condition: Value,
    taken: &Way,
    other: &Way,
    accum: CheckedAccumulate,
) {
    apply_aliases(func, taken, other);
    let line = func.srcloc(branch);
    let active = if accum.in_taken { taken } else { other };
    let empty = if accum.in_taken { other } else { taken };
    // Move `active`'s pure setup instructions (e.g. `pos + 1`, `delta`) before
    // `branch`, leaving `accum.inst` to be inserted right after `select(...)`,
    // and drop `empty`'s redundant instructions since `join` takes `active.args`.
    for &b in &active.blocks {
        let last = func.layout.last_inst(b).expect("a way's block ends");
        let work: Vec<Inst> = func.layout.block_insts(b).take_while(|&inst| inst != last).collect();
        for inst in work {
            func.layout.remove_inst(inst);
            if !site(func, inst) && inst != accum.inst {
                func.layout.insert_inst(inst, branch);
                func.set_srcloc(inst, line);
            }
        }
    }
    for &b in &empty.blocks {
        let last = func.layout.last_inst(b).expect("a way's block ends");
        let work: Vec<Inst> = func.layout.block_insts(b).take_while(|&inst| inst != last).collect();
        for inst in work {
            func.layout.remove_inst(inst);
        }
    }
    let check_inst = active.checks[0].0;
    let InstructionData::Brif { blocks: [cold, _], .. } = func.dfg.insts[check_inst] else {
        unreachable!("a check is a branch")
    };
    let pool = &func.dfg.value_lists;
    let (cold_block, cold_args): (Block, Vec<BlockArg>) = (cold.block(pool), cold.args(pool).collect());
    let acc = func.dfg.resolve_aliases(accum.acc);
    let delta = func.dfg.resolve_aliases(accum.delta);
    let ty = func.dfg.value_type(acc);
    let mut pos = FuncCursor::new(func).at_inst(branch);
    let zero = pos.ins().iconst(ty, 0);
    let condition = pos.func.dfg.resolve_aliases(condition);
    let cond = truth(&mut pos, condition);
    let addend = if accum.in_taken { pos.ins().select(cond, delta, zero) } else { pos.ins().select(cond, zero, delta) };
    // Update the moved checked add/sub in place so its result `Value`s stay
    // valid for both `join` and the cold overflow block.
    match &mut pos.func.dfg.insts[accum.inst] {
        InstructionData::Binary { args, .. } => *args = [acc, addend],
        _ => unreachable!("checked add/sub is binary"),
    }
    pos.func.layout.insert_inst(accum.inst, branch);
    pos.func.set_srcloc(accum.inst, line);
    let v_ovf = pos.func.dfg.inst_results(accum.inst)[1];
    let next = pos.func.dfg.make_block();
    pos.ins().brif(v_ovf, cold_block, &cold_args, next, &[]);
    pos.insert_block(next);
    let args: Vec<BlockArg> = active
        .args
        .iter()
        .map(|&arg| match arg {
            BlockArg::Value(v) => BlockArg::Value(pos.func.dfg.resolve_aliases(v)),
            other => other,
        })
        .collect();
    pos.func.replace(branch).jump(taken.join, &args);
    for way in [taken, other] {
        for &b in &way.blocks {
            let last = pos.func.layout.last_inst(b).expect("a way's block ends");
            pos.func.layout.remove_inst(last);
            pos.func.layout.remove_block(b);
        }
    }
}

/// Replaces a branch by the work of both ways, their
/// checks, and a jump to where they meet with the arguments chosen.
fn choose(func: &mut Function, branch: Inst, condition: Value, taken: &Way, other: &Way) {
    apply_aliases(func, taken, other);
    // Their work, before the branch, on the line of the branch: a line of a
    // way that did not run is not one that ran.
    let line = func.srcloc(branch);
    for way in [taken, other] {
        for &b in &way.blocks {
            let last = func.layout.last_inst(b).expect("a way's block ends");
            let work: Vec<Inst> = func.layout.block_insts(b).take_while(|&inst| inst != last).collect();
            for inst in work {
                func.layout.remove_inst(inst);
                if !site(func, inst) {
                    func.layout.insert_inst(inst, branch);
                    func.set_srcloc(inst, line);
                }
            }
        }
    }
    let checks: Vec<(bool, Inst, bool)> = taken
        .checks
        .iter()
        .map(|&(check, when_set)| (true, check, when_set))
        .chain(other.checks.iter().map(|&(check, when_set)| (false, check, when_set)))
        .collect();
    let mut pos = FuncCursor::new(func).at_inst(branch);
    let condition = pos.func.dfg.resolve_aliases(condition);
    let runs = truth(&mut pos, condition);
    let runs_other = if other.checks.is_empty() { None } else { Some(complement(&mut pos, runs)) };
    for (of_taken, check, when_set) in checks {
        let InstructionData::Brif { arg, blocks: [then, otherwise], .. } = pos.func.dfg.insts[check] else {
            unreachable!("a check is a branch")
        };
        let arg = pos.func.dfg.resolve_aliases(arg);
        let cold = if when_set { then } else { otherwise };
        let pool = &pos.func.dfg.value_lists;
        let (cold_block, cold_args): (Block, Vec<BlockArg>) = (cold.block(pool), cold.args(pool).collect());
        let failing = if when_set { truth(&mut pos, arg) } else { pos.ins().icmp_imm_u(IntCC::Equal, arg, 0) };
        let way_runs = if of_taken { runs } else { runs_other.expect("made for the other way's checks") };
        let fails = pos.ins().band(way_runs, failing);
        let next = pos.func.dfg.make_block();
        pos.ins().brif(fails, cold_block, &cold_args, next, &[]);
        pos.insert_block(next);
    }
    let args: Vec<BlockArg> = taken
        .args
        .iter()
        .zip(&other.args)
        .map(|(&a, &b)| match (a, b) {
            (BlockArg::Value(a), BlockArg::Value(b)) => {
                let a = pos.func.dfg.resolve_aliases(a);
                let b = pos.func.dfg.resolve_aliases(b);
                if a == b || same_pure_value(pos.func, &[], a, &[], b, 3) {
                    BlockArg::Value(a)
                } else {
                    BlockArg::Value(pos.ins().select(runs, a, b))
                }
            }
            _ => a,
        })
        .collect();
    pos.func.replace(branch).jump(taken.join, &args);
    for way in [taken, other] {
        for &b in &way.blocks {
            let last = pos.func.layout.last_inst(b).expect("a way's block ends");
            pos.func.layout.remove_inst(last);
            pos.func.layout.remove_block(b);
        }
    }
}

/// 1 where `truth` is 0 and 0 where it is 1: the opposite comparison, when a
/// comparison of integers made it.
fn complement(pos: &mut FuncCursor, truth: Value) -> Value {
    if let ValueDef::Result(inst, 0) = pos.func.dfg.value_def(truth)
        && let InstructionData::IntCompare { opcode: Opcode::Icmp, cond, args } = pos.func.dfg.insts[inst]
    {
        return pos.ins().icmp(cond.complement(), args[0], args[1]);
    }
    pos.ins().bxor_imm_u(truth, 1)
}

/// `value` as 1 when it is not zero and 0 when it is: as it is when a
/// comparison made it.
fn truth(pos: &mut FuncCursor, value: Value) -> Value {
    if let ValueDef::Result(inst, 0) = pos.func.dfg.value_def(value)
        && matches!(pos.func.dfg.insts[inst].opcode(), Opcode::Icmp | Opcode::Fcmp)
    {
        return value;
    }
    pos.ins().icmp_imm_u(IntCC::NotEqual, value, 0)
}
