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
        for (loan_index, loan) in function.loans.iter().enumerate() {
            writeln!(
                output,
                "  loan l{loan_index}: {:?} {:?} {}",
                loan.kind,
                loan.mode,
                place_text(&loan.place)
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
            capabilities,
            ..
        } => format!(
            "OpaqueResult {{ identity: {identity:?}, arguments: {arguments:?}, capabilities: {capabilities:?} }}"
        ),
        kind => format!("{kind:?}"),
    }
}

fn instruction_text(instruction: &BytecodeInstructionKind) -> String {
    match instruction {
        BytecodeInstructionKind::StorageLive(slot) => format!("storage_live s{}", slot.index()),
        BytecodeInstructionKind::StorageDead(slot) => format!("storage_dead s{}", slot.index()),
        BytecodeInstructionKind::ReserveLoan(loan) => format!("reserve_loan l{}", loan.index()),
        BytecodeInstructionKind::ReleaseLoan(loan) => format!("release_loan l{}", loan.index()),
        BytecodeInstructionKind::Store { destination, value } => format!(
            "store {} <- {:?}:t{}",
            place_text(destination),
            value.kind,
            value.ty.index()
        ),
        BytecodeInstructionKind::EnterTaskScope { scope } => {
            format!("enter_task_scope scope{}", scope.index())
        }
        BytecodeInstructionKind::RegisterDefer {
            scope,
            action,
            guard,
        } => format!(
            "register_defer scope{} {:?}:t{} guard={:?}",
            scope.index(),
            action.kind,
            action.ty.index(),
            guard.as_ref().map(place_text)
        ),
        BytecodeInstructionKind::RegisterFallback { scope, owner } => format!(
            "register_fallback scope{} {}",
            scope.index(),
            place_text(owner)
        ),
        BytecodeInstructionKind::RetargetCleanup { from, to } => {
            format!(
                "retarget_cleanup {} -> {}",
                place_text(from),
                place_text(to)
            )
        }
        BytecodeInstructionKind::DisarmCleanup(place) => {
            format!("disarm_cleanup {}", place_text(place))
        }
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
        BytecodeTerminatorKind::Await {
            awaitable,
            target,
            unwind,
            ..
        } => format!(
            "await {:?} -> b{} unwind b{}",
            awaitable,
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::Spawn {
            operation,
            scope,
            target,
            unwind,
            ..
        } => format!(
            "spawn scope{} {:?}:t{} -> b{} unwind b{}",
            scope.index(),
            operation.kind,
            operation.ty.index(),
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::IteratorNext {
            state,
            destination,
            borrowed_source,
            exhaustion_guard,
            has_value,
            exhausted,
            unwind,
        } => format!(
            "iterator_next {:?} -> {:?} borrowed={:?} exhaustion_guard={:?}; b{}, b{} unwind b{}",
            state,
            destination,
            borrowed_source,
            exhaustion_guard,
            has_value.index(),
            exhausted.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::ValidatePlaces {
            against,
            target,
            unwind,
            ..
        } => format!(
            "validate_places against {:?} -> b{} unwind b{}",
            against
                .iter()
                .map(|loans| loans.iter().map(|loan| loan.index()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::ValidateLoan {
            loan,
            against,
            target,
            unwind,
        } => format!(
            "validate_loan l{} against {:?} -> b{} unwind b{}",
            loan.index(),
            against.iter().map(|loan| loan.index()).collect::<Vec<_>>(),
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::DrainDefers {
            scopes,
            target,
            unwind,
        } => format!(
            "drain_defers {:?} -> b{} unwind b{}",
            scopes.iter().map(|scope| scope.index()).collect::<Vec<_>>(),
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::DrainScopes {
            task_scopes,
            defer_scopes,
            target,
            unwind,
        } => format!(
            "drain_scopes tasks={:?} defers={:?} -> b{} unwind b{}",
            task_scopes
                .iter()
                .map(|scope| scope.index())
                .collect::<Vec<_>>(),
            defer_scopes
                .iter()
                .map(|scope| scope.index())
                .collect::<Vec<_>>(),
            target.index(),
            unwind.index()
        ),
        BytecodeTerminatorKind::DrainUnwind { target } => {
            format!("drain_unwind -> b{}", target.index())
        }
        BytecodeTerminatorKind::Return => "return".into(),
        BytecodeTerminatorKind::ResumePanic => "resume_panic".into(),
        BytecodeTerminatorKind::Unreachable => "unreachable".into(),
    }
}

fn place_text(place: &BytecodePlace) -> String {
    let source = place
        .source_loan
        .map(|loan| format!("@l{}", loan.index()))
        .unwrap_or_default();
    if place.projections.is_empty() {
        format!("s{}{}:t{}", place.slot.index(), source, place.ty.index())
    } else {
        format!(
            "s{}{}{:?}:t{}",
            place.slot.index(),
            source,
            place
                .projections
                .iter()
                .map(|projection| &projection.kind)
                .collect::<Vec<_>>(),
            place.ty.index()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operand() -> BytecodeOperand {
        BytecodeOperand {
            ty: BytecodeTypeId::new(0),
            kind: BytecodeOperandKind::Constant(BytecodeConstant::Unit),
        }
    }

    fn place() -> BytecodePlace {
        BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty: BytecodeTypeId::new(0),
            projections: Vec::new(),
            source_loan: None,
        }
    }

    fn operation() -> BytecodeOperation {
        BytecodeOperation {
            ty: BytecodeTypeId::new(0),
            kind: BytecodeOperationKind::ExplicitPanic { message: operand() },
        }
    }

    #[test]
    fn tooling_text_covers_every_instruction_and_control_flow_shape() {
        let instructions = [
            BytecodeInstructionKind::StorageLive(BytecodeSlotId::new(0)),
            BytecodeInstructionKind::StorageDead(BytecodeSlotId::new(0)),
            BytecodeInstructionKind::ReserveLoan(BytecodeLoanId::new(0)),
            BytecodeInstructionKind::ReleaseLoan(BytecodeLoanId::new(0)),
            BytecodeInstructionKind::Store {
                destination: place(),
                value: BytecodeRvalue {
                    ty: BytecodeTypeId::new(0),
                    kind: BytecodeRvalueKind::Use(operand()),
                },
            },
            BytecodeInstructionKind::EnterTaskScope {
                scope: BytecodeScopeId::new(0),
            },
            BytecodeInstructionKind::RegisterDefer {
                scope: BytecodeScopeId::new(0),
                action: operation(),
                guard: Some(place()),
            },
            BytecodeInstructionKind::RegisterFallback {
                scope: BytecodeScopeId::new(0),
                owner: place(),
            },
            BytecodeInstructionKind::RetargetCleanup {
                from: place(),
                to: place(),
            },
            BytecodeInstructionKind::DisarmCleanup(place()),
        ];
        for instruction in &instructions {
            assert!(!instruction_text(instruction).is_empty());
        }

        let block = BytecodeBlockId::new(0);
        let terminators = [
            BytecodeTerminatorKind::Goto { target: block },
            BytecodeTerminatorKind::BranchBool {
                condition: operand(),
                if_true: block,
                if_false: block,
            },
            BytecodeTerminatorKind::BranchTag {
                value: operand(),
                cases: vec![(BytecodeTag::OptionNone, block)],
                otherwise: block,
            },
            BytecodeTerminatorKind::Invoke {
                operation: operation(),
                destination: Some(place()),
                target: Some(block),
                unwind: block,
            },
            BytecodeTerminatorKind::Invoke {
                operation: operation(),
                destination: None,
                target: None,
                unwind: block,
            },
            BytecodeTerminatorKind::Await {
                awaitable: BytecodeAwaitable::Join(operand()),
                destination: place(),
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::Spawn {
                operation: operation(),
                scope: BytecodeScopeId::new(0),
                destination: place(),
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::IteratorNext {
                state: place(),
                destination: place(),
                borrowed_source: Some(place()),
                exhaustion_guard: Some(place()),
                has_value: block,
                exhausted: block,
                unwind: block,
            },
            BytecodeTerminatorKind::ValidatePlaces {
                places: vec![place()],
                replacements: vec![Some(operand())],
                against: vec![vec![BytecodeLoanId::new(0)]],
                for_write: true,
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::ValidateLoan {
                loan: BytecodeLoanId::new(0),
                against: vec![BytecodeLoanId::new(1)],
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::DrainDefers {
                scopes: vec![BytecodeScopeId::new(0)],
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::DrainScopes {
                task_scopes: vec![BytecodeScopeId::new(0)],
                defer_scopes: vec![BytecodeScopeId::new(1)],
                target: block,
                unwind: block,
            },
            BytecodeTerminatorKind::DrainUnwind { target: block },
            BytecodeTerminatorKind::Return,
            BytecodeTerminatorKind::ResumePanic,
            BytecodeTerminatorKind::Unreachable,
        ];
        for terminator in &terminators {
            assert!(!terminator_text(terminator).is_empty());
        }
    }

    #[test]
    fn complete_program_disassembly_is_deterministic_and_explicit() {
        let ty = BytecodeTypeId::new(0);
        let projected = BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty,
            projections: vec![BytecodeProjection {
                ty,
                kind: BytecodeProjectionKind::TupleField(0),
            }],
            source_loan: Some(BytecodeLoanId::new(0)),
        };
        assert_eq!(place_text(&place()), "s0:t0");
        assert!(place_text(&projected).contains("@l0"));
        assert!(
            type_kind_text(&BytecodeTypeKind::OpaqueResult {
                identity: "test::opaque".into(),
                arguments: vec![ty],
                witness: ty,
                capabilities: BytecodeCapabilitySet::default(),
            })
            .contains("test::opaque")
        );
        assert!(
            type_kind_text(&BytecodeTypeKind::Scalar(BytecodeScalarType::Unit)).contains("Unit")
        );

        let program = BytecodeProgram {
            types: vec![BytecodeType {
                name: "Unit".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
            }],
            nominals: vec![BytecodeNominal {
                name: "Wrapper".into(),
                identity: "test::Wrapper".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Newtype { underlying: ty },
            }],
            callables: vec![BytecodeCallable {
                name: "main".into(),
                generic_arity: 0,
                parameters: Vec::new(),
                outcome: ty,
                function_type: ty,
                implementation: Some(BytecodeFunctionId::new(0)),
                closure: None,
            }],
            constants: vec![BytecodeNamedConstant {
                name: "UNIT".into(),
                value: BytecodeConstantValue {
                    ty,
                    kind: BytecodeConstantValueKind::Unit,
                },
            }],
            functions: vec![BytecodeFunction {
                callable: BytecodeCallableId::new(0),
                source: BytecodeSpan {
                    file: 0,
                    start: 1,
                    end: 2,
                },
                types: vec![ty],
                spans: vec![BytecodeSpan {
                    file: 0,
                    start: 1,
                    end: 2,
                }],
                slots: vec![BytecodeSlot {
                    ty,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Return,
                }],
                loans: vec![BytecodeLoan {
                    kind: BytecodeLoanKind::CallLocal,
                    mode: BytecodeParameterMode::Ref,
                    place: place(),
                }],
                parameters: Vec::new(),
                return_slot: BytecodeSlotId::new(0),
                entry: BytecodeBlockId::new(0),
                unwind: BytecodeBlockId::new(0),
                blocks: vec![BytecodeBlock {
                    kind: BytecodeBlockKind::Normal,
                    instructions: vec![BytecodeInstruction {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeInstructionKind::StorageLive(BytecodeSlotId::new(0)),
                    }],
                    terminator: BytecodeTerminator {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeTerminatorKind::Return,
                    },
                }],
            }],
        };
        let first = disassemble(&program);
        assert_eq!(disassemble(&program), first);
        for expected in [
            "type t0 = Unit",
            "nominal n0 = test::Wrapper",
            "callable c0 main",
            "const k0 UNIT",
            "function f0 c0",
            "slot s0",
            "loan l0",
            "storage_live s0",
            "return",
        ] {
            assert!(
                first.contains(expected),
                "missing `{expected}` in:\n{first}"
            );
        }
    }
}
