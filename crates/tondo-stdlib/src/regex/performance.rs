//! Target-qualified direct Rust kernel probe; oracle and resources precede timing.
#[path = "../../../tondo-reliability/src/regex_model.rs"]
#[allow(dead_code)]
mod reference;

use super::*;
use reference::{ReferenceOptions, ReferenceRegex, ReferenceSpan};
use serde_json::json;
use std::{hint::black_box, mem::size_of, time::Instant};

const BATCH: usize = 16;
const WARMUPS: usize = 3;
const REPETITIONS: usize = 9;
const WORKLOADS: usize = 19;

#[derive(Clone, Copy, Debug)]
enum Operation {
    Compile,
    Find,
    Full,
    Captures,
    All,
    Replace,
    ReplaceAll,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Find => "find",
            Self::Full => "full-match",
            Self::Captures => "captures",
            Self::All => "lazy-enumeration",
            Self::Replace => "replace-first",
            Self::ReplaceAll => "replace-all",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Expected {
    Program { captures: usize, names: Vec<String> },
    Match(Option<Vec<Option<RegexSpan>>>),
    Full(bool),
    All(Vec<Vec<Option<RegexSpan>>>, Option<RegexError>),
    Text(String),
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "The probe keeps the returned program unboxed to avoid an artificial timed allocation."
)]
enum Outcome {
    Program(Regex),
    Match(Option<RegexMatch>),
    Full(bool),
    All(Vec<RegexMatch>, Option<RegexError>),
    Text(String),
}

struct Workload {
    id: &'static str,
    operation: Operation,
    pattern: String,
    input: String,
    replacement: String,
    options: RegexOptions,
    limits: RegexLimits,
    program: Option<Regex>,
    expected: Result<Expected, RegexError>,
    oracle: &'static str,
}

fn spans(found: &RegexMatch) -> Vec<Option<RegexSpan>> {
    (0..found.names.len())
        .map(|index| found.capture(index))
        .collect()
}

fn from_reference(found: reference::ReferenceMatch) -> Vec<Option<RegexSpan>> {
    found
        .captures
        .into_iter()
        .map(|value| {
            value.map(|span| RegexSpan {
                start: span.start,
                end: span.end,
            })
        })
        .collect()
}

impl Workload {
    fn modeled(
        id: &'static str,
        operation: Operation,
        pattern: &str,
        input: &str,
        replacement: &str,
    ) -> Self {
        let reference = ReferenceRegex::compile(pattern, ReferenceOptions::default()).expect(id);
        let expected = match operation {
            Operation::Compile => Expected::Program {
                captures: reference.capture_count(),
                names: reference
                    .capture_names()
                    .iter()
                    .flatten()
                    .cloned()
                    .collect(),
            },
            Operation::Find | Operation::Captures => {
                Expected::Match(reference.find(input).expect(id).map(from_reference))
            }
            Operation::Full => Expected::Full(reference.full_match(input).expect(id).is_some()),
            Operation::All => Expected::All(
                reference
                    .find_all(input)
                    .expect(id)
                    .into_iter()
                    .map(from_reference)
                    .collect(),
                None,
            ),
            Operation::Replace => {
                Expected::Text(reference.replace(input, replacement, false).expect(id))
            }
            Operation::ReplaceAll => {
                Expected::Text(reference.replace(input, replacement, true).expect(id))
            }
        };
        Self::authored(
            id,
            operation,
            pattern,
            input.to_owned(),
            replacement,
            Ok(expected),
            RegexLimits::default(),
            "independent-bounded-model",
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Each fixture states its input, operation, exact oracle and budget together."
    )]
    fn authored(
        id: &'static str,
        operation: Operation,
        pattern: &str,
        input: String,
        replacement: &str,
        expected: Result<Expected, RegexError>,
        limits: RegexLimits,
        oracle: &'static str,
    ) -> Self {
        let options = RegexOptions::default();
        let program = if matches!(operation, Operation::Compile) {
            None
        } else {
            Some(Regex::compile(pattern, options, limits).expect(id))
        };
        Self {
            id,
            operation,
            pattern: pattern.to_owned(),
            input,
            replacement: replacement.to_owned(),
            options,
            limits,
            program,
            expected,
            oracle,
        }
    }

