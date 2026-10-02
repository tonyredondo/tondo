//! Target-qualified scalar-kernel probe with independent bounded wire oracles.
#[path = "../../tondo-reliability/src/cbor_model.rs"]
#[allow(dead_code)]
mod reference;

use reference::ReferenceValue as Value;
use serde_json::json;
use std::hint::black_box;
use std::time::Instant;
use tondo_stdlib::cbor::{
    self, CborDecodeOptions, CborEncodeOptions, CborEntry, CborError, CborErrorKind, CborEvent,
    CborFloat16, CborLimits, CborPath, CborReader, CborTag, CborValue, CborWriter,
};

const BATCH: usize = 16;

#[derive(Clone, Copy, Debug)]
enum Operation {
    Parse,
    View,
    Encode,
    Deterministic,
    Reader,
    Writer,
    RejectParse,
    RejectDeterministic,
}

#[derive(Debug, PartialEq)]
enum Observation {
    Value(CborValue),
    View(usize),
    Bytes(Vec<u8>),
    Events(Vec<CborEvent>),
}

struct Workload {
    id: &'static str,
    operation: Operation,
    input: Vec<u8>,
    value: Option<CborValue>,
    events: Vec<CborEvent>,
    expected: Result<Observation, CborError>,
    options: CborDecodeOptions,
    shape: Shape,
}

impl Workload {
    fn valid(
        id: &'static str,
        operation: Operation,
        value: Value,
        input: Option<Vec<u8>>,
        events: Vec<CborEvent>,
    ) -> Self {
        let ordinary = reference::render_ordinary(&value).expect(id);
        let input = input.unwrap_or_else(|| ordinary.clone());
        assert_eq!(reference::parse_reference(&input).expect(id), value);
        let materialized = kernel(&value);
        let mut stats = Shape::default();
        shape(&value, 1, &mut stats);
        let mut retained = None;
        let mut writer_events = Vec::new();
        let expected = match operation {
            Operation::Parse => Observation::Value(materialized),
            Operation::View => Observation::View(input.len()),
            Operation::Encode => {
                retained = Some(materialized);
                Observation::Bytes(ordinary)
            }
            Operation::Deterministic => {
                retained = Some(materialized);
                Observation::Bytes(reference::render_deterministic(&value).expect(id))
            }
            Operation::Reader => Observation::Events(events),
            Operation::Writer => {
                writer_events = events;
                Observation::Bytes(input.clone())
            }
            Operation::RejectParse | Operation::RejectDeterministic => {
                panic!("rejecting valid fixture")
            }
        };
        Self {
            id,
            operation,
            input,
            value: retained,
            events: writer_events,
            expected: Ok(expected),
            options: CborDecodeOptions::default(),
            shape: stats,
        }
    }

    fn rejection(
        id: &'static str,
        input: Vec<u8>,
        limits: CborLimits,
        expected: CborError,
    ) -> Self {
        let mut stats = Shape::default();
        match reference::parse_reference(&input) {
            Ok(value) => {
                assert_eq!(expected.kind, CborErrorKind::LimitExceeded);
                shape(&value, 1, &mut stats);
            }
            Err(problem) => {
                assert_eq!(problem.kind, reference::ReferenceErrorKind::InvalidSimple);
                assert_eq!(expected.kind, CborErrorKind::InvalidSimpleValue);
            }
        }
        Self {
            id,
            operation: Operation::RejectParse,
            input,
            value: None,
            events: Vec::new(),
            expected: Err(expected),
            options: CborDecodeOptions {
                limits,
                ..CborDecodeOptions::default()
            },
            shape: stats,
        }
    }

