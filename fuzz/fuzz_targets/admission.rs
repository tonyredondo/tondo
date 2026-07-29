#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_compiler::driver::ResourceLimits;
use tondo_reliability::generator::Generator;
use tondo_reliability::harness::{check, run};
use tondo_vm::bytecode::{
    BytecodeIntrinsicType, BytecodeNominal, BytecodeNominalId, BytecodeNominalShape,
    BytecodeProgram, BytecodeScalarType, BytecodeType, BytecodeTypeId, BytecodeTypeKind,
    BytecodeVerificationLimits, verify_bytecode_with_limits,
};

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(64 * 1024)];
    exercise_typed_pipeline(input);
    exercise_structured_pipeline(input);
    exercise_bytecode_admission(input);
});

fn exercise_typed_pipeline(input: &[u8]) {
    let mut seed_bytes = [0_u8; 8];
    let available = input.len().min(seed_bytes.len());
    seed_bytes[..available].copy_from_slice(&input[..available]);
    let seed = u64::from_le_bytes(seed_bytes);
    let mut generator = Generator::new(seed);
    let expression = generator.integer_expression(2);
    let value = i64::try_from(generator.next_u64() % 16).unwrap();
    let expected = expression.evaluate(value).unwrap();
    let source = format!(
        "\
fn main() {{
    let value = {value}
    assert({} == {expected})
}}
",
        expression.render()
    );
    let observation = run(
        "fuzz-admission",
        &source,
        ResourceLimits {
            max_syntax_tokens: 16_384,
            max_syntax_nodes: 32_768,
            max_hir_nodes: 32_768,
            max_mir_statements_per_function: 32_768,
            max_bytecode_instructions_per_function: 65_536,
            max_vm_steps: 1_000_000,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert!(
        observation.accepted && observation.exit_code == 0,
        "typed generator produced {:?}",
        observation.diagnostic_codes
    );

    if input.first().is_some_and(|byte| byte & 1 == 1) {
        let invalid = format!(
            "fn invalid(value: Int): String {{ {} }}\n",
            expression.render()
        );
        let first = check("fuzz-admission-invalid", &invalid).unwrap();
        let second = check("fuzz-admission-invalid", &invalid).unwrap();
        assert_eq!(first, second);
    }
}

fn exercise_structured_pipeline(input: &[u8]) {
    let value = i64::from(input.get(8).copied().unwrap_or(42) % 64);
    let templates = [
        "\
enum Problem {
    Missing
}
fn extract(value: Int?): Int ! Problem {
    match value {
        some(item) => item
        none => fail Problem.Missing
    }
}
fn exercise(): Int ! Problem { extract(some(VALUE))? }
",
        "\
fn observe(value: ref Int): Int { value }
fn update(value: mut Int) {
    value += 1
}
fn exercise(): Int {
    var value = VALUE
    let before = observe(ref value)
    update(mut value)
    before + value
}
",
        "\
fn consume(value: Int) {}
fn exercise(): Int {
    defer consume(VALUE)
    var total = 0
    for item in [1, 2, 3] {
        if item > 1 {
            total += item
        }
    }
    total
}
",
        "\
async fn immediate(value: Int): Int { value }
async fn exercise(): Int {
    scope {
        let pending = spawn immediate(VALUE)
        await pending
    }
}
",
    ];
    let selected = usize::from(input.get(9).copied().unwrap_or(0)) % templates.len();
    let source = templates[selected].replace("VALUE", &value.to_string());
    let first = check("fuzz-structured-admission", &source).unwrap();
    let second = check("fuzz-structured-admission", &source).unwrap();
    assert_eq!(first, second);
    assert!(
        first.accepted,
        "structured generator produced {:?}",
        first.diagnostic_codes
    );
}

fn exercise_bytecode_admission(input: &[u8]) {
    let mut program = BytecodeProgram {
        types: vec![
            BytecodeType {
                name: "Int".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Int),
            },
            BytecodeType {
                name: "Unit".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
            },
        ],
        nominals: Vec::new(),
        callables: Vec::new(),
        constants: Vec::new(),
        functions: Vec::new(),
    };
    for (index, chunk) in input.chunks(4).take(128).enumerate() {
        let first = chunk[0];
        let referenced = BytecodeTypeId::new(u32::from(*chunk.get(1).unwrap_or(&0)));
        let kind = match first % 8 {
            0 => BytecodeTypeKind::Scalar(BytecodeScalarType::Int),
            1 => BytecodeTypeKind::Tuple(vec![referenced]),
            2 => BytecodeTypeKind::Option(referenced),
            3 => BytecodeTypeKind::Result {
                success: referenced,
                error: BytecodeTypeId::new(u32::from(*chunk.get(2).unwrap_or(&0))),
            },
            4 => BytecodeTypeKind::Intrinsic {
                constructor: match chunk.get(2).copied().unwrap_or(0) % 5 {
                    0 => BytecodeIntrinsicType::Array,
                    1 => BytecodeIntrinsicType::Map,
                    2 => BytecodeIntrinsicType::Set,
                    3 => BytecodeIntrinsicType::Range,
                    _ => BytecodeIntrinsicType::Ref,
                },
                arguments: vec![referenced],
            },
            5 => BytecodeTypeKind::Nominal {
                nominal: Some(BytecodeNominalId::new(u32::from(
                    *chunk.get(2).unwrap_or(&0),
                ))),
                identity: format!("fuzz::Type{index}"),
                arguments: vec![referenced],
            },
            6 => BytecodeTypeKind::GenericParameter(u32::from(*chunk.get(2).unwrap_or(&0))),
            _ => BytecodeTypeKind::Union(vec![referenced]),
        };
        program.types.push(BytecodeType {
            name: format!("Fuzz{index}"),
            kind,
        });
        if first & 0x80 != 0 {
            program.nominals.push(BytecodeNominal {
                name: format!("Nominal{index}"),
                identity: format!("fuzz::Nominal{index}"),
                generic_arity: u32::from(*chunk.get(3).unwrap_or(&0) % 3),
                shape: BytecodeNominalShape::Newtype {
                    underlying: referenced,
                },
            });
        }
    }
    let _ = verify_bytecode_with_limits(
        &program,
        BytecodeVerificationLimits {
            max_dataflow_steps: 1_000_000,
        },
    );
}
