//! Common UUID inputs and native Rust-kernel observations for conformance.
//! Provider replay is a test fixture, not a native UUID provider ABI.

use std::collections::BTreeMap;

use tondo_stdlib::json::{self, JsonValue};
use tondo_stdlib::uuid::{Uuid, UuidError, UuidErrorKind as Kind, UuidLimits};

pub const CASES: [&str; 5] = [
    "retained-values",
    "retained-refusals",
    "core-value-laws",
    "provider-transcript",
    "clock-laws",
];
pub const CORPUS: &[u8] =
    include_bytes!("../../../tondo-reliability/tests/fixtures/uuid-cases.json");
pub const KERNEL_ONLY: [&str; 2] = ["v4-length-17", "v5-name-limit"];

#[derive(Debug, Clone)]
pub enum Operation {
    Parse(String),
    Bytes(Vec<u8>),
    V4(Vec<u8>),
    V5 {
        namespace: String,
        name: Vec<u8>,
        limit: usize,
    },
    V7 {
        milliseconds: i128,
        entropy: Vec<u8>,
    },
}

#[derive(Debug, Clone)]
pub struct Vector {
    pub id: String,
    pub operation: Operation,
    pub expected: String,
}

fn field<'a>(value: &'a JsonValue, key: &str) -> &'a JsonValue {
    let JsonValue::Object(items) = value else {
        panic!("fixture object")
    };
    &items
        .iter()
        .find(|item| item.key == key)
        .expect("fixture field")
        .value
}

fn text<'a>(value: &'a JsonValue, key: &str) -> &'a str {
    let JsonValue::String(value) = field(value, key) else {
        panic!("fixture text")
    };
    value
}

fn integer(value: &JsonValue) -> i128 {
    let JsonValue::Number(value) = value else {
        panic!("fixture integer")
    };
    i128::from(value.to_int().unwrap())
}

pub fn hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| char::from(byte).to_digit(16).expect("fixture hex") as u8;
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

pub fn vectors(valid: bool) -> Vec<Vector> {
    let corpus = json::parse(CORPUS).expect("retained UUID corpus");
    let JsonValue::Array(rows) = field(&corpus, if valid { "valid" } else { "invalid" }) else {
        panic!("fixture array")
    };
    assert_eq!(rows.len(), if valid { 17 } else { 37 });
    rows.iter()
        .map(|row| {
            let operation = match text(row, "operation") {
                "parse" => Operation::Parse(text(row, "text").into()),
                "bytes" => Operation::Bytes(hex(text(row, "bytes_hex"))),
                "v4" => Operation::V4(hex(text(row, "entropy_hex"))),
                "v5" => Operation::V5 {
                    namespace: text(row, "namespace").into(),
                    name: hex(text(row, "name_hex")),
                    limit: integer(field(row, "name_limit")).try_into().unwrap(),
                },
                "v7" => Operation::V7 {
                    milliseconds: integer(field(row, "milliseconds")),
                    entropy: hex(text(row, "entropy_hex")),
                },
                unknown => panic!("unregistered UUID operation {unknown}"),
            };
            let expected = if valid {
                text(row, "value").into()
            } else {
                let mut error = text(row, "error").to_owned();
                if !matches!(field(row, "offset"), JsonValue::Null) {
                    error.push_str(&format!(" at byte {}", integer(field(row, "offset"))));
                }
                error
            };
            Vector {
                id: text(row, "id").into(),
                operation,
                expected,
            }
        })
        .collect()
}

pub fn common_vectors(valid: bool) -> Vec<Vector> {
    let rows: Vec<_> = vectors(valid)
        .into_iter()
        .filter(|row| !KERNEL_ONLY.contains(&row.id.as_str()))
        .collect();
    assert_eq!(rows.len(), if valid { 17 } else { 35 });
    rows
}

pub fn kernel(operation: &Operation) -> Result<Uuid, UuidError> {
    match operation {
        Operation::Parse(text) => Uuid::parse(text),
        Operation::Bytes(bytes) => Uuid::from_bytes(bytes),
        Operation::V4(entropy) => Uuid::v4(entropy),
        Operation::V5 {
            namespace,
            name,
            limit,
        } => Uuid::v5(
            Uuid::parse(namespace).unwrap(),
            name,
            UuidLimits {
                max_name_bytes: *limit,
            },
        ),
        Operation::V7 {
            milliseconds,
            entropy,
        } => Uuid::v7(*milliseconds, entropy),
    }
}

pub fn observe_value(value: Uuid) -> String {
    let mut observation = format!("ok:{}", value.try_to_string().unwrap());
    for byte in value.to_bytes() {
        observation.push_str(&format!(":{byte}"));
    }
    observation.push_str(&format!(
        ":{}:{:?}:{}:{}",
        value.version(),
        value.variant(),
        value.is_nil(),
        value.is_max()
    ));
    observation
}

pub fn observe(value: Result<Uuid, UuidError>) -> String {
    match value {
        Ok(value) => observe_value(value),
        Err(error) => format!("err:{error}"),
    }
}

#[derive(Debug, Clone)]
pub struct Providers {
    pub clocks: Vec<Result<i128, Kind>>,
    pub entropy: Vec<Result<Vec<u8>, Kind>>,
}

