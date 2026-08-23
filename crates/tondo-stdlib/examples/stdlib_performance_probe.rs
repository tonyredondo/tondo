//! Owner-aware, allocation-instrumented performance campaign for STD-0.1A.
//!
//! The probe deliberately measures only portable kernels.  Compiler/VM
//! intrinsics and host providers are recorded as not-applicable in the
//! coordinator and are measured by PERF-001, where their complete target
//! identity exists.  Every row below is an exact scalar-oracle operation.

use std::hint::black_box;
use std::time::Instant;

use tondo_stdlib::{format, io, json, math, messagepack, path, protobuf, serialization, testing};

const SAMPLES: usize = 9;
const WARMUPS: usize = 3;
const BATCH: usize = 256;

#[derive(Clone, Copy, Default)]
struct AllocationObservation {
    /// Logical owned buffers published by the operation.  This is deliberately
    /// independent of the Rust allocator implementation and is stable across
    /// targets; heap-call counts remain a PERF-001 backend concern.
    allocations: u64,
    bytes: u64,
}

fn observed(allocations: usize, bytes: usize) -> AllocationObservation {
    AllocationObservation {
        allocations: allocations as u64,
        bytes: bytes as u64,
    }
}

fn sample(
    module: &str,
    operation_id: &str,
    workload: &str,
    mut operation: impl FnMut() -> AllocationObservation,
) {
    for _ in 0..WARMUPS {
        for _ in 0..BATCH {
            operation();
        }
    }
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let mut allocations = 0_u64;
        let mut allocated_bytes = 0_u64;
        for _ in 0..BATCH {
            let observation = operation();
            allocations = allocations.saturating_add(observation.allocations);
            allocated_bytes = allocated_bytes.saturating_add(observation.bytes);
        }
        let nanos = started.elapsed().as_nanos() / BATCH as u128;
        allocations /= BATCH as u64;
        allocated_bytes /= BATCH as u64;
        println!("{module}\t{operation_id}\t{workload}\t{nanos}\t{allocations}\t{allocated_bytes}");
    }
}

fn json_cycle(input: &[u8]) -> AllocationObservation {
    if let Ok(value) = json::parse(black_box(input)) {
        let encoded = json::encode_canonical(&value).expect("canonical JSON");
        let bytes = input.len().saturating_add(encoded.len());
        black_box(encoded);
        observed(2, bytes)
    } else {
        observed(0, 0)
    }
}

fn messagepack_cycle(input: &[u8]) -> AllocationObservation {
    if let Ok(value) = messagepack::decode(black_box(input)) {
        let encoded = messagepack::encode_deterministic(&value).expect("deterministic MessagePack");
        let bytes = input.len().saturating_add(encoded.len());
        black_box(encoded);
        observed(2, bytes)
    } else {
        observed(0, 0)
    }
}

fn protobuf_decode(input: &[u8]) -> AllocationObservation {
    let decoded =
        protobuf::decode_message(black_box(input), protobuf::ProtoDecodeOptions::default());
    let _ = black_box(decoded);
    observed(1, input.len())
}

fn format_join(values: &[String], separator: &str) -> AllocationObservation {
    let output = format::join(values, separator, format::FormatLimits::default());
    let bytes = output.as_ref().map_or(0, |value| value.len());
    let _ = black_box(output);
    observed(1, bytes)
}

fn io_round_trip(input: &[u8], chunk: usize) -> AllocationObservation {
    let mut reader = io::SliceReader::new(input.to_vec(), chunk).expect("positive chunk");
    let output = io::read_all(&mut reader, io::IoLimits::default()).expect("bounded read");
    let mut writer = io::VecWriter::with_max_write(chunk).expect("positive write bound");
    io::write_all(&mut writer, &output).expect("bounded write");
    let bytes = output.len().saturating_add(writer.bytes().len());
    black_box(writer);
    observed(3, bytes)
}

fn path_walk(input: &str, component: &str) -> AllocationObservation {
    let path = path::Path::from_string(input).expect("valid lexical path");
    let joined = path.join(component).expect("valid component");
    let bytes = joined
        .as_bytes()
        .len()
        .saturating_add(joined.to_bytes().len());
    black_box((joined.file_name(), joined.extension(), joined.to_bytes()));
    observed(3, bytes)
}