    fn run(&self) -> Result<Observation, CborError> {
        match self.operation {
            Operation::Parse | Operation::RejectParse => {
                cbor::parse(&self.input, self.options).map(Observation::Value)
            }
            Operation::View => cbor::parse_view(&self.input, self.options)
                .map(|view| Observation::View(view.as_bytes().len())),
            Operation::Encode => {
                cbor::encode(self.value.as_ref().unwrap(), CborEncodeOptions::default())
                    .map(Observation::Bytes)
            }
            Operation::Deterministic | Operation::RejectDeterministic => {
                cbor::encode_deterministic(self.value.as_ref().unwrap(), self.options.limits)
                    .map(Observation::Bytes)
            }
            Operation::Reader => {
                let mut reader =
                    CborReader::from_reader(std::io::Cursor::new(&self.input), self.options)?;
                let mut events = Vec::new();
                while let Some(event) = reader.next()? {
                    events.push(event);
                }
                reader.finish()?;
                Ok(Observation::Events(events))
            }
            Operation::Writer => {
                let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default())?;
                for event in &self.events {
                    writer.write(event.clone())?;
                }
                writer.finish().map(Observation::Bytes)
            }
        }
    }
}

fn indefinite_fixture() -> (Value, Vec<u8>) {
    let value = Value::Array(vec![
        Value::Text("café 🌍".into()),
        Value::Bytes(vec![0, 1, 0xfe, 0xff]),
        Value::Tag(1, Box::new(Value::Float16(0x3e00))),
    ]);
    let mut wire = vec![0x9f, 0x7f, 0x65];
    wire.extend_from_slice("café".as_bytes());
    wire.push(0x65);
    wire.extend_from_slice(" 🌍".as_bytes());
    wire.extend_from_slice(&[
        0xff, 0x5f, 0x42, 0, 1, 0x42, 0xfe, 0xff, 0xff, 0xc1, 0xf9, 0x3e, 0, 0xff,
    ]);
    (value, wire)
}

fn indefinite_events() -> Vec<CborEvent> {
    vec![
        CborEvent::StreamStart,
        CborEvent::StartArray(None),
        CborEvent::StartText(None),
        CborEvent::TextChunk("café".into()),
        CborEvent::TextChunk(" 🌍".into()),
        CborEvent::EndText,
        CborEvent::StartBytes(None),
        CborEvent::ByteChunk(vec![0, 1]),
        CborEvent::ByteChunk(vec![0xfe, 0xff]),
        CborEvent::EndBytes,
        CborEvent::Tag(1),
        CborEvent::Float16(CborFloat16 { bits: 0x3e00 }),
        CborEvent::EndArray,
        CborEvent::StreamEnd,
    ]
}