    fn run(&self) -> Result<Outcome, RegexError> {
        if matches!(self.operation, Operation::Compile) {
            return Regex::compile(&self.pattern, self.options, self.limits).map(Outcome::Program);
        }
        let regex = self.program.as_ref().unwrap();
        match self.operation {
            Operation::Compile => unreachable!(),
            Operation::Find | Operation::Captures => regex.find(&self.input).map(Outcome::Match),
            Operation::Full => regex.is_full_match(&self.input).map(Outcome::Full),
            Operation::All => {
                let mut cursor = regex.find_all(&self.input)?;
                let mut found = Vec::new();
                let mut error = None;
                for item in cursor.by_ref() {
                    match item {
                        Ok(item) => found.push(item),
                        Err(problem) => {
                            error = Some(problem);
                            break;
                        }
                    }
                }
                assert!(
                    cursor.next().is_none() && cursor.next().is_none(),
                    "terminal cursor"
                );
                Ok(Outcome::All(found, error))
            }
            Operation::Replace => regex
                .replace(&self.input, &self.replacement)
                .map(Outcome::Text),
            Operation::ReplaceAll => regex
                .replace_all(&self.input, &self.replacement)
                .map(Outcome::Text),
        }
    }

    fn verify(&self) {
        let observed = self.run().map(|result| match result {
            Outcome::Program(regex) => Expected::Program {
                captures: regex.capture_count(),
                names: regex.capture_names().map(str::to_owned).collect(),
            },
            Outcome::Match(found) => Expected::Match(found.as_ref().map(spans)),
            Outcome::Full(full) => Expected::Full(full),
            Outcome::All(found, error) => Expected::All(found.iter().map(spans).collect(), error),
            Outcome::Text(text) => Expected::Text(text),
        });
        assert_eq!(observed, self.expected, "{} exact oracle", self.id);
    }

    fn rejecting(&self) -> bool {
        self.expected.is_err() || matches!(&self.expected, Ok(Expected::All(_, Some(_))))
    }

    fn matching_steps(&self) -> Option<usize> {
        let regex = self.program.as_ref()?;
        if matches!(
            self.operation,
            Operation::Compile | Operation::Replace | Operation::ReplaceAll
        ) {
            return None;
        }
        if regex.check_input(&self.input, RegexPhase::Match).is_err() {
            return Some(0);
        }
        if matches!(self.operation, Operation::All) {
            let mut cursor = regex.find_all(&self.input).unwrap();
            while cursor.next().is_some() {}
            assert!(cursor.next().is_none());
            return Some(cursor.steps);
        }
        let mut steps = 0;
        let mode = if matches!(self.operation, Operation::Full) {
            SearchMode::Full
        } else {
            SearchMode::At(0)
        };
        let result = regex.search(&self.input, mode, &mut steps, RegexPhase::Match, 0);
        assert_eq!(
            result.is_err(),
            self.expected.is_err(),
            "same search status"
        );
        Some(steps)
    }

