use std::collections::BTreeSet;

use tondo_compiler::driver::ResourceLimits;
use tondo_reliability::generator::Generator;
use tondo_reliability::harness::{check, run};
use tondo_reliability::inventory;
use tondo_reliability::workspace_root;
use tondo_vm::bytecode::{ArraySliceError, normalize_array_index, normalize_array_slice_indices};
use tondo_vm::runtime::conformance::{MemoryScenario, run_memory_scenario};

#[test]
fn map_operation_sequences_match_an_insertion_order_model() {
    for seed in 0..24 {
        let mut generator = Generator::new(seed);
        let mut model = Vec::<(i64, i64)>::new();
        let mut body = String::from("    var values: Map[Int, Int] = [:]\n");
        for _ in 0..32 {
            let key = generator.choose(9) as i64;
            match generator.choose(4) {
                0 | 1 => {
                    let value = generator.choose(100) as i64;
                    if let Some((_, current)) =
                        model.iter_mut().find(|(existing, _)| *existing == key)
                    {
                        *current = value;
                    } else {
                        model.push((key, value));
                    }
                    body.push_str(&format!("    values[{key}] = {value}\n"));
                    body.push_str(&format!("    assert(values[{key}] == some({value}))\n"));
                }
                2 => {
                    let expected = model
                        .iter()
                        .position(|(existing, _)| *existing == key)
                        .map(|position| model.remove(position).1);
                    match expected {
                        Some(value) => body.push_str(&format!(
                            "    assert(values.remove({key}) == some({value}))\n"
                        )),
                        None => {
                            body.push_str(&format!("    assert(values.remove({key}) == none)\n"))
                        }
                    }
                }
                _ => {
                    if let Some((_, value)) = model.iter().find(|(existing, _)| *existing == key) {
                        body.push_str(&format!("    assert(values[{key}] == some({value}))\n"));
                    } else {
                        body.push_str(&format!("    assert(values[{key}] == none)\n"));
                    }
                }
            }
        }
        body.push_str("    var observed = 0\n");
        body.push_str("    for entry in values {\n");
        body.push_str("        observed = observed * 10 + entry.0\n");
        body.push_str("    }\n");
        let expected_order = model.iter().fold(0_i64, |value, (key, _)| value * 10 + key);
        body.push_str(&format!("    assert(observed == {expected_order})\n"));
        let source = format!("fn main() {{\n{body}}}\n");
        let observation = run(
            &format!("map-model-{seed}"),
            &source,
            ResourceLimits::default(),
        )
        .unwrap();
        assert!(
            observation.accepted && observation.exit_code == 0,
            "seed {seed}: {:?}\n{source}",
            observation.diagnostic_codes
        );
    }
}

#[test]
fn array_index_and_slice_helpers_match_a_mathematical_model() {
    let mut generator = Generator::new(0x5eed_f00d);
    for _ in 0..50_000 {
        let length = generator.choose(65);
        let raw = generator.next_u64() as i64;
        let index = i128::from(raw % 160 - 80);
        let expected_index = if index >= 0 {
            usize::try_from(index).ok().filter(|value| *value < length)
        } else {
            i128::try_from(length)
                .ok()
                .and_then(|length| usize::try_from(length + index).ok())
                .filter(|value| *value < length)
        };
        assert_eq!(normalize_array_index(index, length), expected_index);

        let start = i128::from((generator.next_u64() as i64) % 100 - 50);
        let end = i128::from((generator.next_u64() as i64) % 100 - 50);
        let step = i128::from((generator.next_u64() as i64) % 11 - 5);
        let actual = normalize_array_slice_indices(Some(start), Some(end), Some(step), length);
        if step == 0 {
            assert_eq!(actual, Err(ArraySliceError::ZeroStep));
        } else {
            let indices = actual.unwrap();
            assert!(indices.iter().all(|index| *index < length));
            assert!(indices.windows(2).all(|pair| {
                if step > 0 {
                    pair[0] < pair[1]
                } else {
                    pair[0] > pair[1]
                }
            }));
        }
    }
}

