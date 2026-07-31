use std::fs;

use tondo_compiler::driver::{ResourceLimits, SourceForm};
use tondo_reliability::generator::Generator;
use tondo_reliability::harness::{check, decode_hex, format, run};
use tondo_reliability::inventory;
use tondo_reliability::workspace_root;

#[test]
fn generated_integer_programs_are_typed_by_construction_and_reducible() {
    for seed in 0..256 {
        let mut generator = Generator::new(seed);
        let expression = generator.integer_expression(4);
        let name = generator.identifier("generated");
        let source = format!(
            "\
fn {name}(value: Int): Int {{
    if value == 0 {{
        {}
    }} else {{
        {}
    }}
}}
",
            expression.render(),
            expression.render()
        );
        let observation = check(&format!("typed-{seed}"), &source).unwrap();
        assert!(
            observation.accepted,
            "seed {seed} produced {:?}: {}\n{source}",
            observation.diagnostic_codes, observation.diagnostics_jsonl
        );

        for (candidate_index, candidate) in expression.shrink().into_iter().enumerate() {
            let source = format!("fn reduced(value: Int): Int {{ {} }}\n", candidate.render());
            let reduced =
                check(&format!("typed-{seed}-shrink-{candidate_index}"), &source).unwrap();
            assert!(
                reduced.accepted,
                "seed {seed} shrink {candidate_index} was not a typed program"
            );
        }
    }
}

#[test]
fn typed_templates_cover_generics_traits_patterns_ownership_async_and_errors() {
    let templates = [
        (
            "generics",
            "\
fn identity[T](value: T): T { value }
fn exercise(): Int { identity(42) }
",
        ),
        (
            "traits",
            "\
trait Value {
    fn value(self): Int
}
type Item = {
    value: Int
}
impl Value for Item {
    fn value(self): Int { self.value }
}
fn exercise(): Int { Value.value(Item { value: 42 }) }
",
        ),
        (
            "patterns-errors",
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
fn exercise(): Int ! Problem { extract(some(42))? }
",
        ),
        (
            "ownership-borrows",
            "\
fn observe(value: ref Int): Int { value }
fn update(value: mut Int) {
    value += 1
}
fn exercise(): Int {
    var value = 41
    let before = observe(ref value)
    update(mut value)
    before + value
}
",
        ),
        (
            "async-structured",
            "\
async fn immediate(value: Int): Int { value }
async fn exercise(): Int {
    scope {
        let pending = spawn immediate(42)
        await pending
    }
}
",
        ),
        (
            "collections-control",
            "\
fn exercise(values: Array[Int]): Int {
    var total = 0
    for value in values {
        if value > 0 {
            total += value
        }
    }
    total
}
",
        ),
    ];

    for (index, (family, template)) in templates.into_iter().enumerate() {
        for seed in 0..16_u64 {
            let replacement = (seed + 40).to_string();
            let source = template.replace("42", &replacement);
            let observation = check(&format!("family-{index}-{seed}"), &source).unwrap();
            assert!(
                observation.accepted,
                "{family} seed {seed} produced {:?}\n{}\n{source}",
                observation.diagnostic_codes, observation.diagnostics_jsonl
            );
        }
    }
}

#[test]
fn formatter_reconstruction_and_idempotence_hold_for_generated_programs() {
    for seed in 0..128 {
        let mut generator = Generator::new(seed);
        let expression = generator.integer_expression(3);
        let source = format!("fn generated( value:Int ):Int{{{}}}\n", expression.render());
        let first = format(
            &format!("format-{seed}"),
            source.as_bytes(),
            SourceForm::Module,
        )
        .unwrap();
        assert!(first.accepted, "seed {seed}");
        let bytes = decode_hex(&first.stdout_hex).unwrap();
        let second = format(&format!("format-{seed}-again"), &bytes, SourceForm::Module).unwrap();
        assert_eq!(second.stdout_hex, first.stdout_hex, "seed {seed}");
    }
}

#[test]
fn alpha_renaming_parentheses_diagnostics_and_gc_pressure_are_metamorphic() {
    let left = check("alpha-left", "fn compute(value: Int): Int { value + 1 }\n").unwrap();
    let right = check(
        "alpha-right",
        "fn transformed(input: Int): Int { input + 1 }\n",
    )
    .unwrap();
    assert_eq!(left.accepted, right.accepted);
    assert_eq!(left.diagnostic_codes, right.diagnostic_codes);

    let plain = run(
        "parentheses-plain",
        "fn main() { assert(1 + 2 * 3 == 7) }\n",
        ResourceLimits::default(),
    )
    .unwrap();
    let parenthesized = run(
        "parentheses-explicit",
        "fn main() { assert((1 + (2 * 3)) == 7) }\n",
        ResourceLimits::default(),
    )
    .unwrap();
    assert_eq!(plain, parenthesized);

    let invalid_source = "fn invalid(): Int { \"text\" }\n";
    let first = check("diagnostic-stability", invalid_source).unwrap();
    let second = check("diagnostic-stability", invalid_source).unwrap();
    assert_eq!(first, second);

    let source = fs::read_to_string(
        workspace_root(&std::env::current_dir().unwrap())
            .unwrap()
            .join("tests/runtime/value-copy/value.to"),
    )
    .unwrap();
    let ordinary = run("gc-ordinary", &source, ResourceLimits::default()).unwrap();
    let pressure = run(
        "gc-pressure",
        &source,
        ResourceLimits {
            initial_vm_gc_threshold: 1,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert_eq!(ordinary, pressure);
}

#[test]
fn every_required_metamorphic_family_is_visible_in_the_inventory() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let inventory = inventory::build(&root).unwrap();
    let ids = inventory
        .tests
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for id in [
        "rust:tondo-compiler:tests::syntax_robustness:deterministic_arbitrary_byte_corpus_never_panics_or_loses_source",
        "rust:tondo-compiler:tests::formatter_spec:normative_minimum_corpus_matches_byte_for_byte_and_is_idempotent",
        "conformance:determinism/project-source-order",
        "rust:tondo-compiler:src::bytecode::lower:eager_and_cow_match_the_same_value_copy_observable_corpus",
        "rust:tondo-compiler:tests::bootstrap_harness:value_copy_observables_are_stable_under_gc_pressure",
    ] {
        assert!(ids.contains(id), "missing metamorphic evidence `{id}`");
    }
}