    fn counters(&self) -> serde_json::Value {
        // Compilation's resulting program is counted, but oracle construction
        // and dependency parser worklists are explicitly outside this model.
        let compiled = if matches!(self.operation, Operation::Compile) {
            Regex::compile(&self.pattern, self.options, self.limits).ok()
        } else {
            None
        };
        let regex = self.program.as_ref().or(compiled.as_ref());
        let mut program_bytes = 0;
        let mut program_allocations = 0;
        let mut shape = [0; 4];
        if let Some(regex) = regex {
            shape = regex.program.performance_shape();
            program_bytes = size_of::<Regex>()
                + regex.program.logical_storage()
                + Regex::metadata_storage(&regex.pattern, &regex.names);
            // Pattern, capture-name table, three program tables, nonempty
            // range vectors (including word boundaries), and named strings.
            program_allocations = 5 + shape[3] + regex.capture_names().count();
        }
        let (output, matches, result_storage, result_allocations) = match &self.expected {
            Ok(Expected::Match(Some(values))) => (
                0,
                1,
                size_of::<RegexMatch>() + values.len() * size_of::<Option<RegexSpan>>(),
                1,
            ),
            Ok(Expected::All(values, _)) => (
                0,
                values.len(),
                size_of::<Vec<RegexMatch>>()
                    + values
                        .iter()
                        .map(|row| {
                            size_of::<RegexMatch>() + row.len() * size_of::<Option<RegexSpan>>()
                        })
                        .sum::<usize>(),
                usize::from(!values.is_empty()) + values.len(),
            ),
            Ok(Expected::Text(text)) => (
                text.len(),
                0,
                size_of::<String>() + text.len(),
                usize::from(!text.is_empty()),
            ),
            _ => (0, 0, 0, 0),
        };
        let (expected_storage, expected_allocations) = match &self.expected {
            Ok(Expected::Program { names, .. }) => (
                size_of::<Vec<String>>()
                    + names.len() * size_of::<String>()
                    + names.iter().map(String::len).sum::<usize>(),
                usize::from(!names.is_empty())
                    + names.iter().filter(|name| !name.is_empty()).count(),
            ),
            Ok(Expected::Match(Some(values))) => (
                size_of::<Vec<Option<RegexSpan>>>() + values.len() * size_of::<Option<RegexSpan>>(),
                usize::from(!values.is_empty()),
            ),
            Ok(Expected::All(values, _)) => (
                size_of::<Vec<Vec<Option<RegexSpan>>>>()
                    + values
                        .iter()
                        .map(|row| {
                            size_of::<Vec<Option<RegexSpan>>>()
                                + row.len() * size_of::<Option<RegexSpan>>()
                        })
                        .sum::<usize>(),
                usize::from(!values.is_empty())
                    + values.iter().filter(|row| !row.is_empty()).count(),
            ),
            Ok(Expected::Text(text)) => (
                size_of::<String>() + text.len(),
                usize::from(!text.is_empty()),
            ),
            _ => (0, 0),
        };
        let retained = self.pattern.len()
            + self.input.len()
            + self.replacement.len()
            + 3 * size_of::<String>()
            + expected_storage;
        let retained_allocations = [&self.pattern, &self.input, &self.replacement]
            .iter()
            .filter(|value| !value.is_empty())
            .count()
            + expected_allocations;
        let compile = matches!(self.operation, Operation::Compile);
        let replacement = matches!(self.operation, Operation::Replace | Operation::ReplaceAll);
        // This is the engine's conservative admission reservation, not an
        // observed allocator peak. One operation coexists with its fixture.
        let scratch = if compile || self.id == "reject-input" {
            0
        } else {
            regex.map_or(0, |regex| {
                regex.program.performance_search_storage() - regex.program.logical_storage()
            })
        };
        let transported = if self.rejecting() {
            0
        } else if compile {
            self.pattern.len()
        } else if replacement {
            output
        } else {
            0
        };
        let operation_allocations = if compile {
            program_allocations
        } else {
            result_allocations
        };
        json!({
            "pattern_bytes": self.pattern.len(),
            "input_bytes": if compile { self.pattern.len() } else { self.input.len() },
            "output_bytes": output,
            "operations": BATCH,
            "bytes_copied": transported * BATCH,
            "allocations": retained_allocations + if compile { 0 } else { program_allocations }
                + operation_allocations * BATCH,
            "logical_memory_bytes": retained + program_bytes + scratch + result_storage,
            "program_states": shape[0], "semantic_states": shape[1], "class_ranges": shape[2],
            "capture_slots": regex.map_or(0, |regex| 2 * (regex.capture_count() + 1)),
            "matching_steps": self.matching_steps(),
            "matches": matches,
            "adversarial_rejections": if self.rejecting() { BATCH } else { 0 },
            "terminal_open_cursors": 0,
            "native_live_handles": null,
        })
    }
}

