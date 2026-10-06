//! Private hosted bridge measurements; no VM interpreter or native AOT timing.

use super::*;
use crate::test_control::{EnvelopeHandle, EnvelopeLimits, UuidTestProviders};
use serde::Serialize;
use std::time::{Duration, UNIX_EPOCH};
use std::{hint::black_box, time::Instant};

// The oracle has no production imports. Unused reference operations remain
// available to its separately compiled model and fuzz suites.
#[allow(dead_code)]
#[path = "../../../../tondo-reliability/src/uuid_model.rs"]
mod reference;
use reference::{ReferenceErrorKind as RefKind, ReferenceProviders, ReferenceUuid};

const BATCH: usize = 16;
const DISPATCH: &str = "hosted-scalar";
const DNS: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const TEXT: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";
const IDS: [&str; 22] = [
    "parse-dashed",
    "parse-urn",
    "format-canonical",
    "bytes-roundtrip",
    "v4-sealed",
    "v4-os",
    "v5-empty",
    "v5-small",
    "v5-two-block",
    "v5-large",
    "v7-sealed",
    "v7-os",
    "reject-text-length",
    "reject-character",
    "reject-name-limit",
    "reject-entropy-error",
    "reject-entropy-shape",
    "reject-clock-error",
    "reject-clock-negative",
    "reject-clock-high",
    "reject-provider-limit",
    "reject-reply-budget",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Parse,
    Format,
    Bytes,
    V4,
    V5,
    V7,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Format => "format",
            Self::Bytes => "bytes-roundtrip",
            Self::V4 => "v4",
            Self::V5 => "v5",
            Self::V7 => "v7",
        }
    }
    fn host_name(self) -> &'static str {
        match self {
            Self::Parse => "std.uuid.Uuid.parse",
            Self::Format => "std.uuid.Uuid.toString",
            Self::Bytes => "std.uuid.Uuid.fromBytes",
            Self::V4 => "std.uuid.Uuid.v4",
            Self::V5 => "std.uuid.Uuid.v5",
            Self::V7 => "std.uuid.Uuid.v7",
        }
    }
}

struct Fixture {
    operation: Operation,
    host: BootstrapHost,
    arguments: Vec<RuntimeValue>,
    expected: Option<RuntimeValue>,
    budget: VmMemoryBudget,
    envelope: Option<EnvelopeHandle>,
    clocks: Vec<Result<i128, RefKind>>,
    entropy: Vec<Result<Vec<u8>, RefKind>>,
    expected_reads: (usize, usize),
    provider_bytes: u64,
    buffer_bytes: u64,
    input_bytes: u64,
}

fn reference_result(value: Result<ReferenceUuid, reference::ReferenceError>) -> RuntimeValue {
    match value {
        Ok(value) => uuid_result(Ok(Uuid::from_bytes(&value.to_bytes()).unwrap())),
        Err(failure) => uuid_result(Err(UuidError {
            kind: match failure.kind {
                RefKind::InvalidTextLength => UuidErrorKind::InvalidTextLength,
                RefKind::InvalidCharacter => UuidErrorKind::InvalidCharacter,
                RefKind::NameLimitExceeded => UuidErrorKind::NameLimitExceeded,
                RefKind::EntropyFailure => UuidErrorKind::EntropyFailure,
                RefKind::ClockFailure => UuidErrorKind::ClockFailure,
                RefKind::TimestampOutOfRange => UuidErrorKind::TimestampOutOfRange,
                RefKind::ProviderMisconfigured => UuidErrorKind::ProviderMisconfigured,
                RefKind::ResourceLimit => UuidErrorKind::ResourceLimit,
                other => panic!("unexpected fixture reference error: {other:?}"),
            },
            offset: failure.offset,
        })),
    }
}

fn operation(id: &str) -> Operation {
    match id {
        "parse-dashed" | "parse-urn" | "reject-text-length" | "reject-character" => {
            Operation::Parse
        }
        "format-canonical" => Operation::Format,
        "bytes-roundtrip" => Operation::Bytes,
        "v5-empty" | "v5-small" | "v5-two-block" | "v5-large" | "reject-name-limit" => {
            Operation::V5
        }
        "v7-sealed"
        | "v7-os"
        | "reject-clock-error"
        | "reject-clock-negative"
        | "reject-clock-high" => Operation::V7,
        "v4-sealed"
        | "v4-os"
        | "reject-entropy-error"
        | "reject-entropy-shape"
        | "reject-provider-limit"
        | "reject-reply-budget" => Operation::V4,
        _ => panic!("unknown UUID performance workload: {id}"),
    }
}