#[test]
fn arrays_sets_ranges_strings_slices_and_copies_match_pure_models() {
    for seed in 0..12_u64 {
        let mut generator = Generator::new(0xc011_ec71_0000 + seed);
        let mut values = (0..5)
            .map(|_| i64::try_from(generator.choose(8) + 1).unwrap())
            .collect::<Vec<_>>();
        let mut body = format!("    var values = {}\n", int_array(&values));
        for _ in 0..12 {
            let index = generator.choose(values.len());
            if generator.choose(2) == 0 {
                let replacement = i64::try_from(generator.choose(9) + 1).unwrap();
                values[index] = replacement;
                body.push_str(&format!("    values[{index}] = {replacement}\n"));
            } else {
                let increment = i64::try_from(generator.choose(3) + 1).unwrap();
                values.iter_mut().for_each(|value| *value += increment);
                body.push_str(&format!("    values += {increment}\n"));
            }
            body.push_str(&format!("    assert(values == {})\n", int_array(&values)));
        }

        let copied = values.clone();
        let mut source_after = values.clone();
        source_after[0] += 100;
        let mut copy_after = copied.clone();
        copy_after[1] += 200;
        let mut slice_after = values[1..4].to_vec();
        slice_after[0] += 300;
        body.push_str("    var copied = values\n");
        body.push_str("    var sliced = values[1:4]\n");
        body.push_str("    values[0] += 100\n");
        body.push_str("    copied[1] += 200\n");
        body.push_str("    sliced[0] += 300\n");
        body.push_str(&format!(
            "    assert(values == {})\n    assert(copied == {})\n    assert(sliced == {})\n",
            int_array(&source_after),
            int_array(&copy_after),
            int_array(&slice_after)
        ));

        let set_values = (0..8)
            .map(|_| i64::try_from(generator.choose(8) + 1).unwrap())
            .collect::<Vec<_>>();
        let mut unique = Vec::new();
        for value in &set_values {
            if !unique.contains(value) {
                unique.push(*value);
            }
        }
        let set_code = unique.iter().fold(0_i64, |code, value| code * 10 + value);
        let set_literal = set_values
            .iter()
            .map(|value| format!("dynamic({value})"))
            .collect::<Vec<_>>()
            .join(", ");
        body.push_str(&format!(
            "    let unique = Set[{set_literal}]\n    var setCode = 0\n    for value in unique {{\n        setCode = setCode * 10 + value\n    }}\n    assert(setCode == {set_code})\n"
        ));
        for candidate in 1..=8 {
            body.push_str(&format!(
                "    assert({}({candidate} in unique))\n",
                if unique.contains(&candidate) {
                    ""
                } else {
                    "not "
                }
            ));
        }

        let range_start = i64::try_from(generator.choose(4) + 1).unwrap();
        let range_end = range_start + i64::try_from(generator.choose(5) + 1).unwrap();
        let range_code = (range_start..range_end).fold(0_i64, |code, value| code * 10 + value);
        body.push_str(&format!(
            "    var rangeCode = 0\n    for value in {range_start} .. {range_end} {{\n        rangeCode = rangeCode * 10 + value\n    }}\n    assert(rangeCode == {range_code})\n"
        ));

        let alphabet = ['a', 'ñ', '🙂', '\u{301}'];
        let text = (0..6)
            .map(|_| alphabet[generator.choose(alphabet.len())])
            .collect::<String>();
        let scalars = text.chars().collect::<Vec<_>>();
        let reverse = scalars.iter().rev().collect::<String>();
        let middle = scalars[1..5].iter().collect::<String>();
        body.push_str(&format!(
            "    let text = {}\n    assert(text[0] == {})\n    assert(text[-1] == {})\n    assert(text[1:5] == {})\n    assert(text[::-1] == {})\n",
            string_literal(&text),
            char_literal(scalars[0]),
            char_literal(*scalars.last().unwrap()),
            string_literal(&middle),
            string_literal(&reverse)
        ));

        let source = format!("fn dynamic(value: Int): Int {{ value }}\nfn main() {{\n{body}}}\n");
        let observation = run(
            &format!("collection-model-{seed}"),
            &source,
            ResourceLimits::default(),
        )
        .unwrap();
        assert!(
            observation.accepted && observation.exit_code == 0,
            "seed {seed}: {:?}\n{source}",
            observation.diagnostic_codes
        );
    }
}