fn limited(
    kind: RegexErrorKind,
    phase: RegexPhase,
    offset: usize,
    limit: &'static str,
) -> RegexError {
    RegexError::limited(kind, phase, offset, limit)
}

fn fixtures() -> Vec<Workload> {
    let bounded = "(?<word>[a-z]{1,8})(?<n>[0-9]{1,4})?";
    let mut result = vec![
        Workload::modeled("compile-literal", Operation::Compile, "tondo", "", ""),
        Workload::modeled("compile-captures", Operation::Compile, bounded, "", ""),
        Workload::authored(
            "compile-unicode16",
            Operation::Compile,
            r"\p{Script=Tulu_Tigalari}+",
            String::new(),
            "",
            Ok(Expected::Program {
                captures: 0,
                names: vec![],
            }),
            RegexLimits::default(),
            "authored-unicode16",
        ),
        Workload::modeled("find-priority", Operation::Find, "a|ab", "xxab", ""),
        Workload::modeled("full-captures", Operation::Full, bounded, "tondo42", ""),
        Workload::modeled(
            "captures-unicode",
            Operation::Captures,
            "(?<word>é{1,4})(x)?",
            "éé!",
            "",
        ),
        Workload::authored(
            "find-ambiguous-medium",
            Operation::Find,
            "(?:a|aa)*b",
            "a".repeat(64),
            "",
            Ok(Expected::Match(None)),
            RegexLimits::default(),
            "authored-no-terminal-b",
        ),
        Workload::authored(
            "find-ambiguous-large",
            Operation::Find,
            "(?:a|aa)*b",
            "a".repeat(1024),
            "",
            Ok(Expected::Match(None)),
            RegexLimits::default(),
            "authored-no-terminal-b",
        ),
        Workload::modeled(
            "enumerate-dense",
            Operation::All,
            "(?<part>a|bc)",
            "abcabcabcabc",
            "",
        ),
        Workload::modeled("enumerate-empty-unicode", Operation::All, "é*?", "é🙂x", ""),
        Workload::authored(
            "enumerate-rescan",
            Operation::All,
            "a.*z|a",
            "a".repeat(128),
            "",
            Ok(Expected::All(
                (0..128)
                    .map(|at| {
                        vec![Some(RegexSpan {
                            start: at,
                            end: at + 1,
                        })]
                    })
                    .collect(),
                None,
            )),
            RegexLimits::default(),
            "authored-no-z-local-fallback",
        ),
        Workload::modeled(
            "replace-named-first",
            Operation::Replace,
            "(?<word>[a-z]{1,4})(?<n>[0-9]{1,2})",
            "abc12 abc34",
            "${n}:${word}:$$",
        ),
        Workload::modeled(
            "replace-named-all",
            Operation::ReplaceAll,
            "(?<word>[a-z]{1,4})(?<n>[0-9]{1,2})",
            "abc12 abc34",
            "${n}:${word}:$$",
        ),
        Workload::authored(
            "reject-syntax",
            Operation::Compile,
            r"(a)\1",
            String::new(),
            "",
            Err(RegexError::at(
                RegexErrorKind::UnsupportedFeature,
                RegexPhase::Compile,
                3,
                5,
            )),
            RegexLimits::default(),
            "authored-closed-dialect",
        ),
        Workload::authored(
            "reject-program",
            Operation::Compile,
            "abc",
            String::new(),
            "",
            Err(limited(
                RegexErrorKind::ProgramLimitExceeded,
                RegexPhase::Compile,
                0,
                "max_program_states",
            )),
            RegexLimits {
                max_program_states: 2,
                ..RegexLimits::default()
            },
            "authored-admission-limit",
        ),
        Workload::authored(
            "reject-input",
            Operation::Find,
            "x",
            "éx".to_owned(),
            "",
            Err(limited(
                RegexErrorKind::InputLimitExceeded,
                RegexPhase::Match,
                0,
                "max_input_bytes",
            )),
            RegexLimits {
                max_input_bytes: 2,
                ..RegexLimits::default()
            },
            "authored-admission-limit",
        ),
        Workload::authored(
            "reject-steps",
            Operation::Find,
            "a",
            "a".to_owned(),
            "",
            Err(limited(
                RegexErrorKind::StepLimitExceeded,
                RegexPhase::Match,
                0,
                "max_steps",
            )),
            RegexLimits {
                max_steps: 1,
                ..RegexLimits::default()
            },
            "authored-admission-limit",
        ),
        Workload::authored(
            "reject-matches",
            Operation::All,
            "a",
            "aa".to_owned(),
            "",
            Ok(Expected::All(
                vec![vec![Some(RegexSpan { start: 0, end: 1 })]],
                Some(limited(
                    RegexErrorKind::MatchLimitExceeded,
                    RegexPhase::Match,
                    1,
                    "max_matches",
                )),
            )),
            RegexLimits {
                max_matches: 1,
                ..RegexLimits::default()
            },
            "authored-fallible-prefix",
        ),
        Workload::authored(
            "reject-output",
            Operation::ReplaceAll,
            "a",
            "a".to_owned(),
            "aaaa",
            Err(limited(
                RegexErrorKind::OutputLimitExceeded,
                RegexPhase::Replace,
                0,
                "max_output_bytes",
            )),
            RegexLimits {
                max_output_bytes: 2,
                ..RegexLimits::default()
            },
            "authored-atomic-output",
        ),
    ];
    assert_eq!(result.len(), WORKLOADS);
    result.shrink_to_fit();
    result
}

