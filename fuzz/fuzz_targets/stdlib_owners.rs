#![no_main]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;
use tondo_compiler::artifact::sha256;
use tondo_compiler::meta_robust::probe_meta_protocols;
use tondo_compiler::reflect::{
    ReflectCatalog, ReflectFieldTemplate, ReflectPrimitiveKind, ReflectTypeKind,
    ReflectTypeTemplate,
};
use tondo_reliability::harness::check;
use tondo_stdlib::format::{self, FormatLimits};
use tondo_stdlib::io::{self, IoLimits, SliceReader, VecWriter};
use tondo_stdlib::math;
use tondo_stdlib::path::Path;
use tondo_stdlib::serialization::{
    self, Deserializer, Event, EventDeserializer, EventSerializer, Limits, Serializer,
};
use tondo_stdlib::testing::{self, DiffLimits, FloatTolerance};

/// The owner order is part of the fuzz contract. The first byte selects one
/// route; the remaining bytes are payload. Keeping this table explicit makes
/// coverage and corpus ownership auditable without creating 22 near-identical
/// cargo-fuzz binaries.
pub const OWNER_ROUTES: [&str; 22] = [
    "std.async",
    "std.bytes",
    "std.collections",
    "std.console",
    "std.core",
    "std.env",
    "std.format",
    "std.fs",
    "std.io",
    "std.iter",
    "std.json",
    "std.math",
    "std.messagepack",
    "std.meta",
    "std.path",
    "std.process",
    "std.protobuf",
    "std.reflect",
    "std.serialization",
    "std.testing",
    "std.text",
    "std.time",
];

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024;

fn source_probe(owner: &str, source: &str) {
    assert!(source.len() <= MAX_SOURCE_BYTES);
    let observation = check(&format!("stdlib-fuzz-{owner}"), source)
        .unwrap_or_else(|error| panic!("{owner} probe failed to execute: {error}"));
    assert!(
        observation.accepted,
        "{owner} probe was rejected: {}",
        observation.diagnostics_jsonl
    );
}

fn route_std_async() {
    source_probe(
        "std.async",
        "import std.async\nfn ready(): Int suspends { 1 }\nfn main() {\n    let value = ready()\n    _ = value\n}\n",
    );
}

fn route_std_bytes(input: &[u8]) {
    let bounded = &input[..input.len().min(MAX_INPUT_BYTES)];
    let _ = std::str::from_utf8(bounded);
    let _ = String::from_utf8(bounded.to_vec());
    let round_trip = bounded.to_vec();
    assert_eq!(round_trip.as_slice(), bounded);
}

fn route_std_collections(input: &[u8]) {
    let count = input.first().copied().unwrap_or_default() as usize % 16;
    let mut values = Vec::with_capacity(count);
    values.extend((0..count).map(|value| value as i64));
    let mut map = BTreeMap::new();
    let mut set = BTreeSet::new();
    for value in &values {
        map.insert(*value, value.wrapping_mul(2));
        set.insert(*value);
    }
    assert_eq!(map.len(), set.len());
    assert!(values.iter().all(|value| set.contains(value)));
}

fn route_std_console() {
    source_probe(
        "std.console",
        "import std.console\nfn main() { console.print(\"fuzz\") }\n",
    );
}

fn route_std_core() {
    source_probe(
        "std.core",
        "fn exercise(): Int {\n    let value: Int? = some(1)\n    value.unwrapOr(0)\n}\n",
    );
}

fn route_std_env() {
    source_probe(
        "std.env",
        "import std.env\nfn main(): !env.EnvError {\n    let snapshot = env.snapshot()?\n    _ = snapshot\n}\n",
    );
}

fn route_std_format(input: &[u8]) {
    let value = i128::from(input.first().copied().unwrap_or_default());
    let limits = FormatLimits {
        max_bytes: (input.get(1).copied().unwrap_or(16) as usize).saturating_add(1),
    };
    let _ = format::format(&value, limits);
    let values = ["a", "b", "c"];
    let _ = format::join(&values, ",", limits);
}