fn fixtures() -> Vec<Workload> {
    let (indefinite, wire) = indefinite_fixture();
    let empty = Vec::new;
    let mut fixtures = vec![
        Workload::valid(
            "parse-keys-small",
            Operation::Parse,
            Value::Map(vec![
                (Value::UInt(7), Value::Text("café".into())),
                (
                    Value::Array(vec![Value::UInt(1), Value::UInt(2)]),
                    Value::Bool(true),
                ),
                (Value::Bytes(vec![0, 255]), Value::Negative(3)),
                (Value::Tag(42, Box::new(Value::UInt(1))), Value::Undefined),
            ]),
            None,
            empty(),
        ),
        Workload::valid(
            "parse-maps-medium",
            Operation::Parse,
            rows(3),
            None,
            empty(),
        ),
        Workload::valid(
            "parse-maps-large",
            Operation::Parse,
            rows(12),
            None,
            empty(),
        ),
        Workload::valid(
            "parse-indefinite",
            Operation::Parse,
            indefinite.clone(),
            Some(wire.clone()),
            empty(),
        ),
        Workload::valid("parse-view-large", Operation::View, rows(12), None, empty()),
        Workload::valid(
            "encode-maps-large",
            Operation::Encode,
            rows(12),
            None,
            empty(),
        ),
        Workload::valid(
            "deterministic-maps-large",
            Operation::Deterministic,
            rows(12),
            None,
            empty(),
        ),
        // Sorting heterogeneous keys and normalizing floats share one small
        // fixture, so the fifteen routes cover both deterministic costs.
        Workload::valid(
            "deterministic-floats-tags",
            Operation::Deterministic,
            Value::Map(vec![
                (
                    Value::UInt(7),
                    Value::Array(vec![
                        Value::Float16(0x8000),
                        Value::Float32(0x3fc00000),
                        Value::Float64(0x7ff8000000000042),
                        Value::Tag(42, Box::new(Value::Bytes(vec![0, 1, 2, 255]))),
                        Value::Negative(u64::MAX),
                        Value::UInt(u64::MAX),
                        Value::Undefined,
                        Value::Simple(50),
                    ]),
                ),
                (
                    Value::Array(vec![Value::UInt(1), Value::UInt(2)]),
                    Value::Bool(true),
                ),
                (Value::Bytes(vec![0, 255]), Value::Negative(3)),
                (Value::Tag(42, Box::new(Value::UInt(1))), Value::Undefined),
            ]),
            None,
            empty(),
        ),
        Workload::valid(
            "reader-indefinite",
            Operation::Reader,
            indefinite.clone(),
            Some(wire.clone()),
            indefinite_events(),
        ),
        Workload::valid(
            "writer-indefinite",
            Operation::Writer,
            indefinite,
            Some(wire),
            indefinite_events(),
        ),
        Workload::rejection(
            "reject-depth",
            vec![0x81, 0x81, 0x81, 0],
            CborLimits {
                max_depth: 2,
                ..CborLimits::default()
            },
            error(
                CborErrorKind::LimitExceeded,
                2,
                2,
                vec![CborPath::ArrayIndex(0), CborPath::ArrayIndex(0)],
            ),
        ),
        Workload::rejection(
            "reject-scalar",
            b"\x65hello".to_vec(),
            CborLimits {
                max_string_bytes: 4,
                ..CborLimits::default()
            },
            error(CborErrorKind::LimitExceeded, 0, 1, vec![]),
        ),
        Workload::rejection(
            "reject-events",
            vec![0x82, 0, 1],
            CborLimits {
                max_events: 2,
                ..CborLimits::default()
            },
            error(
                CborErrorKind::LimitExceeded,
                2,
                2,
                vec![CborPath::ArrayIndex(1)],
            ),
        ),
        Workload::rejection(
            "reject-simple",
            vec![0x82, 0, 0xf8, 0x18],
            CborLimits::default(),
            error(
                CborErrorKind::InvalidSimpleValue,
                2,
                4,
                vec![CborPath::ArrayIndex(1)],
            ),
        ),
    ];
    let collision = Value::Map(vec![
        (Value::Float16(0x3e00), Value::UInt(1)),
        (Value::Float32(0x3fc00000), Value::UInt(2)),
    ]);
    assert_eq!(
        reference::render_deterministic(&collision)
            .unwrap_err()
            .kind,
        reference::ReferenceErrorKind::KeyCollision
    );
    let mut stats = Shape::default();
    shape(&collision, 1, &mut stats);
    fixtures.push(Workload {
        id: "reject-key-collision",
        operation: Operation::RejectDeterministic,
        input: reference::render_ordinary(&collision).unwrap(),
        value: Some(kernel(&collision)),
        events: empty(),
        expected: Err(error(
            CborErrorKind::DeterministicKeyCollision,
            0,
            0,
            vec![],
        )),
        options: CborDecodeOptions::default(),
        shape: stats,
    });
    fixtures
}

fn event_storage(events: &[CborEvent]) -> (usize, usize, usize) {
    let mut bytes = std::mem::size_of_val(events);
    let mut identities = usize::from(!events.is_empty());
    let mut largest_payload = 0;
    for event in events {
        let payload = match event {
            CborEvent::Text(value) | CborEvent::TextChunk(value) => value.len(),
            CborEvent::Bytes(value) | CborEvent::ByteChunk(value) => value.len(),
            _ => 0,
        };
        bytes += payload;
        identities += usize::from(payload > 0);
        largest_payload = largest_payload.max(payload);
    }
    (bytes, identities, largest_payload)
}

