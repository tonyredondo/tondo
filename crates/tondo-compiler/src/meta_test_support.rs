use tondo_vm::bytecode::{
    BytecodeBlock, BytecodeBlockId, BytecodeBlockKind, BytecodeCallable, BytecodeCallableId,
    BytecodeConstant, BytecodeFunction, BytecodeFunctionId, BytecodeFunctionType,
    BytecodeInstruction, BytecodeInstructionKind, BytecodeOperand, BytecodeOperandKind,
    BytecodePlace, BytecodeProgram, BytecodeRvalue, BytecodeRvalueKind, BytecodeScalarType,
    BytecodeSlot, BytecodeSlotId, BytecodeSlotKind, BytecodeSpan, BytecodeSpanId,
    BytecodeTerminator, BytecodeTerminatorKind, BytecodeType, BytecodeTypeId, BytecodeTypeKind,
};

use crate::meta_vm::MetaVmArtifact;

pub(crate) fn string_artifact(value: &str) -> MetaVmArtifact {
    let string = BytecodeTypeId::new(0);
    let function_type = BytecodeTypeId::new(1);
    let span = BytecodeSpan {
        file: 0,
        start: 0,
        end: 1,
    };
    let place = BytecodePlace {
        slot: BytecodeSlotId::new(0),
        ty: string,
        projections: Vec::new(),
        source_loan: None,
    };
    let program = BytecodeProgram {
        types: vec![
            BytecodeType {
                name: "String".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::String),
            },
            BytecodeType {
                name: "fn(): String".into(),
                kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                    is_async: false,
                    is_selectable: false,
                    is_unsafe: false,
                    parameters: Vec::new(),
                    variadic: None,
                    outcome: string,
                }),
            },
        ],
        nominals: Vec::new(),
        callables: vec![BytecodeCallable {
            name: "meta_provider".into(),
            generic_arity: 0,
            parameters: Vec::new(),
            outcome: string,
            function_type,
            implementation: Some(BytecodeFunctionId::new(0)),
            closure: None,
        }],
        constants: Vec::new(),
        functions: vec![BytecodeFunction {
            callable: BytecodeCallableId::new(0),
            source: span,
            types: vec![string, function_type],
            spans: vec![span],
            slots: vec![BytecodeSlot {
                ty: string,
                span: BytecodeSpanId::new(0),
                kind: BytecodeSlotKind::Return,
            }],
            loans: Vec::new(),
            parameters: Vec::new(),
            return_slot: BytecodeSlotId::new(0),
            entry: BytecodeBlockId::new(0),
            unwind: BytecodeBlockId::new(1),
            blocks: vec![
                BytecodeBlock {
                    kind: BytecodeBlockKind::Normal,
                    instructions: vec![BytecodeInstruction {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeInstructionKind::Store {
                            destination: place,
                            value: BytecodeRvalue {
                                ty: string,
                                kind: BytecodeRvalueKind::Use(BytecodeOperand {
                                    ty: string,
                                    kind: BytecodeOperandKind::Constant(BytecodeConstant::String(
                                        crate::std_meta::MetaRenderer::string(value),
                                    )),
                                }),
                            },
                        },
                    }],
                    terminator: BytecodeTerminator {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeTerminatorKind::Return,
                    },
                },
                BytecodeBlock {
                    kind: BytecodeBlockKind::Cleanup,
                    instructions: Vec::new(),
                    terminator: BytecodeTerminator {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeTerminatorKind::ResumePanic,
                    },
                },
            ],
        }],
    };
    MetaVmArtifact::new(program, BytecodeFunctionId::new(0))
}