fn route_std_fs() {
    source_probe(
        "std.fs",
        "import std.path\nimport std.fs\nfn main(): !(path.PathError | fs.FsError) {\n    let file_path = path.fromString(\"Cargo.toml\")?\n    let contents = fs.readAll(file_path)?\n    _ = contents\n}\n",
    );
}

fn route_std_io(input: &[u8]) {
    let chunk = (input.first().copied().unwrap_or(1) as usize % 8).max(1);
    let bytes = input[..input.len().min(1024)].to_vec();
    let mut reader = SliceReader::new(bytes.clone(), chunk).expect("positive chunk");
    let limits = IoLimits {
        max_bytes: bytes.len().saturating_add(1),
        max_read: chunk,
    };
    let read = io::read_all(&mut reader, limits).expect("bounded reader remains valid");
    assert_eq!(read, bytes);
    let mut writer = VecWriter::with_max_write(chunk).expect("positive writer chunk");
    io::write_all(&mut writer, &bytes).expect("bounded writer remains valid");
    assert_eq!(writer.bytes(), bytes.as_slice());
    assert!(writer.flushed());
}

fn route_std_iter() {
    source_probe(
        "std.iter",
        "import std.collections\nimport std.iter\nfn plus_one(value: Int): Int { value + 1 }\nfn main(): Int {\n    let values = [1, 2].map(plus_one).take(1).collect()\n    _ = values\n    0\n}\n",
    );
}

fn route_std_json(input: &[u8]) {
    let _ = tondo_stdlib::json::validate(input);
    if let Ok(value) = tondo_stdlib::json::parse(input) {
        let encoded = tondo_stdlib::json::encode(&value).expect("parsed JSON encodes");
        let _ = tondo_stdlib::json::validate(&encoded);
    }
}

fn route_std_math(input: &[u8]) {
    let byte = input.first().copied().unwrap_or_default();
    let value = f64::from(byte) - 128.0;
    let _ = math::floor(value);
    let _ = math::ceil(value);
    let _ = math::round(value);
    let _ = math::truncate(value);
    let _ = math::sqrt(value);
    let _ = math::fma(value, 2.0, 1.0);
}

fn route_std_messagepack(input: &[u8]) {
    let _ = tondo_stdlib::messagepack::validate(input, Default::default());
    if let Ok(value) = tondo_stdlib::messagepack::parse(input, Default::default()) {
        let encoded = tondo_stdlib::messagepack::encode_deterministic(&value)
            .expect("parsed MessagePack encodes");
        let _ = tondo_stdlib::messagepack::validate(&encoded, Default::default());
    }
}

fn route_std_meta(input: &[u8]) {
    let first = probe_meta_protocols(input);
    let second = probe_meta_protocols(input);
    assert_eq!(first, second);
}

fn route_std_path(input: &[u8]) {
    let input = &input[..input.len().min(1024)];
    if let Ok(path) = Path::from_bytes(input) {
        let snapshot = path.to_bytes();
        assert_eq!(snapshot, input);
        let _ = path.parent();
        let _ = path.file_name();
        let _ = path.extension();
        let _ = path.to_string();
    }
}

fn route_std_process() {
    source_probe(
        "std.process",
        "import std.process\nfn main() {\n    let command = process.command(\"true\")\n    _ = command\n}\n",
    );
}

fn route_std_protobuf(input: &[u8]) {
    let _ = tondo_stdlib::protobuf::validate::<()>(input, Default::default());
    let mut offset = 0;
    let _ = tondo_stdlib::protobuf::decode_varint(input, &mut offset);
    let _ = tondo_stdlib::protobuf::decode_fields(input);
}

