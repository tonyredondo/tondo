//! Actual public Tondo UUID calls compiled and executed in the hosted VM.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tondo_compiler::driver::{
    BuildTarget, CapabilityName, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition,
    HostProfile, Operation as DriverOperation, ResourceLimits, SourceForm, discover_tests, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::test_control::{
    EnvelopeHandle, EnvelopeLimits, ExecutionPhase, UuidTestProviders,
};
use tondo_reliability::uuid_model::{
    ReferenceError, ReferenceErrorKind, ReferenceProviders, ReferenceUuid,
};
use tondo_stdlib::uuid::UuidErrorKind;

#[path = "../../tondo-stdlib/examples/support/uuid_conformance_cases.rs"]
mod cases;
use cases::{Operation, Providers};

const PRELUDE: &str = r#"import std.uuid
import std.bytes
import std.testing
fn ownedBytes(value: Array[Byte]): bytes.Bytes {
    match bytes.fromArray(value) {
        ok(item) => item
        err(_) => testing.failNow("byte fixture refused")
    }
}
fn requireUuid(value: uuid.Uuid ! uuid.UuidError): uuid.Uuid {
    match value {
        ok(item) => item
        err(_) => testing.failNow("UUID fixture refused")
    }
}
fn copy[T: Copy + Discard + Equatable + Key + Send + Share](value: T): T { value }
fn describe(value: uuid.Uuid ! uuid.UuidError): String {
    match value {
        ok(item) => {
            let array = match item.toBytes().toArray() {
                ok(raw) => raw
                err(_) => testing.failNow("byte result refused")
            }
            var observation = "ok:{item.toString()}"
            for byte in array {
                observation = "{observation}:{byte}"
            }
            let variant = match item.variant() {
                uuid.UuidVariant.Rfc9562 => "Rfc9562"
                uuid.UuidVariant.Ncs => "Ncs"
                uuid.UuidVariant.Microsoft => "Microsoft"
                uuid.UuidVariant.Future => "Future"
            }
            "{observation}:{item.version()}:{variant}:{item.isNil()}:{item.isMax()}"
        }
        err(failure) => "err:{Display.display(failure)}"
    }
}
"#;

fn literal(value: &str) -> String {
    let mut text = String::from("\"");
    for scalar in value.chars() {
        match scalar {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            '{' => text.push_str("{{"),
            '}' => text.push_str("}}"),
            value if value.is_ascii_control() => {
                text.push_str(&format!("\\u{{{:x}}}", value as u32))
            }
            value => text.push(value),
        }
    }
    text.push('"');
    text
}