fn serialize_events(value: i128, bytes: &[u8]) -> AllocationObservation {
    let mut serializer = serialization::EventSerializer::new(serialization::Limits::default());
    serialization::Encoder::<serialization::Json, serialization::SerializationError>::start_array(
        &mut serializer,
        Some(2),
    )
    .expect("array start event");
    serialization::Encoder::<serialization::Json, serialization::SerializationError>::int(
        &mut serializer,
        value as i64,
    )
    .expect("scalar event");
    serialization::Encoder::<serialization::Json, serialization::SerializationError>::bytes(
        &mut serializer,
        bytes,
    )
    .expect("bytes event");
    serialization::Encoder::<serialization::Json, serialization::SerializationError>::end_array(
        &mut serializer,
    )
    .expect("array end event");
    let events = serializer.finish().expect("bounded event stream");
    let owned_bytes = bytes
        .len()
        .saturating_add(std::mem::size_of_val(events.as_slice()));
    black_box(events);
    observed(1, owned_bytes)
}

fn bytes_copy_hash(input: &[u8]) -> AllocationObservation {
    let owned = serialization::Bytes::from_slice(input);
    black_box((
        owned.clone(),
        owned.as_slice().iter().fold(0_u64, |hash, byte| {
            hash.wrapping_mul(1_099_511_628_211)
                .wrapping_add(u64::from(*byte))
        }),
    ));
    observed(2, input.len().saturating_mul(2))
}

fn testing_diff(input: &str) -> AllocationObservation {
    let mut generator = testing::Generator::new(7);
    let generated = generator.next_text(input.len()).expect("bounded text");
    let diff = testing::diff_text(input, input);
    let bytes = generated.len().saturating_add(diff.render().len());
    black_box((generated, diff));
    observed(2, bytes)
}

