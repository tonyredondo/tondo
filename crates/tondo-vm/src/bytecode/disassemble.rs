use std::fmt::Write;

use super::*;

/// Renders deterministic tooling text. The text is intentionally not a stable
/// serialization format and cannot be loaded by the VM.
pub fn disassemble(program: &BytecodeProgram) -> String {
    let mut output = String::new();
    writeln!(output, "; Tondo bootstrap bytecode (tooling only)").unwrap();
    for (index, ty) in program.types.iter().enumerate() {
        writeln!(
            output,
            "type t{index} = {} ; {}",
            ty.name,
            type_kind_text(&ty.kind)
        )
        .unwrap();
    }
    for (index, nominal) in program.nominals.iter().enumerate() {
        writeln!(
            output,
            "nominal n{index} = {} ; {:?}",
            nominal.identity, nominal.shape
        )
        .unwrap();
    }
    for (index, callable) in program.callables.iter().enumerate() {
        writeln!(
            output,
            "callable c{index} {} : t{} -> t{} ; impl={:?} closure={:?}",
            callable.name,
            callable.function_type.index(),
            callable.outcome.index(),
            callable.implementation.map(BytecodeFunctionId::index),
            callable.closure,
        )
        .unwrap();
    }
    for (index, constant) in program.constants.iter().enumerate() {
        writeln!(
            output,
            "const k{index} {} : t{} = {:?}",
            constant.name,
            constant.value.ty.index(),
            constant.value.kind
        )
        .unwrap();
    }
    for (index, function) in program.functions.iter().enumerate() {
        writeln!(
            output,
            "\nfunction f{index} c{} file{}:{}..{} {{",
            function.callable.index(),
            function.source.file,
            function.source.start,
            function.source.end
        )
        .unwrap();
        write!(output, "  types").unwrap();
        for ty in &function.types {
            write!(output, " t{}", ty.index()).unwrap();
        }
        writeln!(output).unwrap();
        for (slot_index, slot) in function.slots.iter().enumerate() {
            writeln!(
                output,
                "  slot s{slot_index}: t{} @p{} ; {:?}",
                slot.ty.index(),
                slot.span.index(),
                slot.kind
            )
            .unwrap();
        }
        for (block_index, block) in function.blocks.iter().enumerate() {
            writeln!(output, "  b{block_index} [{:?}]:", block.kind).unwrap();
            for instruction in &block.instructions {
                writeln!(
                    output,
                    "    @p{} {}",
                    instruction.span.index(),
                    instruction_text(&instruction.kind)
                )
                .unwrap();
            }
            writeln!(
                output,
                "    @p{} {}",
                block.terminator.span.index(),
                terminator_text(&block.terminator.kind)
            )
            .unwrap();
        }
        writeln!(output, "}}").unwrap();
    }
    output
}

fn type_kind_text(kind: &BytecodeTypeKind) -> String {
    match kind {
        BytecodeTypeKind::OpaqueResult {
            identity,
            arguments,
            ..
        } => format!("OpaqueResult {{ identity: {identity:?}, arguments: {arguments:?} }}"),
        kind => format!("{kind:?}"),
    }
}

fn instruction_text(instruction: &BytecodeInstructionKind) -> String {
    match instruction {
        BytecodeInstructionKind::StorageLive(slot) => format!("storage_live s{}", slot.index()),
        BytecodeInstructionKind::StorageDead(slot) => format!("storage_dead s{}", slot.index()),
        BytecodeInstructionKind::Store { destination, value } => format!(
            "store {} <- {:?}:t{}",
            place_text(destination),
            value.kind,
            value.ty.index()
        ),
    }
}

fn terminator_text(terminator: &BytecodeTerminatorKind) -> String {
    match terminator {
        BytecodeTerminatorKind::Goto { target } => format!("goto b{}", target.index()),
        BytecodeTerminatorKind::BranchBool {
            if_true, if_false, ..
        } => format!("branch_bool -> b{}, b{}", if_true.index(), if_false.index()),
        BytecodeTerminatorKind::BranchTag {
            cases, otherwise, ..
        } => format!(
            "branch_tag {:?} otherwise b{}",
            cases
                .iter()
                .map(|(tag, target)| (tag, target.index()))
                .collect::<Vec<_>>(),
            otherwise.index()
        ),
        BytecodeTerminatorKind::Invoke {
            operation,
            target,
            unwind,
            ..
        } => format!(
            "invoke {:?}:t{} -> {:?} unwind b{}",
            operation.kind,
            operation.ty.index(),
            target.map(BytecodeBlockId::index),
            unwind.index()
        ),
        BytecodeTerminatorKind::IteratorNext {
            has_value,
            exhausted,
            unwind,
            ..
        } => format!(
            "iterator_next -> b{}, b{} unwind b{}",
            has_value.index(),
            exhausted.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::ValidatePlaces { target, unwind, .. } => {
            format!(
                "validate_places -> b{} unwind b{}",
                target.index(),
                unwind.index()
            )
        }
        BytecodeTerminatorKind::Return => "return".into(),
        BytecodeTerminatorKind::ResumePanic => "resume_panic".into(),
        BytecodeTerminatorKind::Unreachable => "unreachable".into(),
    }
}

fn place_text(place: &BytecodePlace) -> String {
    if place.projections.is_empty() {
        format!("s{}:t{}", place.slot.index(), place.ty.index())
    } else {
        format!(
            "s{}{:?}:t{}",
            place.slot.index(),
            place
                .projections
                .iter()
                .map(|projection| &projection.kind)
                .collect::<Vec<_>>(),
            place.ty.index()
        )
    }
}