fn measure_process(process: usize) -> Vec<serde_json::Value> {
    assert!((1..=3).contains(&process), "explicit process ordinal");
    let mut rows = Vec::new();
    for workload in fixtures() {
        workload.verify();
        let counters = workload.counters();
        for _ in 0..WARMUPS {
            for _ in 0..BATCH {
                workload.verify();
            }
        }
        for repetition in 0..REPETITIONS {
            let start = Instant::now();
            for _ in 0..BATCH {
                let result = workload.run();
                let rejecting = result.is_err() || matches!(&result, Ok(Outcome::All(_, Some(_))));
                assert_eq!(rejecting, workload.rejecting(), "{} status", workload.id);
                drop(black_box(result));
            }
            let nanos = start.elapsed().as_nanos();
            assert!(nanos > 0);
            workload.verify();
            rows.push(
                json!({ "workload_id": workload.id, "operation": workload.operation.label(),
                "process": process, "repetition": repetition, "nanos": nanos,
                "dispatch": "scalar-fixed-target", "counters": counters }),
            );
        }
    }
    rows
}

#[test]
fn regex_performance_oracles_are_exact_and_independent() {
    for workload in fixtures() {
        workload.verify();
        assert!(workload.pattern.len() <= 128 && workload.input.len() <= 4096);
        assert!(workload.replacement.len() <= 128);
        assert!(!workload.oracle.is_empty());
    }
    // Larger adversarial fixtures have authored algebraic expectations. Their
    // bounded representatives additionally run the independent path oracle.
    let no_b = ReferenceRegex::compile("(?:a|aa)*b", ReferenceOptions::default()).unwrap();
    assert!(no_b.find("aaaaaaaa").unwrap().is_none());
    let rescan = ReferenceRegex::compile("a.*z|a", ReferenceOptions::default()).unwrap();
    let input = "a".repeat(16);
    assert_eq!(
        rescan
            .find_all(&input)
            .unwrap()
            .iter()
            .map(|item| item.span)
            .collect::<Vec<_>>(),
        (0..16)
            .map(|at| ReferenceSpan {
                start: at,
                end: at + 1
            })
            .collect::<Vec<_>>()
    );
    let unicode = Regex::compile(
        r"\p{Script=Tulu_Tigalari}+",
        RegexOptions::default(),
        RegexLimits::default(),
    )
    .unwrap();
    assert!(unicode.is_full_match("\u{11380}\u{11380}").unwrap());
    assert!(!unicode.is_match("abc").unwrap());
}