fn route_std_reflect(input: &[u8]) {
    let suffix = input.first().copied().unwrap_or_default();
    let name = format!("std.fuzz.Record{suffix}");
    let mut catalog = ReflectCatalog::default();
    catalog
        .insert(
            ReflectTypeTemplate::new(
                "std.Int",
                ReflectTypeKind::Primitive(ReflectPrimitiveKind::Int),
            )
            .expect("valid reflection primitive"),
        )
        .expect("unique reflection primitive");
    let field = ReflectFieldTemplate::new("value", "std.Int", 0, None::<String>, true)
        .expect("valid reflection field");
    let template = ReflectTypeTemplate::new(&name, ReflectTypeKind::Record)
        .expect("valid reflection type")
        .fields([field]);
    catalog.insert(template).expect("unique reflection type");
    let artifact_hash = sha256(name.as_bytes());
    let metadata =
        tondo_compiler::reflect::ReflectMetadata::link(&artifact_hash, [&name], &catalog)
            .expect("closed reflection catalog");
    assert_eq!(metadata.retained_len(), 2);
    let root = metadata.roots()[0];
    assert_eq!(metadata.type_info(root).unwrap().qualified_name(), name);
    let _ = ReflectPrimitiveKind::Int;
}

fn route_std_serialization(input: &[u8]) {
    let limits = Limits {
        max_depth: 8,
        max_events: 32,
        max_bytes: 1024,
        max_container_items: 16,
    };
    let mut serializer = EventSerializer::new(limits);
    serializer
        .write_event(Event::String(String::from_utf8_lossy(input).into_owned()))
        .expect("bounded event accepted");
    let events = serializer.finish().expect("scalar event is balanced");
    let mut deserializer = EventDeserializer::new(&events, limits).expect("bounded events");
    assert!(deserializer.next_event().unwrap().is_some());
    deserializer.finish().expect("all events consumed");
    let encoded = serialization::base64_encode(&input[..input.len().min(1024)]);
    let decoded = serialization::base64_decode(&encoded).expect("base64 round trip");
    assert_eq!(decoded, input[..input.len().min(1024)]);
}

fn route_std_testing(input: &[u8]) {
    let actual = String::from_utf8_lossy(input);
    let diff = testing::diff_text_with_limits(
        "expected",
        &actual,
        DiffLimits {
            max_input_bytes: 256,
            max_lines: 32,
            max_hunks: 16,
            max_output_bytes: 512,
        },
    );
    assert!(diff.render().len() <= 1024);
    let tolerance = FloatTolerance::new(0.01, 0.1).expect("finite tolerance");
    assert!(tolerance.is_near(1.0, 1.0));
}

fn route_std_text(input: &[u8]) {
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = text.len();
        let _ = text.chars().count();
        let _ = text.to_lowercase();
        let _ = text.to_uppercase();
    }
}

fn route_std_time() {
    source_probe(
        "std.time",
        "import std.time\nfn main(): !time.ClockError {\n    let instant = time.now()?\n    _ = instant\n}\n",
    );
}

fn exercise(owner: &str, input: &[u8]) {
    match owner {
        "std.async" => route_std_async(),
        "std.bytes" => route_std_bytes(input),
        "std.collections" => route_std_collections(input),
        "std.console" => route_std_console(),
        "std.core" => route_std_core(),
        "std.env" => route_std_env(),
        "std.format" => route_std_format(input),
        "std.fs" => route_std_fs(),
        "std.io" => route_std_io(input),
        "std.iter" => route_std_iter(),
        "std.json" => route_std_json(input),
        "std.math" => route_std_math(input),
        "std.messagepack" => route_std_messagepack(input),
        "std.meta" => route_std_meta(input),
        "std.path" => route_std_path(input),
        "std.process" => route_std_process(),
        "std.protobuf" => route_std_protobuf(input),
        "std.reflect" => route_std_reflect(input),
        "std.serialization" => route_std_serialization(input),
        "std.testing" => route_std_testing(input),
        "std.text" => route_std_text(input),
        "std.time" => route_std_time(),
        _ => unreachable!("owner route is declared in OWNER_ROUTES"),
    }
}

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(MAX_INPUT_BYTES)];
    let index = input.first().copied().map(usize::from).unwrap_or_default() % OWNER_ROUTES.len();
    let payload = input.get(1..).unwrap_or_default();
    let owner = OWNER_ROUTES[index];
    catch_unwind(AssertUnwindSafe(|| exercise(owner, payload)))
        .unwrap_or_else(|_| panic!("owner-aware stdlib fuzz route panicked: {owner}"));
});
