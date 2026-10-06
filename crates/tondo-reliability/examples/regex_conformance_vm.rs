//! Private regex conformance adapter for the verified hosted bytecode VM.
//!
//! The bytecode invokes a bodyless, test-only host callable. This exercises
//! VM host dispatch and result admission without claiming that the public
//! `std.regex` compiler API has been implemented.

#[path = "../../tondo-stdlib/examples/support/regex_conformance_cases.rs"]
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
                name: format!("conformance.regex.{case}"),
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

struct RegexHost;

impl VmHost for RegexHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        if !arguments.is_empty() {
            return Err(VmError::UnsupportedHostCall(name.into()));
        }
        let case = name
            .strip_prefix("conformance.regex.")
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))?;
        cases::run_case(case)
            .map(RuntimeValue::String)
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))
    }
}

fn verify_independent_oracle() {
    use tondo_reliability::regex_model::{ReferenceOptions, ReferenceRegex};
    let corpus: serde_json::Value = serde_json::from_slice(cases::CORPUS).unwrap();
    for fixture in corpus["valid"].as_array().unwrap() {
        if !fixture["model"].as_bool().unwrap() {
            continue;
        }
        let bits = fixture["options"].as_u64().unwrap();
        let options = ReferenceOptions {
            case_insensitive: bits & 1 != 0,
            multi_line: bits & 2 != 0,
            dot_matches_newline: bits & 4 != 0,
            crlf: bits & 8 != 0,
            ungreedy: bits & 16 != 0,
        };
        let regex = ReferenceRegex::compile(fixture["pattern"].as_str().unwrap(), options).unwrap();
        let input = fixture["input"].as_str().unwrap();
        let first = regex.find(input).unwrap().map(|found| {
            found
                .captures
                .into_iter()
                .map(|value| value.map(|span| [span.start, span.end]))
                .collect::<Vec<_>>()
        });
        assert_eq!(serde_json::to_value(first).unwrap(), fixture["first"]);
        assert_eq!(
            regex.full_match(input).unwrap().is_some(),
            fixture["full"].as_bool().unwrap()
        );
        if let Some(all) = fixture.get("all") {
            let spans = regex
                .find_all(input)
                .unwrap()
                .into_iter()
                .map(|found| [found.span.start, found.span.end])
                .collect::<Vec<_>>();
            assert_eq!(serde_json::to_value(spans).unwrap(), *all);
        }
        if let Some(replacement) = fixture.get("replacement") {
            let template = replacement["template"].as_str().unwrap();
            assert_eq!(
                regex.replace(input, template, false).unwrap(),
                replacement["first"].as_str().unwrap()
            );
            assert_eq!(
                regex.replace(input, template, true).unwrap(),
                replacement["all"].as_str().unwrap()
            );
        }
    }
}

fn main() {
    verify_independent_oracle();
    for case in CASES {
        let result = execute(&program(case), BytecodeFunctionId::new(0), &mut RegexHost)
            .expect("verified VM regex execution");
        let VmOutcome::Returned(RuntimeValue::String(line)) = result.outcome else {
            panic!("regex conformance must return a String")
        };
        println!("{line}");
    }
    println!("regex-conformance-ok");
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
                execute(&program(case), BytecodeFunctionId::new(0), &mut RegexHost).unwrap();
            assert_eq!(
                actual.outcome,
                VmOutcome::Returned(RuntimeValue::String(expected))
            );
        }
    }
    #[test]
    fn private_host_rejects_unknown_routes_and_arguments() {
        for name in [
            "std.regex.compile",
            "conformance.regex.missing",
            "conformance.toml.retained-vectors",
        ] {
            assert!(matches!(
                RegexHost.invoke(name, &[]),
                Err(VmError::UnsupportedHostCall(_))
            ));
        }
        assert!(matches!(
            RegexHost.invoke(
                "conformance.regex.retained-vectors",
                &[RuntimeValue::String("unexpected".into())]
            ),
            Err(VmError::UnsupportedHostCall(_))
        ));
    }
}
