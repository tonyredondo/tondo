//! Bounded scalar-kernel performance probe. The runner invokes this test in
//! independent processes after the separate TOML reference-model suite passes.

use std::hint::black_box;
use std::time::Instant;

use serde_json::json;
use tondo_stdlib::toml::{
    self, TomlError, TomlErrorKind, TomlEvent, TomlLimits, TomlOptions, TomlScalar, TomlValue,
};

const BATCH: usize = 16;
const WARMUPS: usize = 3;
const REPETITIONS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Parse,
    ParseView,
    Encode,
    Canonical,
    Reader,
    Writer,
    Reject,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Parse => "materialized-parse",
            Self::ParseView => "borrowed-parse-view",
            Self::Encode => "materialized-encode",
            Self::Canonical => "canonical-encode",
            Self::Reader => "stream-reader-events",
            Self::Writer => "stream-writer-events",
            Self::Reject => "adversarial-reject",
        }
    }
}

#[derive(Debug, PartialEq)]
enum Observation {
    Value(TomlValue),
    ViewBytes(usize),
    Bytes(Vec<u8>),
    Events(Vec<TomlEvent>),
}

struct Workload {
    id: &'static str,
    size_class: &'static str,
    operation: Operation,
    input: Vec<u8>,
    options: TomlOptions,
    value: Option<TomlValue>,
    events: Vec<TomlEvent>,
    expected: Result<Observation, TomlError>,
}

impl Workload {
    fn valid(
        id: &'static str,
        size_class: &'static str,
        operation: Operation,
        input: Vec<u8>,
    ) -> Self {
        let options = TomlOptions::default();
        let value = toml::parse(&input, options).expect(id);
        let mut reader = toml::TomlReader::from_bytes(&input, options).expect(id);
        let mut events = Vec::new();
        loop {
            let event = reader.next().expect(id).expect("StreamEnd is required");
            let done = event == TomlEvent::StreamEnd;
            events.push(event);
            if done {
                break;
            }
        }
        reader.finish().expect(id);
        let expected = match operation {
            Operation::Parse => Observation::Value(value.clone()),
            Operation::ParseView => Observation::ViewBytes(input.len()),
            Operation::Encode => Observation::Bytes(toml::encode(&value, options).expect(id)),
            Operation::Canonical => {
                Observation::Bytes(toml::encode_canonical(&value, options.limits).expect(id))
            }
            Operation::Reader => Observation::Events(events.clone()),
            Operation::Writer => {
                let mut writer = toml::TomlWriter::to_writer(options).expect(id);
                for event in &events {
                    writer.write(event.clone()).expect(id);
                }
                Observation::Bytes(writer.finish().expect(id))
            }
            Operation::Reject => panic!("valid workload cannot reject"),
        };
        Self {
            id,
            size_class,
            operation,
            input,
            options,
            value: Some(value),
            events,
            expected: Ok(expected),
        }
    }

    fn rejecting(
        id: &'static str,
        input: &'static [u8],
        limits: TomlLimits,
        kind: TomlErrorKind,
    ) -> Self {
        let options = TomlOptions::create(limits);
        let error = toml::parse(input, options).expect_err(id);
        assert_eq!(error.kind, kind, "{id}");
        // A valid document under default limits remains a retained fixture;
        // malformed syntax has no materialized fixture value.
        let value = toml::parse(input, TomlOptions::default()).ok();
        Self {
            id,
            size_class: "small",
            operation: Operation::Reject,
            input: input.to_vec(),
            options,
            value,
            events: Vec::new(),
            expected: Err(error),
        }
    }

    fn run(&self) -> Result<Observation, TomlError> {
        match self.operation {
            Operation::Parse => toml::parse(&self.input, self.options).map(Observation::Value),
            Operation::ParseView => toml::parse_view(&self.input, self.options)
                .map(|view| Observation::ViewBytes(view.bytes().len())),
            Operation::Encode => toml::encode(self.value.as_ref().expect(self.id), self.options)
                .map(Observation::Bytes),
            Operation::Canonical => {
                toml::encode_canonical(self.value.as_ref().expect(self.id), self.options.limits)
                    .map(Observation::Bytes)
            }
            Operation::Reader => {
                let mut reader = toml::TomlReader::from_bytes(&self.input, self.options)?;
                let mut events = Vec::new();
                loop {
                    let event = reader.next()?.expect("StreamEnd is required");
                    let done = event == TomlEvent::StreamEnd;
                    events.push(event);
                    if done {
                        break;
                    }
                }
                reader.finish()?;
                Ok(Observation::Events(events))
            }
            Operation::Writer => {
                let mut writer = toml::TomlWriter::to_writer(self.options)?;
                for event in &self.events {
                    writer.write(event.clone())?;
                }
                writer.finish().map(Observation::Bytes)
            }
            Operation::Reject => toml::parse(&self.input, self.options).map(Observation::Value),
        }
    }