pub const TRANSCRIPT: [u8; 16] = [4, 7, 7, 4, 7, 7, 4, 7, 4, 7, 4, 7, 7, 4, 7, 4];

pub fn transcript_providers() -> Providers {
    let r4 = hex("919108f752d133205bacf847db4148a8");
    let r7 = hex("0cc318c4dc0c0c07398f");
    Providers {
        clocks: vec![
            Ok(-1),
            Err(Kind::ClockFailure),
            Ok(1_645_557_742_000),
            Ok(1_645_557_742_000),
            Ok(0),
            Err(Kind::ClockUnavailable),
            Ok(1_645_557_742_001),
            Ok(1_645_557_742_002),
        ],
        entropy: vec![
            Err(Kind::EntropyFailure),
            Ok(r4.clone()),
            Ok(r7.clone()),
            Ok(r7.clone()),
            Ok(vec![]),
            Ok(vec![0; 10]),
            Err(Kind::EntropyUnavailable),
            Ok(r4),
            Ok(r7),
        ],
    }
}

pub fn clock_providers() -> Providers {
    Providers {
        clocks: vec![Ok(100), Ok(100), Ok(99), Ok((1_i128 << 48) - 1), Ok(0)],
        entropy: vec![Ok(hex("0cc318c4dc0c0c07398f")); 5],
    }
}

/// A finite test replay around the actual native Rust kernel, with no OS path.
struct KernelReplay {
    providers: Providers,
    clocks: usize,
    entropy: usize,
}

impl KernelReplay {
    fn new(providers: Providers) -> Self {
        Self {
            providers,
            clocks: 0,
            entropy: 0,
        }
    }

    fn entropy(&mut self) -> Result<Vec<u8>, UuidError> {
        let row = self
            .providers
            .entropy
            .get(self.entropy)
            .cloned()
            .ok_or_else(|| UuidError::new(Kind::EntropyUnavailable, None))?;
        self.entropy += 1;
        row.map_err(|kind| UuidError::new(kind, None))
    }

    fn generate(&mut self, version: u8) -> Result<Uuid, UuidError> {
        match version {
            4 => Uuid::v4(&self.entropy()?),
            7 => {
                let row = self
                    .providers
                    .clocks
                    .get(self.clocks)
                    .copied()
                    .ok_or_else(|| UuidError::new(Kind::ClockUnavailable, None))?;
                self.clocks += 1;
                let milliseconds = row.map_err(|kind| UuidError::new(kind, None))?;
                if !(0..=(1_i128 << 48) - 1).contains(&milliseconds) {
                    return Err(UuidError::new(Kind::TimestampOutOfRange, None));
                }
                Uuid::v7(milliseconds, &self.entropy()?)
            }
            unknown => panic!("unregistered fixture version {unknown}"),
        }
    }
}

pub fn run_kernel_case(id: &str) -> Vec<String> {
    match id {
        "retained-values" | "retained-refusals" => {
            let valid = id == "retained-values";
            common_vectors(valid)
                .iter()
                .map(|row| {
                    let actual = kernel(&row.operation);
                    let text = match actual {
                        Ok(value) => value.try_to_string().unwrap(),
                        Err(error) => error.to_string(),
                    };
                    assert_eq!(text, row.expected, "{}", row.id);
                    format!("{}|{}", row.id, observe(actual))
                })
                .collect()
        }
        "core-value-laws" => {
            let nil = Uuid::nil();
            let maximum = Uuid::max();
            let keys = BTreeMap::from([(nil, 0), (maximum, 1)]);
            let copy = maximum;
            let roundtrip = Uuid::from_bytes(&copy.to_bytes()).unwrap();
            assert_eq!(copy, roundtrip);
            vec![
                observe_value(nil),
                observe_value(roundtrip),
                format!(
                    "laws:{}:{}:{}:{}:{}:{}",
                    keys[&nil],
                    keys[&roundtrip],
                    nil.compare(maximum),
                    maximum.compare(nil),
                    maximum.compare(copy),
                    nil != maximum
                ),
            ]
        }
        "provider-transcript" => {
            let mut replay = KernelReplay::new(transcript_providers());
            let observations = TRANSCRIPT
                .iter()
                .enumerate()
                .map(|(index, version)| format!("{index}|{}", observe(replay.generate(*version))))
                .collect();
            assert_eq!((replay.clocks, replay.entropy), (8, 9));
            observations
        }
        "clock-laws" => {
            let mut replay = KernelReplay::new(clock_providers());
            let values: Vec<_> = (0..5).map(|_| replay.generate(7).unwrap()).collect();
            let mut observations: Vec<_> = values.iter().copied().map(observe_value).collect();
            observations.push(format!(
                "ordering:{}:{}:{}:{}",
                values[0].compare(values[1]),
                values[1].compare(values[2]),
                values[2].compare(values[3]),
                values[3].compare(values[4])
            ));
            assert_eq!((replay.clocks, replay.entropy), (5, 5));
            observations
        }
        unknown => panic!("unregistered UUID case {unknown}"),
    }
}