fn main() {
    if std::env::var_os("TONDO_PERF_STARTUP_ONLY").is_some() {
        return;
    }

    let json_inputs = [
        ("empty", br#"null"#.to_vec()),
        ("small", br#"{}"#.to_vec()),
        (
            "representative",
            br#"{"z":[1,2,3],"a":{"n":true}}"#.to_vec(),
        ),
        (
            "large",
            format!(
                "[{}]",
                (0..256)
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .into_bytes(),
        ),
        ("fragmented_stream", br#"{"chunks":[1,2,3,4]}"#.to_vec()),
        ("adversarial", br#"{"unterminated":[1,2"#.to_vec()),
    ];
    for (workload, input) in &json_inputs {
        sample("json", "std.json.parse_encode", workload, || {
            json_cycle(input)
        });
    }

    let messagepack_values = [
        ("empty", messagepack::Value::Nil),
        ("small", messagepack::Value::UInt(1)),
        (
            "representative",
            messagepack::Value::Map(vec![
                messagepack::MessagePackEntry {
                    key: messagepack::Value::String("b".into()),
                    value: messagepack::Value::UInt(2),
                },
                messagepack::MessagePackEntry {
                    key: messagepack::Value::String("a".into()),
                    value: messagepack::Value::UInt(1),
                },
            ]),
        ),
        (
            "large",
            messagepack::Value::Array((0..256).map(messagepack::Value::UInt).collect()),
        ),
        (
            "fragmented_stream",
            messagepack::Value::Array(vec![messagepack::Value::String("chunks".into())]),
        ),
        ("adversarial", messagepack::Value::String("invalid".into())),
    ];
    for (workload, value) in &messagepack_values {
        let encoded = messagepack::encode(value);
        let input = if *workload == "adversarial" {
            vec![0xc1]
        } else {
            encoded
        };
        sample(
            "messagepack",
            "std.messagepack.decode_encode",
            workload,
            || messagepack_cycle(&input),
        );
    }

    let protobuf_inputs = [
        ("empty", Vec::new()),
        ("small", vec![0x08, 0x01]),
        (
            "representative",
            vec![0x08, 0x01, 0x12, 0x03, b'o', b'k', b'!'],
        ),
        ("large", (0..128).flat_map(|_| [0x08, 0x01]).collect()),
        ("fragmented_stream", vec![0x12, 0x03, b'o', b'k', b'!']),
        ("adversarial", vec![0x80; 12]),
    ];
    for (workload, input) in &protobuf_inputs {
        sample("protobuf", "std.protobuf.decode_message", workload, || {
            protobuf_decode(input)
        });
    }

    let text_inputs = [
        ("empty", "".to_owned()),
        ("small", "ok".to_owned()),
        ("representative", "old\nvalue\n".to_owned()),
        ("large", "line\n".repeat(512)),
        ("fragmented_stream", "a|b|c|d".to_owned()),
        ("adversarial", "x".repeat(4096)),
    ];
    for (workload, input) in &text_inputs {
        sample("testing", "std.testing.generate_diff", workload, || {
            testing_diff(input)
        });
    }

    let math_inputs = [
        ("empty", (0.0, 0.0, 0.0)),
        ("small", (1.0, 2.0, 0.5)),
        ("representative", (1.25, 2.0, 0.5)),
        ("large", (f64::MAX / 2.0, 2.0, -f64::MAX / 4.0)),
        ("fragmented_stream", (1.0 / 3.0, 7.0, -2.0)),
        ("adversarial", (f64::NAN, 1.0, 0.0)),
    ];
    for (workload, (a, b, c)) in math_inputs {
        sample("math", "std.math.fma", workload, || {
            black_box(math::fma(a, b, c));
            observed(0, 0)
        });
    }

    let format_inputs = [
        ("empty", Vec::new()),
        ("small", vec!["ok".to_owned()]),
        (
            "representative",
            vec!["tondo".to_owned(), "stdlib".to_owned()],
        ),
        ("large", (0..256).map(|value| value.to_string()).collect()),
        (
            "fragmented_stream",
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
        ),
        ("adversarial", vec!["x".repeat(16 * 1024)]),
    ];
    for (workload, values) in &format_inputs {
        sample("format", "std.format.join", workload, || {
            format_join(values, ",")
        });
    }

    let io_inputs = [
        ("empty", Vec::new(), 1),
        ("small", b"tondo".to_vec(), 2),
        ("representative", b"tondo standard library".to_vec(), 4),
        ("large", vec![b'x'; 16 * 1024], 64),
        ("fragmented_stream", b"fragmented-stream".to_vec(), 1),
        ("adversarial", vec![b'x'; 64 * 1024], 3),
    ];
    for (workload, input, chunk) in &io_inputs {
        sample("io", "std.io.read_write_all", workload, || {
            io_round_trip(input, *chunk)
        });
    }

    let path_inputs = [
        ("empty", "", "x"),
        ("small", "a", "b"),
        ("representative", "a/../b.txt", "next"),
        ("large", &"segment/".repeat(128), "leaf"),
        ("fragmented_stream", "a/b/c", "d"),
        ("adversarial", &"x".repeat(1024), "leaf"),
    ];
    for (workload, input, component) in &path_inputs {
        sample("path", "std.path.lexical", workload, || {
            path_walk(input, component)
        });
    }

    let bytes_inputs = [
        ("empty", Vec::new()),
        ("small", b"tondo".to_vec()),
        ("representative", b"tondo bytes".to_vec()),
        ("large", vec![0x5a; 16 * 1024]),
        ("fragmented_stream", (0..64).collect()),
        ("adversarial", vec![0xff; 32 * 1024]),
    ];
    for (workload, input) in &bytes_inputs {
        sample("bytes", "std.bytes.copy_hash", workload, || {
            bytes_copy_hash(input)
        });
    }

    let event_inputs = [
        ("empty", 0_i128, Vec::new()),
        ("small", 1, b"x".to_vec()),
        ("representative", 42, b"tondo".to_vec()),
        ("large", 1024, vec![0x5a; 1024]),
        ("fragmented_stream", 7, (0..64).collect()),
        ("adversarial", i128::MAX, vec![0xff; 4096]),
    ];
    for (workload, value, input) in &event_inputs {
        sample(
            "serialization",
            "std.serialization.events",
            workload,
            || serialize_events(*value, input),
        );
    }
}
