//! Private TOML conformance adapter for the verified hosted bytecode VM.
//!
//! The bytecode invokes a bodyless, test-only host callable. This exercises
//! VM host dispatch and result admission without claiming that the public
//! `std.toml` compiler API has been implemented.

use std::collections::BTreeMap;
use std::io::{self, Read};

use tondo_stdlib::toml::{
    self, TomlErrorKind, TomlLimits, TomlOptions, TomlPathSegment, TomlReader, TomlValue,
};
use tondo_vm::bytecode::{
    BytecodeBlock, BytecodeBlockId, BytecodeBlockKind, BytecodeCallProtocol, BytecodeCallable,
    BytecodeCallableId, BytecodeFunction, BytecodeFunctionId, BytecodeFunctionType,
    BytecodeOperand, BytecodeOperandKind, BytecodeOperation, BytecodeOperationKind, BytecodePlace,
    BytecodeProgram, BytecodeScalarType, BytecodeSlot, BytecodeSlotId, BytecodeSlotKind,
    BytecodeSpan, BytecodeSpanId, BytecodeTerminator, BytecodeTerminatorKind, BytecodeType,
    BytecodeTypeId, BytecodeTypeKind,
};
use tondo_vm::runtime::{RuntimeValue, VmError, VmHost, VmOutcome, execute};

const CASES: [&str; 6] = [
    "typed-dynamic",
    "interoperability",
    "streaming",
    "errors-path",
    "limits-lifecycle",
    "route-boundary",
];

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
                name: format!("conformance.toml.{case}"),
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

struct TomlHost;

struct OneByteReader {
    bytes: Vec<u8>,
    offset: usize,
}

impl Read for OneByteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.offset == self.bytes.len() {
            return Ok(0);
        }
        output[0] = self.bytes[self.offset];
        self.offset += 1;
        Ok(1)
    }
}

fn event_count(mut reader: TomlReader) -> usize {
    let mut count = 0;
    while reader.next().expect("TOML event").is_some() {
        count += 1;
    }
    assert_eq!(reader.next().unwrap_err().kind, TomlErrorKind::Closed);
    count
}

impl VmHost for TomlHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        assert!(
            arguments.is_empty(),
            "TOML conformance host takes no arguments"
        );
        let case = name
            .strip_prefix("conformance.toml.")
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))?;
        let options = TomlOptions::defaults();
        let line = match case {
            "typed-dynamic" => {
                let source = include_bytes!(
                    "../../../testing/stdlib-toml-conformance-fixtures/typed-dynamic.toml"
                );
                let value = toml::parse(source, options).expect("dynamic TOML parse");
                let TomlValue::Table(members) = &value else {
                    panic!("root table")
                };
                assert_eq!(members.len(), 3);
                let TomlValue::Text(name) = &members[0].value else {
                    panic!("name")
                };
                let TomlValue::Int(count) = members[1].value else {
                    panic!("count")
                };
                assert_eq!(
                    toml::encode(&value, options).unwrap(),
                    b"name = \"Tondo\"\ncount = 7\nactive = true\n"
                );
                let typed_source =
                    include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/typed.toml");
                let typed: BTreeMap<String, i64> =
                    toml::decode_static(typed_source, options).expect("typed TOML decode");
                assert_eq!(typed.get("a"), Some(&7));
                assert_eq!(typed.get("b"), Some(&9));
                assert_eq!(toml::encode_static(&typed, options).unwrap(), typed_source);
                format!(
                    "typed-dynamic:{}:{name}:{count}:{}",
                    members.len(),
                    typed.len()
                )
            }
            "interoperability" => {
                let source = include_bytes!(
                    "../../../testing/stdlib-toml-conformance-fixtures/interoperability.toml"
                );
                let value = toml::parse(source, options).expect("TOML 1.1 parse");
                let normal = toml::encode(&value, options).expect("normal encode");
                let canonical =
                    toml::encode_canonical(&value, options.limits).expect("canonical encode");
                assert!(normal.starts_with(b"z = \"caf\xC3\xA9\"\n"));
                assert!(canonical.starts_with(b"a = 16\n"));
                let canonical_value = toml::parse(&canonical, options).unwrap();
                assert_eq!(
                    toml::encode_canonical(&canonical_value, options.limits).unwrap(),
                    canonical
                );
                let TomlValue::Table(members) = value else {
                    panic!("root table")
                };
                let TomlValue::Int(radix) = members[1].value else {
                    panic!("radix")
                };
                let TomlValue::Text(text) = &members[0].value else {
                    panic!("text")
                };
                assert!(matches!(members[2].value, TomlValue::OffsetDateTime(_)));
                format!(
                    "interoperability:{}:{radix}:{text}:date-time",
                    members.len()
                )
            }
            "streaming" => {
                let source = include_bytes!(
                    "../../../testing/stdlib-toml-conformance-fixtures/streaming.toml"
                );
                let count =
                    event_count(TomlReader::from_bytes(source, options).expect("TOML reader"));
                let chunk_count = event_count(
                    TomlReader::from_reader(
                        OneByteReader {
                            bytes: source.to_vec(),
                            offset: 0,
                        },
                        options,
                    )
                    .expect("one-byte TOML reader"),
                );
                assert_eq!(count, 12);
                assert_eq!(chunk_count, count);
                let TomlValue::Table(root) = toml::parse(source, options).unwrap() else {
                    panic!("root")
                };
                let TomlValue::Array(rows) = &root[1].value else {
                    panic!("rows")
                };
                format!("streaming:{}:{count}:closed", rows.len())
            }
            "errors-path" => {
                let source = include_bytes!(
                    "../../../testing/stdlib-toml-conformance-fixtures/duplicate.toml"
                );
                let error = toml::parse(source, options).unwrap_err();
                assert_eq!(error.kind, TomlErrorKind::DuplicateKey);
                assert_eq!(error.path, vec![TomlPathSegment::Key("first".into())]);
                assert_eq!(error.span.start_offset, 10);
                assert_eq!((error.span.start_line, error.span.start_column), (2, 1));
                let TomlPathSegment::Key(key) = &error.path[0] else {
                    panic!("key path")
                };
                format!(
                    "errors-path:{:?}:{key}:{}:{}:{}",
                    error.kind,
                    error.span.start_offset,
                    error.span.start_line,
                    error.span.start_column
                )
            }
            "limits-lifecycle" => {
                let mut limits = TomlLimits::defaults();
                limits.max_input_bytes = 3;
                let source =
                    include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/limit.toml");
                let error = toml::parse(source, TomlOptions::create(limits)).unwrap_err();
                assert_eq!(error.kind, TomlErrorKind::ResourceLimit);
                assert_eq!(error.span.start_offset, 0);
                assert!(TomlReader::from_bytes(source, TomlOptions::create(limits)).is_err());
                format!(
                    "limits-lifecycle:{:?}:{}",
                    error.kind, error.span.start_offset
                )
            }
            "route-boundary" => {
                "route-boundary:scalar:simd-not-claimed:native-aot-not-claimed".to_owned()
            }
            _ => return Err(VmError::UnsupportedHostCall(name.into())),
        };
        Ok(RuntimeValue::String(line))
    }
}

fn main() {
    for case in CASES {
        let result = execute(&program(case), BytecodeFunctionId::new(0), &mut TomlHost)
            .expect("verified VM conformance execution");
        let VmOutcome::Returned(RuntimeValue::String(line)) = result.outcome else {
            panic!("TOML conformance must return a String");
        };
        println!("{line}");
    }
    println!("toml-conformance-ok");
}