fn modeled_counters(workload: &Workload) -> (usize, usize, usize) {
    // This model tracks retained owned fixtures and operation payload/result
    // identities. It excludes private stacks, capacity slack, sorting metadata,
    // oracle temporaries and other workloads. It is not an allocator/RSS peak.
    let stats = &workload.shape;
    let value_bytes = stats.nodes * std::mem::size_of::<CborValue>() + stats.payload_bytes;
    let retained_value =
        workload.value.is_some() || matches!(workload.expected, Ok(Observation::Value(_)));
    let (output, expected_events) = match &workload.expected {
        Ok(Observation::Bytes(bytes)) => (bytes.len(), &[][..]),
        Ok(Observation::Events(events)) => (0, &events[..]),
        _ => (0, &[][..]),
    };
    let retained_events = if workload.events.is_empty() {
        expected_events
    } else {
        &workload.events
    };
    let (event_bytes, event_ids, largest_event_payload) = event_storage(retained_events);
    let error_path = workload
        .expected
        .as_ref()
        .err()
        .map_or(0, |error| error.path.len());
    let baseline_bytes = workload.input.len()
        + usize::from(retained_value) * value_bytes
        + output
        + event_bytes
        + error_path * std::mem::size_of::<CborPath>();
    let baseline_ids = 1
        + usize::from(retained_value) * stats.payload_identities
        + usize::from(output > 0)
        + event_ids
        + usize::from(error_path > 0);
    let (copies, identities, operation_bytes) = match workload.operation {
        Operation::Parse | Operation::View => {
            (stats.payload_bytes, stats.payload_identities, value_bytes)
        }
        Operation::Encode => (output, 1, output),
        Operation::Deterministic => (output, stats.nodes, 2 * output),
        Operation::Reader => (
            workload.input.len() + 2 * stats.payload_bytes,
            1 + 2 * event_ids,
            (workload.input.len() + event_bytes).max(2 * event_bytes),
        ),
        Operation::Writer => (
            stats.payload_bytes + 2 * output,
            event_ids + 1,
            (output + largest_event_payload).max(2 * output),
        ),
        Operation::RejectParse => (
            0,
            usize::from(error_path > 0),
            error_path * std::mem::size_of::<CborPath>(),
        ),
        // The two independently specified float keys and two scalar values
        // yield four leaf segments totalling eight bytes before collision.
        Operation::RejectDeterministic => (0, 4, 8),
    };
    (
        copies * BATCH,
        baseline_ids + identities * BATCH,
        baseline_bytes + operation_bytes,
    )
}

#[test]
fn cbor_fixture_oracles_are_exact() {
    let fixtures = fixtures();
    assert_eq!(fixtures.len(), 15);
    let counter = |id| modeled_counters(fixtures.iter().find(|fixture| fixture.id == id).unwrap());
    let parse = counter("parse-maps-large");
    let view = counter("parse-view-large");
    assert_eq!(
        parse.0, view.0,
        "view still materializes payload during validation"
    );
    assert_eq!(
        parse.1 - view.1,
        85,
        "only the retained expected DOM differs"
    );
    assert_eq!(
        parse.2 - view.2,
        4666,
        "retained oracle storage is included in the model"
    );
    assert!(
        counter("reader-indefinite").0 > counter("parse-indefinite").0,
        "reader accounts for buffered input and event delivery clones"
    );
    assert!(
        counter("writer-indefinite").0 > counter("parse-indefinite").0,
        "writer accounts for event clones and both output buffers"
    );
    for fixture in fixtures {
        assert_eq!(
            fixture.run(),
            fixture.expected,
            "{} exact route oracle",
            fixture.id
        );
        let counters = modeled_counters(&fixture);
        if fixture.expected.is_err() {
            assert_eq!(counters.0, 0, "no successful result payload emitted");
        }
        assert!(counters.1 > 0 && counters.2 > 0);
        assert_eq!(
            modeled_counters(&fixture),
            counters,
            "{} stable model",
            fixture.id
        );
        println!(
            "fixture {}: {:?}, wire={}, modeled batch copies/identities/bytes={counters:?}",
            fixture.id,
            fixture.operation,
            fixture.input.len()
        );
    }
}

