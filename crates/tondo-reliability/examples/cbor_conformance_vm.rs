//! Private CBOR conformance adapter for the verified hosted bytecode VM.
//!
//! The bytecode invokes a bodyless, test-only host callable. This exercises
//! VM host dispatch and result admission without claiming that the public
//! `std.cbor` compiler API has been implemented.

#[path = "../../tondo-stdlib/examples/support/cbor_conformance_cases.rs"]
mod cases;

use cases::CASES;
use tondo_vm::bytecode::{
    BytecodeBlock, BytecodeBlockId, BytecodeBlockKind, BytecodeCallProtocol, BytecodeCallable,
    BytecodeCallableId, BytecodeFunction, BytecodeFunctionId, BytecodeFunctionType,
    BytecodeOperand, BytecodeOperandKind, BytecodeOperation, BytecodeOperationKind, BytecodePlace,
    BytecodeProgram, BytecodeScalarType, BytecodeSlot, BytecodeSlotId, BytecodeSlotKind,
    BytecodeSpan, BytecodeSpanId, BytecodeTerminator, BytecodeTerminatorKind, BytecodeType,
    BytecodeTypeId, BytecodeTypeKind,
};
use tondo_vm::runtime::{RuntimeValue, VmError, VmHost, VmOutcome, execute};

fn program(case: &str) -> BytecodeProgram {
    let text = BytecodeTypeId::new(0);
    let function_type = BytecodeTypeId::new(1);
    let span = BytecodeSpan {
        file: 0,
        start: 0,
        end: 0,
    };
    let place = BytecodePlace {
        slot: BytecodeSlotId::new(0),
        ty: text,
        projections: Vec::new(),
        source_loan: None,
    };
    BytecodeProgram {
        reflection: Default::default(),
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
                    outcome: text,
                }),
            },
        ],
        nominals: Vec::new(),
        callables: vec![
            BytecodeCallable {
                assertion_display: None,
                name: "main".into(),
                generic_arity: 0,
                parameters: Vec::new(),
                outcome: text,
                function_type,
                implementation: Some(BytecodeFunctionId::new(0)),
                closure: None,
            },
            BytecodeCallable {
                assertion_display: None,
                name: format!("conformance.cbor.{case}"),
                generic_arity: 0,
                parameters: Vec::new(),
                outcome: text,
                function_type,
                implementation: None,
                closure: None,
            },
        ],
        constants: Vec::new(),
        functions: vec![BytecodeFunction {
            callable: BytecodeCallableId::new(0),
            source: span,
            types: vec![text, function_type],
            spans: vec![span],
            slots: vec![BytecodeSlot {
                ty: text,
                span: BytecodeSpanId::new(0),
                kind: BytecodeSlotKind::Return,
            }],
            loans: Vec::new(),
            parameters: Vec::new(),
            return_slot: BytecodeSlotId::new(0),
            entry: BytecodeBlockId::new(0),
            unwind: BytecodeBlockId::new(2),
            blocks: vec![
                BytecodeBlock {
                    kind: BytecodeBlockKind::Normal,
                    instructions: Vec::new(),
                    terminator: BytecodeTerminator {
                        span: BytecodeSpanId::new(0),
                        kind: BytecodeTerminatorKind::Invoke {
                            operation: BytecodeOperation {
                                ty: text,
                                kind: BytecodeOperationKind::Call {
                                    callee: BytecodeOperand {
                                        ty: function_type,
                                        kind: BytecodeOperandKind::Function {
                                            callable: BytecodeCallableId::new(1),
                                            arguments: Vec::new(),
                                        },
                                    },
                                    arguments: Vec::new(),
                                    signature: function_type,
                                    protocol: BytecodeCallProtocol::Call,
                                    unsafe_call: false,
                                },
                            },
                            destination: Some(place),
                            target: Some(BytecodeBlockId::new(1)),
                            unwind: BytecodeBlockId::new(2),
                        },
                    },
                },
                BytecodeBlock {
                    kind: BytecodeBlockKind::Normal,
                    instructions: Vec::new(),
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
    }
}

struct CborHost;

impl VmHost for CborHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        if !arguments.is_empty() {
            return Err(VmError::UnsupportedHostCall(name.into()));
        }
        let case = name
            .strip_prefix("conformance.cbor.")
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))?;
        cases::run_case(case)
            .map(RuntimeValue::String)
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))
    }
}

fn verify_independent_oracle() {
    use tondo_reliability::cbor_model::{
        ReferenceErrorKind, parse_reference, render_deterministic, render_ordinary,
    };
    let corpus: serde_json::Value = serde_json::from_slice(cases::CORPUS).unwrap();
    for fixture in corpus["valid"].as_array().unwrap() {
        let reference = parse_reference(&cases::unhex(fixture["wire"].as_str().unwrap())).unwrap();
        assert_eq!(
            render_ordinary(&reference).unwrap(),
            cases::unhex(fixture["ordinary"].as_str().unwrap())
        );
        if let Some(expected) = fixture["deterministic"].as_str() {
            assert_eq!(
                render_deterministic(&reference).unwrap(),
                cases::unhex(expected)
            );
        } else {
            assert_eq!(
                render_deterministic(&reference).unwrap_err().kind,
                ReferenceErrorKind::KeyCollision
            );
        }
    }
    for fixture in corpus["invalid"].as_array().unwrap() {
        assert!(parse_reference(&cases::unhex(fixture["wire"].as_str().unwrap())).is_err());
    }
    assert!(parse_reference(cases::TYPED_WIRE).is_ok());
    assert!(parse_reference(cases::PATH_WIRE).is_err());
}

fn main() {
    verify_independent_oracle();
    for case in CASES {
        let result = execute(&program(case), BytecodeFunctionId::new(0), &mut CborHost)
            .expect("verified VM CBOR execution");
        let VmOutcome::Returned(RuntimeValue::String(line)) = result.outcome else {
            panic!("CBOR conformance must return a String")
        };
        println!("{line}");
    }
    println!("cbor-conformance-ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bytecode_dispatch_returns_every_exact_shared_case() {
        verify_independent_oracle();
        for case in CASES {
            let expected = cases::run_case(case).unwrap();
            let actual =
                execute(&program(case), BytecodeFunctionId::new(0), &mut CborHost).unwrap();
            assert_eq!(
                actual.outcome,
                VmOutcome::Returned(RuntimeValue::String(expected))
            );
        }
    }
    #[test]
    fn private_host_rejects_unknown_routes_and_arguments() {
        for name in [
            "std.cbor.parse",
            "conformance.cbor.missing",
            "conformance.toml.wire-model",
        ] {
            assert!(matches!(
                CborHost.invoke(name, &[]),
                Err(VmError::UnsupportedHostCall(_))
            ));
        }
        assert!(matches!(
            CborHost.invoke(
                "conformance.cbor.wire-model",
                &[RuntimeValue::String("unexpected".into())]
            ),
            Err(VmError::UnsupportedHostCall(_))
        ));
    }
}