#[test]
fn regex_performance_resources_and_cursor_lifecycle_are_explicit() {
    let workloads = fixtures();
    let medium = workloads
        .iter()
        .find(|item| item.id == "find-ambiguous-medium")
        .unwrap();
    let large = workloads
        .iter()
        .find(|item| item.id == "find-ambiguous-large")
        .unwrap();
    let medium_steps = medium.matching_steps().unwrap();
    let large_steps = large.matching_steps().unwrap();
    assert!(medium_steps > 64 && large_steps > medium_steps);
    assert!(
        large_steps <= medium_steps * 17,
        "fixed-program input scaling"
    );
    let rescan = workloads
        .iter()
        .find(|item| item.id == "enumerate-rescan")
        .unwrap();
    assert!(
        rescan.matching_steps().unwrap() > 128 * 128,
        "enumeration rescans suffixes"
    );
    for workload in &workloads {
        let counters = workload.counters();
        assert!(counters["native_live_handles"].is_null());
        assert_eq!(counters["terminal_open_cursors"], 0);
        if workload.rejecting() {
            assert_eq!(counters["adversarial_rejections"], BATCH);
            assert_eq!(counters["output_bytes"], 0);
            assert_eq!(counters["bytes_copied"], 0);
        }
        if let Some(regex) = workload.program.as_ref() {
            assert_eq!(counters["program_states"], regex.program_states());
        }
    }
    let limited = workloads
        .iter()
        .find(|item| item.id == "reject-matches")
        .unwrap();
    assert_eq!(limited.counters()["matches"], 1);
    limited.verify();
    // Admission reservation is tied to the actual engine, not an RSS claim.
    let ordinary = Regex::compile("a", RegexOptions::default(), RegexLimits::default()).unwrap();
    let minimum = ordinary.program.performance_search_storage()
        + Regex::metadata_storage(&ordinary.pattern, &ordinary.names);
    let mut options = ordinary.limits;
    options.vm_heap = minimum - 1;
    let below = Regex::compile("a", RegexOptions::default(), options).unwrap();
    assert_eq!(
        below.find("a").unwrap_err().kind,
        RegexErrorKind::OutOfMemory
    );
    options.vm_heap = minimum;
    assert!(
        Regex::compile("a", RegexOptions::default(), options)
            .unwrap()
            .find("a")
            .unwrap()
            .is_some()
    );
}

#[test]
fn regex_performance_batches_preserve_oracles_and_ordinals() {
    let rows = measure_process(1);
    assert_eq!(rows.len(), WORKLOADS * REPETITIONS);
    for workload in fixtures() {
        let selected: Vec<_> = rows
            .iter()
            .filter(|row| row["workload_id"] == workload.id)
            .collect();
        assert_eq!(selected.len(), REPETITIONS);
        for (repetition, row) in selected.into_iter().enumerate() {
            assert_eq!(row["process"], 1);
            assert_eq!(row["repetition"], repetition);
            assert_eq!(row["counters"], workload.counters());
            assert!(row["nanos"].as_u64().unwrap() > 0);
        }
    }
}

#[test]
fn regex_performance_probe() {
    if std::env::var("TONDO_REGEX_PERF_RUN").as_deref() != Ok("1") {
        return;
    }
    let process = std::env::var("TONDO_REGEX_PERF_PROCESS")
        .expect("explicit process ordinal")
        .parse::<usize>()
        .expect("integer process ordinal");
    for row in measure_process(process) {
        eprintln!("TONDO_REGEX_PERF\t{row}");
    }
}