    fn verify(&self) {
        assert_eq!(self.run(), self.expected, "{} exact oracle", self.id);
        if self.operation != Operation::Reject
            && let Some(value) = &self.value
        {
            let canonical = toml::encode_canonical(value, self.options.limits).expect(self.id);
            let round_trip = toml::parse(&canonical, self.options).expect(self.id);
            assert_eq!(
                toml::encode_canonical(&round_trip, self.options.limits).expect(self.id),
                canonical,
                "{} canonical round trip",
                self.id
            );
        }
    }
}

fn fixtures() -> Vec<Workload> {
    let core = b"title = \"Tondo\"\nanswer = 42\nready = true\nratio = 1.25\n".to_vec();
    let nested = br#"title = "Caf\u00e9 \U0001F30D"
numbers = [1, 0x10, -2.5, true]
inline = { name = "nested", flags = [true, false] }
[service]
name = "alpha"
date = 2026-09-24
time = 07:32:00.123456789
[service.extra]
enabled = true
"#
    .to_vec();
    let temporal = br#"created = 2026-09-24T07:32:00+02:00
local = 2026-09-24T07:32:00
date = 2026-09-24
time = 07:32:00.123456789
[[servers]]
name = "alpha"
ports = [80, 443]
[[servers]]
name = "beta"
ports = [8080]
"#
    .to_vec();
    let mut large = String::from("title = \"Tondo rows\"\n");
    for row in 0..32 {
        large.push_str(&format!(
            "[[rows]]\nname = \"n{row} \u{1f30d}\"\nscore = {row}.5\nflags = [true, false, {row}]\n"
        ));
    }
    let large = large.into_bytes();
    let canonical = b"z = 9\na = 1\n".to_vec();
    let defaults = TomlLimits::default();
    vec![
        Workload::valid("parse-core-small", "small", Operation::Parse, core),
        Workload::valid(
            "parse-nested-medium",
            "medium",
            Operation::Parse,
            nested.clone(),
        ),
        Workload::valid("parse-rows-large", "large", Operation::Parse, large),
        Workload::valid(
            "parse-temporal-rows",
            "medium",
            Operation::Parse,
            temporal.clone(),
        ),
        Workload::valid(
            "parse-view-nested",
            "medium",
            Operation::ParseView,
            nested.clone(),
        ),
        Workload::valid("encode-nested", "medium", Operation::Encode, nested),
        Workload::valid("encode-canonical", "small", Operation::Canonical, canonical),
        Workload::valid(
            "reader-temporal-events",
            "medium",
            Operation::Reader,
            temporal.clone(),
        ),
        Workload::valid(
            "writer-temporal-events",
            "medium",
            Operation::Writer,
            temporal,
        ),
        Workload::rejecting(
            "reject-depth",
            b"a = [1]\n",
            TomlLimits {
                max_depth: 1,
                ..defaults
            },
            TomlErrorKind::DepthLimit,
        ),
        Workload::rejecting(
            "reject-nodes",
            b"a = [1]\n",
            TomlLimits {
                max_nodes: 2,
                ..defaults
            },
            TomlErrorKind::NodeLimit,
        ),
        Workload::rejecting(
            "reject-scalar",
            b"value = \"long\"\n",
            TomlLimits {
                max_scalar_bytes: 2,
                ..defaults
            },
            TomlErrorKind::ScalarLimit,
        ),
        Workload::rejecting(
            "reject-duplicate-table",
            b"[a]\nx = 1\n[a]\ny = 2\n",
            defaults,
            TomlErrorKind::DuplicateTable,
        ),
    ]
}

#[derive(Default)]
struct Shape {
    nodes: usize,
    tables: usize,
    array_table_rows: usize,
    depth: usize,
    payload_bytes: usize,
    logical_allocations: usize,
}

fn measure_shape(value: &TomlValue, depth: usize, inside_array: bool, shape: &mut Shape) {
    shape.nodes += 1;
    shape.depth = shape.depth.max(depth);
    match value {
        TomlValue::Table(members) => {
            shape.tables += 1;
            shape.array_table_rows += usize::from(inside_array);
            shape.logical_allocations += usize::from(!members.is_empty());
            for member in members {
                shape.payload_bytes += member.key.len();
                shape.logical_allocations += 1;
                measure_shape(&member.value, depth + 1, false, shape);
            }
        }
        TomlValue::Array(values) => {
            shape.logical_allocations += usize::from(!values.is_empty());
            for item in values {
                measure_shape(item, depth + 1, true, shape);
            }
        }
        TomlValue::Text(text) => {
            shape.payload_bytes += text.len();
            shape.logical_allocations += 1;
        }
        _ => {}
    }
}

fn event_payload_bytes(events: &[TomlEvent]) -> usize {
    events
        .iter()
        .map(|event| match event {
            TomlEvent::TableStart(path)
            | TomlEvent::ArrayTableStart(path)
            | TomlEvent::Key(path) => path.iter().map(String::len).sum(),
            TomlEvent::Scalar(TomlScalar::Text(text)) => text.len(),
            _ => 0,
        })
        .sum()
}