fn fixture(id: &'static str) -> Fixture {
    let operation = operation(id);
    let max_bytes = match id {
        "reject-provider-limit" => 15,
        "reject-name-limit" => 63,
        _ => 8192,
    };
    let budget = VmMemoryBudget::new(if id == "reject-reply-budget" {
        UUID_REPLY_BYTES - 1
    } else {
        16_384
    });
    let mut host = BootstrapHost::with_max_bytes(vec![], max_bytes);
    host.set_test_memory_budget(Some(budget.clone()));
    let mut arguments = vec![];
    let mut expected = None;
    let mut input_bytes = 0;
    let mut buffer_bytes = 0;
    let namespace = ReferenceUuid::parse(DNS).unwrap();
    match operation {
        Operation::Parse => {
            let text = match id {
                "parse-urn" => format!("URN:UUID:{}", TEXT.to_uppercase()),
                "reject-text-length" => "x".repeat(4096),
                "reject-character" => format!("{}!", &TEXT[..35]),
                _ => TEXT.to_string(),
            };
            input_bytes = text.len() as u64;
            expected = Some(reference_result(ReferenceUuid::parse(&text)));
            arguments.push(RuntimeValue::String(text));
        }
        Operation::Format => {
            let value = ReferenceUuid::parse(TEXT).unwrap();
            expected = Some(RuntimeValue::String(value.canonical()));
            arguments.push(uuid_value(Uuid::from_bytes(&value.to_bytes()).unwrap()));
            input_bytes = 16;
        }
        Operation::Bytes => {
            let value = ReferenceUuid::parse(TEXT).unwrap();
            expected = Some(reference_result(Ok(value)));
            arguments.push(host.allocate_bytes(value.to_bytes().to_vec()).unwrap());
            input_bytes = 16;
            buffer_bytes = 32 + 16;
        }
        Operation::V5 => {
            let name = match id {
                "v5-empty" => vec![],
                "v5-small" => b"www.example.com".to_vec(),
                "v5-two-block" => (0..40).collect(),
                "reject-name-limit" => vec![0xff; 64],
                "v5-large" => (0..4096).map(|index| (index % 251) as u8).collect(),
                _ => unreachable!(),
            };
            input_bytes = 16 + name.len() as u64;
            buffer_bytes = 32 + name.len() as u64;
            expected = Some(reference_result(if id == "v5-large" {
                // Independently authored with Python hashlib SHA-1 over DNS
                // network bytes and bytes(index % 251 for index in 0..4096).
                // This is outside the bounded model's 96-byte name domain.
                Ok(ReferenceUuid::parse("788dc126-8af8-5603-9087-9ddc041c9750").unwrap())
            } else {
                namespace.v5(&name, max_bytes as usize)
            }));
            arguments.push(uuid_value(Uuid::from_bytes(&namespace.to_bytes()).unwrap()));
            arguments.push(host.allocate_bytes(name).unwrap());
        }
        Operation::V4 | Operation::V7 => {}
    }
    let sealed = matches!(operation, Operation::V4 | Operation::V7) && !id.ends_with("-os");
    let mut clocks = vec![];
    let mut entropy = vec![];
    let mut expected_reads = (0, 0);
    let mut provider_bytes = 0;
    let envelope = if sealed {
        if operation == Operation::V7 {
            clocks = vec![
                match id {
                    "reject-clock-error" => Err(RefKind::ClockFailure),
                    "reject-clock-negative" => Ok(-1),
                    "reject-clock-high" => Ok(reference::MAX_MILLISECONDS + 1),
                    _ => Ok(1_700_000_000_123),
                };
                BATCH
            ];
        }
        let length = if operation == Operation::V7 { 10 } else { 16 };
        entropy = vec![
            match id {
                "reject-entropy-error" => Err(RefKind::EntropyFailure),
                "reject-entropy-shape" => Ok(vec![0xa5; 15]),
                _ => Ok(vec![0xa5; length]),
            };
            BATCH
        ];
        let mut oracle = ReferenceProviders::new(clocks.clone(), entropy.clone()).unwrap();
        for _ in 0..BATCH {
            let result = if id == "reject-reply-budget" {
                None
            } else if operation == Operation::V4 {
                Some(reference_result(oracle.v4(max_bytes as usize)))
            } else {
                Some(reference_result(oracle.v7(max_bytes as usize)))
            };
            if let Some(previous) = &expected {
                assert_eq!(Some(previous), result.as_ref());
            }
            expected = result;
        }
        expected_reads = oracle.consumed();
        let providers = UuidTestProviders::new(
            clocks
                .iter()
                .map(|row| {
                    row.map(|ms| {
                        if ms < 0 {
                            UNIX_EPOCH - Duration::from_millis((-ms) as u64)
                        } else {
                            UNIX_EPOCH + Duration::from_millis(ms as u64)
                        }
                    })
                    .map_err(|_| UuidErrorKind::ClockFailure)
                })
                .collect(),
            entropy
                .iter()
                .map(|row| row.clone().map_err(|_| UuidErrorKind::EntropyFailure))
                .collect(),
        )
        .unwrap();
        provider_bytes = providers.logical_bytes();
        let envelope = EnvelopeHandle::new(
            "uuid-performance",
            EnvelopeLimits::new(16_384, 16_384, 16_384),
        );
        envelope.with_uuid_providers(providers).unwrap();
        host.install_testing_envelope(envelope.clone());
        Some(envelope)
    } else {
        None
    };
    Fixture {
        operation,
        host,
        arguments,
        expected,
        budget,
        envelope,
        clocks,
        entropy,
        expected_reads,
        provider_bytes,
        buffer_bytes,
        input_bytes,
    }
}