fn int_array(values: &[i64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn string_literal(value: &str) -> String {
    format!("\"{value}\"")
}

fn char_literal(value: char) -> String {
    format!("'{value}'")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoanState {
    Available,
    Shared,
    Exclusive,
}

impl LoanState {
    fn transition(self, operation: LoanOperation) -> Option<Self> {
        match (self, operation) {
            (Self::Available, LoanOperation::BorrowShared) => Some(Self::Shared),
            (Self::Available, LoanOperation::BorrowExclusive) => Some(Self::Exclusive),
            (Self::Shared | Self::Exclusive, LoanOperation::Release) => Some(Self::Available),
            (Self::Available, LoanOperation::Write) => Some(Self::Available),
            (Self::Shared, LoanOperation::Read) => Some(Self::Shared),
            (Self::Exclusive, LoanOperation::Read | LoanOperation::Write) => Some(Self::Exclusive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LoanOperation {
    BorrowShared,
    BorrowExclusive,
    Release,
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinState {
    Absent,
    Running,
    Awaited,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StructuredState {
    loan: LoanState,
    join: JoinState,
    deferred: Vec<u8>,
    cleanup: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum StructuredOperation {
    RegisterDefer(u8),
    SpawnShared,
    Await,
    Cancel,
    Read,
    Write,
    ExitScope,
}

impl StructuredState {
    fn new() -> Self {
        Self {
            loan: LoanState::Available,
            join: JoinState::Absent,
            deferred: Vec::new(),
            cleanup: Vec::new(),
        }
    }

    fn apply(&mut self, operation: StructuredOperation) -> bool {
        match operation {
            StructuredOperation::RegisterDefer(id) => {
                self.deferred.push(id);
                true
            }
            StructuredOperation::SpawnShared
                if self.join == JoinState::Absent && self.loan == LoanState::Available =>
            {
                self.join = JoinState::Running;
                self.loan = LoanState::Shared;
                true
            }
            StructuredOperation::Await if self.join == JoinState::Running => {
                self.join = JoinState::Awaited;
                self.loan = LoanState::Available;
                true
            }
            StructuredOperation::Cancel if self.join == JoinState::Running => {
                self.join = JoinState::Cancelled;
                self.loan = LoanState::Available;
                true
            }
            StructuredOperation::Read if self.loan != LoanState::Exclusive => true,
            StructuredOperation::Write if self.loan == LoanState::Available => true,
            StructuredOperation::ExitScope => {
                if self.join == JoinState::Running {
                    self.join = JoinState::Cancelled;
                    self.loan = LoanState::Available;
                }
                self.cleanup.extend(self.deferred.drain(..).rev());
                true
            }
            _ => false,
        }
    }
}

#[test]
fn ownership_model_and_compiler_agree_on_aliasing_conflicts() {
    let valid = [
        LoanOperation::BorrowShared,
        LoanOperation::Read,
        LoanOperation::Release,
        LoanOperation::Write,
    ];
    let invalid = [LoanOperation::BorrowExclusive, LoanOperation::BorrowShared];
    assert!(apply_model(&valid).is_some());
    assert!(apply_model(&invalid).is_none());

    let accepted = check(
        "loan-model-valid",
        "\
fn read(value: ref Int): Int { value }
fn valid() {
    var value = 1
    let observed = read(ref value)
    value = observed + 1
}
",
    )
    .unwrap();
    assert!(accepted.accepted);

    let rejected = check(
        "loan-model-invalid",
        "\
fn pair(left: mut Int, right: ref Int) {}
fn invalid() {
    var value = 1
    pair(mut value, ref value)
}
",
    )
    .unwrap();
    assert!(!rejected.accepted);
    assert!(
        rejected
            .diagnostic_codes
            .iter()
            .any(|code| matches!(code.as_str(), "E1403" | "E1407"))
    );
}

fn apply_model(operations: &[LoanOperation]) -> Option<LoanState> {
    operations
        .iter()
        .copied()
        .try_fold(LoanState::Available, LoanState::transition)
}

#[test]
fn structured_concurrency_model_covers_loans_terminals_cancellation_and_cleanup() {
    for seed in 0..4_096_u64 {
        let mut generator = Generator::new(0x57ac_7000 + seed);
        let mut state = StructuredState::new();
        for step in 0..24_u8 {
            let operation = match generator.choose(7) {
                0 => StructuredOperation::RegisterDefer(step),
                1 => StructuredOperation::SpawnShared,
                2 => StructuredOperation::Await,
                3 => StructuredOperation::Cancel,
                4 => StructuredOperation::Read,
                5 => StructuredOperation::Write,
                _ => StructuredOperation::ExitScope,
            };
            let before = state.clone();
            let cleanup_start = state.cleanup.len();
            if !state.apply(operation) {
                assert_eq!(state, before, "invalid transitions must be atomic");
            } else if matches!(operation, StructuredOperation::ExitScope) {
                assert!(
                    state.cleanup[cleanup_start..]
                        .windows(2)
                        .all(|pair| pair[0] > pair[1])
                );
            }
            if state.join == JoinState::Running {
                assert_eq!(state.loan, LoanState::Shared);
            }
            if matches!(state.join, JoinState::Awaited | JoinState::Cancelled) {
                assert_eq!(state.loan, LoanState::Available);
            }
        }
        state.apply(StructuredOperation::ExitScope);
        assert_ne!(state.join, JoinState::Running);
        assert_eq!(state.loan, LoanState::Available);
    }

    let accepted = check(
        "structured-model-valid",
        "\
fn observe(value: ref Int): Int suspends { value }
fn valid(): Int suspends {
    var value = 1
    scope {
        let pending = spawn observe(ref value)
        let observed = await pending
        value = observed + 1
    }
    value
}
",
    )
    .unwrap();
    assert!(accepted.accepted);

    let rejected = check(
        "structured-model-invalid",
        "\
fn observe(value: ref Int): Int suspends { value }
fn invalid(): Int suspends {
    var value = 1
    scope {
        let pending = spawn observe(ref value)
        value = 2
        await pending
    }
}
",
    )
    .unwrap();
    assert!(!rejected.accepted);
    assert!(
        rejected
            .diagnostic_codes
            .iter()
            .any(|code| matches!(code.as_str(), "E1403" | "E1407"))
    );
}

#[test]
fn collector_scenarios_match_the_root_cycle_and_retry_model() {
    for scenario in [
        MemoryScenario::ReachableRoots,
        MemoryScenario::UnreachableCycles,
        MemoryScenario::SustainedPressure,
        MemoryScenario::RetryBeforeOom,
    ] {
        let observation = run_memory_scenario(scenario).unwrap();
        assert_eq!(observation.scenario, scenario.name());
        assert!(observation.collections > 0);
        assert!(observation.roots_preserved);
        assert!(observation.cycles_reclaimed);
        assert_eq!(
            observation.retry_before_success,
            scenario == MemoryScenario::RetryBeforeOom
        );
        assert_eq!(
            observation.retry_before_oom,
            scenario == MemoryScenario::RetryBeforeOom
        );
    }
}

#[test]
fn runtime_host_and_concurrency_models_have_persistent_public_evidence() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let inventory = inventory::build(&root).unwrap();
    let groups = inventory
        .tests
        .iter()
        .filter(|entry| entry.kind == "conformance-case")
        .map(|entry| entry.group.as_str())
        .collect::<BTreeSet<_>>();
    for group in ["concurrency", "hosted", "memory", "runtime"] {
        assert!(
            groups.contains(group),
            "missing model evidence for `{group}`"
        );
    }
    for required in [
        "conformance:memory/reachable-roots",
        "conformance:memory/retry-before-oom",
        "conformance:concurrency/m7-async-structured",
        "conformance:hosted/m8-process-001",
    ] {
        assert!(
            inventory.tests.iter().any(|entry| entry.id == required),
            "missing `{required}`"
        );
    }
}