fn byte_input(bytes: &[u8]) -> String {
    format!(
        "ownedBytes([{}])",
        bytes
            .iter()
            .map(|value| format!("Byte({value}u8)"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn expression(operation: &Operation) -> String {
    match operation {
        Operation::Parse(text) => format!("uuid.Uuid.parse({})", literal(text)),
        Operation::Bytes(bytes) => format!("uuid.Uuid.fromBytes({})", byte_input(bytes)),
        Operation::V4(_) => "uuid.Uuid.v4()".into(),
        Operation::V5 {
            namespace, name, ..
        } => format!(
            "uuid.Uuid.v5(requireUuid(uuid.Uuid.parse({})), {})",
            literal(namespace),
            byte_input(name)
        ),
        Operation::V7 { .. } => "uuid.Uuid.v7()".into(),
    }
}

fn clock(milliseconds: i128) -> SystemTime {
    let elapsed = Duration::from_millis(milliseconds.unsigned_abs().try_into().unwrap());
    if milliseconds < 0 {
        UNIX_EPOCH - elapsed
    } else {
        UNIX_EPOCH + elapsed
    }
}

fn request(
    source: &str,
    operation: DriverOperation,
    capabilities: BTreeSet<CapabilityName>,
) -> CompilationRequest {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:uuid-conformance").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("uuid-conformance.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    CompilationRequest::new(
        operation,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        capabilities,
        DiagnosticFormat::Json,
        SourceForm::Module,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).unwrap(),
        sources,
        root,
    )
    .unwrap()
}

fn run(body: &str, providers: Providers, capabilities: BTreeSet<CapabilityName>) -> Vec<String> {
    let envelope = envelope(providers);
    run_with_envelope(body, &envelope, capabilities)
}

fn envelope(providers: Providers) -> EnvelopeHandle {
    let envelope = EnvelopeHandle::new(
        "uuid-conformance",
        EnvelopeLimits::new(1_048_576, 1_048_576, 1_048_576),
    );
    envelope
        .with_uuid_providers(
            UuidTestProviders::new(
                providers
                    .clocks
                    .into_iter()
                    .map(|row| row.map(clock))
                    .collect(),
                providers.entropy,
            )
            .unwrap(),
        )
        .unwrap();
    envelope.set_phase(ExecutionPhase::Body).unwrap();
    envelope
}

fn run_with_envelope(
    body: &str,
    envelope: &EnvelopeHandle,
    capabilities: BTreeSet<CapabilityName>,
) -> Vec<String> {
    let source = format!("{PRELUDE}\ntest conformance {{\n{body}\n}}\n");
    let base = request(&source, DriverOperation::Test, capabilities);
    let entries = discover_tests(&base).unwrap();
    if entries.len() != 1 {
        let output = execute(base).unwrap();
        panic!(
            "expected one executable fixture; {}",
            output.diagnostics().human()
        );
    }
    let output = execute(
        base.for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone()),
    )
    .unwrap();
    assert_eq!(
        output.status(),
        CompilationStatus::Success,
        "{}",
        output.diagnostics().human()
    );
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    assert!(envelope.report().unwrap().terminal().is_none());
    let observed = envelope
        .report()
        .unwrap()
        .logs()
        .iter()
        .map(|row| row.message().to_owned())
        .collect();
    envelope.close().unwrap();
    assert_eq!(envelope.report().unwrap().phase(), ExecutionPhase::Closed);
    assert!(
        envelope
            .with_uuid_providers(UuidTestProviders::new(vec![], vec![]).unwrap())
            .is_err()
    );
    observed
}

fn reference(operation: &Operation) -> Result<ReferenceUuid, ReferenceError> {
    match operation {
        Operation::Parse(text) => ReferenceUuid::parse(text),
        Operation::Bytes(bytes) => ReferenceUuid::from_bytes(bytes),
        Operation::V4(entropy) => ReferenceUuid::v4(entropy),
        Operation::V5 {
            namespace,
            name,
            limit,
        } => ReferenceUuid::parse(namespace).unwrap().v5(name, *limit),
        Operation::V7 {
            milliseconds,
            entropy,
        } => ReferenceUuid::v7(*milliseconds, entropy),
    }
}

fn reference_observation(value: Result<ReferenceUuid, ReferenceError>) -> String {
    match value {
        Err(error) => format!("err:{error}"),
        Ok(value) => {
            let mut text = format!("ok:{}", value.canonical());
            for byte in value.to_bytes() {
                text.push_str(&format!(":{byte}"));
            }
            text.push_str(&format!(
                ":{}:{}:{}:{}",
                value.version(),
                value.variant(),
                value == ReferenceUuid::nil(),
                value == ReferenceUuid::max()
            ));
            text
        }
    }
}

fn provider_kind(kind: UuidErrorKind) -> ReferenceErrorKind {
    match kind {
        UuidErrorKind::EntropyUnavailable => ReferenceErrorKind::EntropyUnavailable,
        UuidErrorKind::EntropyFailure => ReferenceErrorKind::EntropyFailure,
        UuidErrorKind::ClockUnavailable => ReferenceErrorKind::ClockUnavailable,
        UuidErrorKind::ClockFailure => ReferenceErrorKind::ClockFailure,
        unknown => panic!("unregistered provider failure {unknown:?}"),
    }
}

fn reference_providers(providers: &Providers) -> ReferenceProviders {
    ReferenceProviders::new(
        providers
            .clocks
            .iter()
            .map(|row| row.map_err(provider_kind))
            .collect(),
        providers
            .entropy
            .iter()
            .map(|row| row.clone().map_err(provider_kind))
            .collect(),
    )
    .unwrap()
}

fn generator_capabilities(version: u8) -> BTreeSet<CapabilityName> {
    let names: &[&str] = match version {
        4 => &["entropy"],
        7 => &["civil-clock", "entropy"],
        unknown => panic!("unregistered fixture version {unknown}"),
    };
    names
        .iter()
        .map(|name| CapabilityName::new(*name).unwrap())
        .collect()
}

fn capability_observations() -> Vec<serde_json::Value> {
    let mut observations = Vec::new();
    for version in [4, 7] {
        let required = generator_capabilities(version);
        for (form, body) in [
            ("direct", format!("_ = uuid.Uuid.v{version}()")),
            (
                "alias",
                format!("let generate = uuid.Uuid.v{version}\n_ = generate()"),
            ),
            ("defer", format!("defer {{\n_ = uuid.Uuid.v{version}()\n}}")),
        ] {
            for omitted in required.iter().map(Some).chain(std::iter::once(None)) {
                let capabilities = required
                    .iter()
                    .filter(|name| Some(*name) != omitted)
                    .cloned()
                    .collect();
                let source = format!("import std.uuid\nfn main() {{\n{body}\n}}\n");
                let entropy = cases::hex(if version == 4 {
                    "919108f752d133205bacf847db4148a8"
                } else {
                    "0cc318c4dc0c0c07398f"
                });
                let envelope = envelope(Providers {
                    clocks: vec![Ok(1_645_557_742_000)],
                    entropy: vec![Ok(entropy)],
                });
                let output = execute(
                    request(&source, DriverOperation::Check, capabilities)
                        .with_test_envelope(envelope.clone()),
                )
                .unwrap();
                let result = if let Some(omitted) = omitted {
                    assert_eq!(output.status(), CompilationStatus::Rejected);
                    assert!(
                        output
                            .diagnostics()
                            .diagnostics()
                            .iter()
                            .any(|diagnostic| diagnostic.code() == "E1008"
                                && diagnostic.message().contains(omitted.as_str()))
                    );
                    "E1008"
                } else {
                    assert_eq!(
                        output.status(),
                        CompilationStatus::Success,
                        "{}",
                        output.diagnostics().human()
                    );
                    "accepted"
                };
                // Replay the same fixture after the static check: the first
                // supplied value must still be available, including on refusal.
                let replay = run_with_envelope(
                    &format!("testing.log(requireUuid(uuid.Uuid.v{version}()).toString())"),
                    &envelope,
                    required.clone(),
                );
                let expected = if version == 4 {
                    "919108f7-52d1-4320-9bac-f847db4148a8"
                } else {
                    "017f22e2-79b0-7cc3-98c4-dc0c0c07398f"
                };
                assert_eq!(replay, vec![expected]);
                observations.push(serde_json::json!({
                    "version": version, "form": form,
                    "missing": omitted.map(CapabilityName::as_str), "result": result,
                    "sealed_replay": replay[0], "envelope_closed": true,
                }));
            }
        }
    }
    assert_eq!(observations.len(), 15);
    observations
}

fn run_case(id: &str) -> Vec<String> {
    match id {
        "retained-values" | "retained-refusals" => {
            let valid = id == "retained-values";
            let mut observed = Vec::new();
            for row in cases::common_vectors(valid) {
                let expected = reference(&row.operation);
                assert_eq!(
                    match expected {
                        Ok(value) => value.canonical(),
                        Err(error) => error.to_string(),
                    },
                    row.expected,
                    "{}",
                    row.id
                );
                let expected = format!("{}|{}", row.id, reference_observation(expected));
                let providers = match &row.operation {
                    Operation::V4(entropy) => Providers {
                        clocks: vec![],
                        entropy: vec![Ok(entropy.clone())],
                    },
                    Operation::V7 {
                        milliseconds,
                        entropy,
                    } => Providers {
                        clocks: vec![Ok(*milliseconds)],
                        entropy: vec![Ok(entropy.clone())],
                    },
                    _ => Providers {
                        clocks: vec![],
                        entropy: vec![],
                    },
                };
                let capabilities = match row.operation {
                    Operation::V4(_) => BTreeSet::from([CapabilityName::new("entropy").unwrap()]),
                    Operation::V7 { .. } => BTreeSet::from([
                        CapabilityName::new("entropy").unwrap(),
                        CapabilityName::new("civil-clock").unwrap(),
                    ]),
                    _ => BTreeSet::new(),
                };
                let body = format!(
                    "testing.log(\"{}|{{describe({})}}\")",
                    row.id,
                    expression(&row.operation)
                );
                let result = run(&body, providers, capabilities);
                assert_eq!(result, vec![expected], "{}", row.id);
                observed.extend(result);
            }
            observed
        }
        "core-value-laws" => run(
            r#"
            let nil = uuid.Uuid.nil()
            let maximum = uuid.Uuid.max()
            let keys: Map[uuid.Uuid, Int] = [nil: 0, maximum: 1]
            let duplicate = copy(maximum)
            let roundtrip = requireUuid(uuid.Uuid.fromBytes(duplicate.toBytes()))
            if duplicate != roundtrip {
                testing.failNow("byte roundtrip differs")
            }
            let zero = match keys[nil] {
                some(value) => value
                none => testing.failNow("nil key absent")
            }
            let one = match keys[roundtrip] {
                some(value) => value
                none => testing.failNow("max key absent")
            }
            testing.log(describe(ok(nil)))
            testing.log(describe(ok(roundtrip)))
            testing.log("laws:{zero}:{one}:{nil.compare(maximum)}:{maximum.compare(nil)}:{maximum.compare(duplicate)}:{nil != maximum}")
        "#,
            Providers {
                clocks: vec![],
                entropy: vec![],
            },
            BTreeSet::new(),
        ),
        "provider-transcript" => {
            let providers = cases::transcript_providers();
            let mut model = reference_providers(&providers);
            let expected: Vec<_> = cases::TRANSCRIPT
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    format!(
                        "{index}|{}",
                        reference_observation(if *version == 4 {
                            model.v4(16)
                        } else {
                            model.v7(16)
                        })
                    )
                })
                .collect();
            assert_eq!(model.consumed(), (8, 9));
            let body = cases::TRANSCRIPT
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    format!("testing.log(\"{index}|{{describe(uuid.Uuid.v{version}())}}\")\n")
                })
                .collect::<String>();
            let observed = run(&body, providers, generator_capabilities(7));
            assert_eq!(observed, expected);
            observed
        }
        "clock-laws" => {
            let providers = cases::clock_providers();
            let mut model = reference_providers(&providers);
            let values: Vec<_> = (0..5).map(|_| model.v7(16).unwrap()).collect();
            let mut expected: Vec<_> = values
                .iter()
                .copied()
                .map(|value| reference_observation(Ok(value)))
                .collect();
            expected.push(format!(
                "ordering:{}:{}:{}:{}",
                values[0].compare(values[1]),
                values[1].compare(values[2]),
                values[2].compare(values[3]),
                values[3].compare(values[4])
            ));
            let mut body = (0..5).map(|index| format!("let v{index} = requireUuid(uuid.Uuid.v7())\ntesting.log(describe(ok(v{index})))\n")).collect::<String>();
            body.push_str(r#"testing.log("ordering:{v0.compare(v1)}:{v1.compare(v2)}:{v2.compare(v3)}:{v3.compare(v4)}")"#);
            let observed = run(&body, providers, generator_capabilities(7));
            assert_eq!(observed, expected);
            observed
        }
        unknown => panic!("unregistered UUID case {unknown}"),
    }
}