// Selected owned identities and detached value storage, not allocator calls.
fn retained(value: &RuntimeValue) -> (u64, u64) {
    let mut bytes = 32;
    let mut identities = 0;
    match value {
        RuntimeValue::String(text) => {
            bytes += text.len() as u64;
            identities += 1;
        }
        RuntimeValue::Record { name, values } | RuntimeValue::Variant { name, values, .. } => {
            bytes += name.len() as u64;
            identities += 1 + u64::from(!values.is_empty());
            for value in values {
                let (child_bytes, child_ids) = retained(value);
                bytes += child_bytes;
                identities += child_ids;
            }
        }
        RuntimeValue::ResultOk(value)
        | RuntimeValue::ResultErr(value)
        | RuntimeValue::OptionSome(value) => {
            let (child_bytes, child_ids) = retained(value);
            bytes += child_bytes;
            identities += 1 + child_ids;
        }
        RuntimeValue::Integer(_)
        | RuntimeValue::Bool(_)
        | RuntimeValue::OptionNone
        | RuntimeValue::Host { .. } => {}
        other => panic!("unmodeled fixture storage: {other:?}"),
    }
    (bytes, identities)
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Counters {
    input_bytes: u64,
    output_bytes: u64,
    operations: u64,
    bytes_copied: u64,
    allocations: u64,
    logical_memory_bytes: u64,
    reply_admission_bytes: u64,
    reply_retained_bytes: u64,
    host_handles_created: u64,
    terminal_live_handles: u64,
    terminal_budget_bytes: u64,
    entropy_requests: u64,
    clock_requests: u64,
    sha1_blocks: u64,
    adversarial_rejections: u64,
    native_live_handles: Option<u64>,
}

fn batch(id: &'static str) -> (u64, Counters) {
    let mut fixture = fixture(id);
    let mut fixture_bytes = fixture.provider_bytes + fixture.buffer_bytes;
    let mut allocations = u64::from(!fixture.arguments.is_empty())
        + u64::from(fixture.buffer_bytes != 0)
        + u64::from(fixture.envelope.is_some())
        + u64::from(!fixture.clocks.is_empty())
        + u64::from(!fixture.entropy.is_empty());
    for value in fixture.arguments.iter().chain(fixture.expected.iter()) {
        let (bytes, identities) = retained(value);
        fixture_bytes += bytes;
        allocations += identities;
    }
    allocations += fixture
        .entropy
        .iter()
        .filter(|row| row.as_ref().is_ok_and(|bytes| !bytes.is_empty()))
        .count() as u64;
    let initial_handles = fixture.host.next_value;
    let initial_budget = fixture.budget.live_bytes();
    let mut output_bytes = 0;
    let mut reply_bytes = 0;
    let mut rejections = 0;
    let roundtrip_bytes = ReferenceUuid::parse(TEXT).unwrap().to_bytes();
    let started = Instant::now();
    for _ in 0..BATCH {
        let reply = black_box(fixture.host.invoke_admitted(
            fixture.operation.host_name(),
            black_box(&fixture.arguments),
            Some(&fixture.budget),
        ));
        if id == "reject-reply-budget" {
            assert!(matches!(
                reply,
                Err(VmError::ResourceLimit {
                    resource: "memory",
                    limit: 181
                })
            ));
            rejections += 1;
            continue;
        }
        let reply = reply.unwrap();
        let retained_reply = reply.memory.as_ref().unwrap().bytes();
        reply_bytes = reply_bytes.max(retained_reply);
        allocations += retained(&reply.value).1;
        if id.ends_with("-os") {
            let RuntimeValue::ResultOk(value) = &reply.value else {
                panic!("OS provider failed: {:?}", reply.value)
            };
            let value = uuid_input(value).unwrap();
            assert_eq!(
                value.version(),
                if fixture.operation == Operation::V4 {
                    4
                } else {
                    7
                }
            );
            assert_eq!(value.variant(), tondo_stdlib::uuid::UuidVariant::Rfc9562);
        } else {
            assert_eq!(&reply.value, fixture.expected.as_ref().unwrap());
        }
        match &reply.value {
            RuntimeValue::ResultErr(_) => {
                rejections += 1;
            }
            RuntimeValue::String(text) => {
                output_bytes = text.len() as u64;
            }
            RuntimeValue::ResultOk(value) if fixture.operation == Operation::Bytes => {
                let output = fixture
                    .host
                    .invoke_admitted(
                        "std.uuid.Uuid.toBytes",
                        std::slice::from_ref(value),
                        Some(&fixture.budget),
                    )
                    .unwrap();
                let bytes = fixture.host.bytes(&output.value).unwrap();
                assert_eq!(bytes, roundtrip_bytes);
                output_bytes = bytes.len() as u64;
                reply_bytes =
                    reply_bytes.max(retained_reply + output.memory.as_ref().unwrap().bytes());
                allocations += 1; // the new output buffer
                let RuntimeValue::Host { id, .. } = &output.value else {
                    unreachable!()
                };
                fixture.host.values.remove(id);
                fixture.host.buffer_memory.remove(id);
                drop(output);
            }
            RuntimeValue::ResultOk(_) => {
                output_bytes = 16;
            }
            other => panic!("unexpected UUID reply: {other:?}"),
        }
        drop(reply);
        assert_eq!(fixture.budget.live_bytes(), initial_budget);
    }
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    // Verify finite provider consumption outside timing, including rows that
    // must remain untouched on admission, clock and byte-limit refusals.
    if let Some(envelope) = &fixture.envelope {
        for row in fixture.clocks.iter().skip(fixture.expected_reads.0) {
            let actual = envelope.uuid_clock().unwrap().unwrap();
            let expected = row
                .map(|ms| {
                    if ms < 0 {
                        UNIX_EPOCH - Duration::from_millis((-ms) as u64)
                    } else {
                        UNIX_EPOCH + Duration::from_millis(ms as u64)
                    }
                })
                .map_err(|_| error(UuidErrorKind::ClockFailure));
            assert_eq!(actual, expected);
        }
        assert_eq!(
            envelope.uuid_clock().unwrap(),
            Some(Err(error(UuidErrorKind::ClockUnavailable)))
        );
        for row in fixture.entropy.iter().skip(fixture.expected_reads.1) {
            let length = row.as_ref().map_or(16, Vec::len);
            let mut snapshot = vec![0; length];
            let actual = envelope.uuid_entropy(&mut snapshot).unwrap().unwrap();
            assert_eq!(
                actual,
                row.as_ref()
                    .map(|_| ())
                    .map_err(|_| error(UuidErrorKind::EntropyFailure))
            );
            if let Ok(expected) = row {
                assert_eq!(&snapshot, expected);
            }
        }
        assert_eq!(
            envelope.uuid_entropy(&mut [0; 16]).unwrap(),
            Some(Err(error(UuidErrorKind::EntropyUnavailable)))
        );
        envelope.close().unwrap();
    }
    let host_handles_created = fixture.host.next_value - initial_handles;
    for value in &fixture.arguments {
        if let RuntimeValue::Host { id, .. } = value {
            fixture.host.values.remove(id);
            fixture.host.buffer_memory.remove(id);
        }
    }
    assert!(fixture.host.values.is_empty() && fixture.host.buffer_memory.is_empty());
    assert_eq!(fixture.budget.live_bytes(), 0);
    let (clock_requests, entropy_requests) = if id.ends_with("-os") {
        (
            u64::from(fixture.operation == Operation::V7) * BATCH as u64,
            BATCH as u64,
        )
    } else {
        (
            fixture.expected_reads.0 as u64,
            fixture.expected_reads.1 as u64,
        )
    };
    let copies = match fixture.operation {
        Operation::Format => 36 * BATCH as u64,
        Operation::Bytes => 32 * BATCH as u64,
        _ if id.ends_with("-sealed") => {
            entropy_requests
                * if fixture.operation == Operation::V7 {
                    10
                } else {
                    16
                }
        }
        _ => 0,
    };
    let admission = if id == "reject-reply-budget" {
        0
    } else {
        UUID_REPLY_BYTES
    };
    let extra = if fixture.operation == Operation::Bytes {
        132 + 48
    } else {
        0
    };
    let counters = Counters {
        input_bytes: fixture.input_bytes,
        output_bytes,
        operations: BATCH as u64,
        bytes_copied: copies,
        allocations,
        logical_memory_bytes: fixture_bytes + admission + extra,
        reply_admission_bytes: admission,
        reply_retained_bytes: reply_bytes,
        host_handles_created,
        terminal_live_handles: 0,
        terminal_budget_bytes: fixture.budget.live_bytes(),
        entropy_requests,
        clock_requests,
        sha1_blocks: if fixture.operation == Operation::V5 && rejections == 0 {
            (fixture.input_bytes + 9).div_ceil(64)
        } else {
            0
        },
        adversarial_rejections: rejections,
        native_live_handles: None,
    };
    (nanos, counters)
}

#[test]
fn uuid_performance_fixtures_and_lifecycle_match_independent_expectations() {
    for id in IDS {
        let (_, counters) = batch(id);
        assert_eq!(
            counters.adversarial_rejections,
            if id.starts_with("reject-") { 16 } else { 0 }
        );
    }
}

#[test]
fn uuid_performance_counter_shapes_and_padding_are_stable() {
    for id in IDS {
        let (_, first) = batch(id);
        let (_, second) = batch(id);
        assert_eq!(first, second, "{id}");
        assert_eq!(
            first.reply_admission_bytes,
            if id == "reject-reply-budget" { 0 } else { 182 }
        );
        assert_eq!(first.terminal_budget_bytes + first.terminal_live_handles, 0);
    }
    assert_eq!(batch("v5-empty").1.sha1_blocks, 1);
    assert_eq!(batch("v5-two-block").1.sha1_blocks, 2);
    assert_eq!(batch("v5-large").1.sha1_blocks, 65);
    assert_eq!(batch("bytes-roundtrip").1.host_handles_created, 16);
    assert_eq!(batch("reject-provider-limit").1.entropy_requests, 0);
    assert_eq!(batch("reject-reply-budget").1.entropy_requests, 0);
    for id in [
        "reject-clock-error",
        "reject-clock-negative",
        "reject-clock-high",
    ] {
        assert_eq!(batch(id).1.entropy_requests, 0);
    }
}

#[test]
#[should_panic(expected = "unknown UUID performance workload")]
fn uuid_performance_unknown_fixture_is_refused() {
    fixture("unrecorded");
}

#[test]
fn uuid_performance_probe() {
    if std::env::var("TONDO_UUID_PERF_RUN").as_deref() != Ok("1") {
        return;
    }
    let process = std::env::var("TONDO_UUID_PERF_PROCESS")
        .unwrap()
        .parse::<u8>()
        .unwrap();
    assert!((1..=3).contains(&process));
    for id in IDS {
        for _ in 0..3 {
            black_box(batch(id));
        }
        for repetition in 0..9 {
            let (nanos, counters) = batch(id);
            println!(
                "TONDO_UUID_PERF\t{}",
                serde_json::json!({
                    "workload_id":id, "operation":operation(id).label(), "process":process,
                    "repetition":repetition, "nanos":nanos, "dispatch":DISPATCH, "counters":counters,
                })
            );
        }
    }
}