fn kernel(value: &Value) -> CborValue {
    match value {
        Value::Null => CborValue::Null,
        Value::Undefined => CborValue::Undefined,
        Value::Bool(value) => CborValue::Bool(*value),
        Value::Simple(value) => CborValue::Simple(*value),
        Value::UInt(value) => CborValue::UInt(*value),
        Value::Negative(value) => CborValue::Negative(*value),
        Value::Float16(bits) => CborValue::Float16(CborFloat16 { bits: *bits }),
        Value::Float32(bits) => CborValue::Float32(*bits),
        Value::Float64(bits) => CborValue::Float64(*bits),
        Value::Bytes(value) => CborValue::Bytes(value.clone()),
        Value::Text(value) => CborValue::Text(value.clone()),
        Value::Array(values) => CborValue::Array(values.iter().map(kernel).collect()),
        Value::Map(pairs) => CborValue::Map(
            pairs
                .iter()
                .map(|(key, value)| CborEntry {
                    key: kernel(key),
                    value: kernel(value),
                })
                .collect(),
        ),
        Value::Tag(number, value) => CborValue::Tag(CborTag {
            number: *number,
            value: Box::new(kernel(value)),
        }),
    }
}

fn rows(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|row| {
                Value::Map(vec![
                    (
                        Value::Text("name".into()),
                        Value::Text(format!("row-{row} café 🌍")),
                    ),
                    (
                        Value::Text("score".into()),
                        Value::Float64((row as f64 + 0.5).to_bits()),
                    ),
                    (
                        Value::Text("data".into()),
                        Value::Bytes(vec![row as u8; 64]),
                    ),
                    (Value::Text("ready".into()), Value::Bool(row % 2 == 0)),
                ])
            })
            .collect(),
    )
}

#[derive(Default, Debug)]
struct Shape {
    nodes: usize,
    depth: usize,
    arrays: usize,
    maps: usize,
    pairs: usize,
    tags: usize,
    payload_bytes: usize,
    payload_identities: usize,
}

fn shape(value: &Value, depth: usize, result: &mut Shape) {
    result.nodes += 1;
    result.depth = result.depth.max(depth);
    match value {
        Value::Array(values) => {
            result.arrays += 1;
            result.payload_identities += usize::from(!values.is_empty());
            for value in values {
                shape(value, depth + 1, result);
            }
        }
        Value::Map(pairs) => {
            result.maps += 1;
            result.pairs += pairs.len();
            result.payload_identities += usize::from(!pairs.is_empty());
            for (key, value) in pairs {
                shape(key, depth + 1, result);
                shape(value, depth + 1, result);
            }
        }
        Value::Tag(_, value) => {
            result.tags += 1;
            result.payload_identities += 1;
            shape(value, depth + 1, result);
        }
        Value::Text(value) => {
            result.payload_bytes += value.len();
            result.payload_identities += usize::from(!value.is_empty());
        }
        Value::Bytes(value) => {
            result.payload_bytes += value.len();
            result.payload_identities += usize::from(!value.is_empty());
        }
        _ => {}
    }
}

fn error(kind: CborErrorKind, start: usize, end: usize, path: Vec<CborPath>) -> CborError {
    CborError {
        kind,
        start_offset: start,
        end_offset: end,
        path,
    }
}

const WARMUPS: usize = 3;
const REPETITIONS: usize = 9;

fn counters(workload: &Workload) -> serde_json::Value {
    let (copies, allocations, memory) = modeled_counters(workload);
    let output = match &workload.expected {
        Ok(Observation::Bytes(bytes)) => bytes.len(),
        _ => 0,
    };
    let events = match &workload.expected {
        Ok(Observation::Events(events)) => events.len(),
        _ => workload.events.len(),
    };
    json!({
        "input_bytes": workload.input.len(),
        "output_bytes": output,
        "operations": BATCH,
        "bytes_copied": copies,
        "allocations": allocations,
        "logical_memory_bytes": memory,
        "nodes": workload.shape.nodes,
        "depth": workload.shape.depth,
        "arrays": workload.shape.arrays,
        "maps": workload.shape.maps,
        "map_pairs": workload.shape.pairs,
        "tags": workload.shape.tags,
        "chunks": if workload.id.contains("indefinite") { 4 } else { 0 },
        "event_count": events,
        "adversarial_rejections": if workload.expected.is_err() { BATCH } else { 0 },
        "terminal_open_streams": 0,
        "native_live_handles": null,
    })
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Parse => "materialized-parse",
            Self::View => "borrowed-parse-view",
            Self::Encode => "materialized-encode",
            Self::Deterministic => "deterministic-encode",
            Self::Reader => "stream-reader-events",
            Self::Writer => "stream-writer-events",
            Self::RejectParse | Self::RejectDeterministic => "adversarial-reject",
        }
    }
}