fn main() {
    // Both complete retained classes are independently checked before VM work;
    // two kernel-only configurations do not become public VM conformance cases.
    for row in cases::vectors(true)
        .into_iter()
        .chain(cases::vectors(false))
    {
        let actual = reference(&row.operation);
        assert_eq!(
            match actual {
                Ok(value) => value.canonical(),
                Err(error) => error.to_string(),
            },
            row.expected,
            "{}",
            row.id
        );
    }
    for id in cases::CASES {
        let observations = run_case(id);
        let expected = cases::run_kernel_case(id);
        assert_eq!(observations, expected, "{id}");
        println!(
            "{}",
            serde_json::json!({"id": id, "observations": observations, "envelopes_closed": true})
        );
    }
    println!(
        "{}",
        serde_json::json!({"id": "vm-capabilities", "observations": capability_observations()})
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_literals_preserve_braces_controls_quotes_dollars_and_unicode() {
        let input = "{}$\"\\\0\n\té";
        let observed = run(
            &format!("testing.log({})", literal(input)),
            Providers {
                clocks: vec![],
                entropy: vec![],
            },
            BTreeSet::new(),
        );
        assert_eq!(observed, vec![input]);
    }

    #[test]
    fn public_capability_checks_preserve_the_sealed_first_provider_values() {
        let observations = capability_observations();
        assert_eq!(
            observations
                .iter()
                .filter(|row| row["result"] == "E1008")
                .count(),
            9
        );
        assert_eq!(
            observations
                .iter()
                .filter(|row| row["result"] == "accepted")
                .count(),
            6
        );
    }

    #[test]
    #[should_panic(expected = "unregistered UUID case")]
    fn unknown_case_never_selects_an_implicit_fixture() {
        run_case("unregistered");
    }
}