fn counters(workload: &Workload) -> serde_json::Value {
    let mut shape = Shape::default();
    if let Some(value) = &workload.value {
        measure_shape(value, 1, false, &mut shape);
    }
    let output_bytes = match &workload.expected {
        Ok(Observation::Bytes(bytes)) => bytes.len(),
        _ => 0,
    };
    let event_bytes = event_payload_bytes(&workload.events);
    let operation_copy_bytes = match workload.operation {
        Operation::Parse | Operation::ParseView => shape.payload_bytes,
        Operation::Encode | Operation::Canonical => output_bytes,
        Operation::Reader => event_bytes,
        Operation::Writer => event_bytes + output_bytes,
        Operation::Reject => 0,
    };
    let operation_allocations = match workload.operation {
        Operation::Parse | Operation::ParseView => shape.logical_allocations,
        Operation::Encode | Operation::Canonical => 1,
        Operation::Reader => shape.logical_allocations + workload.events.len() + 1,
        Operation::Writer => workload.events.len() + 1,
        Operation::Reject => 0,
    };
    let retained_value_bytes = shape.nodes * std::mem::size_of::<TomlValue>()
        + shape.payload_bytes
        + shape.tables * std::mem::size_of::<Vec<toml::TomlMember>>();
    let modeled_peak = workload.input.len()
        + retained_value_bytes
        + event_bytes
        + output_bytes
        + operation_copy_bytes;
    json!({
        "input_bytes": workload.input.len(),
        "output_bytes": output_bytes,
        "operations": BATCH,
        "bytes_copied": operation_copy_bytes * BATCH,
        "allocations": 1 + shape.logical_allocations + usize::from(!workload.events.is_empty())
            + usize::from(output_bytes > 0) + BATCH * operation_allocations,
        "logical_memory_bytes": modeled_peak,
        "live_handles": 0,
        "depth": shape.depth.max(1),
        "nodes": shape.nodes,
        "tables": shape.tables,
        "array_table_rows": shape.array_table_rows,
        "event_count": workload.events.len(),
        "adversarial_rejections": if workload.operation == Operation::Reject { BATCH } else { 0 },
    })
}

#[test]
fn fixture_oracles_are_exact() {
    let fixtures = fixtures();
    assert_eq!(fixtures.len(), 13);
    for workload in &fixtures {
        workload.verify();
    }
    let core = fixtures
        .iter()
        .find(|item| item.id == "parse-core-small")
        .unwrap();
    assert_eq!(
        core.value.as_ref().unwrap(),
        &TomlValue::Table(vec![
            toml::TomlMember {
                key: "title".into(),
                value: TomlValue::Text("Tondo".into())
            },
            toml::TomlMember {
                key: "answer".into(),
                value: TomlValue::Int(42)
            },
            toml::TomlMember {
                key: "ready".into(),
                value: TomlValue::Bool(true)
            },
            toml::TomlMember {
                key: "ratio".into(),
                value: TomlValue::Float(1.25)
            },
        ])
    );
    let canonical = fixtures
        .iter()
        .find(|item| item.id == "encode-canonical")
        .unwrap();
    assert_eq!(
        canonical.expected,
        Ok(Observation::Bytes(b"a = 1\nz = 9\n".to_vec()))
    );
    let large = fixtures
        .iter()
        .find(|item| item.id == "parse-rows-large")
        .unwrap();
    let mut shape = Shape::default();
    measure_shape(large.value.as_ref().unwrap(), 1, false, &mut shape);
    assert_eq!(shape.array_table_rows, 32);
    assert!(shape.tables >= 33 && shape.nodes > 150);
    let temporal = fixtures
        .iter()
        .find(|item| item.id == "parse-temporal-rows")
        .unwrap();
    let TomlValue::Table(members) = temporal.value.as_ref().unwrap() else {
        panic!("root table")
    };
    assert!(matches!(members[0].value, TomlValue::OffsetDateTime(_)));
    assert!(matches!(members[1].value, TomlValue::LocalDateTime(_)));
    assert!(matches!(members[2].value, TomlValue::LocalDate(_)));
    assert!(matches!(members[3].value, TomlValue::LocalTime(_)));
}

#[test]
fn toml_performance_probe() {
    if std::env::var_os("TONDO_TOML_PERF_RUN").is_none() {
        return;
    }
    let fixtures = fixtures();
    for workload in &fixtures {
        workload.verify();
        for _ in 0..WARMUPS {
            for _ in 0..BATCH {
                assert_eq!(workload.run(), workload.expected, "{} warmup", workload.id);
            }
        }
        for _ in 0..REPETITIONS {
            let start = Instant::now();
            for _ in 0..BATCH {
                let result = workload.run();
                assert_eq!(
                    result.is_err(),
                    workload.expected.is_err(),
                    "{} status",
                    workload.id
                );
                let _ = black_box(result);
            }
            let nanos = start.elapsed().as_nanos().max(1);
            assert_eq!(
                workload.run(),
                workload.expected,
                "{} post-sample",
                workload.id
            );
            let counters = counters(workload);
            let row = json!({
                "workload_id": workload.id,
                "operation": workload.operation.label(),
                "size_class": workload.size_class,
                "nanos": nanos,
                "dispatch": "scalar-fixed-target",
                "counters": counters,
            });
            eprintln!("TONDO_TOML_PERF\t{row}");
        }
    }
}
