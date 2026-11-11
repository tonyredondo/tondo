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
const BLOCKS: usize = 8;

/// One way out of a branch: the blocks it runs, the checks they make, and the
/// block where the ways meet with the arguments this way gives it.
struct Way {
    blocks: Vec<Block>,
    /// Each check's branch, and whether it fails when its condition is not
    /// zero, rather than when it is.
    checks: Vec<(Inst, bool)>,
    join: Block,
    args: Vec<BlockArg>,
    work: usize,
}

/// Makes the branches it can choices; whether it made any. The control flow
/// graph is what it was before.
pub(crate) fn run(func: &mut Function, cfg: &ControlFlowGraph) -> bool {
    // How many times each value is used, as the branches made choices leave
    // it or use it less: whether a block passes its parameters on alone.
    let mut uses: SecondaryMap<Value, u32> = SecondaryMap::new();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for value in func.dfg.inst_values(inst) {
                uses[value] += 1;
            }
        }
    }
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
        if !convertible(func, block, &taken, &other)
            || taken.blocks.iter().chain(&other.blocks).chain([&taken.join]).any(|&b| touched[b])
        {
            continue;
        }
        for &b in taken.blocks.iter().chain(&other.blocks).chain([&block, &taken.join]) {
            touched[b] = true;
        }
        choose(func, branch, condition, &taken, &other);
        changed = true;
    }
    changed
}

/// The way that starts at `start`, which `from` branches to with `args`.
fn follow(
    func: &Function,
    cfg: &ControlFlowGraph,
    uses: &SecondaryMap<Value, u32>,
    from: Block,
    start: Block,
    args: Vec<BlockArg>,
) -> Option<Way> {
    // A block with parameters, or reached from elsewhere too, is where the
    // ways meet: this one runs nothing on its way there.
    if !func.dfg.block_params(start).is_empty() || cfg.pred_iter(start).count() != 1 || func.layout.is_cold(start) {
        let (join, args) = forwarded(func, uses, start, args);
        return Some(Way { blocks: Vec::new(), checks: Vec::new(), join, args, work: 0 });
    }
    let pool = &func.dfg.value_lists;
    let mut way = Way { blocks: Vec::new(), checks: Vec::new(), join: start, args: Vec::new(), work: 0 };
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
                // On to a block only this way reaches, as the e-graph leaves
                // those it emptied.
                if destination.len(pool) == 0
                    && func.dfg.block_params(next).is_empty()
                    && cfg.pred_iter(next).count() == 1
                    && !func.layout.is_cold(next)
                {
                    block = next;
                    continue;
                }
                (way.join, way.args) = forwarded(func, uses, next, destination.args(pool).collect());
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
    uses: &SecondaryMap<Value, u32>,
    mut block: Block,
    mut args: Vec<BlockArg>,
) -> (Block, Vec<BlockArg>) {
    for _ in 0..2 {
        let Some(last) = func.layout.last_inst(block) else { break };
        let InstructionData::Jump { destination, .. } = func.dfg.insts[last] else { break };
        if func.layout.is_cold(block)
            || !func.layout.block_insts(block).take_while(|&inst| inst != last).all(|inst| site(func, inst))
            || !only_passed(func, uses, block, last)
        {
            break;
        }
        let params = func.dfg.block_params(block);
        let pool = &func.dfg.value_lists;
        let next = destination.block(pool);
        if next == block {
            break;
        }
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
fn only_passed(func: &Function, uses: &SecondaryMap<Value, u32>, block: Block, last: Inst) -> bool {
    func.dfg.block_params(block).iter().all(|&param| {
        let passed = func.dfg.inst_values(last).filter(|&value| value == param).count() as u32;
        uses[param] == passed
    })
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

/// Replaces a branch by the work of both ways, their
/// checks, and a jump to where they meet with the arguments chosen.
fn choose(func: &mut Function, branch: Inst, condition: Value, taken: &Way, other: &Way) {
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
    let runs = truth(&mut pos, condition);
    let runs_other = if other.checks.is_empty() { None } else { Some(complement(&mut pos, runs)) };
    for (of_taken, check, when_set) in checks {
        let InstructionData::Brif { arg, blocks: [then, otherwise], .. } = pos.func.dfg.insts[check] else {
            unreachable!("a check is a branch")
        };
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
            (BlockArg::Value(a), BlockArg::Value(b)) if a != b => BlockArg::Value(pos.ins().select(runs, a, b)),
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