fn measure_process(process: usize) -> Vec<serde_json::Value> {
    assert!(
        (1..=3).contains(&process),
        "declared independent process ordinal"
    );
    let mut samples = Vec::new();
    for workload in fixtures() {
        assert_eq!(
            workload.run(),
            workload.expected,
            "{} before timing",
            workload.id
        );
        for _ in 0..WARMUPS {
            for _ in 0..BATCH {
                assert_eq!(workload.run(), workload.expected, "{} warmup", workload.id);
            }
        }
        for repetition in 0..REPETITIONS {
            let start = Instant::now();
            for _ in 0..BATCH {
                let result = workload.run();
                assert_eq!(
                    result.is_err(),
                    workload.expected.is_err(),
                    "{} status",
                    workload.id
                );
                drop(black_box(result));
            }
            let nanos = start.elapsed().as_nanos();
            assert!(nanos > 0, "monotonic batch must be measurable");
            assert_eq!(
                workload.run(),
                workload.expected,
                "{} after timing",
                workload.id
            );
            samples.push(json!({
                "workload_id": workload.id,
                "operation": workload.operation.label(),
                "process": process,
                "repetition": repetition,
                "nanos": nanos,
                "dispatch": "scalar-fixed-target",
                "counters": counters(&workload),
            }));
        }
    }
    samples
}

#[test]
fn cbor_stream_lifecycle_is_terminal() {
    let (_, input) = indefinite_fixture();
    let view = cbor::parse_view(&input, CborDecodeOptions::default()).unwrap();
    assert_eq!(view.as_bytes().as_ptr(), input.as_ptr());
    let expected = indefinite_events();
    let mut reader =
        CborReader::from_reader(std::io::Cursor::new(&input), CborDecodeOptions::default())
            .unwrap();
    let mut observed = Vec::new();
    while let Some(event) = reader.next().unwrap() {
        observed.push(event);
    }
    assert_eq!(observed, expected);
    reader.finish().unwrap();
    assert_eq!(
        reader.next().unwrap_err(),
        error(CborErrorKind::Closed, 0, 0, vec![])
    );
    let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
    for event in expected {
        writer.write(event).unwrap();
    }
    assert_eq!(writer.finish().unwrap(), input);
    assert_eq!(
        writer.finish().unwrap_err(),
        error(CborErrorKind::Closed, 0, 0, vec![])
    );
}

#[test]
fn cbor_sample_batches_preserve_exact_oracles_and_ordinals() {
    let rows = measure_process(1);
    assert_eq!(rows.len(), 15 * REPETITIONS);
    for workload in fixtures() {
        let selected: Vec<_> = rows
            .iter()
            .filter(|row| row["workload_id"] == workload.id)
            .collect();
        assert_eq!(selected.len(), REPETITIONS);
        for (repetition, row) in selected.into_iter().enumerate() {
            assert_eq!(row["process"], 1);
            assert_eq!(row["repetition"], repetition);
            assert_eq!(row["counters"], counters(&workload));
            assert!(row["nanos"].as_u64().unwrap() > 0);
        }
    }
}

#[test]
fn cbor_performance_probe() {
    if std::env::var("TONDO_CBOR_PERF_RUN").as_deref() != Ok("1") {
        return;
    }
    let process = std::env::var("TONDO_CBOR_PERF_PROCESS")
        .expect("explicit process ordinal")
        .parse::<usize>()
        .expect("integer process ordinal");
    for sample in measure_process(process) {
        eprintln!("TONDO_CBOR_PERF\t{sample}");
    }
}
